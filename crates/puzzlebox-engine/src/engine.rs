//! Program lifecycle, trigger dispatch and HA-style run modes.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use puzzlebox_ir::{BlockLowerer, CompileError, HandlerKind, LogLevel, NoExternals, Program, Trigger};
use puzzlebox_storage::{LogRecord, ProgramRecord, RunRecord, Store, now_ms};
use serde::Serialize;
use serde_json::{Map, Value, json};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::debug::{Debugger, Unpaused};
use crate::ha::HaApi;
use crate::interp::{ErrorKind, HttpClient, PluginHost, RunCtx, RunHooks, Trace, Waiting};
use crate::sources::{TriggerSource, TriggerSources};

const RUN_HISTORY: usize = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    Single,
    Restart,
    Queued,
    Parallel,
}

impl RunMode {
    pub fn parse(s: &str) -> Self {
        match s {
            "restart" => RunMode::Restart,
            "queued" => RunMode::Queued,
            "parallel" => RunMode::Parallel,
            _ => RunMode::Single,
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ProgramStatus {
    /// `disabled`, `invalid`, `error`, `idle` or `running`.
    pub state: String,
    pub active_runs: usize,
    pub last_run: Option<i64>,
    pub last_error: Option<String>,
    /// How the last run ended (`ok`, `error`, `cancelled`), also after a restart.
    pub last_status: Option<String>,
    pub errors: Vec<CompileError>,
    /// What starts the program, in words ("when Movement turns on (+1)").
    pub summary: String,
    /// Runs paused at a breakpoint.
    pub paused: usize,
}

/// Why runs were stopped, kept with the stopped run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Saved,
    TurnedOff,
    Deleted,
    /// The blocks it uses changed (HA or a plugin changed them).
    BlocksChanged,
    StopButton,
    /// Restart mode: triggered again.
    TriggeredAgain,
    Shutdown,
    /// Loaded at startup: nothing was running.
    Startup,
}

impl StopReason {
    pub fn text(self) -> &'static str {
        match self {
            StopReason::Saved => "stopped because the program was saved",
            StopReason::TurnedOff => "stopped because the program was turned off",
            StopReason::Deleted => "stopped because the program was deleted",
            StopReason::BlocksChanged => {
                "stopped because blocks it uses changed (Home Assistant or a plugin changed them)"
            }
            StopReason::StopButton => "stopped with the Stop button",
            StopReason::TriggeredAgain => {
                "stopped because it was triggered again (if already running: start over)"
            }
            StopReason::Shutdown => "stopped because Puzzlebox stopped or restarted",
            StopReason::Startup => "stopped",
        }
    }

    /// Timers belong to the program the user changed; a restart Puzzlebox did by itself keeps them.
    fn clears_timers(self) -> bool {
        matches!(self, StopReason::Saved | StopReason::TurnedOff | StopReason::Deleted)
    }
}

/// A timer counting down.
#[derive(Debug, Clone, Serialize)]
pub struct TimerInfo {
    pub name: String,
    /// When it goes off (ms since the epoch).
    pub due: i64,
}

/// What saving a program would do to it.
#[derive(Debug, Clone, Serialize)]
pub struct SaveImpact {
    /// The saved program differs from the running one, so it starts again.
    pub restarts: bool,
    /// Runs and timers the restart would stop.
    pub runs: Vec<ActiveRun>,
    pub timers: Vec<TimerInfo>,
}

/// A run in progress and what it is doing.
#[derive(Debug, Clone, Serialize)]
pub struct ActiveRun {
    pub run_id: String,
    pub program_id: String,
    pub started_at: i64,
    /// What started it ("Movement turned on").
    pub summary: String,
    /// The block it is at.
    pub block_id: Option<String>,
    pub waiting: Option<Waiting>,
}

/// A line of a program's log: its "log" blocks, warnings and errors.
#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub ts: i64,
    pub level: LogLevel,
    pub message: String,
    pub run_id: Option<String>,
}

/// Log lines kept per program.
const LOG_KEPT: usize = 200;

/// How long new log lines wait to be written, so a burst of them is one write.
const LOG_FLUSH_DELAY: Duration = Duration::from_secs(2);

/// How often the recycle bin is emptied of programs past `BIN_DAYS`.
const PURGE_EVERY: Duration = Duration::from_secs(24 * 3600);

/// Untouched "New program"s younger than this survive a periodic purge, since the editor may
/// still have one open.
const STRAY_GRACE_MS: i64 = 24 * 3600 * 1000;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineEvent {
    Status {
        program_id: String,
        status: ProgramStatus,
    },
    RunStarted {
        program_id: String,
        run_id: String,
        trigger: Value,
        summary: String,
    },
    /// A run started (`Some`) or stopped (`None`) waiting.
    Waiting {
        program_id: String,
        run_id: String,
        waiting: Option<Waiting>,
    },
    RunFinished {
        program_id: String,
        run_id: String,
        status: String,
        error: Option<String>,
        block_id: Option<String>,
    },
    Log {
        program_id: String,
        run_id: String,
        level: LogLevel,
        message: String,
        ts: i64,
    },
    Step {
        program_id: String,
        run_id: String,
        block_id: String,
    },
    Paused {
        program_id: String,
        run_id: String,
        block_id: String,
        variables: Value,
        /// What started the run.
        summary: String,
    },
    Resumed {
        program_id: String,
        run_id: String,
    },
    Removed {
        program_id: String,
    },
    RegistryChanged,
    /// A global (shared, persistent) variable was set.
    GlobalChanged {
        name: String,
        value: Value,
    },
}

struct TimerEntry {
    task: JoinHandle<()>,
    due: i64,
}

type Timers = Arc<Mutex<HashMap<String, TimerEntry>>>;

struct RunningProgram {
    program: Arc<Program>,
    mode: String,
    max_runs: u32,
    /// A trigger couldn't be set up; saving again retries.
    trigger_errors: bool,
    labels: Arc<Mutex<Arc<Value>>>,
    /// Named timers: name → task that dispatches the timer's handler when it fires.
    timers: Timers,
    /// Throttle keys → when they last let a run through.
    throttles: Mutex<HashMap<String, Instant>>,
    program_vars: Arc<Mutex<Map<String, Value>>>,
    tasks: Vec<JoinHandle<()>>,
    runs: Arc<Mutex<HashMap<String, CancellationToken>>>,
    tx: mpsc::UnboundedSender<Dispatch>,
}

struct Dispatch {
    handler: usize,
    trigger: Value,
    /// Set by "run program … and wait": told when the run starts and how it ended. Dropped
    /// unused when the run is skipped (run mode).
    waiter: Option<RunWaiter>,
}

struct RunWaiter {
    started: tokio::sync::oneshot::Sender<CancellationToken>,
    finished: tokio::sync::oneshot::Sender<(String, Option<String>)>,
}

/// How deep programs may start each other ("run program"), so a loop ends.
const MAX_PROGRAM_DEPTH: u64 = 10;

