//! Async tree-walking interpreter over the IR.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures_util::future::{BoxFuture, join_all};
use futures_util::stream::StreamExt;
use puzzlebox_ir::*;
use puzzlebox_storage::Store;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::ha::{ActionError, ConditionError, HaApi, OnError, TemplateStream};
use crate::value::{self, to_num, to_text, truthy};

pub const MAX_STEPS: u64 = 5_000_000;
const MAX_TRACE: usize = 5_000;
const MAX_CALL_DEPTH: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Error,
    /// `stop` block: ends the run successfully.
    Stopped,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunError {
    pub block_id: Option<BlockId>,
    pub message: String,
    pub kind: ErrorKind,
    /// Whether "carry on if this fails" skips it, as HA's `continue_on_error` would.
    #[serde(skip)]
    pub on_error: OnError,
}

impl RunError {
    fn at(block: &str, message: impl Into<String>) -> Self {
        Self {
            block_id: Some(block.to_owned()),
            message: message.into(),
            kind: ErrorKind::Error,
            on_error: OnError::Stop,
        }
    }
}

type R<T> = Result<T, RunError>;

trait AtBlock<T> {
    fn at(self, block: &str) -> R<T>;
}
impl<T> AtBlock<T> for Result<T, String> {
    fn at(self, block: &str) -> R<T> {
        self.map_err(|m| RunError::at(block, m))
    }
}
impl<T> AtBlock<T> for Result<T, ActionError> {
    fn at(self, block: &str) -> R<T> {
        self.map_err(|e| RunError { on_error: e.on_error, ..RunError::at(block, e.message) })
    }
}

pub enum Flow {
    Normal,
    Break,
    Continue,
    Return(Value),
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceEntry {
    pub block: BlockId,
    /// Milliseconds since run start.
    pub t: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// How the step went: which way an "if" went, how a wait ended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct Trace {
    pub steps: Vec<TraceEntry>,
    pub truncated: bool,
}

/// What a run is waiting for right now.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Waiting {
    pub block_id: String,
    /// "waiting 30 s", "waiting until Movement turns off".
    pub text: String,
    /// When the wait ends at the latest (ms since the epoch).
    pub until: Option<i64>,
}

/// Engine callbacks: live events and the debugger's pause point.
#[async_trait]
pub trait RunHooks: Send + Sync {
    async fn on_step(&self, _ctx: &RunCtx, _block: &str) {}
    /// A wait started (`Some`) or ended (`None`).
    fn on_wait(&self, _ctx: &RunCtx, _waiting: Option<Waiting>) {}
    fn on_log(&self, _ctx: &RunCtx, _level: LogLevel, _message: &str) {}
    fn on_global(&self, _name: &str, _value: &Value) {}
    /// Starts (or restarts/keeps) a program timer that runs the handler `handler` later.
    fn timer_start(
        &self,
        _ctx: &RunCtx,
        _name: &str,
        _seconds: f64,
        _mode: TimerMode,
        _handler: &str,
        _data: Value,
    ) -> Result<(), String> {
        Err("timers are not available".into())
    }
    fn timer_cancel(&self, _ctx: &RunCtx, _name: &str) {}
    /// "run program": starts another program and, with `wait`, waits for that run to end.
    async fn run_program(
        &self,
        _ctx: &RunCtx,
        _block: &str,
        _program_id: &str,
        _wait: bool,
    ) -> Result<(), String> {
        Err("running other programs is not available here".into())
    }
    fn timer_running(&self, _ctx: &RunCtx, _name: &str) -> bool {
        false
    }
    /// True at most once per `seconds` for `key` within a program.
    fn throttle(&self, _ctx: &RunCtx, _key: &str, _seconds: f64) -> bool {
        true
    }
}

/// Runtime for third-party blocks (Python bridge, WASM).
#[async_trait]
pub trait PluginHost: Send + Sync {
    async fn call(
        &self,
        plugin: &str,
        block: &str,
        args: Map<String, Value>,
        ctx: Value,
    ) -> Result<Value, String>;
}

#[async_trait]
pub trait HttpClient: Send + Sync {
    async fn request(
        &self,
        method: &str,
        url: &str,
        body: Option<Value>,
        headers: Option<Value>,
    ) -> Result<Value, String>;
}

#[path = "fade.rs"]
mod fade;

pub struct RunCtx {
    pub program_id: String,
    pub run_id: String,
    pub program: Arc<Program>,
    pub ha: Arc<dyn HaApi>,
    pub store: Option<Store>,
    pub program_vars: Arc<Mutex<Map<String, Value>>>,
    pub globals: Arc<Mutex<Map<String, Value>>>,
    pub trigger: Value,
    pub locals: Mutex<Map<String, Value>>,
    pub hooks: Arc<dyn RunHooks>,
    pub plugins: Option<Arc<dyn PluginHost>>,
    pub http: Option<Arc<dyn HttpClient>>,
    pub steps: AtomicU64,
    pub trace: Mutex<Trace>,
    pub started: Instant,
}

/// A function activation: parameters shadow the (workspace-global, as in Blockly) run variables.
pub struct Frame {
    params: Arc<Mutex<Map<String, Value>>>,
    depth: usize,
    /// HA's `repeat` for the innermost loop running this code; loop rounds get their own
    /// frame sharing the parameters, so nested loops shadow it and parallel branches keep it.
    repeat: Option<Value>,
}

impl Frame {
    pub fn root() -> Self {
        Self { params: Arc::new(Mutex::new(Map::new())), depth: 0, repeat: None }
    }

