//! Async tree-walking interpreter over the IR.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures_util::future::{BoxFuture, join_all};
use futures_util::stream::{StreamExt, select_all};
use puzzlebox_ir::*;
use puzzlebox_storage::Store;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::ha::HaApi;
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
}

impl RunError {
    fn at(block: &str, message: impl Into<String>) -> Self {
        Self { block_id: Some(block.to_owned()), message: message.into(), kind: ErrorKind::Error }
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
}

#[derive(Debug, Default, Serialize)]
pub struct Trace {
    pub steps: Vec<TraceEntry>,
    pub truncated: bool,
}

/// Engine callbacks: live events and the debugger's pause point.
#[async_trait]
pub trait RunHooks: Send + Sync {
    async fn on_step(&self, _ctx: &RunCtx, _block: &str) {}
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
    ) -> Result<(), String> {
        Err("timers are not available".into())
    }
    fn timer_cancel(&self, _ctx: &RunCtx, _name: &str) {}
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
    params: Mutex<Map<String, Value>>,
    depth: usize,
}

impl Frame {
    pub fn root() -> Self {
        Self { params: Mutex::new(Map::new()), depth: 0 }
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
                    trace.steps.push(TraceEntry { block: b.clone(), t, error: Some(e.message.clone()) });
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
            trace.steps.push(TraceEntry { block: block.to_owned(), t, error: None });
        } else {
            trace.truncated = true;
        }
        Ok(())
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
                for (cond, body) in branches {
                    if truthy(&self.eval(frame, cond).await?) {
                        return self.exec_block(frame, body).await;
                    }
                }
                if let Some(body) = otherwise {
                    return self.exec_block(frame, body).await;
                }
            }
            StmtKind::While { cond, body, until } => loop {
                let c = truthy(&self.eval(frame, cond).await?);
                if c == *until {
                    break;
                }
                self.step(id)?;
                match self.exec_block(frame, body).await? {
                    Flow::Break => break,
                    Flow::Return(v) => return Ok(Flow::Return(v)),
                    _ => {}
                }
            },
            StmtKind::Repeat { times, body } => {
                let n = to_num(&self.eval(frame, times).await?).at(id)?.max(0.0) as u64;
                for _ in 0..n {
                    self.step(id)?;
                    match self.exec_block(frame, body).await? {
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
                let mut i = from;
                while (step > 0.0 && i <= to) || (step < 0.0 && i >= to) {
                    self.step(id)?;
                    self.set_var(frame, var, VarScope::Local, value::num(i)).await;
                    match self.exec_block(frame, body).await? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        _ => {}
                    }
                    i += step;
                }
            }
            StmtKind::ForEach { var, list, body } => {
                let items = match self.eval(frame, list).await? {
                    Value::Array(a) => a,
                    Value::Object(o) => o.into_iter().map(|(k, _)| json!(k)).collect(),
                    Value::Null => vec![],
                    other => return Err(RunError::at(id, format!("cannot loop over {other}"))),
                };
                for item in items {
                    self.step(id)?;
                    self.set_var(frame, var, VarScope::Local, item).await;
                    match self.exec_block(frame, body).await? {
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
                return Err(RunError {
                    block_id: Some(id.into()),
                    message: "stopped".into(),
                    kind: ErrorKind::Stopped,
                });
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
                    Some(t) => Some(value::to_target(self.eval(frame, t).await?).at(id)?),
                    None => None,
                };
                let data = match data {
                    Some(d) => match self.eval(frame, d).await? {
                        Value::Null => None,
                        v @ Value::Object(_) => Some(v),
                        other => {
                            return Err(RunError::at(
                                id,
                                format!("action data must be a dictionary, got {other}"),
                            ));
                        }
                    },
                    None => None,
                };
                let res = self.ha.call_service(service, data, target).await.at(id)?;
                if let Some(var) = response_var {
                    let resp = res.get("response").cloned().unwrap_or(Value::Null);
                    self.set_var(frame, var, VarScope::Local, resp).await;
                }
            }
            StmtKind::HaScript { sequence, args } => {
                let sequence = self.substitute(frame, sequence, args).await?;
                self.ha.execute_script(sequence, self.ha_vars(frame)).await.at(id)?;
            }
            StmtKind::Wait { seconds } => {
                let s = to_num(&self.eval(frame, seconds).await?).at(id)?;
                if s > 0.0 {
                    tokio::time::sleep(Duration::from_secs_f64(s)).await;
                }
            }
            StmtKind::WaitForTrigger { triggers, timeout, result_var } => {
                let timeout = match timeout {
                    Some(t) => Some(to_num(&self.eval(frame, t).await?).at(id)?).filter(|t| *t > 0.0),
                    None => None,
                };
                let mut streams = Vec::new();
                for t in triggers {
                    streams.push(self.ha.subscribe_trigger(t.clone()).await.at(id)?);
                }
                let mut all = select_all(streams);
                let fired = match timeout {
                    Some(t) => {
                        tokio::time::timeout(Duration::from_secs_f64(t), all.next()).await.ok().flatten()
                    }
                    None => all.next().await,
                };
                let var = result_var.clone().unwrap_or_else(|| "wait".into());
                self.set_var(frame, &var, VarScope::Local, json!({"trigger": fired})).await;
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
            StmtKind::TimerStart { name, seconds, if_running, handler } => {
                let secs = to_num(&self.eval(frame, seconds).await?).at(id)?.max(0.0);
                self.hooks.timer_start(self, name, secs, *if_running, handler).at(id)?;
            }
            StmtKind::TimerCancel { name } => self.hooks.timer_cancel(self, name),
            StmtKind::WaitUntil { cond, hold, timeout } => {
                let hold = match hold {
                    Some(h) => to_num(&self.eval(frame, h).await?).at(id)?.max(0.0),
                    None => 0.0,
                };
                let deadline = match timeout {
                    Some(t) => {
                        let t = to_num(&self.eval(frame, t).await?).at(id)?;
                        (t > 0.0).then(|| Instant::now() + Duration::from_secs_f64(t))
                    }
                    None => None,
                };
                let mut true_since: Option<Instant> = None;
                let completed = loop {
                    if truthy(&self.eval(frame, cond).await?) {
                        let since = *true_since.get_or_insert_with(Instant::now);
                        if since.elapsed().as_secs_f64() >= hold {
                            break true;
                        }
                    } else {
                        true_since = None;
                    }
                    if deadline.is_some_and(|d| Instant::now() >= d) {
                        break false;
                    }
                    // Conditions read HA's state cache, which updates on every state change;
                    // polling once a second is cheap and catches any expression.
                    tokio::time::sleep(Duration::from_secs(1)).await;
                };
                self.set_var(frame, "wait", VarScope::Local, json!({"completed": completed})).await;
            }
        }
        Ok(Flow::Normal)
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
                    let items = match self.eval(frame, list).await? {
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
                    json!(self.ha.test_condition(config, self.ha_vars(frame)).await.at(id)?)
                }
                ExprKind::Template { template } => {
                    self.ha.render_template(template, self.ha_vars(frame)).await.at(id)?
                }
                ExprKind::TriggerData { path } => {
                    let mut cur = &self.trigger;
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
            _ => {
                let lookup = |e: &str| self.ha.state(e);
                value::ha_builtin(name, a, &lookup)?
            }
        })
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
        let inner = Frame { params: Mutex::new(params), depth: frame.depth + 1 };
        if let Flow::Return(v) = self.exec_block(&inner, &f.body).await? {
            return Ok(v);
        }
        match &f.returns {
            Some(r) => self.eval(&inner, r).await,
            None => Ok(Value::Null),
        }
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