/// Cancels the run it holds when dropped, i.e. when the run waiting for it is stopped.
struct CancelOnDrop(Option<CancellationToken>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(t) = self.0.take() {
            t.cancel();
        }
    }
}

/// Timers are kept in the store so they still go off after a restart. One worker applies the
/// changes in order, since timers start and stop from code that can't wait for the store.
enum TimerOp {
    Set { program: String, name: String, value: Value },
    Delete { program: String, name: String },
    Clear { program: String },
    Load { program: String, reply: oneshot::Sender<Map<String, Value>> },
    Flush { reply: oneshot::Sender<()> },
}

fn timer_scope(program: &str) -> String {
    format!("timers:{program}")
}

async fn timer_worker(store: Store, mut rx: mpsc::UnboundedReceiver<TimerOp>) {
    while let Some(op) = rx.recv().await {
        let res = match op {
            TimerOp::Set { program, name, value } => {
                store.kv_set(&timer_scope(&program), &name, &value).await
            }
            TimerOp::Delete { program, name } => store.kv_delete(&timer_scope(&program), &name).await,
            TimerOp::Clear { program } => {
                let scope = timer_scope(&program);
                match store.kv_all(&scope).await {
                    Ok(all) => {
                        for name in all.keys() {
                            if let Err(e) = store.kv_delete(&scope, name).await {
                                warn!(error = %e, "failed to forget a timer");
                            }
                        }
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            }
            TimerOp::Load { program, reply } => {
                let _ = reply.send(store.kv_all(&timer_scope(&program)).await.unwrap_or_default());
                Ok(())
            }
            TimerOp::Flush { reply } => {
                let _ = reply.send(());
                Ok(())
            }
        };
        if let Err(e) = res {
            warn!(error = %e, "failed to store a timer");
        }
    }
}

/// Timers that went off while Puzzlebox was stopped still go off, up to this late.
const TIMER_GRACE_MS: i64 = 60 * 60 * 1000;

pub(crate) struct Inner {
    pub(crate) ha: Arc<dyn HaApi>,
    pub(crate) store: Store,
    globals: Arc<Mutex<Map<String, Value>>>,
    pub(crate) events: broadcast::Sender<EngineEvent>,
    running: Mutex<HashMap<String, RunningProgram>>,
    status: Mutex<HashMap<String, ProgramStatus>>,
    lowerer: RwLock<Arc<dyn BlockLowerer>>,
    plugins: RwLock<Option<Arc<dyn PluginHost>>>,
    http: RwLock<Option<Arc<dyn HttpClient>>>,
    sources: TriggerSources,
    pub(crate) debugger: Debugger,
    active: Mutex<HashMap<String, ActiveRun>>,
    logs: Mutex<HashMap<String, VecDeque<LogEntry>>>,
    /// Run id → why it was stopped, until the run has finished.
    stop_reasons: Mutex<HashMap<String, StopReason>>,
    timer_store: mpsc::UnboundedSender<TimerOp>,
    /// Log lines not yet written to the store.
    log_pending: Mutex<Vec<LogRecord>>,
    log_flush_scheduled: AtomicBool,
    log_writing: tokio::sync::Mutex<()>,
}

#[derive(Clone)]
pub struct Engine {
    pub(crate) inner: Arc<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn new_id() -> String {
    format!("{:016x}", rand::random::<u64>())
}

impl Engine {
    pub async fn new(ha: Arc<dyn HaApi>, store: Store) -> anyhow::Result<Self> {
        let globals = store.kv_all("global").await?;
        let mut logs: HashMap<String, VecDeque<LogEntry>> = HashMap::new();
        for r in store.all_logs().await? {
            let Ok(level) = serde_json::from_value(Value::String(r.level)) else { continue };
            let log = logs.entry(r.program_id).or_default();
            if log.len() >= LOG_KEPT {
                log.pop_front();
            }
            log.push_back(LogEntry { ts: r.ts, level, message: r.message, run_id: r.run_id });
        }
        let (events, _) = broadcast::channel(1024);
        let (timer_store, timer_rx) = mpsc::unbounded_channel();
        tokio::spawn(timer_worker(store.clone(), timer_rx));
        Ok(Self {
            inner: Arc::new(Inner {
                ha,
                store,
                globals: Arc::new(Mutex::new(globals)),
                events,
                running: Mutex::new(HashMap::new()),
                status: Mutex::new(HashMap::new()),
                lowerer: RwLock::new(Arc::new(NoExternals)),
                plugins: RwLock::new(None),
                http: RwLock::new(None),
                sources: TriggerSources::default(),
                debugger: Debugger::default(),
                active: Mutex::new(HashMap::new()),
                stop_reasons: Mutex::new(HashMap::new()),
                timer_store,
                logs: Mutex::new(logs),
                log_pending: Mutex::new(Vec::new()),
                log_flush_scheduled: AtomicBool::new(false),
                log_writing: tokio::sync::Mutex::new(()),
            }),
        })
    }

    pub fn ha(&self) -> &Arc<dyn HaApi> {
        &self.inner.ha
    }

    pub fn store(&self) -> &Store {
        &self.inner.store
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.inner.events.subscribe()
    }

    pub fn set_lowerer(&self, l: Arc<dyn BlockLowerer>) {
        *self.inner.lowerer.write().unwrap_or_else(|p| p.into_inner()) = l;
    }

    pub fn set_plugin_host(&self, p: Arc<dyn PluginHost>) {
        *self.inner.plugins.write().unwrap_or_else(|p| p.into_inner()) = Some(p);
    }

    pub fn set_http_client(&self, h: Arc<dyn HttpClient>) {
        *self.inner.http.write().unwrap_or_else(|p| p.into_inner()) = Some(h);
    }

    /// Registers an engine-native trigger source (cron, webhook, MQTT).
    pub fn add_trigger_source(&self, kind: &'static str, source: Arc<dyn TriggerSource>) {
        self.inner.sources.add(kind, source);
    }

    pub fn debugger(&self) -> &Debugger {
        &self.inner.debugger
    }

    pub fn compile(&self, workspace: &Value) -> Result<Program, Vec<CompileError>> {
        let lowerer = self.inner.lowerer.read().unwrap_or_else(|p| p.into_inner()).clone();
        puzzlebox_ir::compile(workspace, lowerer.as_ref())
    }

    fn compile_partial(&self, workspace: &Value) -> (Program, Vec<CompileError>) {
        let lowerer = self.inner.lowerer.read().unwrap_or_else(|p| p.into_inner()).clone();
        puzzlebox_ir::compile_partial(workspace, lowerer.as_ref())
    }

    pub fn statuses(&self) -> HashMap<String, ProgramStatus> {
        lock(&self.inner.status).clone()
    }

    pub fn status(&self, id: &str) -> Option<ProgramStatus> {
        lock(&self.inner.status).get(id).cloned()
    }

    fn update_status(&self, id: &str, f: impl FnOnce(&mut ProgramStatus)) {
        let snapshot = {
            let mut all = lock(&self.inner.status);
            let st = all.entry(id.to_owned()).or_default();
            f(st);
            if st.state != "disabled" && st.state != "invalid" && st.state != "error" {
                st.state = if st.active_runs > 0 { "running".into() } else { "idle".into() };
            }
            st.clone()
        };
        let _ = self.inner.events.send(EngineEvent::Status { program_id: id.to_owned(), status: snapshot });
    }

    pub async fn load_all(&self) -> anyhow::Result<()> {
        self.purge_old(0).await;
        for p in self.inner.store.list_programs().await? {
            // Saved (and so started) while Puzzlebox was starting up.
            if self.status(&p.id).is_some() {
                continue;
            }
            self.start(&p, StopReason::Startup).await;
        }
        Ok(())
    }

    /// Empties the recycle bin of old programs (see `Store::purge_old`), stopping and
    /// forgetting any that were still loaded. Returns how many were removed.
    pub async fn purge_old(&self, strays_after_ms: i64) -> usize {
        let ids = match self.inner.store.purge_old(strays_after_ms).await {
            Ok(ids) => ids,
            Err(e) => {
                warn!(error = %e, "failed to empty the recycle bin");
                return 0;
            }
        };
        for id in &ids {
            self.stop_program(id, StopReason::Deleted);
            lock(&self.inner.status).remove(id);
            lock(&self.inner.logs).remove(id);
            let _ = self.inner.events.send(EngineEvent::Removed { program_id: id.clone() });
        }
        if !ids.is_empty() {
            info!("removed {} programs from the recycle bin or never used", ids.len());
        }
        ids.len()
    }

    /// Runs `purge_old` once a day in the background.
    pub fn spawn_housekeeping(&self) -> JoinHandle<()> {
        let engine = self.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(PURGE_EVERY);
            every.tick().await;
            loop {
                every.tick().await;
                engine.purge_old(STRAY_GRACE_MS).await;
            }
        })
    }

    /// Writes log lines still waiting to the store; call before shutting down.
    pub async fn flush_log(&self) {
        // One write at a time, so lines reach the store in order.
        let _writing = self.inner.log_writing.lock().await;
        self.inner.log_flush_scheduled.store(false, Ordering::SeqCst);
        let lines = std::mem::take(&mut *lock(&self.inner.log_pending));
        if let Err(e) = self.inner.store.append_log(lines, LOG_KEPT).await {
            warn!(error = %e, "failed to save program logs");
        }
    }

    /// Swaps the block lowerer and restarts programs that failed to compile, since
    /// they may reference blocks that only just became available.
    pub async fn registry_changed(&self, lowerer: Arc<dyn BlockLowerer>) -> anyhow::Result<()> {
        self.set_lowerer(lowerer);
        let _ = self.inner.events.send(EngineEvent::RegistryChanged);
        for rec in self.inner.store.list_programs().await? {
            // Not loaded yet (the registry is built before `load_all` at startup).
            let Some(state) = self.status(&rec.id).map(|s| s.state) else { continue };
            // Failing programs may compile now; running ones restart only if the change
            // affects them (e.g. a renamed device changed a trigger's config).
            let restart = match state.as_str() {
                "invalid" | "error" => true,
                "disabled" => false,
                _ => {
                    let running = lock(&self.inner.running).get(&rec.id).map(|rp| rp.program.clone());
                    match (running, self.compile(&rec.workspace)) {
                        (Some(old), Ok(new)) => *old != new,
                        _ => true,
                    }
                }
            };
            if restart {
                self.start(&rec, StopReason::BlocksChanged).await;
            }
        }
        Ok(())
    }

    /// Persists the program and (re)starts it. Compile errors are returned but the
    /// record is saved anyway so work is never lost.
    pub async fn save(&self, record: ProgramRecord) -> anyhow::Result<(ProgramRecord, Vec<CompileError>)> {
        self.save_with_labels(record, None).await
    }

    /// Like `save`, also storing the editor's text for each block (see `Store::labels`).
    pub async fn save_with_labels(
        &self,
        record: ProgramRecord,
        labels: Option<Value>,
    ) -> anyhow::Result<(ProgramRecord, Vec<CompileError>)> {
        let was_on = self.inner.store.get_program(&record.id).await?.is_some_and(|p| p.enabled);
        let saved = self.inner.store.save_program(record).await?;
        if let Some(l) = labels.filter(Value::is_object) {
            self.inner.store.save_labels(&saved.id, &l).await?;
        }
        let reason = if was_on && !saved.enabled { StopReason::TurnedOff } else { StopReason::Saved };
        let errors = self.start(&saved, reason).await;
        Ok((saved, errors))
    }

    /// Deletes a program in the recycle bin for good.
    pub async fn purge(&self, id: &str) -> anyhow::Result<bool> {
        lock(&self.inner.logs).remove(id);
        lock(&self.inner.log_pending).retain(|l| l.program_id != id);
        self.inner.store.purge_program(id).await
    }

    /// Stops the program and moves it to the recycle bin.
    pub async fn delete(&self, id: &str) -> anyhow::Result<bool> {
        self.stop_program(id, StopReason::Deleted);
        lock(&self.inner.status).remove(id);
        let existed = self.inner.store.delete_program(id).await?;
        let _ = self.inner.events.send(EngineEvent::Removed { program_id: id.to_owned() });
        Ok(existed)
    }

    /// Takes a program out of the recycle bin and starts it again.
    pub async fn restore(&self, id: &str) -> anyhow::Result<Option<ProgramRecord>> {
        let Some(rec) = self.inner.store.restore_program(id).await? else { return Ok(None) };
        self.start(&rec, StopReason::Saved).await;
        Ok(Some(rec))
    }

    /// Runs in progress for a program, oldest first.
    pub fn active_runs(&self, program_id: &str) -> Vec<ActiveRun> {
        let mut runs: Vec<ActiveRun> =
            lock(&self.inner.active).values().filter(|r| r.program_id == program_id).cloned().collect();
        runs.sort_by_key(|r| r.started_at);
        runs
    }

    /// The program's recent log lines, oldest first.
    pub fn log(&self, program_id: &str) -> Vec<LogEntry> {
        lock(&self.inner.logs).get(program_id).map(|l| l.iter().cloned().collect()).unwrap_or_default()
    }

    fn push_log(&self, program_id: &str, run_id: Option<&str>, level: LogLevel, message: &str) {
        let ts = now_ms();
        {
            let mut logs = lock(&self.inner.logs);
            let log = logs.entry(program_id.to_owned()).or_default();
            if log.len() >= LOG_KEPT {
                log.pop_front();
            }
            log.push_back(LogEntry {
                ts,
                level,
                message: message.to_owned(),
                run_id: run_id.map(str::to_owned),
            });
        }
        lock(&self.inner.log_pending).push(LogRecord {
            program_id: program_id.to_owned(),
            ts,
            level: serde_json::to_value(level)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default(),
            message: message.to_owned(),
            run_id: run_id.map(str::to_owned),
        });
        if !self.inner.log_flush_scheduled.swap(true, Ordering::SeqCst)
            && let Ok(rt) = tokio::runtime::Handle::try_current()
        {
            let engine = self.clone();
            rt.spawn(async move {
                tokio::time::sleep(LOG_FLUSH_DELAY).await;
                engine.flush_log().await;
            });
        }
        let _ = self.inner.events.send(EngineEvent::Log {
            program_id: program_id.to_owned(),
            run_id: run_id.unwrap_or_default().to_owned(),
            level,
            message: message.to_owned(),
            ts,
        });
    }

    /// The running program's triggers, in words, by block id.
    pub fn triggers(&self, id: &str) -> Vec<(String, String)> {
        let running = lock(&self.inner.running);
        let Some(rp) = running.get(id) else { return vec![] };
        let ha = self.inner.ha.as_ref();
        rp.program
            .handlers
            .iter()
            .filter_map(|h| match &h.kind {
                HandlerKind::Triggers { triggers } => Some(triggers),
                _ => None,
            })
            .flatten()
            .map(|t| (t.block_id().to_owned(), crate::describe::trigger(t, ha)))
            .collect()
    }

    /// Runs the handler of the trigger block `trigger_block` as if that trigger fired, with
    /// made-up but plausible trigger data.
    pub fn run_as_trigger(&self, id: &str, trigger_block: &str) -> bool {
        let running = lock(&self.inner.running);
        let Some(rp) = running.get(id) else { return false };
        let found = rp.program.handlers.iter().enumerate().find_map(|(idx, h)| match &h.kind {
            HandlerKind::Triggers { triggers } => {
                triggers.iter().find(|t| t.block_id() == trigger_block).map(|t| (idx, t))
            }
            _ => None,
        });
        let Some((handler, t)) = found else { return false };
        let trigger = crate::describe::simulate(t, self.inner.ha.as_ref());
        rp.tx.send(Dispatch { handler, trigger, waiter: None }).is_ok()
    }

    pub async fn set_enabled(&self, id: &str, enabled: bool) -> anyhow::Result<Option<ProgramRecord>> {
        let Some(mut rec) = self.inner.store.get_program(id).await? else { return Ok(None) };
        rec.enabled = enabled;
        Ok(Some(self.save(rec).await?.0))
    }

    /// Sets a persistent variable: program-scoped when `program_id` is given, else global.
    pub async fn set_variable(
        &self,
        program_id: Option<&str>,
        name: &str,
        value: Value,
    ) -> anyhow::Result<()> {
        match program_id {
            Some(id) => {
                self.inner.store.kv_set(&format!("program:{id}"), name, &value).await?;
                if let Some(rp) = lock(&self.inner.running).get(id) {
                    lock(&rp.program_vars).insert(name.to_owned(), value);
                }
            }
            None => {
                self.inner.store.kv_set("global", name, &value).await?;
                lock(&self.inner.globals).insert(name.to_owned(), value.clone());
                let _ = self.inner.events.send(EngineEvent::GlobalChanged { name: name.to_owned(), value });
            }
        }
        Ok(())
    }

    /// Forgets a persistent variable; programs read it as empty until they set it again.
    pub async fn delete_variable(&self, program_id: Option<&str>, name: &str) -> anyhow::Result<()> {
        match program_id {
            Some(id) => {
                self.inner.store.kv_delete(&format!("program:{id}"), name).await?;
                if let Some(rp) = lock(&self.inner.running).get(id) {
                    lock(&rp.program_vars).remove(name);
                }
            }
            None => {
                self.inner.store.kv_delete("global", name).await?;
                lock(&self.inner.globals).remove(name);
                let _ = self
                    .inner
                    .events
                    .send(EngineEvent::GlobalChanged { name: name.to_owned(), value: Value::Null });
            }
        }
        Ok(())
    }

    /// Current global variables.
    pub fn globals(&self) -> Map<String, Value> {
        lock(&self.inner.globals).iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// Persistent variables: `{"global": {...}, "programs": {id: {...}}}`.
    pub async fn variables(&self) -> anyhow::Result<Value> {
        let mut programs = Map::new();
        for p in self.inner.store.list_programs().await? {
            let vars = self.inner.store.kv_all(&format!("program:{}", p.id)).await?;
            if !vars.is_empty() {
                programs.insert(p.id, Value::Object(vars));
            }
        }
        Ok(json!({"global": Value::Object(lock(&self.inner.globals).clone()), "programs": programs}))
    }

    /// Cancels all active runs of a program without disabling it; returns how many.
    pub fn cancel_runs(&self, id: &str) -> usize {
        let Some(rp) = lock(&self.inner.running).get(id).map(|rp| rp.runs.clone()) else { return 0 };
        let runs = lock(&rp);
        self.cancel_with(&runs, StopReason::StopButton);
        runs.len()
    }

    fn cancel_with(&self, runs: &HashMap<String, CancellationToken>, reason: StopReason) {
        let mut reasons = lock(&self.inner.stop_reasons);
        for (run_id, tok) in runs {
            reasons.entry(run_id.clone()).or_insert(reason);
            tok.cancel();
        }
    }

    /// Timers counting down, soonest first.
    pub fn timers(&self, id: &str) -> Vec<TimerInfo> {
        let Some(timers) = lock(&self.inner.running).get(id).map(|rp| rp.timers.clone()) else {
            return vec![];
        };
        let mut out: Vec<TimerInfo> = lock(&timers)
            .iter()
            .filter(|(_, t)| !t.task.is_finished())
            .map(|(name, t)| TimerInfo { name: name.clone(), due: t.due })
            .collect();
        out.sort_by_key(|t| t.due);
        out
    }

    /// Whether saving `rec` would restart the program, and what that would stop.
    pub fn save_impact(&self, rec: &ProgramRecord) -> SaveImpact {
        let (program, _) = self.compile_partial(&rec.workspace);
        let running = lock(&self.inner.running).contains_key(&rec.id);
        let restarts = running && !self.keeps_running(rec, &program);
        let (runs, timers) =
            if restarts { (self.active_runs(&rec.id), self.timers(&rec.id)) } else { (vec![], vec![]) };
        SaveImpact { restarts, runs, timers }
    }

    /// The running program is the same as `rec` would be, so saving needn't restart it.
    fn keeps_running(&self, rec: &ProgramRecord, program: &Program) -> bool {
        rec.enabled
            && lock(&self.inner.running).get(&rec.id).is_some_and(|rp| {
                !rp.trigger_errors
                    && rp.mode == rec.mode
                    && rp.max_runs == rec.max_runs
                    && *rp.program == *program
            })
    }

    /// Stops every run (recording why) before Puzzlebox exits; timers are kept for next time.
    pub async fn shutdown(&self) {
        {
            let running = lock(&self.inner.running);
            for rp in running.values() {
                for (_, t) in lock(&rp.timers).drain() {
                    t.task.abort();
                }
                self.cancel_with(&lock(&rp.runs), StopReason::Shutdown);
            }
        }
        // Give the stopped runs a moment to store their records.
        for _ in 0..60 {
            if lock(&self.inner.active).is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let (reply, done) = oneshot::channel();
        if self.inner.timer_store.send(TimerOp::Flush { reply }).is_ok() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), done).await;
        }
    }

    /// Index of the handler whose entry (or loose stack) is the block `block_id`.
    pub fn handler_index(&self, id: &str, block_id: &str) -> Option<usize> {
        lock(&self.inner.running).get(id)?.program.handlers.iter().position(|h| h.id == block_id)
    }

    /// Manually runs a handler (default: the first one), bypassing its triggers.
    pub fn run_now(&self, id: &str, handler: Option<usize>, trigger: Option<Value>) -> bool {
        let running = lock(&self.inner.running);
        let Some(rp) = running.get(id) else { return false };
        rp.tx
            .send(Dispatch {
                handler: handler.unwrap_or(0),
                trigger: trigger.unwrap_or_else(|| json!({"platform": "manual"})),
                waiter: None,
            })
            .is_ok()
    }

    fn stop_program(&self, id: &str, reason: StopReason) {
        if let Some(rp) = lock(&self.inner.running).remove(id) {
            for t in rp.tasks {
                t.abort();
            }
            for (_, t) in lock(&rp.timers).drain() {
                t.task.abort();
            }
            self.cancel_with(&lock(&rp.runs), reason);
        }
        if reason.clears_timers() {
            let _ = self.inner.timer_store.send(TimerOp::Clear { program: id.to_owned() });
        }
        self.inner.sources.unregister_program(id);
    }

    /// Starts a timer: dispatches handler `idx` at `due` (ms since the epoch).
    #[allow(clippy::too_many_arguments)]
    fn arm_timer(
        &self,
        program_id: &str,
        timers: &Timers,
        tx: &mpsc::UnboundedSender<Dispatch>,
        name: &str,
        idx: usize,
        trigger: Value,
        due: i64,
    ) {
        let (tx, timers_ref, key) = (tx.clone(), timers.clone(), name.to_owned());
        let (store, program) = (self.inner.timer_store.clone(), program_id.to_owned());
        let wait = std::time::Duration::from_millis(due.saturating_sub(now_ms()).max(0) as u64);
        let task = tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            lock(&timers_ref).remove(&key);
            let _ = store.send(TimerOp::Delete { program, name: key });
            let _ = tx.send(Dispatch { handler: idx, trigger, waiter: None });
        });
        if let Some(old) = lock(timers).insert(name.to_owned(), TimerEntry { task, due }) {
            old.task.abort();
        }
    }

    /// Starts the timers that were counting down when the program last stopped.
    async fn rearm_timers(
        &self,
        rec: &ProgramRecord,
        program: &Program,
        timers: &Timers,
        tx: &mpsc::UnboundedSender<Dispatch>,
    ) {
        let (reply, stored) = oneshot::channel();
        if self.inner.timer_store.send(TimerOp::Load { program: rec.id.clone(), reply }).is_err() {
            return;
        }
        let Ok(stored) = stored.await else { return };
        let now = now_ms();
        for (name, v) in stored {
            let handler = v["handler"].as_str().unwrap_or("");
            let due = v["due"].as_i64().unwrap_or(0);
            let idx = program
                .handlers
                .iter()
                .position(|h| h.id == handler && matches!(h.kind, HandlerKind::Timer { .. }));
            match idx {
                Some(idx) if due >= now - TIMER_GRACE_MS => {
                    self.arm_timer(&rec.id, timers, tx, &name, idx, v["trigger"].clone(), due);
                }
                _ => {
                    let _ = self
                        .inner
                        .timer_store
                        .send(TimerOp::Delete { program: rec.id.clone(), name: name.clone() });
                    if idx.is_some() {
                        let at = crate::clock::local(
                            chrono::DateTime::from_timestamp_millis(due).unwrap_or_default(),
                        );
                        self.push_log(
                            &rec.id,
                            None,
                            LogLevel::Warning,
                            &format!(
                                "timer “{name}” was due at {} while Puzzlebox was stopped; that's too long ago to run it now",
                                at.format("%Y-%m-%d %H:%M")
                            ),
                        );
                    }
                }
            }
        }
    }

    async fn start(&self, rec: &ProgramRecord, reason: StopReason) -> Vec<CompileError> {
        // Stacks with problems are left out; the rest runs. Nothing to run at all = invalid.
        let (program, compile_errors) = self.compile_partial(&rec.workspace);
        if self.keeps_running(rec, &program) {
            // Only the name, description, folder or block positions changed: runs and timers go on.
            let labels = Arc::new(self.inner.store.labels(&rec.id).await.unwrap_or_default());
            if let Some(rp) = lock(&self.inner.running).get(&rec.id) {
                *lock(&rp.labels) = labels;
            }
            let msg = compile_errors.iter().map(|e| e.message.as_str()).collect::<Vec<_>>().join("; ");
            self.update_status(&rec.id, |s| {
                s.state = if compile_errors.is_empty() { "idle".into() } else { "error".into() };
                s.last_error = (!compile_errors.is_empty()).then_some(msg);
                s.errors = compile_errors.clone();
            });
            return compile_errors;
        }
        self.stop_program(&rec.id, reason);
        // Status is in memory; after a restart, take "last run" from the stored history.
        if self.status(&rec.id).is_none_or(|s| s.last_run.is_none())
            && let Ok(runs) = self.inner.store.list_runs(&rec.id, 1).await
            && let Some(last) = runs.first()
        {
            let mut all = lock(&self.inner.status);
            let st = all.entry(rec.id.clone()).or_default();
            st.last_run = Some(last.started_at);
            st.last_status = Some(last.status.clone());
            if let Some(e) = &last.error {
                st.last_error = Some(e.clone());
            }
        }
        {
            let mut all = lock(&self.inner.status);
            let st = all.entry(rec.id.clone()).or_default();
            st.summary = crate::describe::program(&program, self.inner.ha.as_ref());
        }
        let labels =
            Arc::new(Mutex::new(Arc::new(self.inner.store.labels(&rec.id).await.unwrap_or_default())));
        if !compile_errors.is_empty() && program.handlers.is_empty() {
            self.update_status(&rec.id, |s| {
                s.state = "invalid".into();
                s.errors = compile_errors.clone();
            });
            return compile_errors;
        }
        if !rec.enabled {
            self.update_status(&rec.id, |s| {
                s.state = "disabled".into();
                s.errors = compile_errors.clone();
                s.active_runs = 0;
            });
            return compile_errors;
        }
        self.update_status(&rec.id, |s| {
            s.state = "idle".into();
            s.errors.clear();
            s.last_error = None;
        });

        let program = Arc::new(program);
        let program_vars = match self.inner.store.kv_all(&format!("program:{}", rec.id)).await {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "failed to load program variables");
                Map::new()
            }
        };
        let program_vars = Arc::new(Mutex::new(program_vars));
        let (tx, rx) = mpsc::unbounded_channel();
        let runs = Arc::new(Mutex::new(HashMap::new()));
        let mut tasks = Vec::new();
        let mut errors = compile_errors;
        let compile_problems = errors.len();

        for (idx, h) in program.handlers.iter().enumerate() {
            let HandlerKind::Triggers { triggers } = &h.kind else { continue };
            for trig in triggers {
                match trig {
                    Trigger::Ha { id, config } => match self.inner.ha.subscribe_trigger(config.clone()).await
                    {
                        Ok(mut stream) => {
                            let tx = tx.clone();
                            tasks.push(tokio::spawn(async move {
                                while let Some(t) = stream.next().await {
                                    if tx.send(Dispatch { handler: idx, trigger: t, waiter: None }).is_err() {
                                        break;
                                    }
                                }
                            }));
                        }
                        Err(e) => errors.push(CompileError { block_id: Some(id.clone()), message: e }),
                    },
                    other => {
                        let tx = tx.clone();
                        let trigger_id = other.block_id().to_owned();
                        // Engine triggers report their block id like HA triggers do (`trigger.id`).
                        let sink = Box::new(move |mut t: Value| {
                            if let Some(o) = t.as_object_mut() {
                                o.entry("id").or_insert_with(|| json!(trigger_id));
                            }
                            tx.send(Dispatch { handler: idx, trigger: t, waiter: None }).is_ok()
                        });
                        if let Err(e) = self.inner.sources.register(&rec.id, other, sink) {
                            errors.push(CompileError { block_id: Some(other.block_id().into()), message: e });
                        }
                    }
                }
            }
        }

        let ctx = DispatchCtx {
            engine: self.clone(),
            program_id: rec.id.clone(),
            program: program.clone(),
            program_vars: program_vars.clone(),
            runs: runs.clone(),
            mode: RunMode::parse(&rec.mode),
            max: rec.max_runs.max(1) as usize,
            labels: labels.clone(),
        };
        tasks.push(tokio::spawn(ctx.dispatch(rx)));

        for (idx, h) in program.handlers.iter().enumerate() {
            if matches!(h.kind, HandlerKind::OnStart) {
                let _ =
                    tx.send(Dispatch { handler: idx, trigger: json!({"platform": "start"}), waiter: None });
            }
        }

        let timers: Timers = Arc::new(Mutex::new(HashMap::new()));
        if !reason.clears_timers() {
            self.rearm_timers(rec, &program, &timers, &tx).await;
        }
        lock(&self.inner.running).insert(
            rec.id.clone(),
            RunningProgram {
                program: program.clone(),
                mode: rec.mode.clone(),
                max_runs: rec.max_runs,
                trigger_errors: errors.len() > compile_problems,
                labels,
                timers,
                throttles: Mutex::new(HashMap::new()),
                program_vars,
                tasks,
                runs,
                tx,
            },
        );
        if !errors.is_empty() {
            let msg = errors.iter().map(|e| e.message.as_str()).collect::<Vec<_>>().join("; ");
            self.update_status(&rec.id, |s| {
                s.state = "error".into();
                s.last_error = Some(msg);
                s.errors = errors.clone();
            });
        }
        info!(program = %rec.name, handlers = program.handlers.len(), "program started");
        errors
    }
}