    /// A loop round: `index` counts from 1; `last` is known for loops with a count.
    fn round(&self, index: u64, last: Option<bool>, item: Option<&Value>) -> Frame {
        let mut r = Map::new();
        r.insert("first".into(), json!(index == 1));
        r.insert("index".into(), json!(index));
        if let Some(last) = last {
            r.insert("last".into(), json!(last));
        }
        // HA leaves `item` out when it is None.
        if let Some(item) = item.filter(|i| !i.is_null()) {
            r.insert("item".into(), item.clone());
        }
        Frame { params: self.params.clone(), depth: self.depth, repeat: Some(Value::Object(r)) }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl RunCtx {
    pub async fn run_body(&self, body: &[Stmt]) -> R<()> {
        let frame = Frame::root();
        match self.exec_block(&frame, body).await {
            Ok(_) => Ok(()),
            Err(e) if e.kind == ErrorKind::Stopped => Ok(()),
            Err(e) => {
                let mut trace = lock(&self.trace);
                if let Some(b) = &e.block_id {
                    let t = self.started.elapsed().as_millis() as u64;
                    trace.steps.push(TraceEntry {
                        note: None,
                        block: b.clone(),
                        t,
                        error: Some(e.message.clone()),
                    });
                }
                Err(e)
            }
        }
    }

    pub fn variables_snapshot(&self) -> Value {
        let mut all = lock(&self.globals).clone();
        all.extend(lock(&self.program_vars).clone());
        all.extend(lock(&self.locals).clone());
        Value::Object(all)
    }

    fn ha_vars(&self, frame: &Frame) -> Value {
        let Value::Object(mut all) = self.variables_snapshot() else { unreachable!() };
        all.extend(lock(&frame.params).clone());
        if let Some(r) = &frame.repeat {
            all.insert("repeat".into(), r.clone());
        }
        all.insert("trigger".into(), self.trigger.clone());
        Value::Object(all)
    }

    fn get_var(&self, frame: &Frame, name: &str, scope: VarScope) -> Value {
        let found = match scope {
            VarScope::Local => {
                lock(&frame.params).get(name).cloned().or_else(|| lock(&self.locals).get(name).cloned())
            }
            VarScope::Program => lock(&self.program_vars).get(name).cloned(),
            VarScope::Global => lock(&self.globals).get(name).cloned(),
        };
        found.unwrap_or(Value::Null)
    }

    async fn set_var(&self, frame: &Frame, name: &str, scope: VarScope, v: Value) {
        match scope {
            VarScope::Local => {
                let mut params = lock(&frame.params);
                if let Some(slot) = params.get_mut(name) {
                    *slot = v;
                } else {
                    drop(params);
                    lock(&self.locals).insert(name.to_owned(), v);
                }
            }
            VarScope::Program | VarScope::Global => {
                let (map, kv_scope) = if scope == VarScope::Program {
                    (&self.program_vars, format!("program:{}", self.program_id))
                } else {
                    (&self.globals, "global".to_owned())
                };
                lock(map).insert(name.to_owned(), v.clone());
                if scope == VarScope::Global {
                    self.hooks.on_global(name, &v);
                }
                if let Some(store) = &self.store
                    && let Err(e) = store.kv_set(&kv_scope, name, &v).await
                {
                    tracing::warn!(error = %e, "failed to persist variable {name}");
                }
            }
        }
    }

    fn step(&self, block: &str) -> R<()> {
        let n = self.steps.fetch_add(1, Ordering::Relaxed);
        if n >= MAX_STEPS {
            return Err(RunError::at(block, format!("step limit of {MAX_STEPS} reached (infinite loop?)")));
        }
        let mut trace = lock(&self.trace);
        if trace.steps.len() < MAX_TRACE {
            let t = self.started.elapsed().as_millis() as u64;
            trace.steps.push(TraceEntry { note: None, block: block.to_owned(), t, error: None });
        } else {
            trace.truncated = true;
        }
        Ok(())
    }

    /// Notes how the latest step of `block` went.
    fn note(&self, block: &str, note: impl Into<String>) {
        let mut trace = lock(&self.trace);
        if let Some(s) = trace.steps.iter_mut().rev().find(|s| s.block == block && s.error.is_none()) {
            s.note = Some(note.into());
        }
    }

    pub(crate) fn waiting(&self, block: &str, text: String, timeout: Option<f64>) {
        let until = timeout.map(|t| puzzlebox_storage::now_ms() + (t * 1000.0) as i64);
        self.hooks.on_wait(self, Some(Waiting { block_id: block.to_owned(), text, until }));
    }

    pub(crate) fn waited(&self, block: &str, note: String) {
        self.hooks.on_wait(self, None);
        self.note(block, note);
    }

    /// Like HA's scripts: a condition that raised counts as false, with a warning.
    fn condition_raised(&self, block: &str, message: &str) {
        let message = format!("condition counted as false: {message}");
        {
            let mut trace = lock(&self.trace);
            // "wait until" checks every second; warn once per block and message.
            if trace.steps.iter().any(|s| s.block == block && s.error.as_ref() == Some(&message)) {
                return;
            }
            if trace.steps.len() < MAX_TRACE {
                let t = self.started.elapsed().as_millis() as u64;
                trace.steps.push(TraceEntry {
                    note: None,
                    block: block.to_owned(),
                    t,
                    error: Some(message.clone()),
                });
            }
        }
        self.hooks.on_log(self, LogLevel::Warning, &message);
    }

    pub fn exec_block<'a>(&'a self, frame: &'a Frame, stmts: &'a [Stmt]) -> BoxFuture<'a, R<Flow>> {
        Box::pin(async move {
            for s in stmts {
                self.step(&s.id)?;
                self.hooks.on_step(self, &s.id).await;
                if self.steps.load(Ordering::Relaxed).is_multiple_of(1024) {
                    tokio::task::yield_now().await;
                }
                match self.exec(frame, s).await? {
                    Flow::Normal => {}
                    other => return Ok(other),
                }
            }
            Ok(Flow::Normal)
        })
    }

