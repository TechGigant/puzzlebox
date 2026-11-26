use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Blockly block id; every node keeps it so runtime events map back onto the canvas.
pub type BlockId = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Program {
    pub handlers: Vec<Handler>,
    pub functions: BTreeMap<String, Function>,
    /// Program-scoped persistent variables (survive restarts).
    pub persistent: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Handler {
    pub id: BlockId,
    pub kind: HandlerKind,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HandlerKind {
    Triggers {
        triggers: Vec<Trigger>,
    },
    OnStart,
    /// Body of a named timer; runs when the timer started by `StmtKind::TimerStart` fires.
    Timer {
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Trigger {
    /// Native HA trigger config, delegated via `subscribe_trigger`.
    Ha { id: BlockId, config: Value },
    /// Engine-native cron schedule (seconds-precision cron expression).
    Cron { id: BlockId, expr: String },
    /// Inbound webhook handled by the app server.
    Webhook { id: BlockId, webhook_id: String, methods: Vec<String> },
    /// MQTT topic subscription (wildcards allowed).
    Mqtt {
        id: BlockId,
        topic: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<MqttFilter>,
    },
}

/// Fires only for JSON payloads whose `key` matches (Zigbee2MQTT device messages).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MqttFilter {
    pub key: String,
    /// Fire only when the value equals this (loosely, so "on" matches "ON").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<Value>,
    /// Fire only when the value differs from the previous message.
    #[serde(default)]
    pub changed: bool,
}

impl Trigger {
    pub fn block_id(&self) -> &str {
        match self {
            Trigger::Ha { id, .. }
            | Trigger::Cron { id, .. }
            | Trigger::Webhook { id, .. }
            | Trigger::Mqtt { id, .. } => id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Function {
    pub id: BlockId,
    pub name: String,
    pub params: Vec<String>,
    pub body: Vec<Stmt>,
    pub returns: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stmt {
    pub id: BlockId,
    pub kind: StmtKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VarScope {
    Local,
    /// Persisted per program.
    Program,
    /// Persisted and shared between all programs.
    Global,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum StmtKind {
    Expr {
        expr: Expr,
    },
    Set {
        name: String,
        scope: VarScope,
        value: Expr,
    },
    If {
        branches: Vec<(Expr, Vec<Stmt>)>,
        otherwise: Option<Vec<Stmt>>,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
        until: bool,
    },
    Repeat {
        times: Expr,
        body: Vec<Stmt>,
    },
    For {
        var: String,
        from: Expr,
        to: Expr,
        by: Expr,
        body: Vec<Stmt>,
    },
    ForEach {
        var: String,
        list: Expr,
        body: Vec<Stmt>,
    },
    Break,
    Continue,
    Return {
        value: Option<Expr>,
    },
    Stop,
    TryCatch {
        body: Vec<Stmt>,
        error_var: String,
        handler: Vec<Stmt>,
    },
    Throw {
        message: Expr,
    },
    Parallel {
        branches: Vec<Vec<Stmt>>,
    },
    CallService {
        service: String,
        target: Option<Expr>,
        data: Option<Expr>,
        response_var: Option<String>,
    },
    /// Native HA action sequence via `execute_script`. `{"$arg": name}` placeholders
    /// are replaced by the evaluated `args`.
    HaScript {
        sequence: Value,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        args: BTreeMap<String, Expr>,
    },
    Wait {
        seconds: Expr,
    },
    WaitForTrigger {
        triggers: Vec<Value>,
        timeout: Option<Expr>,
        result_var: Option<String>,
    },
    Log {
        level: LogLevel,
        message: Expr,
    },
    FireEvent {
        event_type: Expr,
        data: Option<Expr>,
    },
    /// Dict/list mutation: `container[key] = value`.
    SetItem {
        name: String,
        scope: VarScope,
        key: Expr,
        value: Expr,
    },
    Plugin {
        plugin: String,
        block: String,
        args: BTreeMap<String, Expr>,
    },
    /// Starts the program's timer `name`; after `seconds` the handler with block id
    /// `handler` runs as its own run.
    TimerStart {
        name: String,
        seconds: Expr,
        if_running: TimerMode,
        handler: BlockId,
    },
    TimerCancel {
        name: String,
    },
    /// Waits until `cond` is true (and stays true for `hold` seconds), or `timeout` passes.
    /// Sets the local variable `wait` to `{"completed": bool}`.
    WaitUntil {
        cond: Expr,
        hold: Option<Expr>,
        timeout: Option<Expr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimerMode {
    /// Start over with the new duration (e.g. "off 5 min after the *last* motion").
    Restart,
    /// Keep the running timer.
    Keep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Expr {
    pub id: BlockId,
    pub kind: ExprKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Mod,
    Eq,
    Neq,
    Lt,
    Lte,
    Gt,
    Gte,
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ExprKind {
    Lit {
        value: Value,
    },
    Var {
        name: String,
        scope: VarScope,
    },
    Binary {
        bin: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Not {
        expr: Box<Expr>,
    },
    Ternary {
        cond: Box<Expr>,
        then: Box<Expr>,
        otherwise: Box<Expr>,
    },
    List {
        items: Vec<Expr>,
    },
    Dict {
        entries: Vec<(String, Expr)>,
    },
    /// Built-in pure function (math/text/list/dict/time helpers).
    Builtin {
        name: String,
        args: Vec<Expr>,
    },
    Call {
        function: String,
        args: Vec<Expr>,
    },
    /// Entity state string, or an attribute when `attribute` is set.
    State {
        entity: Box<Expr>,
        attribute: Option<Box<Expr>>,
    },
    /// Native HA condition evaluated via `test_condition`; placeholders as in `HaScript`.
    Condition {
        config: Value,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        args: BTreeMap<String, Expr>,
    },
    Template {
        template: String,
    },
    /// Dotted path into the current trigger payload.
    TriggerData {
        path: String,
    },
    Http {
        method: String,
        url: Box<Expr>,
        body: Option<Box<Expr>>,
        headers: Option<Box<Expr>>,
    },
    Plugin {
        plugin: String,
        block: String,
        args: BTreeMap<String, Expr>,
    },
}

impl Expr {
    pub fn lit(id: impl Into<BlockId>, value: Value) -> Self {
        Expr { id: id.into(), kind: ExprKind::Lit { value } }
    }
}

fn placeholder(v: &Value) -> Option<&str> {
    match v {
        Value::Object(map) if map.len() == 1 => map.get("$arg").and_then(Value::as_str),
        _ => None,
    }
}

/// Replaces `{"$arg": name}` placeholders. Object keys whose placeholder resolves to null are
/// dropped, so optional parameters disappear instead of being sent as null.
pub fn substitute_args(template: &Value, args: &BTreeMap<String, Value>) -> Value {
    if let Some(name) = placeholder(template) {
        return args.get(name).cloned().unwrap_or(Value::Null);
    }
    match template {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                let resolved = substitute_args(v, args);
                if placeholder(v).is_some() && resolved.is_null() {
                    continue;
                }
                out.insert(k.clone(), resolved);
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(|i| substitute_args(i, args)).collect()),
        other => other.clone(),
    }
}

pub fn arg_placeholder(name: &str) -> Value {
    serde_json::json!({ "$arg": name })
}