#[derive(Clone)]
struct DispatchCtx {
    engine: Engine,
    program_id: String,
    program: Arc<Program>,
    program_vars: Arc<Mutex<Map<String, Value>>>,
    runs: Arc<Mutex<HashMap<String, CancellationToken>>>,
    mode: RunMode,
    max: usize,
    /// The editor's text for each block when the program was saved.
    labels: Arc<Mutex<Arc<Value>>>,
}

impl DispatchCtx {
    async fn dispatch(self, mut rx: mpsc::UnboundedReceiver<Dispatch>) {
        if self.mode == RunMode::Queued {
            // Sequential: a run must finish before the next one starts.
            let (qtx, mut qrx) = mpsc::channel::<Dispatch>(self.max);
            let worker = self.clone();
            let w = tokio::spawn(async move {
                while let Some(d) = qrx.recv().await {
                    let _ = worker.spawn_run(d).await;
                }
            });
            while let Some(d) = rx.recv().await {
                if self.is_timer(&d) {
                    let _handle = self.spawn_run(d);
                    continue;
                }
                if qtx.try_send(d).is_err() {
                    self.skipped("too many runs waiting (if already running: run after)");
                }
            }
            w.abort();
            return;
        }
        while let Some(d) = rx.recv().await {
            // A timer firing is the continuation of an earlier run, not a new trigger.
            if self.is_timer(&d) {
                let _handle = self.spawn_run(d);
                continue;
            }
            let active = lock(&self.runs).len();
            match self.mode {
                RunMode::Single if active > 0 => {
                    self.skipped("it was still running (if already running: ignore)");
                    continue;
                }
                RunMode::Restart => self.engine.cancel_with(&lock(&self.runs), StopReason::TriggeredAgain),
                RunMode::Parallel if active >= self.max => {
                    self.skipped("too many runs at once (if already running: run both)");
                    continue;
                }
                _ => {}
            }
            let _handle = self.spawn_run(d);
        }
    }

