//! Lowers registry blocks into IR.

use std::collections::{BTreeMap, HashMap};

use puzzlebox_ir::{Block, BlockLowerer, Compiler, Expr, ExprKind, StmtKind, Trigger, arg_placeholder};
use serde_json::{Map, Value, json};

use crate::model::{BlockDef, BlockKind, Lowering, Param, ParamInput};

pub struct RegistryLowerer {
    pub(crate) defs: HashMap<String, BlockDef>,
}

fn hex_to_rgb(hex: &str) -> Option<Value> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some(json!([p(0)?, p(2)?, p(4)?]))
}

/// Static value of a field-backed param.
fn field_value(p: &Param, raw: &Value) -> Value {
    match (&p.input, raw) {
        (ParamInput::Boolean, Value::String(s)) => json!(s == "TRUE"),
        (ParamInput::Color, Value::String(s)) => hex_to_rgb(s).unwrap_or(Value::Null),
        (
            ParamInput::StateValue { multiple: true, .. }
            | ParamInput::Select { multiple: true, .. }
            | ParamInput::Registry { multiple: true, .. },
            Value::String(s),
        ) => split_list(s),
        (_, Value::String(s)) if s.is_empty() => Value::Null,
        (_, v) => v.clone(),
    }
}

fn split_list(s: &str) -> Value {
    let items: Vec<&str> = s.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    if items.is_empty() { Value::Null } else { json!(items) }
}

/// Normalizes a constant target expression (entity id, list, or target dict) to an HA target.
fn const_target(v: &Value) -> Value {
    match v {
        Value::String(s) => {
            let ids: Vec<&str> = s.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
            json!({ "entity_id": ids })
        }
        Value::Array(items) => json!({ "entity_id": items }),
        other => other.clone(),
    }
}

/// Turns a dictionary of literals into one literal, so fixed thresholds work in triggers.
fn const_fold(e: Expr) -> Expr {
    fn lit(e: &Expr) -> Option<Value> {
        match &e.kind {
            ExprKind::Lit { value } => Some(value.clone()),
            ExprKind::Dict { entries } => {
                let mut m = Map::new();
                for (k, v) in entries {
                    m.insert(k.clone(), lit(v)?);
                }
                Some(Value::Object(m))
            }
            _ => None,
        }
    }
    match lit(&e) {
        Some(v) => Expr::lit(e.id, v),
        None => e,
    }
}

struct Args {
    /// Param name → expression (fields become literals).
    exprs: BTreeMap<String, Expr>,
    target: Option<Expr>,
}

impl RegistryLowerer {
    fn collect(&self, def: &BlockDef, b: Block<'_>, c: &mut Compiler<'_>) -> Args {
        let mut exprs = BTreeMap::new();
        for p in &def.params {
            let slot = p.slot();
            let e = if let ParamInput::Threshold { units, .. } = &p.input {
                Self::threshold(p, units, b, c)
            } else if let Some(raw) = b.field(&slot) {
                let v = field_value(p, raw);
                (!v.is_null()).then(|| Expr::lit(b.id(), v))
            } else {
                c.value_opt(&b, &slot).and_then(|e| match (&p.input, &e.kind) {
                    // An empty YAML value means "not set".
                    (_, ExprKind::Lit { value: Value::Null }) => None,
                    (
                        ParamInput::Entity { multiple: true, .. },
                        ExprKind::Lit { value: Value::String(s) },
                    ) => Some(Expr::lit(e.id.clone(), split_list(s))),
                    _ => Some(e),
                })
            };
            match e {
                Some(e) => {
                    exprs.insert(p.name.clone(), e);
                }
                None if p.required && p.default.is_none() => c.err(&b, format!("'{}' is required", p.label)),
                None => {
                    if p.required
                        && let Some(d) = &p.default
                    {
                        exprs.insert(p.name.clone(), Expr::lit(b.id(), d.clone()));
                    }
                }
            }
        }
        let target = c.value_opt(&b, "TARGET");
        if target.is_none() && def.target.as_ref().is_some_and(|t| t.required) {
            c.err(&b, "choose a target");
        }
        Args { exprs, target }
    }

