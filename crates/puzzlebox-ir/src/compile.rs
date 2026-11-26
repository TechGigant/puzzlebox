//! Blockly workspace JSON (serialization API format) → IR.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::ir::*;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompileError {
    pub block_id: Option<BlockId>,
    pub message: String,
}

/// Lowers block types the core compiler does not know (autogen, packs, plugins).
/// Returning `None` means "not mine".
pub trait BlockLowerer: Send + Sync {
    fn lower_stmt(&self, _b: Block<'_>, _c: &mut Compiler<'_>) -> Option<StmtKind> {
        None
    }
    fn lower_expr(&self, _b: Block<'_>, _c: &mut Compiler<'_>) -> Option<ExprKind> {
        None
    }
    fn lower_trigger(&self, _b: Block<'_>, _c: &mut Compiler<'_>) -> Option<Trigger> {
        None
    }
}

pub struct NoExternals;
impl BlockLowerer for NoExternals {}

#[derive(Clone, Copy)]
pub struct Block<'a>(pub &'a Value);

impl<'a> Block<'a> {
    pub fn id(&self) -> &'a str {
        self.0.get("id").and_then(Value::as_str).unwrap_or("")
    }
    pub fn ty(&self) -> &'a str {
        self.0.get("type").and_then(Value::as_str).unwrap_or("")
    }
    pub fn enabled(&self) -> bool {
        let disabled_flag = self.0.get("enabled").and_then(Value::as_bool) == Some(false);
        let reasons = self.0.get("disabledReasons").and_then(Value::as_array).is_some_and(|r| !r.is_empty());
        !disabled_flag && !reasons
    }
    pub fn field(&self, name: &str) -> Option<&'a Value> {
        self.0.get("fields")?.get(name)
    }
    pub fn field_str(&self, name: &str) -> Option<&'a str> {
        self.field(name).and_then(Value::as_str)
    }
    pub fn field_f64(&self, name: &str) -> Option<f64> {
        match self.field(name)? {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        }
    }
    pub fn input(&self, name: &str) -> Option<Block<'a>> {
        let input = self.0.get("inputs")?.get(name)?;
        input.get("block").or_else(|| input.get("shadow")).map(Block)
    }
    pub fn next(&self) -> Option<Block<'a>> {
        let n = self.0.get("next")?;
        n.get("block").or_else(|| n.get("shadow")).map(Block)
    }
    pub fn extra(&self, key: &str) -> Option<&'a Value> {
        self.0.get("extraState")?.get(key)
    }
    pub fn extra_count(&self, key: &str) -> usize {
        self.extra(key).and_then(Value::as_u64).unwrap_or(0) as usize
    }
}

pub struct Compiler<'a> {
    vars: HashMap<String, (String, VarScope)>,
    errors: Vec<CompileError>,
    ext: &'a dyn BlockLowerer,
    persistent: Vec<String>,
}

