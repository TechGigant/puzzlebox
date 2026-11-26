//! Blockly workspace JSON (serialization API format) → IR.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::ir::*;

#[path = "compile_helpers.rs"]
mod helpers;

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
    /// Timer bodies, compiled into their own handlers.
    timer_handlers: Vec<Handler>,
    /// Item variables of the enclosing "filter"/"map" blocks (innermost last).
    item_vars: Vec<String>,
    /// Number of loops around the block being compiled.
    loops: usize,
}

pub fn compile(workspace: &Value, ext: &dyn BlockLowerer) -> Result<Program, Vec<CompileError>> {
    let (program, errors) = compile_partial(workspace, ext);
    if errors.is_empty() { Ok(program) } else { Err(errors) }
}

/// Compiles everything that can be compiled: an entry block, loose stack or function with a
/// problem is left out (and reported), so one broken stack doesn't stop the rest.
pub fn compile_partial(workspace: &Value, ext: &dyn BlockLowerer) -> (Program, Vec<CompileError>) {
    let mut c = Compiler {
        vars: HashMap::new(),
        errors: Vec::new(),
        ext,
        persistent: Vec::new(),
        timer_handlers: Vec::new(),
        item_vars: Vec::new(),
        loops: 0,
    };
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
    let mut loose = Vec::new();
    let tops = workspace.pointer("/blocks/blocks").and_then(Value::as_array).cloned().unwrap_or_default();
    for top in &tops {
        let b = Block(top);
        if !b.enabled() {
            continue;
        }
        let (errors_before, handlers_before, timers_before, loose_before) =
            (c.errors.len(), program.handlers.len(), c.timer_handlers.len(), loose.len());
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
            // Loose statement stacks become handlers that only run on request. Anything that
            // doesn't compile as statements (values, triggers, half-built stacks) is skipped
            // without making the program invalid.
            _ => {
                let (errors, timers) = (c.errors.len(), c.timer_handlers.len());
                let body = c.stmt_chain(Some(b));
                if c.errors.len() == errors && !body.is_empty() {
                    loose.push(Handler { id: b.id().into(), kind: HandlerKind::Manual, body });
                } else {
                    c.errors.truncate(errors);
                    c.timer_handlers.truncate(timers);
                }
            }
        }
        if c.errors.len() > errors_before {
            program.handlers.truncate(handlers_before);
            c.timer_handlers.truncate(timers_before);
            loose.truncate(loose_before);
            if b.ty().starts_with("procedures_def") {
                program.functions.remove(b.field_str("NAME").unwrap_or(""));
            }
        }
    }
    program.handlers.append(&mut loose);
    program.persistent = std::mem::take(&mut c.persistent);
    program.handlers.append(&mut c.timer_handlers);
    // Every HA trigger reports its block id as `trigger.id`, for "triggered by".
    for h in &mut program.handlers {
        if let HandlerKind::Triggers { triggers } = &mut h.kind {
            for t in triggers {
                if let Trigger::Ha { id, config: Value::Object(cfg) } = t {
                    cfg.entry("id").or_insert_with(|| json!(id.clone()));
                }
            }
        }
    }

    // Functions calling missing (or broken) functions go first, then the handlers using them.
    loop {
        let fns = program.functions.clone();
        let broken: Vec<String> = fns
            .iter()
            .filter(|(_, f)| {
                let before = c.errors.len();
                check_calls(&f.body, &fns, &mut c);
                c.errors.len() > before
            })
            .map(|(n, _)| n.clone())
            .collect();
        if broken.is_empty() {
            break;
        }
        for n in broken {
            program.functions.remove(&n);
        }
    }
    let fns = program.functions.clone();
    program.handlers.retain(|h| {
        let before = c.errors.len();
        check_calls(&h.body, &fns, &mut c);
        c.errors.len() == before
    });

    (program, c.errors)
}

fn check_calls(body: &[Stmt], fns: &BTreeMap<String, Function>, c: &mut Compiler<'_>) {
    let mut missing = Vec::new();
    for s in body {
        stmt_exprs(s, &mut |e| {
            if let ExprKind::Call { function, .. } = &e.kind
                && !fns.contains_key(function)
            {
                missing.push(CompileError {
                    block_id: Some(e.id.clone()),
                    message: format!("the function \"{function}\" is missing or has a problem"),
                });
            }
        });
    }
    c.errors.extend(missing);
}