    fn is_timer(&self, d: &Dispatch) -> bool {
        self.program.handlers.get(d.handler).is_some_and(|h| matches!(h.kind, HandlerKind::Timer { .. }))
    }

    fn skipped(&self, why: &str) {
        warn!(program = %self.program_id, "trigger skipped: {why}");
        self.engine.push_log(
            &self.program_id,
            None,
            LogLevel::Warning,
            &format!("a trigger was ignored: {why}"),
        );
    }

    fn spawn_run(&self, mut d: Dispatch) -> JoinHandle<()> {
        let run_id = new_id();
        let token = CancellationToken::new();
        lock(&self.runs).insert(run_id.clone(), token.clone());
        let finished = d.waiter.take().map(|w| {
            let _ = w.started.send(token.clone());
            w.finished
        });
        let this = self.clone();
        let engine = self.engine.clone();
        engine.update_status(&self.program_id, |s| {
            s.active_runs += 1;
            s.last_run = Some(now_ms());
        });
        tokio::spawn(async move {
            let Some(handler) = this.program.handlers.get(d.handler) else {
                lock(&this.runs).remove(&run_id);
                engine.update_status(&this.program_id, |s| s.active_runs = s.active_runs.saturating_sub(1));
                return;
            };
            let inner = &engine.inner;
            let started_at = now_ms();
            let ctx = RunCtx {
                program_id: this.program_id.clone(),
                run_id: run_id.clone(),
                program: this.program.clone(),
                ha: inner.ha.clone(),
                store: Some(inner.store.clone()),
                program_vars: this.program_vars.clone(),
                globals: inner.globals.clone(),
                trigger: d.trigger.clone(),
                locals: Mutex::new(Map::new()),
                hooks: Arc::new(EngineHooks { engine: engine.clone() }),
                plugins: inner.plugins.read().unwrap_or_else(|p| p.into_inner()).clone(),
                http: inner.http.read().unwrap_or_else(|p| p.into_inner()).clone(),
                steps: AtomicU64::new(0),
                trace: Mutex::new(Trace::default()),
                started: Instant::now(),
            };
            let summary = crate::describe::fired(&d.trigger, inner.ha.as_ref());
            lock(&inner.active).insert(
                run_id.clone(),
                ActiveRun {
                    run_id: run_id.clone(),
                    program_id: this.program_id.clone(),
                    started_at,
                    summary: summary.clone(),
                    block_id: None,
                    waiting: None,
                },
            );
            let _ = inner.events.send(EngineEvent::RunStarted {
                program_id: this.program_id.clone(),
                run_id: run_id.clone(),
                trigger: d.trigger.clone(),
                summary: summary.clone(),
            });
            let result = tokio::select! {
                r = ctx.run_body(&handler.body) => r,
                _ = token.cancelled() => Err(crate::interp::RunError {
                    block_id: None, message: "cancelled".into(), kind: ErrorKind::Cancelled,
                    on_error: Default::default(),
                }),
            };
            inner.debugger.run_finished(&run_id);
            let stopped = lock(&inner.stop_reasons).remove(&run_id);
            let (status, error, block_id) = match &result {
                Ok(()) => ("ok", None, None),
                Err(e) if e.kind == ErrorKind::Cancelled => {
                    ("cancelled", stopped.map(|r| r.text().to_owned()), None)
                }
                Err(e) => ("error", Some(e.message.clone()), e.block_id.clone()),
            };
            lock(&inner.active).remove(&run_id);
            if let (Some(why), "cancelled") = (&error, status) {
                engine.push_log(&this.program_id, Some(&run_id), LogLevel::Info, &format!("a run was {why}"));
            } else if let Some(err) = &error {
                warn!(program = %this.program_id, block = ?block_id, "run failed: {err}");
                engine.push_log(
                    &this.program_id,
                    Some(&run_id),
                    LogLevel::Error,
                    &format!("the run failed: {err}"),
                );
            }
            let trace = std::mem::take(&mut *lock(&ctx.trace));
            // The blocks as they read when this run ran, for describing it after later edits.
            let all_labels = lock(&this.labels).clone();
            let labels: Map<String, Value> = trace
                .steps
                .iter()
                .filter_map(|s| Some((s.block.clone(), all_labels.get(&s.block)?.clone())))
                .collect();
            let record = RunRecord {
                id: run_id.clone(),
                program_id: this.program_id.clone(),
                started_at,
                finished_at: Some(now_ms()),
                status: status.into(),
                error: error.clone(),
                trigger: Some(d.trigger),
                trace: Some(json!({
                    "steps": trace.steps,
                    "truncated": trace.truncated,
                    "variables": ctx.variables_snapshot(),
                    "error_block": block_id,
                    "labels": labels,
                })),
                summary: Some(summary),
            };
            if let Err(e) = inner.store.save_run(record, RUN_HISTORY).await {
                warn!(error = %e, "failed to store run");
            }
            lock(&this.runs).remove(&run_id);
            engine.update_status(&this.program_id, |s| {
                s.active_runs = s.active_runs.saturating_sub(1);
                s.last_status = Some(status.into());
                if let (Some(e), "error") = (&error, status) {
                    s.last_error = Some(e.clone());
                }
            });
            if let Some(f) = finished {
                let _ = f.send((status.to_owned(), error.clone()));
            }
            let _ = inner.events.send(EngineEvent::RunFinished {
                program_id: this.program_id.clone(),
                run_id,
                status: status.into(),
                error,
                block_id,
            });
        })
    }
}