pub fn compile(workspace: &Value, ext: &dyn BlockLowerer) -> Result<Program, Vec<CompileError>> {
    let mut c = Compiler { vars: HashMap::new(), errors: Vec::new(), ext, persistent: Vec::new() };
    for v in workspace.get("variables").and_then(Value::as_array).into_iter().flatten() {
        let (Some(id), Some(name)) =
            (v.get("id").and_then(Value::as_str), v.get("name").and_then(Value::as_str))
        else {
            continue;
        };
        let scope = match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "Program" => VarScope::Program,
            "Global" => VarScope::Global,
            _ => VarScope::Local,
        };
        if scope == VarScope::Program {
            c.persistent.push(name.to_owned());
        }
        c.vars.insert(id.to_owned(), (name.to_owned(), scope));
    }

    let mut program = Program::default();
    let tops = workspace.pointer("/blocks/blocks").and_then(Value::as_array).cloned().unwrap_or_default();
    for top in &tops {
        let b = Block(top);
        if !b.enabled() {
            continue;
        }
        match b.ty() {
            "ha_when" => {
                let triggers = c.trigger_chain(b.input("TRIGGERS"));
                if triggers.is_empty() {
                    c.err(&b, "add at least one trigger");
                }
                let body = c.stmt_chain(b.input("DO"));
                program.handlers.push(Handler {
                    id: b.id().into(),
                    kind: HandlerKind::Triggers { triggers },
                    body,
                });
            }
            "ha_on_run" => {
                // Only runs on demand (Run button, `puzzlebox.run_program`).
                let body = c.stmt_chain(b.input("DO"));
                program.handlers.push(Handler {
                    id: b.id().into(),
                    kind: HandlerKind::Triggers { triggers: vec![] },
                    body,
                });
            }
            "ha_on_start" => {
                let body = c.stmt_chain(b.input("DO"));
                program.handlers.push(Handler { id: b.id().into(), kind: HandlerKind::OnStart, body });
            }
            "procedures_defnoreturn" | "procedures_defreturn" => {
                let name = b.field_str("NAME").unwrap_or("").to_owned();
                let params = b
                    .extra("params")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|p| p.get("name").and_then(Value::as_str).map(str::to_owned))
                    .collect();
                let body = c.stmt_chain(b.input("STACK"));
                let returns = if b.ty() == "procedures_defreturn" { c.value_opt(&b, "RETURN") } else { None };
                if program.functions.contains_key(&name) {
                    c.err(&b, format!("duplicate function '{name}'"));
                }
                program
                    .functions
                    .insert(name.clone(), Function { id: b.id().into(), name, params, body, returns });
            }
            // Loose value/statement blocks are ignored like Blockly generators do, but an
            // unknown top-level type is most likely an entry point this version can't run.
            ty if ty.starts_with("ha_on") || ty.starts_with("ha_when") => {
                c.err(&b, format!("unknown entry block '{ty}'"));
            }
            _ => {}
        }
    }
    program.persistent = std::mem::take(&mut c.persistent);

    for h in &program.handlers {
        check_calls(&h.body, &program.functions, &mut c);
    }
    for f in program.functions.values() {
        check_calls(&f.body, &program.functions, &mut c);
    }

    if c.errors.is_empty() { Ok(program) } else { Err(c.errors) }
}

fn check_calls(body: &[Stmt], fns: &BTreeMap<String, Function>, c: &mut Compiler<'_>) {
    fn expr(e: &Expr, fns: &BTreeMap<String, Function>, errs: &mut Vec<CompileError>) {
        if let ExprKind::Call { function, .. } = &e.kind
            && !fns.contains_key(function)
        {
            errs.push(CompileError {
                block_id: Some(e.id.clone()),
                message: format!("unknown function '{function}'"),
            });
        }
    }
    for s in body {
        match &s.kind {
            StmtKind::Expr { expr: e } => expr(e, fns, &mut c.errors),
            StmtKind::If { branches, otherwise } => {
                for (_, b) in branches {
                    check_calls(b, fns, c);
                }
                if let Some(o) = otherwise {
                    check_calls(o, fns, c);
                }
            }
            StmtKind::While { body, .. }
            | StmtKind::Repeat { body, .. }
            | StmtKind::For { body, .. }
            | StmtKind::ForEach { body, .. } => check_calls(body, fns, c),
            StmtKind::TryCatch { body, handler, .. } => {
                check_calls(body, fns, c);
                check_calls(handler, fns, c);
            }
            StmtKind::Parallel { branches } => {
                for b in branches {
                    check_calls(b, fns, c);
                }
            }
            _ => {}
        }
    }
}

/// Plugin id the runtime routes to its MQTT client.
pub const BUILTIN_MQTT: &str = "__mqtt";

/// Whole numbers stay integers so they reach HA as `40`, not `40.0`.
fn number(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() < 9.0e15 { json!(f as i64) } else { json!(f) }
}

fn parse_config(text: &str) -> Result<Value, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("configuration is empty".into());
    }
    serde_yaml::from_str::<Value>(text).map_err(|e| format!("invalid YAML/JSON: {e}"))
}

fn duration_or_none(seconds: Option<f64>) -> Option<Value> {
    match seconds {
        Some(s) if s > 0.0 => {
            let s = s as u64;
            Some(json!({"hours": s / 3600, "minutes": (s / 60) % 60, "seconds": s % 60}))
        }
        _ => None,
    }
}

impl<'a> Compiler<'a> {
    pub fn err(&mut self, b: &Block<'_>, msg: impl Into<String>) {
        self.errors.push(CompileError { block_id: Some(b.id().into()), message: msg.into() });
    }