    async fn exec(&self, frame: &Frame, s: &Stmt) -> R<Flow> {
        let id = s.id.as_str();
        match &s.kind {
            StmtKind::Expr { expr } => {
                self.eval(frame, expr).await?;
            }
            StmtKind::Set { name, scope, value } => {
                let v = self.eval(frame, value).await?;
                self.set_var(frame, name, *scope, v).await;
            }
            StmtKind::SetItem { name, scope, key, value } => {
                let k = self.eval(frame, key).await?;
                let v = self.eval(frame, value).await?;
                let mut container = self.get_var(frame, name, *scope);
                value::set_item(&mut container, &k, v).at(id)?;
                self.set_var(frame, name, *scope, container).await;
            }
            StmtKind::If { branches, otherwise } => {
                for (i, (cond, body)) in branches.iter().enumerate() {
                    if truthy(&self.eval(frame, cond).await?) {
                        self.note(
                            id,
                            match i {
                                0 => "true".to_owned(),
                                n => format!("“else if” number {n} was true"),
                            },
                        );
                        return self.exec_block(frame, body).await;
                    }
                }
                let all = if branches.len() > 1 { "all false" } else { "false" };
                if let Some(body) = otherwise {
                    self.note(id, format!("{all} → else"));
                    return self.exec_block(frame, body).await;
                }
                self.note(id, format!("{all}, skipped"));
            }
            StmtKind::While { cond, body, until } => {
                for i in 1.. {
                    // As in HA, the condition already sees this round's `repeat`.
                    let round = frame.round(i, None, None);
                    let c = truthy(&self.eval(&round, cond).await?);
                    if c == *until {
                        break;
                    }
                    self.step(id)?;
                    match self.exec_block(&round, body).await? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        _ => {}
                    }
                }
            }
            StmtKind::Repeat { times, body } => {
                let n = to_num(&self.eval(frame, times).await?).at(id)?.max(0.0) as u64;
                for i in 1..=n {
                    self.step(id)?;
                    match self.exec_block(&frame.round(i, Some(i == n), None), body).await? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        _ => {}
                    }
                }
            }
            StmtKind::For { var, from, to, by, body } => {
                let from = to_num(&self.eval(frame, from).await?).at(id)?;
                let to = to_num(&self.eval(frame, to).await?).at(id)?;
                let by = to_num(&self.eval(frame, by).await?).at(id)?.abs();
                if by == 0.0 {
                    return Err(RunError::at(id, "step must not be 0"));
                }
                let step = if from > to { -by } else { by };
                let in_range = |i: f64| (step > 0.0 && i <= to) || (step < 0.0 && i >= to);
                let mut i = from;
                let mut round = 1;
                while in_range(i) {
                    self.step(id)?;
                    self.set_var(frame, var, VarScope::Local, value::num(i)).await;
                    let r = frame.round(round, Some(!in_range(i + step)), None);
                    match self.exec_block(&r, body).await? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        _ => {}
                    }
                    i += step;
                    round += 1;
                }
            }
            StmtKind::ForEach { var, list, body } => {
                let evaluated = self.eval(frame, list).await?;
                let items = match self.expand_entities(evaluated).await.at(id)? {
                    Value::Array(a) => a,
                    Value::Object(o) => o.into_iter().map(|(k, _)| json!(k)).collect(),
                    Value::Null => vec![],
                    // A comma-separated list of entities, like the entity block allows.
                    Value::String(s) => {
                        value::entity_list(&json!(s)).into_iter().map(Value::String).collect()
                    }
                    other => return Err(RunError::at(id, format!("cannot loop over {other}"))),
                };
                let n = items.len() as u64;
                for (i, item) in (1..).zip(items) {
                    self.step(id)?;
                    let round = frame.round(i, Some(i == n), Some(&item));
                    self.set_var(frame, var, VarScope::Local, item).await;
                    match self.exec_block(&round, body).await? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        _ => {}
                    }
                }
            }
            StmtKind::Break => return Ok(Flow::Break),
            StmtKind::Continue => return Ok(Flow::Continue),
            StmtKind::Return { value } => {
                let v = match value {
                    Some(e) => self.eval(frame, e).await?,
                    None => Value::Null,
                };
                return Ok(Flow::Return(v));
            }
            StmtKind::Stop => {
                return Err(RunError { kind: ErrorKind::Stopped, ..RunError::at(id, "stopped") });
            }
            StmtKind::TryCatch { body, error_var, handler } => match self.exec_block(frame, body).await {
                Ok(flow) => return Ok(flow),
                Err(e) if e.kind == ErrorKind::Error => {
                    self.set_var(frame, error_var, VarScope::Local, json!(e.message)).await;
                    return self.exec_block(frame, handler).await;
                }
                Err(e) => return Err(e),
            },
            StmtKind::Throw { message } => {
                let m = to_text(&self.eval(frame, message).await?);
                return Err(RunError::at(id, m));
            }
            StmtKind::Parallel { branches } => {
                let results = join_all(branches.iter().map(|b| self.exec_block(frame, b))).await;
                for r in results {
                    if let Flow::Return(v) = r? {
                        return Ok(Flow::Return(v));
                    }
                }
            }
            StmtKind::CallService { service, target, data, response_var } => {
                let target = match target {
                    Some(t) => {
                        let v = self.eval(frame, t).await.map_err(|e| template_failed(e, t))?;
                        Some(value::to_target(v).at(id).map_err(|e| rendered_wrong(e, t))?)
                    }
                    None => None,
                };
                let data = match data {
                    Some(d) => match self.eval(frame, d).await.map_err(|e| template_failed(e, d))? {
                        Value::Null => None,
                        v @ Value::Object(_) => Some(v),
                        other => {
                            let e =
                                RunError::at(id, format!("action data must be a dictionary, got {other}"));
                            return Err(rendered_wrong(e, d));
                        }
                    },
                    None => None,
                };
                let resp = self
                    .ha
                    .call_service_checked(service, data, target, response_var.is_some())
                    .await
                    .at(id)?;
                if let Some(var) = response_var {
                    self.set_var(frame, var, VarScope::Local, resp).await;
                }
            }
            StmtKind::HaScript { sequence, args } => {
                let sequence = self.substitute(frame, sequence, args).await?;
                self.ha.execute_script_checked(sequence, self.ha_vars(frame)).await.at(id)?;
            }
            StmtKind::Wait { seconds } => {
                let s = to_num(&self.eval(frame, seconds).await?).at(id)?;
                if s > 0.0 {
                    let d = crate::describe::seconds(s);
                    self.waiting(id, format!("waiting {d}"), Some(s));
                    tokio::time::sleep(Duration::from_secs_f64(s)).await;
                    self.waited(id, format!("waited {d}"));
                }
            }
            StmtKind::WaitForTrigger { triggers, timeout, result_var, zero_is_no_limit } => {
                let timeout = self.time_limit(frame, id, timeout.as_ref(), *zero_is_no_limit).await?;
                let var = result_var.clone().unwrap_or_else(|| "wait".into());
                // Like HA, which doesn't even set the triggers up.
                if timeout == Some(0.0) {
                    let wait = json!({"remaining": 0.0, "completed": false, "trigger": null});
                    self.set_var(frame, &var, VarScope::Local, wait).await;
                    return Ok(Flow::Normal);
                }
                let started = Instant::now();
                // One subscription for all of them, so HA numbers them (`trigger.idx`, the
                // default `trigger.id`) as in a script, and renders their templates with the
                // run's variables.
                let config = match triggers.as_slice() {
                    [one] => one.clone(),
                    many => Value::Array(many.to_vec()),
                };
                let what = match triggers.as_slice() {
                    [one] => crate::describe::trigger_config(one, self.ha.as_ref()),
                    many => format!("one of {} triggers", many.len()),
                };
                let mut fired_by =
                    self.ha.subscribe_trigger_with(config, self.ha_vars(frame)).await.at(id)?;
                self.waiting(id, format!("waiting until {what}"), timeout);
                let fired = match timeout {
                    Some(t) => {
                        tokio::time::timeout(Duration::from_secs_f64(t), fired_by.next()).await.ok().flatten()
                    }
                    None => fired_by.next().await,
                };
                let completed = fired.is_some();
                let took = crate::describe::seconds(started.elapsed().as_secs_f64());
                self.waited(
                    id,
                    if completed {
                        format!("happened after {took}")
                    } else {
                        format!("gave up after {took}")
                    },
                );
                let wait = json!({
                    "remaining": remaining(timeout, started, completed),
                    "completed": completed,
                    "trigger": fired,
                });
                self.set_var(frame, &var, VarScope::Local, wait).await;
            }
            StmtKind::Log { level, message } => {
                let m = to_text(&self.eval(frame, message).await?);
                self.hooks.on_log(self, *level, &m);
            }
            StmtKind::FireEvent { event_type, data } => {
                let ty = to_text(&self.eval(frame, event_type).await?);
                let data = match data {
                    Some(d) => Some(self.eval(frame, d).await?),
                    None => None,
                };
                self.ha.fire_event(&ty, data).await.at(id)?;
            }
            StmtKind::Plugin { plugin, block, args } => {
                self.plugin_call(frame, id, plugin, block, args).await?;
            }
            StmtKind::TimerStart { name, seconds, if_running, handler, data, key } => {
                let secs = to_num(&self.eval(frame, seconds).await?).at(id)?.max(0.0);
                let data = match data {
                    Some(d) => self.eval(frame, d).await?,
                    None => Value::Null,
                };
                let name = match key {
                    Some(k) => format!("{name}:{}", self.eval(frame, k).await?),
                    None => name.clone(),
                };
                self.hooks.timer_start(self, &name, secs, *if_running, handler, data).at(id)?;
            }
            StmtKind::TimerCancel { name } => self.hooks.timer_cancel(self, name),
            StmtKind::WaitUntil { cond, hold, timeout, zero_is_no_limit } => {
                let hold = match hold {
                    Some(h) => to_num(&self.eval(frame, h).await?).at(id)?.max(0.0),
                    None => 0.0,
                };
                let timeout = self.time_limit(frame, id, timeout.as_ref(), *zero_is_no_limit).await?;
                let started = Instant::now();
                let until = Until {
                    hold: Duration::from_secs_f64(hold),
                    deadline: timeout.map(|t| started + Duration::from_secs_f64(t)),
                };
                let subscribed = match &cond.kind {
                    ExprKind::Template { template } => {
                        self.ha.subscribe_template(template, self.ha_vars(frame)).await
                    }
                    _ => None,
                };
                let hold_text = if hold > 0.0 {
                    format!(" for {}", crate::describe::seconds(hold))
                } else {
                    String::new()
                };
                self.waiting(id, format!("waiting until the condition is true{hold_text}"), timeout);
                let (completed, at_once) = match subscribed {
                    Some(results) => self.wait_template(&cond.id, results.at(&cond.id)?, &until).await?,
                    None => self.poll_until(frame, cond, &until).await?,
                };
                let took = crate::describe::seconds(started.elapsed().as_secs_f64());
                self.waited(
                    id,
                    match (completed, at_once) {
                        (true, true) => "was already true".to_owned(),
                        (true, false) => format!("became true after {took}"),
                        _ => format!("gave up after {took}"),
                    },
                );
                // HA leaves `remaining` at the whole timeout when the condition holds right away.
                let remaining = match (at_once, timeout) {
                    (true, Some(t)) => json!(t),
                    _ => remaining(timeout, started, completed),
                };
                let wait = json!({"remaining": remaining, "completed": completed});
                self.set_var(frame, "wait", VarScope::Local, wait).await;
            }
            StmtKind::ContinueOnError { body } => match self.exec_block(frame, body).await {
                Err(e) if e.kind == ErrorKind::Error && skips(&e, body) => self.error_skipped(&e),
                other => return other,
            },
            StmtKind::RunProgram { program, wait } => {
                self.hooks.run_program(self, id, program, *wait).await.at(id)?;
            }
            StmtKind::Fade { target, from, to, seconds } => {
                self.fade(frame, id, target, from, to, seconds).await?
            }
        }
        Ok(Flow::Normal)
    }

    /// A wait's time limit in seconds; `None` waits as long as it takes.
    async fn time_limit(
        &self,
        frame: &Frame,
        id: &str,
        timeout: Option<&Expr>,
        zero_is_no_limit: bool,
    ) -> R<Option<f64>> {
        let Some(t) = timeout else { return Ok(None) };
        let v = self.eval(frame, t).await?;
        if v.is_null() {
            return Ok(None);
        }
        let t = to_num(&v).at(id)?;
        if zero_is_no_limit {
            return Ok(Some(t).filter(|t| *t > 0.0));
        }
        if t < 0.0 {
            return Err(RunError::at(id, format!("the time limit must not be negative, got {t}")));
        }
        Ok(Some(t))
    }

    /// HA marks the step as failed and goes on.
    fn error_skipped(&self, e: &RunError) {
        let message = format!("carried on after an error: {}", e.message);
        if let Some(b) = &e.block_id {
            let mut trace = lock(&self.trace);
            if trace.steps.len() < MAX_TRACE {
                let t = self.started.elapsed().as_millis() as u64;
                trace.steps.push(TraceEntry {
                    note: None,
                    block: b.clone(),
                    t,
                    error: Some(message.clone()),
                });
            }
        }
        self.hooks.on_log(self, LogLevel::Warning, &message);
    }

    /// "wait until" a condition: `(completed, at_once)`.
    async fn poll_until(&self, frame: &Frame, cond: &Expr, until: &Until) -> R<(bool, bool)> {
        // A template counts as true the way HA reads a template condition.
        let is_template = matches!(cond.kind, ExprKind::Template { .. });
        let mut true_since: Option<Instant> = None;
        for first in std::iter::once(true).chain(std::iter::repeat(false)) {
            let v = self.eval(frame, cond).await?;
            if if is_template { ha_bool(&v) } else { truthy(&v) } {
                let since = *true_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= until.hold {
                    return Ok((true, first));
                }
            } else {
                true_since = None;
            }
            if until.deadline.is_some_and(|d| Instant::now() >= d) {
                break;
            }
            // Conditions read HA's state cache, which updates on every state change;
            // polling once a second is cheap and catches any expression.
            let now = Instant::now();
            let mut nap = Duration::from_secs(1);
            for wake in [until.deadline, true_since.map(|s| s + until.hold)].into_iter().flatten() {
                nap = nap.min(wake.saturating_duration_since(now));
            }
            tokio::time::sleep(nap).await;
        }
        Ok((false, false))
    }

    /// "wait until" a template HA re-renders whenever the entities it uses change, as with
    /// `wait_template`: `(completed, at_once)`. Only the first render's error ends the run;
    /// later ones count as false, as in HA.
    async fn wait_template(&self, id: &str, mut results: TemplateStream, until: &Until) -> R<(bool, bool)> {
        let mut true_since: Option<Instant> = None;
        let mut first = true;
        loop {
            let wake = [until.deadline, true_since.map(|s| s + until.hold)].into_iter().flatten().min();
            // HA looks at the template before it starts counting, so even a limit of 0 waits
            // for the first result.
            let next = match wake.filter(|_| !first) {
                Some(w) => tokio::time::timeout_at(w.into(), results.next()).await.ok(),
                None => Some(results.next().await),
            };
            match next {
                // A timer: the hold is over or the time is up.
                None => {
                    if true_since.is_some_and(|s| s.elapsed() >= until.hold) {
                        return Ok((true, false));
                    }
                    if until.deadline.is_some_and(|d| Instant::now() >= d) {
                        return Ok((false, false));
                    }
                }
                Some(None) => return Err(RunError::at(id, "lost the connection to Home Assistant")),
                Some(Some(Err(e))) if first => return Err(RunError::at(id, e)),
                Some(Some(result)) => {
                    if result.is_ok_and(|v| ha_bool(&v)) {
                        let since = *true_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= until.hold {
                            return Ok((true, first));
                        }
                    } else {
                        true_since = None;
                    }
                }
            }
            first = false;
        }
    }

    async fn plugin_call(
        &self,
        frame: &Frame,
        id: &str,
        plugin: &str,
        block: &str,
        args: &BTreeMap<String, Expr>,
    ) -> R<Value> {
        let host = self.plugins.as_ref().ok_or_else(|| RunError::at(id, "plugins are not available"))?;
        let mut evaluated = Map::new();
        for (k, e) in args {
            evaluated.insert(k.clone(), self.eval(frame, e).await?);
        }
        let ctx = json!({"program_id": self.program_id, "run_id": self.run_id, "trigger": self.trigger});
        host.call(plugin, block, evaluated, ctx).await.at(id)
    }

    pub fn eval<'a>(&'a self, frame: &'a Frame, e: &'a Expr) -> BoxFuture<'a, R<Value>> {
        Box::pin(async move {
            let id = e.id.as_str();
            Ok(match &e.kind {
                ExprKind::Lit { value } => value.clone(),
                ExprKind::Var { name, scope } => self.get_var(frame, name, *scope),
                ExprKind::Binary { bin: BinOp::And, left, right } => {
                    json!(truthy(&self.eval(frame, left).await?) && truthy(&self.eval(frame, right).await?))
                }
                ExprKind::Binary { bin: BinOp::Or, left, right } => {
                    json!(truthy(&self.eval(frame, left).await?) || truthy(&self.eval(frame, right).await?))
                }
                ExprKind::Binary { bin, left, right } => {
                    let l = self.eval(frame, left).await?;
                    let r = self.eval(frame, right).await?;
                    value::binary(*bin, &l, &r).at(id)?
                }
                ExprKind::Not { expr } => json!(!truthy(&self.eval(frame, expr).await?)),
                ExprKind::Each { list, var, expr, map } => {
                    let evaluated = self.eval(frame, list).await?;
                    let items = match self.expand_entities(evaluated).await.at(id)? {
                        Value::Array(a) => a,
                        Value::Null => vec![],
                        other => value::entity_list(&other).into_iter().map(Value::String).collect(),
                    };
                    let saved = lock(&self.locals).get(var).cloned();
                    let mut out = Vec::new();
                    for item in items {
                        lock(&self.locals).insert(var.clone(), item.clone());
                        let v = self.eval(frame, expr).await;
                        let v = match v {
                            Ok(v) => v,
                            Err(e) => {
                                restore(&self.locals, var, saved);
                                return Err(e);
                            }
                        };
                        if *map {
                            out.push(v);
                        } else if truthy(&v) {
                            out.push(item);
                        }
                    }
                    restore(&self.locals, var, saved);
                    Value::Array(out)
                }
                ExprKind::Ternary { cond, then, otherwise } => {
                    if truthy(&self.eval(frame, cond).await?) {
                        self.eval(frame, then).await?
                    } else {
                        self.eval(frame, otherwise).await?
                    }
                }
                ExprKind::List { items } => {
                    let mut out = Vec::with_capacity(items.len());
                    for i in items {
                        out.push(self.eval(frame, i).await?);
                    }
                    Value::Array(out)
                }
                ExprKind::Dict { entries } => {
                    let mut out = Map::new();
                    for (k, v) in entries {
                        out.insert(k.clone(), self.eval(frame, v).await?);
                    }
                    Value::Object(out)
                }
                ExprKind::Builtin { name, args } if name == "list_get" && is_random_index(args) => {
                    let list = self.eval(frame, &args[0]).await?;
                    value::builtin("list_random", &[list]).at(id)?
                }
                ExprKind::Builtin { name, args }
                    if name.starts_with("engine:") || name.starts_with("ha:") =>
                {
                    let mut vals = Vec::with_capacity(args.len());
                    for a in args {
                        vals.push(self.eval(frame, a).await?);
                    }
                    self.context_builtin(name, &vals).await.at(id)?
                }
                ExprKind::Builtin { name, args } if name == "template_with" => {
                    let template = to_text(&self.eval(frame, &args[0]).await?);
                    let extra = self.eval(frame, &args[1]).await?;
                    let mut vars = self.ha_vars(frame);
                    if let (Some(v), Value::Object(e)) = (vars.as_object_mut(), extra) {
                        v.extend(e);
                    }
                    self.ha.render_template(&template, vars).await.at(id)?
                }
                ExprKind::Builtin { name, args } if name == "loop_info" => {
                    let what = match args.first() {
                        Some(a) => to_text(&self.eval(frame, a).await?),
                        None => "index".into(),
                    };
                    frame.repeat.as_ref().and_then(|r| r.get(&what)).cloned().unwrap_or(Value::Null)
                }
                ExprKind::Builtin { name, args } if name == "service_response" => {
                    let service = to_text(&self.eval(frame, &args[0]).await?);
                    let data = self.eval(frame, &args[1]).await?;
                    let target = match args.get(2) {
                        Some(t) => Some(value::to_target(self.eval(frame, t).await?).at(id)?),
                        None => None,
                    };
                    let data = data.as_object().filter(|d| !d.is_empty()).map(|_| data.clone());
                    self.ha.call_service_response(&service, data, target).await.at(id)?
                }
                ExprKind::Builtin { name, args } => {
                    let mut vals = Vec::with_capacity(args.len());
                    for a in args {
                        vals.push(self.eval(frame, a).await?);
                    }
                    value::builtin(name, &vals).at(id)?
                }
                ExprKind::Call { function, args } => self.call(frame, id, function, args).await?,
                ExprKind::State { entity, attribute } => {
                    let entity = to_text(&self.eval(frame, entity).await?);
                    let state = self.ha.state(&entity);
                    match attribute {
                        None => state.and_then(|s| s.get("state").cloned()).unwrap_or(Value::Null),
                        Some(a) => {
                            let attr = to_text(&self.eval(frame, a).await?);
                            state
                                .and_then(|s| s.pointer(&format!("/attributes/{attr}")).cloned())
                                .unwrap_or(Value::Null)
                        }
                    }
                }
                ExprKind::Condition { config, args } => {
                    let config = self.substitute(frame, config, args).await?;
                    match self.ha.check_condition(config, self.ha_vars(frame)).await {
                        Ok(b) => json!(b),
                        Err(ConditionError::Raised(m)) => {
                            self.condition_raised(id, &m);
                            json!(false)
                        }
                        Err(ConditionError::Failed(m)) => return Err(RunError::at(id, m)),
                    }
                }
                ExprKind::Template { template } => {
                    self.ha.render_template(template, self.ha_vars(frame)).await.at(id)?
                }
                ExprKind::TriggerData { path } => {
                    let walk = |root: &Value| {
                        let mut cur = root;
                        for part in path.split('.').filter(|p| !p.is_empty()) {
                            cur = match cur {
                                Value::Object(o) => o.get(part).unwrap_or(&Value::Null),
                                Value::Array(a) => {
                                    part.parse::<usize>().ok().and_then(|i| a.get(i)).unwrap_or(&Value::Null)
                                }
                                _ => &Value::Null,
                            };
                        }
                        cur.clone()
                    };
                    match walk(&self.trigger) {
                        // A timer's run continues the run that started it: use that trigger.
                        Value::Null if self.trigger.get("platform") == Some(&json!("timer")) => {
                            walk(self.trigger.get("started_by").unwrap_or(&Value::Null))
                        }
                        v => v,
                    }
                }
                ExprKind::Http { method, url, body, headers } => {
                    let http = self.http.as_ref().ok_or_else(|| RunError::at(id, "HTTP is not available"))?;
                    let url = to_text(&self.eval(frame, url).await?);
                    let body = match body {
                        Some(b) => Some(self.eval(frame, b).await?),
                        None => None,
                    };
                    let headers = match headers {
                        Some(h) => Some(self.eval(frame, h).await?),
                        None => None,
                    };
                    http.request(method, &url, body, headers).await.at(id)?
                }
                ExprKind::Plugin { plugin, block, args } => {
                    self.plugin_call(frame, id, plugin, block, args).await?
                }
            })
        })
    }

    async fn substitute(&self, frame: &Frame, template: &Value, args: &BTreeMap<String, Expr>) -> R<Value> {
        if args.is_empty() {
            return Ok(template.clone());
        }
        let mut vals = BTreeMap::new();
        for (k, e) in args {
            vals.insert(k.clone(), self.eval(frame, e).await?);
        }
        Ok(puzzlebox_ir::substitute_args(template, &vals))
    }

    /// Builtins that need the engine (timers, throttles) or Home Assistant data.
    async fn context_builtin(&self, name: &str, a: &[Value]) -> Result<Value, String> {
        let arg = |i: usize| a.get(i).cloned().unwrap_or(Value::Null);
        let text = |i: usize| to_text(&arg(i));
        Ok(match name {
            "engine:timer_running" => json!(self.hooks.timer_running(self, &text(0))),
            "engine:throttle" => json!(self.hooks.throttle(self, &text(0), to_num(&arg(1))?)),
            "ha:history" => {
                let seconds = to_num(&arg(2))?.max(1.0);
                let end = chrono::Utc::now();
                let start = end - chrono::Duration::milliseconds((seconds * 1000.0) as i64);
                let points = self.ha.history(&text(0), &start.to_rfc3339(), &end.to_rfc3339()).await?;
                value::history_stat(
                    &points,
                    start.timestamp_millis() as f64 / 1000.0,
                    end.timestamp_millis() as f64 / 1000.0,
                    &text(1),
                    &text(3),
                )?
            }
            "ha:state_check" | "ha:aggregate" | "ha:last_scene" | "ha:next_scene" => {
                let mut args = a.to_vec();
                if let Some(first) = args.first_mut() {
                    *first = self.expand_entities(std::mem::take(first)).await?;
                }
                let lookup = |e: &str| self.ha.state(e);
                value::ha_builtin(name, &args, &lookup)?
            }
            _ => {
                let lookup = |e: &str| self.ha.state(e);
                value::ha_builtin(name, a, &lookup)?
            }
        })
    }

    /// Entity ids from an entity, a comma list, a list, or area/floor/label/device targets
    /// (`{"area_id": [...]}`, also inside lists); other values pass through unchanged.
    async fn expand_entities(&self, v: Value) -> Result<Value, String> {
        const KINDS: [(&str, &str); 4] = [
            ("area_id", "area_entities"),
            ("floor_id", "floor_entities"),
            ("label_id", "label_entities"),
            ("device_id", "device_entities"),
        ];
        // Only what area/floor/label/device blocks make: nothing but target keys, and at least
        // one that isn't entity_id. Other data (states, records) is left alone.
        let is_target = |m: &Map<String, Value>| {
            !m.is_empty()
                && m.keys().all(|k| k == "entity_id" || KINDS.iter().any(|(t, _)| t == k))
                && m.keys().any(|k| KINDS.iter().any(|(t, _)| t == k))
        };
        let has_target = match &v {
            Value::Object(m) => is_target(m),
            // A list mixing entity ids and area/… blocks.
            Value::Array(items) => {
                items.iter().any(|i| i.as_object().is_some_and(is_target))
                    && items.iter().all(|i| i.is_string() || i.as_object().is_some_and(is_target))
            }
            _ => false,
        };
        if !has_target {
            return Ok(v);
        }
        let target = puzzlebox_ir::normalize_target(&v)?;
        let mut parts = vec![];
        for (key, func) in KINDS {
            for id in value::entity_list(target.get(key).unwrap_or(&Value::Null)) {
                parts.push(format!("{func}('{}')", id.replace(['\\', '\''], "")));
            }
        }
        let mut ids = value::entity_list(target.get("entity_id").unwrap_or(&Value::Null));
        if !parts.is_empty() {
            let template = format!("{{{{ ({}) | unique | list }}}}", parts.join(" + "));
            let rendered = self.ha.render_template(&template, json!({})).await?;
            for id in value::entity_list(&rendered) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        Ok(json!(ids))
    }

    async fn call(&self, frame: &Frame, id: &str, name: &str, args: &[Expr]) -> R<Value> {
        let program = self.program.clone();
        let f = program
            .functions
            .get(name)
            .ok_or_else(|| RunError::at(id, format!("unknown function '{name}'")))?;
        if frame.depth >= MAX_CALL_DEPTH {
            return Err(RunError::at(id, "maximum recursion depth reached"));
        }
        let mut params = Map::new();
        for (i, p) in f.params.iter().enumerate() {
            let v = match args.get(i) {
                Some(a) => self.eval(frame, a).await?,
                None => Value::Null,
            };
            params.insert(p.clone(), v);
        }
        let inner = Frame { params: Arc::new(Mutex::new(params)), depth: frame.depth + 1, repeat: None };
        if let Flow::Return(v) = self.exec_block(&inner, &f.body).await? {
            return Ok(v);
        }
        match &f.returns {
            Some(r) => self.eval(&inner, r).await,
            None => Ok(Value::Null),
        }
    }
}