struct EngineHooks {
    engine: Engine,
}

impl EngineHooks {
    fn with_program<T>(&self, ctx: &RunCtx, f: impl FnOnce(&RunningProgram) -> T) -> Option<T> {
        lock(&self.engine.inner.running).get(&ctx.program_id).map(f)
    }
}

#[async_trait::async_trait]
impl RunHooks for EngineHooks {
    fn timer_start(
        &self,
        ctx: &RunCtx,
        name: &str,
        seconds: f64,
        mode: puzzlebox_ir::TimerMode,
        handler: &str,
        data: Value,
    ) -> Result<(), String> {
        let started = self.with_program(ctx, |rp| {
            // A loose stack starting with this block shares its id; only the timer body counts.
            let idx = rp
                .program
                .handlers
                .iter()
                .position(|h| h.id == handler && matches!(h.kind, HandlerKind::Timer { .. }))
                .ok_or("timer body not found")?;
            if let Some(t) = lock(&rp.timers).get(name)
                && !t.task.is_finished()
                && mode == puzzlebox_ir::TimerMode::Keep
            {
                return Ok(());
            }
            let trigger = json!({"platform": "timer", "timer": name, "id": handler, "started_by": ctx.trigger.clone(), "data": data});
            let due = now_ms() + (seconds.max(0.0) * 1000.0) as i64;
            let engine = &self.engine;
            engine.arm_timer(&ctx.program_id, &rp.timers, &rp.tx, name, idx, trigger.clone(), due);
            let value = json!({"due": due, "handler": handler, "trigger": trigger});
            let _ = engine.inner.timer_store.send(TimerOp::Set {
                program: ctx.program_id.clone(),
                name: name.to_owned(),
                value,
            });
            Ok(())
        });
        started.unwrap_or(Err("the program is not running".into()))
    }