    /// Builds HA's numeric threshold object from the type dropdown (`P_x`), optional unit
    /// dropdown (`P_x__unit`) and number inputs (`P_x__a`, `P_x__b`).
    fn threshold(p: &Param, units: &[Option<String>], b: Block<'_>, c: &mut Compiler<'_>) -> Option<Expr> {
        let slot = p.slot();
        let kind = b.field_str(&slot)?.to_owned();
        if kind == "any" {
            return Some(Expr::lit(b.id(), json!({"type": "any"})));
        }
        let unit = if units.is_empty() {
            None
        } else {
            Some(match b.field_str(&format!("{slot}__unit")) {
                Some("__none__") | None => Value::Null,
                Some(u) => json!(u),
            })
        };
        let entry = |c: &mut Compiler<'_>, input: &str| -> Expr {
            let number = c.value(&b, &format!("{slot}__{input}"));
            let mut entries = vec![("number".to_owned(), number)];
            if let Some(u) = &unit {
                entries.push(("unit_of_measurement".to_owned(), Expr::lit(b.id(), u.clone())));
            }
            Expr { id: b.id().into(), kind: ExprKind::Dict { entries } }
        };
        let mut entries = vec![("type".to_owned(), Expr::lit(b.id(), json!(kind)))];
        if kind == "between" || kind == "outside" {
            entries.push(("value_min".to_owned(), entry(c, "a")));
            entries.push(("value_max".to_owned(), entry(c, "b")));
        } else {
            entries.push(("value".to_owned(), entry(c, "a")));
        }
        Some(const_fold(Expr { id: b.id().into(), kind: ExprKind::Dict { entries } }))
    }

    /// Triggers are subscribed once, so every argument must be a constant.
    fn constants(
        &self,
        args: &Args,
        b: Block<'_>,
        c: &mut Compiler<'_>,
    ) -> (Map<String, Value>, Option<Value>) {
        let mut out = Map::new();
        for (k, e) in &args.exprs {
            match &e.kind {
                ExprKind::Lit { value } => {
                    out.insert(k.clone(), value.clone());
                }
                _ => c.err(&b, format!("'{k}' must be a fixed value for triggers")),
            }
        }
        let target = match &args.target {
            Some(Expr { kind: ExprKind::Lit { value }, .. }) => Some(const_target(value)),
            Some(_) => {
                c.err(&b, "the trigger target must be a fixed entity or list");
                None
            }
            None => None,
        };
        (out, target)
    }

    fn placeholders(args: &Args) -> (Map<String, Value>, BTreeMap<String, Expr>) {
        let mut data = Map::new();
        let mut exprs = BTreeMap::new();
        for (k, e) in &args.exprs {
            data.insert(k.clone(), arg_placeholder(&format!("p:{k}")));
            exprs.insert(format!("p:{k}"), e.clone());
        }
        (data, exprs)
    }

    fn template_args(args: &Args) -> BTreeMap<String, Expr> {
        let mut out: BTreeMap<String, Expr> = args.exprs.clone();
        if let Some(t) = &args.target {
            let wrapped = ExprKind::Builtin { name: "to_target".into(), args: vec![t.clone()] };
            out.insert("target".into(), Expr { id: t.id.clone(), kind: wrapped });
        }
        out
    }
}

impl BlockLowerer for RegistryLowerer {
    fn lower_stmt(&self, b: Block<'_>, c: &mut Compiler<'_>) -> Option<StmtKind> {
        let def = self.defs.get(b.ty())?;
        if def.kind != BlockKind::Action {
            c.err(&b, "this block belongs elsewhere (trigger slot or as a value)");
            return None;
        }
        let args = self.collect(def, b, c);
        Some(match &def.lowering {
            Lowering::Service { service, .. } => {
                let entries: Vec<(String, Expr)> = args.exprs.into_iter().collect();
                let data = (!entries.is_empty())
                    .then(|| Expr { id: b.id().into(), kind: ExprKind::Dict { entries } });
                StmtKind::CallService {
                    service: service.clone(),
                    target: args.target,
                    data,
                    response_var: None,
                }
            }
            Lowering::HaAction { sequence } => {
                StmtKind::HaScript { sequence: sequence.clone(), args: Self::template_args(&args) }
            }
            Lowering::Plugin { plugin, function } => StmtKind::Plugin {
                plugin: plugin.clone(),
                block: function.clone(),
                args: Self::template_args(&args),
            },
            other => {
                c.err(&b, format!("lowering {other:?} cannot be used for an action"));
                return None;
            }
        })
    }