/// Calls `f` on every expression in a statement, nested statements and sub-expressions included.
fn stmt_exprs(s: &Stmt, f: &mut dyn FnMut(&Expr)) {
    let e = |x: &Expr, f: &mut dyn FnMut(&Expr)| walk_expr(x, f);
    let body = |b: &[Stmt], f: &mut dyn FnMut(&Expr)| b.iter().for_each(|s| stmt_exprs(s, f));
    match &s.kind {
        StmtKind::Expr { expr } => e(expr, f),
        StmtKind::Set { value, .. } => e(value, f),
        StmtKind::If { branches, otherwise } => {
            for (cond, b) in branches {
                e(cond, f);
                body(b, f);
            }
            if let Some(o) = otherwise {
                body(o, f);
            }
        }
        StmtKind::While { cond, body: b, .. } => {
            e(cond, f);
            body(b, f);
        }
        StmtKind::Repeat { times, body: b } => {
            e(times, f);
            body(b, f);
        }
        StmtKind::For { from, to, by, body: b, .. } => {
            e(from, f);
            e(to, f);
            e(by, f);
            body(b, f);
        }
        StmtKind::ForEach { list, body: b, .. } => {
            e(list, f);
            body(b, f);
        }
        StmtKind::Return { value: Some(v) } => e(v, f),
        StmtKind::TryCatch { body: b, handler, .. } => {
            body(b, f);
            body(handler, f);
        }
        StmtKind::Throw { message } | StmtKind::Log { message, .. } => e(message, f),
        StmtKind::Parallel { branches } => branches.iter().for_each(|b| body(b, f)),
        StmtKind::CallService { target, data, .. } => {
            target.iter().chain(data.iter()).for_each(|x| e(x, f));
        }
        StmtKind::HaScript { args, .. } | StmtKind::Plugin { args, .. } => {
            args.values().for_each(|x| e(x, f))
        }
        StmtKind::Wait { seconds } => e(seconds, f),
        StmtKind::WaitForTrigger { timeout: Some(t), .. } => e(t, f),
        StmtKind::FireEvent { event_type, data } => {
            e(event_type, f);
            data.iter().for_each(|x| e(x, f));
        }
        StmtKind::SetItem { key, value, .. } => {
            e(key, f);
            e(value, f);
        }
        StmtKind::TimerStart { seconds, data, key, .. } => {
            e(seconds, f);
            data.iter().chain(key.iter()).for_each(|x| e(x, f));
        }
        StmtKind::WaitUntil { cond, hold, timeout } => {
            e(cond, f);
            hold.iter().chain(timeout.iter()).for_each(|x| e(x, f));
        }
        _ => {}
    }
}

fn walk_expr(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    match &e.kind {
        ExprKind::Binary { left, right, .. } => {
            walk_expr(left, f);
            walk_expr(right, f);
        }
        ExprKind::Not { expr } => walk_expr(expr, f),
        ExprKind::Ternary { cond, then, otherwise } => {
            walk_expr(cond, f);
            walk_expr(then, f);
            walk_expr(otherwise, f);
        }
        ExprKind::List { items } => items.iter().for_each(|x| walk_expr(x, f)),
        ExprKind::Dict { entries } => entries.iter().for_each(|(_, x)| walk_expr(x, f)),
        ExprKind::Builtin { args, .. } | ExprKind::Call { args, .. } => {
            args.iter().for_each(|x| walk_expr(x, f))
        }
        ExprKind::Each { list, expr, .. } => {
            walk_expr(list, f);
            walk_expr(expr, f);
        }
        ExprKind::State { entity, attribute } => {
            walk_expr(entity, f);
            attribute.iter().for_each(|x| walk_expr(x, f));
        }
        ExprKind::Condition { args, .. } | ExprKind::Plugin { args, .. } => {
            args.values().for_each(|x| walk_expr(x, f))
        }
        ExprKind::Http { url, body, headers, .. } => {
            walk_expr(url, f);
            body.iter().chain(headers.iter()).for_each(|x| walk_expr(x, f));
        }
        _ => {}
    }
}