    async fn run_program(
        &self,
        ctx: &RunCtx,
        block: &str,
        program_id: &str,
        wait: bool,
    ) -> Result<(), String> {
        let inner = &self.engine.inner;
        let caller = &ctx.trigger;
        let from_program = caller["platform"] == "program";
        let depth = if from_program { caller["depth"].as_u64().unwrap_or(0) } else { 0 };
        if depth >= MAX_PROGRAM_DEPTH {
            return Err(format!(
                "programs started each other more than {MAX_PROGRAM_DEPTH} deep; check for programs that start each other in a loop"
            ));
        }
        let Some(rec) = inner.store.get_program(program_id).await.map_err(|e| e.to_string())? else {
            return Err("the program to run was deleted; choose another one".into());
        };
        let name = &rec.name;
        // Programs waiting for this run, so waiting in a circle fails instead of hanging.
        let mut waiting: Vec<Value> =
            if from_program { caller["waiting"].as_array().cloned().unwrap_or_default() } else { vec![] };
        if wait {
            if program_id == ctx.program_id || waiting.iter().any(|w| w == program_id) {
                return Err(format!(
                    "“{name}” is waiting for this run already, so it can't wait for “{name}” in turn"
                ));
            }
            waiting.push(json!(ctx.program_id));
        } else {
            waiting.clear();
        }
        let trigger = json!({
            "platform": "program",
            "program_id": ctx.program_id,
            "run_id": ctx.run_id,
            "depth": depth + 1,
            "waiting": waiting,
        });
        let (tx, handler) = {
            let running = lock(&inner.running);
            match running.get(program_id) {
                // Its "when run" blocks, as the Run button and run_program in Home Assistant.
                Some(rp) => {
                    let handlers = &rp.program.handlers;
                    let on_run = handlers.iter().position(
                        |h| matches!(&h.kind, HandlerKind::Triggers { triggers } if triggers.is_empty()),
                    );
                    let first_when =
                        handlers.iter().position(|h| matches!(h.kind, HandlerKind::Triggers { .. }));
                    (rp.tx.clone(), on_run.or(first_when).unwrap_or(0))
                }
                None if !rec.enabled => return Err(format!("“{name}” is turned off")),
                None => return Err(format!("“{name}” can't run: it has problems to fix first")),
            }
        };
        if !wait {
            return tx
                .send(Dispatch { handler, trigger, waiter: None })
                .map_err(|_| format!("“{name}” is restarting; try again"));
        }
        let (started_tx, started) = tokio::sync::oneshot::channel();
        let (finished_tx, finished) = tokio::sync::oneshot::channel();
        let waiter = Some(RunWaiter { started: started_tx, finished: finished_tx });
        tx.send(Dispatch { handler, trigger, waiter })
            .map_err(|_| format!("“{name}” is restarting; try again"))?;
        ctx.waiting(block, format!("waiting for “{name}” to finish"), None);
        let Ok(token) = started.await else {
            ctx.waited(block, format!("“{name}” didn't start"));
            return Err(format!(
                "“{name}” didn't start: it is already running and its run mode doesn't start another run"
            ));
        };
        // Stopping this run stops the one it waits for.
        let mut guard = CancelOnDrop(Some(token));
        let outcome = finished.await;
        guard.0 = None;
        match outcome {
            Ok((status, _)) if status == "ok" => {
                ctx.waited(block, format!("“{name}” finished"));
                Ok(())
            }
            Ok((status, error)) if status == "error" => {
                ctx.waited(block, format!("“{name}” failed"));
                Err(format!("“{name}” failed: {}", error.unwrap_or_default()))
            }
            _ => {
                ctx.waited(block, format!("“{name}” was stopped"));
                Err(format!("“{name}” was stopped before it finished"))
            }
        }
    }