    pub fn var(&mut self, b: &Block<'_>, field: &str) -> (String, VarScope) {
        let id = b.field(field).and_then(|f| f.get("id")).and_then(Value::as_str);
        match id.and_then(|id| self.vars.get(id)) {
            Some(v) => v.clone(),
            None => {
                self.err(b, "unknown variable");
                (String::new(), VarScope::Local)
            }
        }
    }

    pub fn required_str(&mut self, b: &Block<'_>, field: &str) -> String {
        match b.field_str(field).map(str::trim) {
            Some(s) if !s.is_empty() => s.to_owned(),
            _ => {
                self.err(b, format!("{} is required", field.to_lowercase()));
                String::new()
            }
        }
    }

    pub fn value(&mut self, b: &Block<'_>, input: &str) -> Expr {
        match b.input(input) {
            Some(inner) => self.expr(inner),
            None => {
                self.err(b, format!("missing input {}", input.to_lowercase()));
                Expr::lit(b.id(), Value::Null)
            }
        }
    }

    pub fn value_opt(&mut self, b: &Block<'_>, input: &str) -> Option<Expr> {
        b.input(input).map(|inner| self.expr(inner))
    }

    pub fn stmt_chain(&mut self, first: Option<Block<'_>>) -> Vec<Stmt> {
        let mut out = Vec::new();
        let mut cur = first;
        while let Some(b) = cur {
            if b.enabled()
                && let Some(kind) = self.stmt(b)
            {
                out.push(Stmt { id: b.id().into(), kind });
            }
            cur = b.next();
        }
        out
    }

    pub fn trigger_chain(&mut self, first: Option<Block<'_>>) -> Vec<Trigger> {
        let mut out = Vec::new();
        let mut cur = first;
        while let Some(b) = cur {
            if b.enabled()
                && let Some(t) = self.trigger(b)
            {
                out.push(t);
            }
            cur = b.next();
        }
        out
    }

    /// Trigger chain as raw HA configs, for `wait_for_trigger`.
    fn ha_trigger_configs(&mut self, first: Option<Block<'_>>) -> Vec<Value> {
        self.trigger_chain(first)
            .into_iter()
            .filter_map(|t| match t {
                Trigger::Ha { config, .. } => Some(config),
                other => {
                    self.errors.push(CompileError {
                        block_id: Some(other.block_id().into()),
                        message: "only Home Assistant triggers can be waited for".into(),
                    });
                    None
                }
            })
            .collect()
    }