/// When a "wait until" is done: its condition held for `hold`, or `deadline` passed.
struct Until {
    hold: Duration,
    deadline: Option<Instant>,
}

/// HA turns any problem with the templates of an action's target or data into a
/// `HomeAssistantError`, which `continue_on_error` skips: here, an error rendering one of the
/// templates in `expr`...
fn template_failed(mut e: RunError, expr: &Expr) -> RunError {
    fn has(v: &Value, id: &str) -> bool {
        match v {
            Value::Object(m) => {
                (m.get("id").and_then(Value::as_str) == Some(id)
                    && m.get("kind").and_then(|k| k.get("op")).and_then(Value::as_str) == Some("template"))
                    || m.values().any(|x| has(x, id))
            }
            Value::Array(a) => a.iter().any(|x| has(x, id)),
            _ => false,
        }
    }
    // HA's answer, not a lost connection.
    let from_ha = e.message.starts_with("Home Assistant returned an error (");
    if let Some(b) = &e.block_id
        && from_ha
        && e.kind == ErrorKind::Error
        && has(&serde_json::to_value(expr).unwrap_or_default(), b)
    {
        e.on_error = OnError::Skip;
    }
    e
}

/// ...or a template that gave something that isn't a target (or data).
fn rendered_wrong(mut e: RunError, expr: &Expr) -> RunError {
    if matches!(expr.kind, ExprKind::Template { .. }) {
        e.on_error = OnError::Skip;
    }
    e
}