    fn timer_cancel(&self, ctx: &RunCtx, name: &str) {
        self.with_program(ctx, |rp| {
            if let Some(t) = lock(&rp.timers).remove(name) {
                t.task.abort();
            }
        });
        let _ = self
            .engine
            .inner
            .timer_store
            .send(TimerOp::Delete { program: ctx.program_id.clone(), name: name.to_owned() });
    }

    fn timer_running(&self, ctx: &RunCtx, name: &str) -> bool {
        self.with_program(ctx, |rp| lock(&rp.timers).get(name).is_some_and(|t| !t.task.is_finished()))
            .unwrap_or(false)
    }

    fn throttle(&self, ctx: &RunCtx, key: &str, seconds: f64) -> bool {
        self.with_program(ctx, |rp| {
            let mut map = lock(&rp.throttles);
            let now = Instant::now();
            match map.get(key) {
                Some(last) if now.duration_since(*last).as_secs_f64() < seconds => false,
                _ => {
                    map.insert(key.to_owned(), now);
                    true
                }
            }
        })
        .unwrap_or(true)
    }

    fn on_wait(&self, ctx: &RunCtx, waiting: Option<Waiting>) {
        let inner = &self.engine.inner;
        if let Some(r) = lock(&inner.active).get_mut(&ctx.run_id) {
            r.waiting = waiting.clone();
        }
        let _ = inner.events.send(EngineEvent::Waiting {
            program_id: ctx.program_id.clone(),
            run_id: ctx.run_id.clone(),
            waiting,
        });
    }