    fn trigger(&mut self, b: Block<'_>) -> Option<Trigger> {
        let id: BlockId = b.id().into();
        let ha = |config: Value| Some(Trigger::Ha { id: id.clone(), config });
        match b.ty() {
            "ha_trigger_state" => {
                let mut cfg = Map::new();
                cfg.insert("trigger".into(), json!("state"));
                cfg.insert("entity_id".into(), json!(self.required_str(&b, "ENTITY")));
                for (field, key) in [("FROM", "from"), ("TO", "to")] {
                    if let Some(v) = b.field_str(field).map(str::trim).filter(|s| !s.is_empty()) {
                        cfg.insert(key.into(), json!(v));
                    }
                }
                if let Some(d) = duration_or_none(b.field_f64("FOR")) {
                    cfg.insert("for".into(), d);
                }
                ha(Value::Object(cfg))
            }
            "ha_trigger_numeric" => {
                let mut cfg =
                    json!({"trigger": "numeric_state", "entity_id": self.required_str(&b, "ENTITY")});
                // A number, or an entity id (e.g. input_number.limit), as HA accepts both.
                for (field, key) in [("ABOVE", "above"), ("BELOW", "below")] {
                    match b.field_str(field).map(str::trim).filter(|s| !s.is_empty()) {
                        Some(raw) => match raw.parse::<f64>() {
                            Ok(n) => cfg[key] = number(n),
                            Err(_) if raw.contains('.') => cfg[key] = json!(raw),
                            Err(_) => self.err(&b, format!("'{}' must be a number or an entity id", key)),
                        },
                        None => {
                            if let Some(n) = b.field_f64(field) {
                                cfg[key] = number(n);
                            }
                        }
                    }
                }
                if cfg.get("above").is_none() && cfg.get("below").is_none() {
                    self.err(&b, "set 'above' and/or 'below'");
                }
                ha(cfg)
            }
            "ha_trigger_time" => ha(json!({"trigger": "time", "at": self.required_str(&b, "AT")})),
            "ha_trigger_event" => {
                ha(json!({"trigger": "event", "event_type": self.required_str(&b, "EVENT_TYPE")}))
            }
            "ha_trigger_sun" => {
                let mut cfg = json!({"trigger": "sun", "event": b.field_str("EVENT").unwrap_or("sunset")});
                if let Some(off) = b.field_f64("OFFSET").filter(|o| *o != 0.0) {
                    let sign = if off < 0.0 { "-" } else { "" };
                    let s = off.abs() as u64;
                    cfg["offset"] =
                        json!(format!("{sign}{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60));
                }
                ha(cfg)
            }
            "ha_trigger_ha_start" => ha(json!({"trigger": "homeassistant", "event": "start"})),
            "ha_trigger_raw" => match parse_config(b.field_str("CONFIG").unwrap_or("")) {
                Ok(v) => ha(v),
                Err(e) => {
                    self.err(&b, e);
                    None
                }
            },
            "engine_trigger_cron" => Some(Trigger::Cron { id, expr: self.required_str(&b, "CRON") }),
            "engine_trigger_webhook" => {
                let methods = b.field_str("METHOD").unwrap_or("POST");
                Some(Trigger::Webhook {
                    id,
                    webhook_id: self.required_str(&b, "WEBHOOK_ID"),
                    methods: if methods == "ANY" { vec![] } else { vec![methods.to_owned()] },
                })
            }
            "engine_trigger_mqtt" => Some(Trigger::Mqtt { id, topic: self.required_str(&b, "TOPIC") }),
            _ => {
                let ext = self.ext;
                let t = ext.lower_trigger(b, self);
                if t.is_none() {
                    self.err(&b, format!("'{}' is not a trigger", b.ty()));
                }
                t
            }
        }
    }

    fn stmt(&mut self, b: Block<'_>) -> Option<StmtKind> {
        Some(match b.ty() {
            "controls_if" => {
                let mut branches = Vec::new();
                for i in 0..=b.extra_count("elseIfCount") {
                    let cond = self.value(&b, &format!("IF{i}"));
                    let body = self.stmt_chain(b.input(&format!("DO{i}")));
                    branches.push((cond, body));
                }
                let otherwise = (b.extra("hasElse").and_then(Value::as_bool) == Some(true))
                    .then(|| self.stmt_chain(b.input("ELSE")));
                StmtKind::If { branches, otherwise }
            }
            "controls_repeat_ext" => {
                StmtKind::Repeat { times: self.value(&b, "TIMES"), body: self.stmt_chain(b.input("DO")) }
            }
            "controls_repeat" => StmtKind::Repeat {
                times: Expr::lit(b.id(), json!(b.field_f64("TIMES").unwrap_or(0.0))),
                body: self.stmt_chain(b.input("DO")),
            },
            "controls_whileUntil" => StmtKind::While {
                cond: self.value(&b, "BOOL"),
                body: self.stmt_chain(b.input("DO")),
                until: b.field_str("MODE") == Some("UNTIL"),
            },
            "controls_for" => StmtKind::For {
                var: self.var(&b, "VAR").0,
                from: self.value(&b, "FROM"),
                to: self.value(&b, "TO"),
                by: self.value(&b, "BY"),
                body: self.stmt_chain(b.input("DO")),
            },
            "controls_forEach" => StmtKind::ForEach {
                var: self.var(&b, "VAR").0,
                list: self.value(&b, "LIST"),
                body: self.stmt_chain(b.input("DO")),
            },
            "controls_flow_statements" => match b.field_str("FLOW") {
                Some("CONTINUE") => StmtKind::Continue,
                _ => StmtKind::Break,
            },
            "procedures_ifreturn" => {
                let cond = self.value(&b, "CONDITION");
                let value = self.value_opt(&b, "VALUE");
                StmtKind::If {
                    branches: vec![(
                        cond,
                        vec![Stmt { id: b.id().into(), kind: StmtKind::Return { value } }],
                    )],
                    otherwise: None,
                }
            }
            "procedures_callnoreturn" | "procedures_callreturn" => {
                StmtKind::Expr { expr: Expr { id: b.id().into(), kind: self.call(&b) } }
            }
            "variables_set" | "variables_set_dynamic" => {
                let (name, scope) = self.var(&b, "VAR");
                StmtKind::Set { name, scope, value: self.value(&b, "VALUE") }
            }
            "math_change" => {
                let (name, scope) = self.var(&b, "VAR");
                let delta = self.value(&b, "DELTA");
                let current = Expr { id: b.id().into(), kind: ExprKind::Var { name: name.clone(), scope } };
                let sum =
                    ExprKind::Binary { bin: BinOp::Add, left: Box::new(current), right: Box::new(delta) };
                StmtKind::Set { name, scope, value: Expr { id: b.id().into(), kind: sum } }
            }
            "text_append" => {
                let (name, scope) = self.var(&b, "VAR");
                let text = self.value(&b, "TEXT");
                let current = Expr { id: b.id().into(), kind: ExprKind::Var { name: name.clone(), scope } };
                let joined = ExprKind::Builtin { name: "text_join".into(), args: vec![current, text] };
                StmtKind::Set { name, scope, value: Expr { id: b.id().into(), kind: joined } }
            }
            "lists_setIndex" => {
                let Some(lb) = b.input("LIST") else {
                    self.err(&b, "missing list");
                    return None;
                };
                if lb.ty() != "variables_get" {
                    self.err(&b, "the list must be a variable");
                    return None;
                }
                let (name, scope) = self.var(&lb, "VAR");
                let key = self.list_index(&b);
                StmtKind::SetItem { name, scope, key, value: self.value(&b, "TO") }
            }
            "dict_set" => {
                let (name, scope) = self.var(&b, "VAR");
                StmtKind::SetItem { name, scope, key: self.value(&b, "KEY"), value: self.value(&b, "VALUE") }
            }
            "control_try" => StmtKind::TryCatch {
                body: self.stmt_chain(b.input("TRY")),
                error_var: self.var(&b, "ERROR").0,
                handler: self.stmt_chain(b.input("CATCH")),
            },
            "control_throw" => StmtKind::Throw { message: self.value(&b, "MESSAGE") },
            "control_parallel" => {
                let mut branches = Vec::new();
                let mut cur = b.input("BRANCHES");
                while let Some(br) = cur {
                    if br.enabled() && br.ty() == "control_branch" {
                        branches.push(self.stmt_chain(br.input("DO")));
                    }
                    cur = br.next();
                }
                StmtKind::Parallel { branches }
            }
            "control_stop" => StmtKind::Stop,
            "ha_call_service" => {
                let service = self.required_str(&b, "SERVICE");
                if !service.is_empty() && !service.contains('.') {
                    self.err(&b, "action must look like domain.service");
                }
                let target = self.value_opt(&b, "TARGET");
                let data = self.value_opt(&b, "DATA");
                StmtKind::CallService { service, target, data, response_var: None }
            }
            "ha_action_raw" => match parse_config(b.field_str("CONFIG").unwrap_or("")) {
                Ok(v) => StmtKind::HaScript {
                    sequence: if v.is_array() { v } else { json!([v]) },
                    args: BTreeMap::new(),
                },
                Err(e) => {
                    self.err(&b, e);
                    return None;
                }
            },
            "ha_wait" => StmtKind::Wait { seconds: self.value(&b, "SECONDS") },
            "ha_wait_for_trigger" => {
                let triggers = self.ha_trigger_configs(b.input("TRIGGERS"));
                if triggers.is_empty() {
                    self.err(&b, "add at least one trigger");
                }
                StmtKind::WaitForTrigger {
                    triggers,
                    timeout: self.value_opt(&b, "TIMEOUT"),
                    result_var: None,
                }
            }
            "ha_log" => StmtKind::Log {
                level: match b.field_str("LEVEL") {
                    Some("debug") => LogLevel::Debug,
                    Some("warning") => LogLevel::Warning,
                    Some("error") => LogLevel::Error,
                    _ => LogLevel::Info,
                },
                message: self.value(&b, "MESSAGE"),
            },
            "ha_fire_event" => StmtKind::FireEvent {
                event_type: self.value(&b, "EVENT_TYPE"),
                data: self.value_opt(&b, "DATA"),
            },
            _ => {
                let ext = self.ext;
                if let Some(k) = ext.lower_stmt(b, self) {
                    return Some(k);
                }
                if b.0.get("output").is_some() || self.is_value_block(&b) {
                    self.err(&b, "this block produces a value; plug it into another block");
                } else {
                    self.err(&b, format!("unsupported block '{}'", b.ty()));
                }
                return None;
            }
        })
    }

    fn is_value_block(&self, b: &Block<'_>) -> bool {
        let t = b.ty();
        ["math_", "text", "logic_", "lists_", "dict_", "ha_state", "ha_attr", "ha_is_", "ha_template"]
            .iter()
            .any(|p| t.starts_with(p))
    }

    fn call(&mut self, b: &Block<'_>) -> ExprKind {
        let function = b.extra("name").and_then(Value::as_str).unwrap_or("").to_owned();
        let n = b.extra("params").and_then(Value::as_array).map_or(0, Vec::len);
        let args = (0..n).map(|i| self.value(b, &format!("ARG{i}"))).collect();
        ExprKind::Call { function, args }
    }

    fn builtin(&mut self, name: &str, args: Vec<Expr>) -> ExprKind {
        ExprKind::Builtin { name: name.into(), args }
    }

    /// Lowers Blockly's (WHERE, AT) list addressing into a 0-based index expression
    /// (negative = from end); `first`/`last`/`random` map to sentinel builtins.
    fn list_index(&mut self, b: &Block<'_>) -> Expr {
        let id = b.id();
        let at = |c: &mut Self| c.value_opt(b, "AT").unwrap_or_else(|| Expr::lit(id, json!(1)));
        let kind = match b.field_str("WHERE").unwrap_or("FROM_START") {
            "FIRST" => return Expr::lit(id, json!(0)),
            "LAST" => return Expr::lit(id, json!(-1)),
            "RANDOM" => ExprKind::Builtin { name: "random_index".into(), args: vec![] },
            "FROM_END" => {
                let a = at(self);
                ExprKind::Builtin { name: "neg".into(), args: vec![a] }
            }
            _ => {
                let a = at(self);
                ExprKind::Binary {
                    bin: BinOp::Sub,
                    left: Box::new(a),
                    right: Box::new(Expr::lit(id, json!(1))),
                }
            }
        };
        Expr { id: id.into(), kind }
    }

    pub fn expr(&mut self, b: Block<'_>) -> Expr {
        let kind = self.expr_kind(&b);
        Expr { id: b.id().into(), kind }
    }

    fn expr_kind(&mut self, b: &Block<'_>) -> ExprKind {
        let lit = |v: Value| ExprKind::Lit { value: v };
        match b.ty() {
            "logic_boolean" => lit(json!(b.field_str("BOOL") == Some("TRUE"))),
            "logic_null" => lit(Value::Null),
            "math_number" => lit(number(b.field_f64("NUM").unwrap_or(0.0))),
            "text" | "text_multiline" => lit(json!(b.field_str("TEXT").unwrap_or(""))),
            "logic_compare" => {
                let bin = match b.field_str("OP") {
                    Some("NEQ") => BinOp::Neq,
                    Some("LT") => BinOp::Lt,
                    Some("LTE") => BinOp::Lte,
                    Some("GT") => BinOp::Gt,
                    Some("GTE") => BinOp::Gte,
                    _ => BinOp::Eq,
                };
                self.binary(b, bin, "A", "B")
            }
            "logic_operation" => {
                let bin = if b.field_str("OP") == Some("OR") { BinOp::Or } else { BinOp::And };
                self.binary(b, bin, "A", "B")
            }
            "logic_negate" => ExprKind::Not { expr: Box::new(self.value(b, "BOOL")) },
            "logic_ternary" => ExprKind::Ternary {
                cond: Box::new(self.value(b, "IF")),
                then: Box::new(self.value(b, "THEN")),
                otherwise: Box::new(self.value(b, "ELSE")),
            },
            "math_arithmetic" => {
                let bin = match b.field_str("OP") {
                    Some("MINUS") => BinOp::Sub,
                    Some("MULTIPLY") => BinOp::Mul,
                    Some("DIVIDE") => BinOp::Div,
                    Some("POWER") => BinOp::Pow,
                    _ => BinOp::Add,
                };
                self.binary(b, bin, "A", "B")
            }
            "math_modulo" => self.binary(b, BinOp::Mod, "DIVIDEND", "DIVISOR"),
            "math_single" | "math_trig" | "math_round" => {
                let op = b.field_str("OP").unwrap_or("ABS").to_lowercase();
                let arg = self.value(b, "NUM");
                self.builtin(&format!("math_{op}"), vec![arg])
            }
            "math_constant" => lit(json!(match b.field_str("CONSTANT") {
                Some("E") => std::f64::consts::E,
                Some("GOLDEN_RATIO") => std::f64::consts::GOLDEN_RATIO,
                Some("SQRT2") => std::f64::consts::SQRT_2,
                Some("SQRT1_2") => std::f64::consts::FRAC_1_SQRT_2,
                Some("INFINITY") => f64::MAX,
                _ => std::f64::consts::PI,
            })),
            "math_number_property" => {
                let prop = b.field_str("PROPERTY").unwrap_or("EVEN").to_lowercase();
                let mut args = vec![self.value(b, "NUMBER_TO_CHECK")];
                if prop == "divisible_by" {
                    args.push(self.value(b, "DIVISOR"));
                }
                self.builtin(&format!("is_{prop}"), args)
            }
            "math_constrain" => {
                let args = vec![self.value(b, "VALUE"), self.value(b, "LOW"), self.value(b, "HIGH")];
                self.builtin("constrain", args)
            }
            "math_random_int" => {
                let args = vec![self.value(b, "FROM"), self.value(b, "TO")];
                self.builtin("random_int", args)
            }
            "math_random_float" => self.builtin("random_float", vec![]),
            "math_on_list" => {
                let op = b.field_str("OP").unwrap_or("SUM").to_lowercase();
                let args = vec![self.value(b, "LIST")];
                self.builtin(&format!("list_{op}"), args)
            }
            "text_join" | "lists_create_with" => {
                let items =
                    (0..b.extra_count("itemCount")).filter_map(|i| self.value_opt(b, &format!("ADD{i}")));
                let items: Vec<Expr> = items.collect();
                if b.ty() == "text_join" {
                    self.builtin("text_join", items)
                } else {
                    ExprKind::List { items }
                }
            }
            "text_length" | "lists_length" => {
                let a = self.value(b, "VALUE");
                self.builtin("length", vec![a])
            }
            "text_isEmpty" | "lists_isEmpty" => {
                let a = self.value(b, "VALUE");
                self.builtin("is_empty", vec![a])
            }
            "text_changeCase" => {
                let a = self.value(b, "TEXT");
                let case = b.field_str("CASE").unwrap_or("UPPERCASE").to_lowercase();
                self.builtin(&format!("text_{case}"), vec![a])
            }
            "text_trim" => {
                let a = self.value(b, "TEXT");
                self.builtin("text_trim", vec![a])
            }
            "text_indexOf" | "lists_indexOf" => {
                let args = vec![self.value(b, "VALUE"), self.value(b, "FIND")];
                let end = b.field_str("END") == Some("LAST");
                self.builtin(if end { "last_index_of" } else { "index_of" }, args)
            }
            "text_contains" => {
                let args = vec![self.value(b, "TEXT"), self.value(b, "FIND")];
                self.builtin("contains", args)
            }
            "text_to_number" => {
                let a = self.value(b, "TEXT");
                self.builtin("to_number", vec![a])
            }
            "to_text" => {
                let a = self.value(b, "VALUE");
                self.builtin("to_text", vec![a])
            }
            "lists_getIndex" => {
                let list = self.value(b, "VALUE");
                let idx = self.list_index(b);
                let name = match b.field_str("MODE") {
                    Some("GET") | None => "list_get",
                    Some(_) => {
                        self.err(b, "only 'get' is supported in expressions; use 'set' to modify lists");
                        "list_get"
                    }
                };
                self.builtin(name, vec![list, idx])
            }
            "lists_repeat" => {
                let args = vec![self.value(b, "ITEM"), self.value(b, "NUM")];
                self.builtin("list_repeat", args)
            }
            "lists_reverse" => {
                let a = self.value(b, "LIST");
                self.builtin("list_reverse", vec![a])
            }
            "lists_sort" => {
                let a = self.value(b, "LIST");
                let desc = b.field_str("DIRECTION") == Some("-1");
                self.builtin(if desc { "list_sort_desc" } else { "list_sort" }, vec![a])
            }
            "lists_split" => {
                let args = vec![self.value(b, "INPUT"), self.value(b, "DELIM")];
                self.builtin(
                    if b.field_str("MODE") == Some("JOIN") { "list_join" } else { "text_split" },
                    args,
                )
            }
            "dict_create" => {
                let mut entries = Vec::new();
                let mut cur = b.input("ITEMS");
                while let Some(pair) = cur {
                    if pair.enabled() && pair.ty() == "dict_pair" {
                        let key = pair.field_str("KEY").unwrap_or("").to_owned();
                        let v = self.value(&pair, "VALUE");
                        entries.push((key, v));
                    }
                    cur = pair.next();
                }
                ExprKind::Dict { entries }
            }
            "dict_get" => {
                let args = vec![self.value(b, "DICT"), self.value(b, "KEY")];
                self.builtin("dict_get", args)
            }
            "dict_keys" | "dict_values" => {
                let a = self.value(b, "DICT");
                self.builtin(b.ty(), vec![a])
            }
            "dict_has" => {
                let args = vec![self.value(b, "DICT"), self.value(b, "KEY")];
                self.builtin("dict_has", args)
            }
            "json_parse" | "json_stringify" => {
                let a = self.value(b, "VALUE");
                self.builtin(b.ty(), vec![a])
            }
            "time_now" => self.builtin("now", vec![]),
            "time_part" => {
                let part = b.field_str("PART").unwrap_or("hour").to_lowercase();
                self.builtin(&format!("time_{part}"), vec![])
            }
            "variables_get" | "variables_get_dynamic" => {
                let (name, scope) = self.var(b, "VAR");
                ExprKind::Var { name, scope }
            }
            "procedures_callreturn" => self.call(b),
            "ha_state" => ExprKind::State { entity: Box::new(self.entity_expr(b)), attribute: None },
            "ha_attribute" => {
                let attr = b.field_str("ATTRIBUTE").unwrap_or("").to_owned();
                ExprKind::State {
                    entity: Box::new(self.entity_expr(b)),
                    attribute: Some(Box::new(Expr::lit(b.id(), json!(attr)))),
                }
            }
            "ha_is_state" => {
                let entity = self.required_str(b, "ENTITY");
                let state = b.field_str("STATE").unwrap_or("").to_owned();
                ExprKind::Condition {
                    config: json!({"condition": "state", "entity_id": entity, "state": state}),
                    args: BTreeMap::new(),
                }
            }
            "ha_condition_raw" => match parse_config(b.field_str("CONFIG").unwrap_or("")) {
                Ok(v) => ExprKind::Condition { config: v, args: BTreeMap::new() },
                Err(e) => {
                    self.err(b, e);
                    lit(json!(false))
                }
            },
            "ha_template" => {
                ExprKind::Template { template: b.field_str("TEMPLATE").unwrap_or("").to_owned() }
            }
            "ha_trigger_data" => ExprKind::TriggerData { path: b.field_str("PATH").unwrap_or("").to_owned() },
            "ha_entity" => lit(json!(self.required_str(b, "ENTITY"))),
            "http_request" => ExprKind::Http {
                method: b.field_str("METHOD").unwrap_or("GET").to_owned(),
                url: Box::new(self.value(b, "URL")),
                body: self.value_opt(b, "BODY").map(Box::new),
                headers: self.value_opt(b, "HEADERS").map(Box::new),
            },
            _ => {
                let ext = self.ext;
                if let Some(k) = ext.lower_expr(*b, self) {
                    return k;
                }
                self.err(b, format!("'{}' does not produce a value", b.ty()));
                lit(Value::Null)
            }
        }
    }

    fn entity_expr(&mut self, b: &Block<'_>) -> Expr {
        match b.input("ENTITY") {
            Some(inner) => self.expr(inner),
            None => Expr::lit(b.id(), json!(self.required_str(b, "ENTITY"))),
        }
    }

    fn binary(&mut self, b: &Block<'_>, bin: BinOp, l: &str, r: &str) -> ExprKind {
        ExprKind::Binary { bin, left: Box::new(self.value(b, l)), right: Box::new(self.value(b, r)) }
    }
}