    fn lower_expr(&self, b: Block<'_>, c: &mut Compiler<'_>) -> Option<ExprKind> {
        let def = self.defs.get(b.ty())?;
        if !matches!(def.kind, BlockKind::Condition | BlockKind::Value) {
            c.err(&b, "this block does not produce a value");
            return Some(ExprKind::Lit { value: Value::Null });
        }
        let args = self.collect(def, b, c);
        Some(match &def.lowering {
            Lowering::HaConditionPlatform { condition } => {
                let (options, mut pargs) = Self::placeholders(&args);
                let mut config = json!({"condition": condition, "options": options});
                if let Some(t) = args.target {
                    config["target"] = arg_placeholder("target");
                    let wrapped = ExprKind::Builtin { name: "to_target".into(), args: vec![t.clone()] };
                    pargs.insert("target".into(), Expr { id: t.id, kind: wrapped });
                }
                ExprKind::Condition { config, args: pargs }
            }
            Lowering::HaCondition { config } => {
                ExprKind::Condition { config: config.clone(), args: Self::template_args(&args) }
            }
            Lowering::Template { template } => {
                // Params reach Jinja as variables through a dict merged into the run variables.
                let entries = Self::template_args(&args).into_iter().collect();
                ExprKind::Builtin {
                    name: "template_with".into(),
                    args: vec![
                        Expr::lit(b.id(), json!(template)),
                        Expr { id: b.id().into(), kind: ExprKind::Dict { entries } },
                    ],
                }
            }
            Lowering::Plugin { plugin, function } => ExprKind::Plugin {
                plugin: plugin.clone(),
                block: function.clone(),
                args: Self::template_args(&args),
            },
            Lowering::Service { service, response: true } => {
                // Value-returning action (`return_response`).
                let entries: Vec<(String, Expr)> = args.exprs.into_iter().collect();
                let mut fn_args = vec![Expr::lit(b.id(), json!(service))];
                fn_args.push(Expr { id: b.id().into(), kind: ExprKind::Dict { entries } });
                if let Some(t) = args.target {
                    fn_args.push(t);
                }
                ExprKind::Builtin { name: "service_response".into(), args: fn_args }
            }
            other => {
                c.err(&b, format!("lowering {other:?} cannot be used as a value"));
                ExprKind::Lit { value: Value::Null }
            }
        })
    }

    fn lower_trigger(&self, b: Block<'_>, c: &mut Compiler<'_>) -> Option<Trigger> {
        let def = self.defs.get(b.ty())?;
        if def.kind != BlockKind::Trigger {
            c.err(&b, "only trigger blocks fit here");
            return None;
        }
        let args = self.collect(def, b, c);
        let (options, target) = self.constants(&args, b, c);
        let config = match &def.lowering {
            Lowering::HaTriggerPlatform { trigger } => {
                let mut cfg = json!({"trigger": trigger, "options": options});
                if let Some(t) = target {
                    cfg["target"] = t;
                }
                cfg
            }
            Lowering::HaTrigger { config } => {
                let mut vals: BTreeMap<String, Value> = options.into_iter().collect();
                if let Some(t) = target {
                    vals.insert("target".into(), t);
                }
                puzzlebox_ir::substitute_args(config, &vals)
            }
            other => {
                c.err(&b, format!("lowering {other:?} cannot be used as a trigger"));
                return None;
            }
        };
        Some(Trigger::Ha { id: b.id().into(), config })
    }
}
