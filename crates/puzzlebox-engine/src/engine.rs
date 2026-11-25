//! Program lifecycle, trigger dispatch and HA-style run modes.

use std::collections::HashMap;
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
use crate::interp::{ErrorKind, HttpClient, PluginHost, RunCtx, RunHooks, Trace};
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
    pub errors: Vec<CompileError>,
}

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
}

struct RunningProgram {
    program_vars: Arc<Mutex<Map<String, Value>>>,
    tasks: Vec<JoinHandle<()>>,
    runs: Arc<Mutex<HashMap<String, CancellationToken>>>,
    tx: mpsc::UnboundedSender<Dispatch>,
}

struct Dispatch {
    handler: usize,
    trigger: Value,
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
        let invalid: Vec<String> = lock(&self.inner.status)
            .iter()
            .filter(|(_, s)| s.state == "invalid" || s.state == "error")
            .map(|(id, _)| id.clone())
            .collect();
        for id in invalid {
            if let Some(rec) = self.inner.store.get_program(&id).await? {
                self.start(&rec).await;
            }
        }
        Ok(())
    }

    /// Persists the program and (re)starts it. Compile errors are returned but the
    /// record is saved anyway so work is never lost.
    pub async fn save(&self, record: ProgramRecord) -> anyhow::Result<(ProgramRecord, Vec<CompileError>)> {
        let saved = self.inner.store.save_program(record).await?;
        let errors = self.start(&saved).await;
        Ok((saved, errors))
    }

    pub async fn delete(&self, id: &str) -> anyhow::Result<bool> {
        self.stop_program(id);
        lock(&self.inner.status).remove(id);
        let existed = self.inner.store.delete_program(id).await?;
        let _ = self.inner.events.send(EngineEvent::Removed { program_id: id.to_owned() });
        Ok(existed)
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
                lock(&self.inner.globals).insert(name.to_owned(), value);
            }
        }
        Ok(())
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

    /// Cancels all active runs of a program without disabling it.
    pub fn cancel_runs(&self, id: &str) {
        if let Some(rp) = lock(&self.inner.running).get(id) {
            for tok in lock(&rp.runs).values() {
                tok.cancel();
            }
        }
    }

    /// Manually runs a handler (default: the first one), bypassing its triggers.
    pub fn run_now(&self, id: &str, handler: Option<usize>, trigger: Option<Value>) -> bool {
        let running = lock(&self.inner.running);
        let Some(rp) = running.get(id) else { return false };
        rp.tx
            .send(Dispatch {
                handler: handler.unwrap_or(0),
                trigger: trigger.unwrap_or_else(|| json!({"platform": "manual"})),
            })
            .is_ok()
    }

    fn stop_program(&self, id: &str) {
        if let Some(rp) = lock(&self.inner.running).remove(id) {
            for t in rp.tasks {
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
            let (at, error) = (last.started_at, last.error.clone());
            lock(&self.inner.status).entry(rec.id.clone()).or_default().last_run = Some(at);
            if let Some(e) = error {
                lock(&self.inner.status).entry(rec.id.clone()).or_default().last_error = Some(e);
            }
        }
        let program = match self.compile(&rec.workspace) {
            Ok(p) => p,
            Err(errors) => {
                self.update_status(&rec.id, |s| {
                    s.state = "invalid".into();
                    s.errors = errors.clone();
                });
                return errors;
            }
        };
        if !rec.enabled {
            self.update_status(&rec.id, |s| {
                s.state = "disabled".into();
                s.errors.clear();
                s.active_runs = 0;
            });
            return vec![];
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
        let mut errors = Vec::new();

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
                                    if tx.send(Dispatch { handler: idx, trigger: t }).is_err() {
                                        break;
                                    }
                                }
                            }));
                        }
                        Err(e) => errors.push(CompileError { block_id: Some(id.clone()), message: e }),
                    },
                    other => {
                        let tx = tx.clone();
                        let sink =
                            Box::new(move |t: Value| tx.send(Dispatch { handler: idx, trigger: t }).is_ok());
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
        };
        tasks.push(tokio::spawn(ctx.dispatch(rx)));

        for (idx, h) in program.handlers.iter().enumerate() {
            if matches!(h.kind, HandlerKind::OnStart) {
                let _ = tx.send(Dispatch { handler: idx, trigger: json!({"platform": "start"}) });
            }
        }

        lock(&self.inner.running).insert(rec.id.clone(), RunningProgram { program_vars, tasks, runs, tx });
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
                if qtx.try_send(d).is_err() {
                    self.skipped("queue is full");
                }
            }
            w.abort();
            return;
        }
        while let Some(d) = rx.recv().await {
            let active = lock(&self.runs).len();
            match self.mode {
                RunMode::Single if active > 0 => {
                    self.skipped("already running (mode: single)");
                    continue;
                }
                RunMode::Restart => {
                    for tok in lock(&self.runs).values() {
                        tok.cancel();
                    }
                }
                RunMode::Parallel if active >= self.max => {
                    self.skipped("maximum parallel runs reached");
                    continue;
                }
                _ => {}
            }
            let _handle = self.spawn_run(d);
        }
    }

    fn skipped(&self, why: &str) {
        warn!(program = %self.program_id, "trigger skipped: {why}");
    }

    fn spawn_run(&self, d: Dispatch) -> JoinHandle<()> {
        let run_id = new_id();
        let token = CancellationToken::new();
        lock(&self.runs).insert(run_id.clone(), token.clone());
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
            let _ = inner.events.send(EngineEvent::RunStarted {
                program_id: this.program_id.clone(),
                run_id: run_id.clone(),
                trigger: d.trigger.clone(),
            });
            let result = tokio::select! {
                r = ctx.run_body(&handler.body) => r,
                _ = token.cancelled() => Err(crate::interp::RunError {
                    block_id: None, message: "cancelled".into(), kind: ErrorKind::Cancelled,
                }),
            };
            inner.debugger.run_finished(&run_id);
            let (status, error, block_id) = match &result {
                Ok(()) => ("ok", None, None),
                Err(e) if e.kind == ErrorKind::Cancelled => ("cancelled", None, None),
                Err(e) => ("error", Some(e.message.clone()), e.block_id.clone()),
            };
            if let Some(err) = &error {
                warn!(program = %this.program_id, block = ?block_id, "run failed: {err}");
            }
            let trace = std::mem::take(&mut *lock(&ctx.trace));
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
                })),
            };
            if let Err(e) = inner.store.save_run(record, RUN_HISTORY).await {
                warn!(error = %e, "failed to store run");
            }
            lock(&this.runs).remove(&run_id);
            engine.update_status(&this.program_id, |s| {
                s.active_runs = s.active_runs.saturating_sub(1);
                if let Some(e) = &error {
                    s.last_error = Some(e.clone());
                }
            });
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

#[async_trait::async_trait]
impl RunHooks for EngineHooks {
    async fn on_step(&self, ctx: &RunCtx, block: &str) {
        let inner = &self.engine.inner;
        if inner.debugger.is_watched(&ctx.program_id) {
            let _ = inner.events.send(EngineEvent::Step {
                program_id: ctx.program_id.clone(),
                run_id: ctx.run_id.clone(),
                block_id: block.to_owned(),
            });
        }
        inner.debugger.maybe_pause(ctx, block, &inner.events).await;
    }

    fn on_log(&self, ctx: &RunCtx, level: LogLevel, message: &str) {
        match level {
            LogLevel::Error => tracing::error!(program = %ctx.program_id, "{message}"),
            LogLevel::Warning => tracing::warn!(program = %ctx.program_id, "{message}"),
            LogLevel::Debug => tracing::debug!(program = %ctx.program_id, "{message}"),
            LogLevel::Info => tracing::info!(program = %ctx.program_id, "{message}"),
        }
        let _ = self.engine.inner.events.send(EngineEvent::Log {
            program_id: ctx.program_id.clone(),
            run_id: ctx.run_id.clone(),
            level,
            message: message.to_owned(),
            ts: now_ms(),
        });
    }
}