/// Blocks that run their body in rounds, each with HA's `repeat` variable.
const LOOPS: [&str; 5] =
    ["controls_repeat_ext", "controls_repeat", "controls_whileUntil", "controls_for", "controls_forEach"];

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
                // The editor replaces the quoted name with the field's label.
                let msg = if field == "ENTITY" {
                    "choose an entity".to_owned()
                } else {
                    format!("fill in \u{201c}{}\u{201d}", field.to_lowercase())
                };
                self.err(b, msg);
                String::new()
            }
        }
    }

    pub fn value(&mut self, b: &Block<'_>, input: &str) -> Expr {
        match b.input(input) {
            Some(inner) => self.expr(inner),
            None => {
                self.err(
                    b,
                    format!("put a block in the empty \u{201c}{}\u{201d} slot", input.to_lowercase()),
                );
                Expr::lit(b.id(), Value::Null)
            }
        }
    }

    /// A number that must be known when the program starts (trigger settings): a number or
    /// duration block in the socket, or the older number field of the same name.
    fn fixed_number(&mut self, b: &Block<'_>, name: &str) -> Option<f64> {
        match self.value_opt(b, name) {
            Some(Expr { kind: ExprKind::Lit { value }, .. }) => value.as_f64(),
            Some(_) => {
                self.err(b, "a trigger needs a fixed number here, not a variable or calculation");
                None
            }
            None => b.field_f64(name),
        }
    }

    pub fn value_opt(&mut self, b: &Block<'_>, input: &str) -> Option<Expr> {
        b.input(input).map(|inner| self.expr(inner))
    }

    pub fn stmt_chain(&mut self, first: Option<Block<'_>>) -> Vec<Stmt> {
        let mut out = Vec::new();
        let mut cur = first;
        while let Some(b) = cur {
            if b.enabled() {
                let is_loop = LOOPS.contains(&b.ty());
                self.loops += usize::from(is_loop);
                let kind = self.stmt(b);
                self.loops -= usize::from(is_loop);
                if let Some(kind) = kind {
                    out.push(Stmt { id: b.id().into(), kind });
                }
            }
            cur = b.next();
        }
        out
    }

    fn case_chain<'b>(&self, first: Option<Block<'b>>) -> Vec<Block<'b>> {
        let mut out = Vec::new();
        let mut cur = first;
        while let Some(b) = cur {
            if b.enabled() {
                out.push(b);
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
                        message: "only Home Assistant triggers can be waited for, not schedules, webhooks or MQTT messages".into(),
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
                if let Some(d) = duration_or_none(self.fixed_number(&b, "FOR")) {
                    cfg.insert("for".into(), d);
                }
                // "changes" means the state; without from/to HA would also fire on attribute changes.
                if !cfg.contains_key("from") && !cfg.contains_key("to") {
                    cfg.insert("to".into(), Value::Null);
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
                if let Some(off) = self.fixed_number(&b, "OFFSET").filter(|o| *o != 0.0) {
                    let sign = if off < 0.0 { "-" } else { "" };
                    let s = off.abs() as u64;
                    cfg["offset"] =
                        json!(format!("{sign}{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60));
                }
                ha(cfg)
            }
            "ha_trigger_ha_start" => ha(json!({"trigger": "homeassistant", "event": "start"})),
            "ha_trigger_every" => {
                let n = b.field_f64("N").unwrap_or(5.0).max(1.0) as u64;
                let (unit, max) = match b.field_str("UNIT") {
                    Some("seconds") => ("seconds", 60),
                    Some("hours") => ("hours", 24),
                    _ => ("minutes", 60),
                };
                // HA's time pattern counts from the top of the minute/hour/day, so a whole number
                // of the next unit becomes that unit (60 minutes = every hour).
                if n >= max && n.is_multiple_of(max) {
                    let m = n / max;
                    return match unit {
                        "seconds" => ha(json!({"trigger": "time_pattern", "minutes": format!("/{m}")})),
                        "minutes" if m < 24 && 24u64.is_multiple_of(m) => {
                            ha(json!({"trigger": "time_pattern", "hours": format!("/{m}"), "minutes": 0}))
                        }
                        "hours" if m == 1 => ha(json!({"trigger": "time", "at": "00:00:00"})),
                        _ => {
                            self.err(&b, format!("Home Assistant can't repeat every {n} {unit}; use a timer that starts itself again"));
                            None
                        }
                    };
                }
                if n >= max {
                    self.err(&b, format!("Home Assistant can't repeat every {n} {unit}; use a timer that starts itself again"));
                }
                // Values that don't divide evenly (e.g. 7 minutes) restart at the top of the hour.
                ha(json!({"trigger": "time_pattern", unit: format!("/{n}")}))
            }
            "ha_trigger_raw" => match parse_config(b.field_str("CONFIG").unwrap_or("")) {
                Ok(v) => ha(v),
                Err(e) => {
                    self.err(&b, e);
                    None
                }
            },
            "device_trigger" => {
                let (mut config, args) = self.device_parts(&b);
                for (k, e) in args {
                    match e.kind {
                        ExprKind::Lit { value } => config[k.trim_start_matches("p:")] = value,
                        ExprKind::Builtin { name, args } if name == "duration_dict" => {
                            match args.first().map(|a| &a.kind) {
                                Some(ExprKind::Lit { value }) => {
                                    config[k.trim_start_matches("p:")] =
                                        duration_or_none(value.as_f64()).unwrap_or(json!({"seconds": 0}))
                                }
                                _ => self.err(
                                    &b,
                                    format!(
                                        "'{}' must be a fixed value for triggers",
                                        k.trim_start_matches("p:")
                                    ),
                                ),
                            }
                        }
                        _ => self.err(
                            &b,
                            format!("'{}' must be a fixed value for triggers", k.trim_start_matches("p:")),
                        ),
                    }
                }
                ha(config)
            }
            "engine_trigger_cron" => Some(Trigger::Cron { id, expr: self.required_str(&b, "CRON") }),
            "engine_trigger_webhook" => {
                let methods = b.field_str("METHOD").unwrap_or("POST");
                Some(Trigger::Webhook {
                    id,
                    webhook_id: self.required_str(&b, "WEBHOOK_ID"),
                    methods: if methods == "ANY" { vec![] } else { vec![methods.to_owned()] },
                })
            }
            "engine_trigger_mqtt" => {
                Some(Trigger::Mqtt { id, topic: self.required_str(&b, "TOPIC"), filter: None })
            }
            _ => {
                if let Some(t) = self.helper_trigger(&b) {
                    return t;
                }
                let ext = self.ext;
                let t = ext.lower_trigger(b, self);
                if t.is_none() {
                    self.err(&b, "this block isn't a trigger; triggers go in the slot of a \"when\" block");
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
            // A label for organising a stack; does nothing.
            "control_section" => return None,
            "light_set_colour"
            | "light_set_white"
            | "light_set_brightness"
            | "light_power"
            | "light_step" => {
                let opt = |c: &mut Self, slot: &str| {
                    c.value_opt(&b, slot).unwrap_or_else(|| Expr::lit(b.id(), Value::Null))
                };
                let lit_ = |v: Value| Expr::lit(b.id(), v);
                let (service, args) = match b.ty() {
                    "light_set_colour" => (
                        "light.turn_on",
                        vec![
                            lit_(json!("colour")),
                            self.value(&b, "COLOUR"),
                            opt(self, "BRIGHTNESS"),
                            opt(self, "TRANSITION"),
                        ],
                    ),
                    "light_set_white" => (
                        "light.turn_on",
                        vec![
                            lit_(json!("white")),
                            self.value(&b, "KELVIN"),
                            opt(self, "BRIGHTNESS"),
                            opt(self, "TRANSITION"),
                        ],
                    ),
                    "light_set_brightness" => (
                        "light.turn_on",
                        vec![
                            lit_(Value::Null),
                            lit_(Value::Null),
                            self.value(&b, "BRIGHTNESS"),
                            opt(self, "TRANSITION"),
                        ],
                    ),
                    "light_power" => {
                        let service = match b.field_str("MODE") {
                            Some("off") => "light.turn_off",
                            Some("toggle") => "light.toggle",
                            _ => "light.turn_on",
                        };
                        (
                            service,
                            vec![
                                lit_(Value::Null),
                                lit_(Value::Null),
                                lit_(Value::Null),
                                opt(self, "TRANSITION"),
                            ],
                        )
                    }
                    _ => ("light.turn_on", vec![]),
                };
                let data = if b.ty() == "light_step" {
                    let args = vec![
                        self.value(&b, "STEP"),
                        lit_(json!(b.field_str("DIRECTION").unwrap_or("brighten"))),
                        opt(self, "TRANSITION"),
                    ];
                    Expr { id: b.id().into(), kind: self.builtin("light_step", args) }
                } else {
                    Expr { id: b.id().into(), kind: self.builtin("light_data", args) }
                };
                let target = self.value_opt(&b, "TARGET");
                if target.is_none() {
                    self.err(&b, "choose the lights");
                }
                StmtKind::CallService {
                    service: service.into(),
                    target,
                    data: Some(data),
                    response_var: None,
                }
            }
            "control_switch" => {
                // The value is computed once, then compared with each case in order.
                let var = format!("__switch_{}", b.id());
                let value = self.value(&b, "VALUE");
                let mut branches = Vec::new();
                let mut otherwise = None;
                for case in self.case_chain(b.input("CASES")) {
                    if case.ty() == "control_case_default" {
                        if otherwise.is_some() {
                            self.err(&case, "only one \"otherwise\" per switch");
                        }
                        otherwise = Some(self.stmt_chain(case.input("DO")));
                        continue;
                    }
                    if case.ty() != "control_case" {
                        self.err(&case, "only case blocks fit in a switch");
                        continue;
                    }
                    let target = Expr {
                        id: b.id().into(),
                        kind: ExprKind::Var { name: var.clone(), scope: VarScope::Local },
                    };
                    let case_value = self.value(&case, "VALUE");
                    let cond = Expr {
                        id: case.id().into(),
                        kind: self.builtin("case_matches", vec![target, case_value]),
                    };
                    branches.push((cond, self.stmt_chain(case.input("DO"))));
                }
                if branches.is_empty() && otherwise.is_none() {
                    self.err(&b, "add a case");
                }
                let set = Stmt {
                    id: b.id().into(),
                    kind: StmtKind::Set { name: var, scope: VarScope::Local, value },
                };
                let check = Stmt { id: b.id().into(), kind: StmtKind::If { branches, otherwise } };
                // Two statements under one block id: wrap them in a single-branch "if true".
                return Some(StmtKind::If {
                    branches: vec![(Expr::lit(b.id(), json!(true)), vec![set, check])],
                    otherwise: None,
                });
            }
            "control_case" | "control_case_default" => {
                self.err(&b, "put this inside a switch block");
                return None;
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
                let value = self.value(&b, "TO");
                if b.field_str("MODE") == Some("INSERT") {
                    // Inserting "at" an index puts the item before it; "last" appends.
                    let list = Expr { id: b.id().into(), kind: ExprKind::Var { name: name.clone(), scope } };
                    let position = Expr::lit(b.id(), json!(b.field_str("WHERE").unwrap_or("FROM_START")));
                    let inserted = Expr {
                        id: b.id().into(),
                        kind: self.builtin("list_insert", vec![list, key, value, position]),
                    };
                    StmtKind::Set { name, scope, value: inserted }
                } else {
                    StmtKind::SetItem { name, scope, key, value }
                }
            }
            "lists_getIndex" if b.field_str("MODE") == Some("REMOVE") => {
                let Some(lb) = b.input("VALUE").filter(|l| l.ty() == "variables_get") else {
                    self.err(&b, "the list to remove from must be a variable");
                    return None;
                };
                let (name, scope) = self.var(&lb, "VAR");
                let key = self.list_index(&b);
                let list = Expr { id: b.id().into(), kind: ExprKind::Var { name: name.clone(), scope } };
                let removed = Expr { id: b.id().into(), kind: self.builtin("list_remove", vec![list, key]) };
                StmtKind::Set { name, scope, value: removed }
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
            "timer_start" => {
                let name = self.required_str(&b, "NAME");
                // The body runs later, on its own, outside any loop around this block.
                let loops = std::mem::take(&mut self.loops);
                let body = self.stmt_chain(b.input("DO"));
                self.loops = loops;
                self.timer_handlers.push(Handler {
                    id: b.id().into(),
                    kind: HandlerKind::Timer { name: name.clone() },
                    body,
                });
                StmtKind::TimerStart {
                    name,
                    seconds: self.value(&b, "SECONDS"),
                    if_running: if b.field_str("MODE") == Some("keep") {
                        TimerMode::Keep
                    } else {
                        TimerMode::Restart
                    },
                    handler: b.id().into(),
                    data: None,
                    key: None,
                }
            }
            "device_action" => {
                let (mut config, args) = self.device_parts(&b);
                for k in args.keys() {
                    config[k.trim_start_matches("p:")] = arg_placeholder(k);
                }
                StmtKind::HaScript { sequence: json!([config]), args }
            }
            "timer_cancel" => StmtKind::TimerCancel { name: self.required_str(&b, "NAME") },
            "wait_until" => StmtKind::WaitUntil {
                cond: self.value(&b, "COND"),
                hold: self.value_opt(&b, "HOLD"),
                timeout: self.value_opt(&b, "TIMEOUT"),
            },
            "ha_scene_next" => {
                let scenes = self.value(&b, "SCENES");
                let dir = Expr::lit(b.id(), json!(b.field_str("DIRECTION").unwrap_or("next")));
                let target =
                    Expr { id: b.id().into(), kind: self.builtin("ha:next_scene", vec![scenes, dir]) };
                StmtKind::CallService {
                    service: "scene.turn_on".into(),
                    target: Some(target),
                    data: None,
                    response_var: None,
                }
            }
            "ha_call_service" | "ha_call_service_response" => {
                let service = self.required_str(&b, "SERVICE");
                if !service.is_empty() && !service.contains('.') {
                    self.err(&b, "action must look like domain.service");
                }
                let target = self.value_opt(&b, "TARGET");
                let data = self.value_opt(&b, "DATA");
                if b.ty() == "ha_call_service" {
                    return Some(StmtKind::CallService { service, target, data, response_var: None });
                }
                let (name, scope) = self.var(&b, "RESPONSE");
                if scope == VarScope::Local {
                    return Some(StmtKind::CallService { service, target, data, response_var: Some(name) });
                }
                // The response lands in a run variable; copy it into the program/global one.
                let tmp = format!("__response_{}", b.id());
                let call = StmtKind::CallService { service, target, data, response_var: Some(tmp.clone()) };
                let value =
                    Expr { id: b.id().into(), kind: ExprKind::Var { name: tmp, scope: VarScope::Local } };
                let set = StmtKind::Set { name, scope, value };
                return Some(StmtKind::If {
                    branches: vec![(
                        Expr::lit(b.id(), json!(true)),
                        vec![Stmt { id: b.id().into(), kind: call }, Stmt { id: b.id().into(), kind: set }],
                    )],
                    otherwise: None,
                });
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
                if let Some(k) = self.helper_stmt(&b) {
                    return Some(k);
                }
                let ext = self.ext;
                if let Some(k) = ext.lower_stmt(b, self) {
                    return Some(k);
                }
                if b.0.get("output").is_some() || self.is_value_block(&b) {
                    self.err(&b, "this block produces a value; plug it into another block");
                } else {
                    self.err(
                        &b,
                        format!(
                            "this block isn't available any more (its integration or plugin may be gone: {})",
                            b.ty()
                        ),
                    );
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
                let name = match b.field_str("MODE") {
                    Some("LEFT") => "text_left",
                    Some("RIGHT") => "text_right",
                    _ => "text_trim",
                };
                self.builtin(name, vec![a])
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
                        self.err(b, "\"get and remove\" isn't supported: get the item first, then use a separate \"remove\" block");
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
                let ty = Expr::lit(b.id(), json!(b.field_str("TYPE").unwrap_or("NUMERIC")));
                self.builtin(if desc { "list_sort_desc" } else { "list_sort" }, vec![a, ty])
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
                let mut args = BTreeMap::new();
                args.insert("entity".to_owned(), self.entity_expr(b));
                let state = b.field_str("STATE").unwrap_or("").to_owned();
                ExprKind::Condition {
                    config: json!({"condition": "state", "entity_id": arg_placeholder("entity"), "state": state}),
                    args,
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
            "yaml_value" if b.field_str("CONFIG").unwrap_or("").trim().is_empty() => lit(Value::Null),
            "yaml_value" => match parse_config(b.field_str("CONFIG").unwrap_or("")) {
                Ok(v) => lit(v),
                Err(e) => {
                    self.err(b, e);
                    lit(Value::Null)
                }
            },
            "duration" => {
                let factor = match b.field_str("UNIT") {
                    Some("minutes") => 60.0,
                    Some("hours") => 3600.0,
                    Some("days") => 86400.0,
                    _ => 1.0,
                };
                let n = self.value(b, "N");
                match &n.kind {
                    ExprKind::Lit { value } if value.is_number() => {
                        lit(number(value.as_f64().unwrap_or(0.0) * factor))
                    }
                    _ => ExprKind::Binary {
                        bin: BinOp::Mul,
                        left: Box::new(n),
                        right: Box::new(Expr::lit(b.id(), number(factor))),
                    },
                }
            }
            "device_condition" => {
                let (mut config, args) = self.device_parts(b);
                config["condition"] = json!("device");
                for k in args.keys() {
                    config[k.trim_start_matches("p:")] = arg_placeholder(k);
                }
                ExprKind::Condition { config, args }
            }
            "timer_running" => {
                let name = Expr::lit(b.id(), json!(self.required_str(b, "NAME")));
                self.builtin("engine:timer_running", vec![name])
            }
            "throttle" => {
                // Each throttle block limits on its own unless blocks share a key.
                let key = b.field_str("KEY").map(str::trim).filter(|k| !k.is_empty()).unwrap_or(b.id());
                let key = Expr::lit(b.id(), json!(key));
                let secs = self.value(b, "SECONDS");
                self.builtin("engine:throttle", vec![key, secs])
            }
            "ha_state_for" => {
                let state = b.field_str("STATE").unwrap_or("").to_owned();
                let mut args = BTreeMap::new();
                args.insert("entity".to_owned(), self.entity_expr(b));
                args.insert("for".to_owned(), self.value(b, "SECONDS"));
                ExprKind::Condition {
                    config: json!({"condition": "state", "entity_id": arg_placeholder("entity"), "state": state,
                        "for": {"seconds": arg_placeholder("for")}}),
                    args,
                }
            }
            "time_is_between" => {
                let mut cfg = json!({"condition": "time"});
                for (field, key) in [("AFTER", "after"), ("BEFORE", "before")] {
                    if let Some(v) = b.field_str(field).map(str::trim).filter(|v| !v.is_empty()) {
                        cfg[key] = json!(v);
                    }
                }
                let days: Vec<&str> = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"]
                    .into_iter()
                    .filter(|d| b.field_str(&d.to_uppercase()) == Some("TRUE"))
                    .collect();
                if !days.is_empty() && days.len() < 7 {
                    cfg["weekday"] = json!(days);
                }
                ExprKind::Condition { config: cfg, args: BTreeMap::new() }
            }
            "ha_area" | "ha_device" | "ha_floor" | "ha_label" => {
                let key = format!("{}_id", b.ty().trim_start_matches("ha_"));
                let ids: Vec<&str> = b
                    .field_str("ID")
                    .unwrap_or("")
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect();
                if ids.is_empty() {
                    let what = b.ty().trim_start_matches("ha_");
                    self.err(
                        b,
                        format!(
                            "choose {} {what}",
                            if what.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" }
                        ),
                    );
                }
                lit(json!({ key: ids }))
            }
            "ha_has_value" | "ha_friendly_name" => {
                let e = self.entity_expr(b);
                let name = if b.ty() == "ha_has_value" { "ha:has_value" } else { "ha:friendly_name" };
                self.builtin(name, vec![e])
            }
            "ha_seconds_since" => {
                let e = self.entity_expr(b);
                let field = Expr::lit(b.id(), json!(b.field_str("FIELD").unwrap_or("last_changed")));
                self.builtin("ha:seconds_since", vec![e, field])
            }
            "ha_number_state" => {
                let e = self.entity_expr(b);
                let d = self.value_opt(b, "DEFAULT").unwrap_or_else(|| Expr::lit(b.id(), json!(0)));
                self.builtin("ha:number_state", vec![e, d])
            }
            "ha_state_check" => {
                let list = self.value(b, "LIST");
                let f = |k: &str, d: &str| Expr::lit(b.id(), json!(b.field_str(k).unwrap_or(d)));
                let (mode, op, state) = (f("MODE", "any"), f("OP", "is"), f("STATE", "on"));
                self.builtin("ha:state_check", vec![list, mode, op, state])
            }
            "ha_states_aggregate" => {
                let list = self.value(b, "LIST");
                let op = Expr::lit(b.id(), json!(b.field_str("OP").unwrap_or("average")));
                self.builtin("ha:aggregate", vec![list, op])
            }
            "ha_scene_last" => {
                let scenes = self.value(b, "SCENES");
                self.builtin("ha:last_scene", vec![scenes])
            }
            "ha_history" => {
                let e = self.entity_expr(b);
                let op = Expr::lit(b.id(), json!(b.field_str("OP").unwrap_or("average")));
                let secs = self.value(b, "SECONDS");
                let state = Expr::lit(b.id(), json!(b.field_str("STATE").unwrap_or("")));
                self.builtin("ha:history", vec![e, op, secs, state])
            }
            "ha_entities_in" => {
                let kind = b.field_str("KIND").unwrap_or("area");
                let name = b.field_str("NAME").unwrap_or("").trim().replace(['\\', '\''], "");
                let domain =
                    b.field_str("DOMAIN").unwrap_or("").trim().trim_matches('*').replace(['\\', '\''], "");
                let func = match kind {
                    "floor" => "floor_entities",
                    "label" => "label_entities",
                    "device" => "device_entities",
                    "integration" => "integration_entities",
                    _ => "area_entities",
                };
                if name.is_empty() {
                    self.err(b, "name is required");
                }
                let filter = if domain.is_empty() {
                    String::new()
                } else {
                    format!(" | select('match', '{domain}\\.')")
                };
                ExprKind::Template { template: format!("{{{{ {func}('{name}'){filter} | list }}}}") }
            }
            "colour_picker" => lit(json!(b.field_str("COLOUR").unwrap_or("#ffffff").to_lowercase())),
            "colour_rgb255" => {
                let args = vec![self.value(b, "RED"), self.value(b, "GREEN"), self.value(b, "BLUE")];
                self.builtin("colour_rgb", args)
            }
            "colour_hsv" => {
                let args = vec![self.value(b, "HUE"), self.value(b, "SATURATION"), self.value(b, "VALUE")];
                self.builtin("colour_hsv", args)
            }
            "colour_mix" => {
                let args = vec![self.value(b, "A"), self.value(b, "B"), self.value(b, "RATIO")];
                self.builtin("colour_blend", args)
            }
            "colour_random" => self.builtin("colour_random", vec![]),
            "colour_part" => {
                let args = vec![
                    self.value(b, "COLOUR"),
                    Expr::lit(b.id(), json!(b.field_str("PART").unwrap_or("hex"))),
                ];
                self.builtin("colour_part", args)
            }
            "light_colour" | "light_brightness" => {
                let e = self.entity_expr(b);
                let name = if b.ty() == "light_colour" { "ha:light_colour" } else { "ha:light_brightness" };
                self.builtin(name, vec![e])
            }
            "logic_or_default" => {
                let args = vec![self.value(b, "A"), self.value(b, "B")];
                self.builtin("or_default", args)
            }
            "math_between" => {
                let args = vec![self.value(b, "VALUE"), self.value(b, "LOW"), self.value(b, "HIGH")];
                self.builtin("between", args)
            }
            "text_regex" => {
                let op = match b.field_str("OP") {
                    Some("match") => "regex_match",
                    Some("replace") => "regex_replace",
                    Some("all") => "regex_all",
                    _ => "regex_test",
                };
                let mut args = vec![
                    self.value(b, "TEXT"),
                    Expr::lit(b.id(), json!(b.field_str("PATTERN").unwrap_or(""))),
                ];
                if op == "regex_replace" {
                    args.push(self.value_opt(b, "WITH").unwrap_or_else(|| Expr::lit(b.id(), json!(""))));
                }
                self.builtin(op, args)
            }
            "text_split_at" => {
                let name = if b.field_str("WHERE") == Some("after") { "text_after" } else { "text_before" };
                let args = vec![self.value(b, "TEXT"), self.value(b, "SEP")];
                self.builtin(name, args)
            }
            "lists_filter" | "lists_map" => {
                let var = format!("__item_{}", b.id());
                let list = self.value(b, "LIST");
                self.item_vars.push(var.clone());
                let slot = if b.ty() == "lists_filter" { "COND" } else { "EXPR" };
                let expr = self.value(b, slot);
                self.item_vars.pop();
                ExprKind::Each { list: Box::new(list), var, expr: Box::new(expr), map: b.ty() == "lists_map" }
            }
            "lists_item" => match self.item_vars.last() {
                Some(var) => ExprKind::Var { name: var.clone(), scope: VarScope::Local },
                None => {
                    self.err(b, "\"item\" only works inside a filter or transform block");
                    lit(Value::Null)
                }
            },
            "loop_info" => {
                if self.loops == 0 {
                    self.err(b, "this only works inside a repeat or \"for each\" loop");
                }
                let what = Expr::lit(b.id(), json!(b.field_str("WHAT").unwrap_or("index")));
                self.builtin("loop_info", vec![what])
            }
            "time_today_at" => {
                let t = Expr::lit(b.id(), json!(b.field_str("TIME").unwrap_or("00:00")));
                self.builtin("today_at", vec![t])
            }
            "time_add" => {
                let args = vec![self.value(b, "DATETIME"), self.value(b, "SECONDS")];
                self.builtin("datetime_add", args)
            }
            "time_seconds_between" => {
                let args = vec![self.value(b, "A"), self.value(b, "B")];
                self.builtin("seconds_between", args)
            }
            "format_duration" => {
                let args = vec![
                    self.value(b, "SECONDS"),
                    Expr::lit(b.id(), json!(b.field_str("STYLE").unwrap_or("words"))),
                ];
                self.builtin("format_duration", args)
            }
            "trigger_info" => {
                let attr = b.field_str("ATTR").unwrap_or("").trim().to_owned();
                let path = match b.field_str("WHAT").unwrap_or("entity") {
                    "from" => "from_state.state".to_owned(),
                    "to" => "to_state.state".to_owned(),
                    "from_attr" => format!("from_state.attributes.{attr}"),
                    "to_attr" => format!("to_state.attributes.{attr}"),
                    "id" => "id".to_owned(),
                    "platform" => "platform".to_owned(),
                    _ => "entity_id".to_owned(),
                };
                ExprKind::TriggerData { path }
            }
            "triggered_by" => {
                let id = b.field_str("TRIGGER").unwrap_or("").to_owned();
                ExprKind::Binary {
                    bin: BinOp::Eq,
                    left: Box::new(Expr {
                        id: b.id().into(),
                        kind: ExprKind::TriggerData { path: "id".into() },
                    }),
                    right: Box::new(Expr::lit(b.id(), json!(id))),
                }
            }
            "http_request" => ExprKind::Http {
                method: b.field_str("METHOD").unwrap_or("GET").to_owned(),
                url: Box::new(self.value(b, "URL")),
                body: self.value_opt(b, "BODY").map(Box::new),
                headers: self.value_opt(b, "HEADERS").map(Box::new),
            },
            _ => {
                if let Some(k) = self.helper_expr(b) {
                    return k;
                }
                let ext = self.ext;
                if let Some(k) = ext.lower_expr(*b, self) {
                    return k;
                }
                self.err(b, format!("'{}' does not produce a value", b.ty()));
                lit(Value::Null)
            }
        }
    }

    /// A device trigger/condition/action block: HA's config stored in the block, plus its
    /// option fields (`P_<name>`) as expressions keyed `p:<name>`.
    fn device_parts(&mut self, b: &Block<'_>) -> (Value, BTreeMap<String, Expr>) {
        let config = b.extra("config").cloned().unwrap_or_default();
        if !config.is_object() {
            self.err(b, "this device block is missing its configuration; drag it in again from Devices");
            return (json!({}), BTreeMap::new());
        }
        let mut args = BTreeMap::new();
        for p in b.extra("params").and_then(Value::as_array).into_iter().flatten() {
            let Some(name) = p.get("name").and_then(Value::as_str) else { continue };
            let slot = format!("P_{name}");
            let expr = if let Some(raw) = b.field(&slot) {
                let v = match (p.get("input").and_then(Value::as_str), raw) {
                    (Some("boolean"), Value::String(s)) => json!(s == "TRUE"),
                    (_, Value::String(s)) if s.is_empty() => continue,
                    (_, v) => v.clone(),
                };
                Expr::lit(b.id(), v)
            } else if let Some(e) = self.value_opt(b, &slot) {
                if matches!(&e.kind, ExprKind::Lit { value: Value::Null }) {
                    continue;
                }
                e
            } else {
                if p.get("required").and_then(Value::as_bool) == Some(true) {
                    self.err(
                        b,
                        format!("'{}' is required", p.get("label").and_then(Value::as_str).unwrap_or(name)),
                    );
                }
                continue;
            };
            let expr = if p.get("duration_dict").and_then(Value::as_bool) == Some(true) {
                Expr {
                    id: b.id().into(),
                    kind: ExprKind::Builtin { name: "duration_dict".into(), args: vec![expr] },
                }
            } else {
                expr
            };
            args.insert(format!("p:{name}"), expr);
        }
        (config, args)
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
