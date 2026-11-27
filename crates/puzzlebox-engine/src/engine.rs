//! Program lifecycle, trigger dispatch and HA-style run modes.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use futures_util::StreamExt;
use puzzlebox_ir::{BlockLowerer, CompileError, HandlerKind, LogLevel, NoExternals, Program, Trigger};
use puzzlebox_storage::{ProgramRecord, RunRecord, Store, now_ms};
use serde::Serialize;
use serde_json::{Map, Value, json};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::debug::Debugger;
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

struct RunningProgram {
    program: Arc<Program>,
    /// Named timers: name → task that dispatches the timer's handler when it fires.
    timers: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
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
        let (events, _) = broadcast::channel(1024);
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
                logs: Mutex::new(HashMap::new()),
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
        match self.inner.store.purge_old().await {
            Ok(0) => {}
            Ok(n) => info!("removed {n} programs from the recycle bin or never used"),
            Err(e) => warn!(error = %e, "failed to empty the recycle bin"),
        }
        for p in self.inner.store.list_programs().await? {
            self.start(&p).await;
        }
        Ok(())
    }

    /// Swaps the block lowerer and restarts programs that failed to compile, since
    /// they may reference blocks that only just became available.
    pub async fn registry_changed(&self, lowerer: Arc<dyn BlockLowerer>) -> anyhow::Result<()> {
        self.set_lowerer(lowerer);
        let _ = self.inner.events.send(EngineEvent::RegistryChanged);
        for rec in self.inner.store.list_programs().await? {
            let state = self.status(&rec.id).map(|s| s.state).unwrap_or_default();
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
                self.start(&rec).await;
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
        let saved = self.inner.store.save_program(record).await?;
        if let Some(l) = labels.filter(Value::is_object) {
            self.inner.store.save_labels(&saved.id, &l).await?;
        }
        let errors = self.start(&saved).await;
        Ok((saved, errors))
    }

    /// Deletes a program in the recycle bin for good.
    pub async fn purge(&self, id: &str) -> anyhow::Result<bool> {
        lock(&self.inner.logs).remove(id);
        self.inner.store.purge_program(id).await
    }

    /// Stops the program and moves it to the recycle bin.
    pub async fn delete(&self, id: &str) -> anyhow::Result<bool> {
        self.stop_program(id);
        lock(&self.inner.status).remove(id);
        let existed = self.inner.store.delete_program(id).await?;
        let _ = self.inner.events.send(EngineEvent::Removed { program_id: id.to_owned() });
        Ok(existed)
    }

    /// Takes a program out of the recycle bin and starts it again.
    pub async fn restore(&self, id: &str) -> anyhow::Result<Option<ProgramRecord>> {
        let Some(rec) = self.inner.store.restore_program(id).await? else { return Ok(None) };
        self.start(&rec).await;
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
        for tok in runs.values() {
            tok.cancel();
        }
        runs.len()
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

    fn stop_program(&self, id: &str) {
        if let Some(rp) = lock(&self.inner.running).remove(id) {
            for t in rp.tasks {
                t.abort();
            }
            for (_, t) in lock(&rp.timers).drain() {
                t.abort();
            }
            for tok in lock(&rp.runs).values() {
                tok.cancel();
            }
        }
        self.inner.sources.unregister_program(id);
    }

    async fn start(&self, rec: &ProgramRecord) -> Vec<CompileError> {
        self.stop_program(&rec.id);
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
        // Stacks with problems are left out; the rest runs. Nothing to run at all = invalid.
        let (program, compile_errors) = self.compile_partial(&rec.workspace);
        {
            let mut all = lock(&self.inner.status);
            let st = all.entry(rec.id.clone()).or_default();
            st.summary = crate::describe::program(&program, self.inner.ha.as_ref());
        }
        let labels = Arc::new(self.inner.store.labels(&rec.id).await.unwrap_or_default());
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
            labels,
        };
        tasks.push(tokio::spawn(ctx.dispatch(rx)));

        for (idx, h) in program.handlers.iter().enumerate() {
            if matches!(h.kind, HandlerKind::OnStart) {
                let _ =
                    tx.send(Dispatch { handler: idx, trigger: json!({"platform": "start"}), waiter: None });
            }
        }

        lock(&self.inner.running).insert(
            rec.id.clone(),
            RunningProgram {
                program: program.clone(),
                timers: Arc::new(Mutex::new(HashMap::new())),
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
    labels: Arc<Value>,
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
                RunMode::Restart => {
                    for tok in lock(&self.runs).values() {
                        tok.cancel();
                    }
                }
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
            let (status, error, block_id) = match &result {
                Ok(()) => ("ok", None, None),
                Err(e) if e.kind == ErrorKind::Cancelled => ("cancelled", None, None),
                Err(e) => ("error", Some(e.message.clone()), e.block_id.clone()),
            };
            lock(&inner.active).remove(&run_id);
            if let Some(err) = &error {
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
            let labels: Map<String, Value> = trace
                .steps
                .iter()
                .filter_map(|s| Some((s.block.clone(), this.labels.get(&s.block)?.clone())))
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
                if let Some(e) = &error {
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
            let mut timers = lock(&rp.timers);
            if let Some(t) = timers.get(name) {
                if !t.is_finished() && mode == puzzlebox_ir::TimerMode::Keep {
                    return Ok(());
                }
                t.abort();
            }
            let (tx, timers_ref, key) = (rp.tx.clone(), rp.timers.clone(), name.to_owned());
            let trigger = json!({"platform": "timer", "timer": name, "id": handler, "started_by": ctx.trigger.clone(), "data": data});
            let task = tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs_f64(seconds)).await;
                lock(&timers_ref).remove(&key);
                let _ = tx.send(Dispatch { handler: idx, trigger , waiter: None });
            });
            timers.insert(name.to_owned(), task);
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
                t.abort();
            }
        });
    }

    fn timer_running(&self, ctx: &RunCtx, name: &str) -> bool {
        self.with_program(ctx, |rp| lock(&rp.timers).get(name).is_some_and(|t| !t.is_finished()))
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
        inner.debugger.maybe_pause(ctx, block, &inner.events).await;
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