/// Whether "carry on if this fails" around `body` skips `e`.
fn skips(e: &RunError, body: &[Stmt]) -> bool {
    match e.on_error {
        OnError::Stop => false,
        OnError::Skip => true,
        // Only when the flag is on the call itself: the one block inside.
        OnError::SkipIfCall => matches!(body, [only] if e.block_id.as_deref() == Some(only.id.as_str())),
    }
}

/// HA's `wait.remaining`: the time left, 0 after a timeout, null without a timeout.
fn remaining(timeout: Option<f64>, started: Instant, completed: bool) -> Value {
    match timeout {
        None => Value::Null,
        Some(_) if !completed => json!(0.0),
        Some(t) => json!((t - started.elapsed().as_secs_f64()).max(0.0)),
    }
}

/// How HA reads a template condition's result (`cv.boolean`, anything else is false).
fn ha_bool(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => {
            matches!(s.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on" | "enable")
        }
        _ => false,
    }
}

fn is_random_index(args: &[Expr]) -> bool {
    matches!(args.get(1), Some(Expr { kind: ExprKind::Builtin { name, .. }, .. }) if name == "random_index")
}

/// Puts back a local variable that a filter/map block borrowed for its items.
fn restore(locals: &Mutex<Map<String, Value>>, var: &str, saved: Option<Value>) {
    let mut l = lock(locals);
    match saved {
        Some(v) => l.insert(var.to_owned(), v),
        None => l.remove(var),
    };
}