    async fn on_step(&self, ctx: &RunCtx, block: &str) {
        let inner = &self.engine.inner;
        if let Some(r) = lock(&inner.active).get_mut(&ctx.run_id) {
            r.block_id = Some(block.to_owned());
        }
        if inner.debugger.is_watched(&ctx.program_id) {
            let _ = inner.events.send(EngineEvent::Step {
                program_id: ctx.program_id.clone(),
                run_id: ctx.run_id.clone(),
                block_id: block.to_owned(),
            });
        }
        let Some(resume) = inner.debugger.should_pause(ctx, block) else { return };
        let summary = lock(&inner.active).get(&ctx.run_id).map(|r| r.summary.clone()).unwrap_or_default();
        // Also counts down when the run is stopped while paused.
        struct Paused<'a>(&'a Engine, &'a str);
        impl Drop for Paused<'_> {
            fn drop(&mut self) {
                self.0.update_status(self.1, |s| s.paused = s.paused.saturating_sub(1));
            }
        }
        self.engine.update_status(&ctx.program_id, |s| s.paused += 1);
        let _paused = Paused(&self.engine, &ctx.program_id);
        let _ = inner.events.send(EngineEvent::Paused {
            program_id: ctx.program_id.clone(),
            run_id: ctx.run_id.clone(),
            block_id: block.to_owned(),
            variables: ctx.variables_snapshot(),
            summary,
        });
        if let Unpaused::TimedOut = inner.debugger.wait(&ctx.run_id, resume).await {
            let mins = inner.debugger.pause_limit().as_secs().div_ceil(60);
            self.engine.push_log(
                &ctx.program_id,
                Some(&ctx.run_id),
                LogLevel::Warning,
                &format!("a run was paused at a breakpoint for {mins} min without being continued, so it carried on by itself"),
            );
        }
        let _ = inner
            .events
            .send(EngineEvent::Resumed { program_id: ctx.program_id.clone(), run_id: ctx.run_id.clone() });
    }

    fn on_global(&self, name: &str, value: &Value) {
        let _ = self
            .engine
            .inner
            .events
            .send(EngineEvent::GlobalChanged { name: name.to_owned(), value: value.clone() });
    }

    fn on_log(&self, ctx: &RunCtx, level: LogLevel, message: &str) {
        match level {
            LogLevel::Error => tracing::error!(program = %ctx.program_id, "{message}"),
            LogLevel::Warning => tracing::warn!(program = %ctx.program_id, "{message}"),
            LogLevel::Debug => tracing::debug!(program = %ctx.program_id, "{message}"),
            LogLevel::Info => tracing::info!(program = %ctx.program_id, "{message}"),
        }
        self.engine.push_log(&ctx.program_id, Some(&ctx.run_id), level, message);
    }
}
