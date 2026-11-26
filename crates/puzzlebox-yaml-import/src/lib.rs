//! Home Assistant automation/script YAML → Blockly workspace JSON.
//!
//! Common constructs become native blocks; anything that cannot be expressed exactly
//! becomes a raw "HA trigger/condition/action (YAML)" block, so imports are lossless.

mod actions;
mod conditions;
mod documents;
mod templates;
mod triggers;

use serde::Serialize;
use serde_json::{Map, Value, json};

#[derive(Debug, Serialize, PartialEq)]
pub struct Imported {
    pub name: String,
    pub description: String,
    pub mode: String,
    pub max_runs: u32,
    pub workspace: Value,
    /// Constructs kept as raw YAML blocks (still work, just not editable as blocks).
    pub raw_blocks: usize,
}

/// Accepts a single automation/script, a list of them, or a `{script_id: {...}}` map.
pub fn import_yaml(text: &str) -> Result<Vec<Imported>, String> {
    let out: Vec<Imported> =
        documents::configs(text)?.into_iter().map(|(key, v)| import_one(&v, key.as_deref())).collect();
    if out.is_empty() {
        return Err("no automations or scripts found".into());
    }
    Ok(out)
}

pub(crate) fn is_config(v: &Value) -> bool {
    ["triggers", "trigger", "actions", "action", "sequence"].iter().any(|k| v.get(k).is_some())
}

pub fn import_one(cfg: &Value, key: Option<&str>) -> Imported {
    let mut b = Builder::default();
    let name = cfg["alias"]
        .as_str()
        .map(str::to_owned)
        .or(key.map(str::to_owned))
        .unwrap_or_else(|| "Imported".into());

    let mut body = Vec::new();
    if let Some(vars) = cfg.get("variables").and_then(Value::as_object) {
        body.extend(b.variables(vars));
    }
    let actions = list(cfg.get("actions").or_else(|| cfg.get("action")).or_else(|| cfg.get("sequence")));
    let action_blocks = b.actions(&actions);
    let conds = list(cfg.get("conditions").or_else(|| cfg.get("condition")));
    if conds.is_empty() {
        body.extend(action_blocks);
    } else {
        let cond = b.and_all(&conds);
        body.push(b.block(
            "controls_if",
            json!({}),
            vec![("IF0", Some(cond)), ("DO0", chain(action_blocks))],
            None,
        ));
    }

    let triggers = list(cfg.get("triggers").or_else(|| cfg.get("trigger")));
    let top = if triggers.is_empty() {
        // Scripts have no triggers: run them manually (Run button / service).
        b.block("ha_on_run", json!({}), vec![("DO", chain(body))], None)
    } else {
        let trig = b.triggers(&triggers);
        b.block("ha_when", json!({}), vec![("TRIGGERS", chain(trig)), ("DO", chain(body))], None)
    };
    let mut top = top;
    top["x"] = json!(40);
    top["y"] = json!(40);

    let variables: Vec<Value> = b.vars.iter().map(|(name, id)| json!({"name": name, "id": id})).collect();
    Imported {
        name,
        description: cfg["description"].as_str().unwrap_or("").to_owned(),
        mode: cfg["mode"].as_str().unwrap_or("single").to_owned(),
        max_runs: cfg["max"].as_u64().unwrap_or(10) as u32,
        workspace: json!({"blocks": {"languageVersion": 0, "blocks": [top]}, "variables": variables}),
        raw_blocks: b.raw,
    }
}

pub(crate) fn list(v: Option<&Value>) -> Vec<Value> {
    match v {
        Some(Value::Array(a)) => a.clone(),
        Some(Value::Null) | None => vec![],
        Some(other) => vec![other.clone()],
    }
}

/// Links statement blocks via `next` and returns the first.
pub(crate) fn chain(mut blocks: Vec<Value>) -> Option<Value> {
    let mut next: Option<Value> = None;
    while let Some(mut cur) = blocks.pop() {
        if let Some(n) = next.take() {
            cur["next"] = json!({"block": n});
        }
        next = Some(cur);
    }
    next
}

pub(crate) fn is_template(s: &str) -> bool {
    s.contains("{{") || s.contains("{%")
}

/// Seconds from `"HH:MM:SS"`, `{hours, minutes, seconds, milliseconds}` or a number.
pub(crate) fn seconds(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) if !is_template(s) => {
            let parts: Vec<f64> = s.split(':').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
            Some(parts.iter().fold(0.0, |acc, p| acc * 60.0 + p))
        }
        Value::Object(m) => {
            let g = |k: &str| m.get(k).and_then(Value::as_f64).unwrap_or(0.0);
            Some(
                g("days") * 86400.0
                    + g("hours") * 3600.0
                    + g("minutes") * 60.0
                    + g("seconds")
                    + g("milliseconds") / 1000.0,
            )
        }
        _ => None,
    }
}

pub(crate) fn only_keys(m: &Map<String, Value>, allowed: &[&str]) -> bool {
    m.keys().all(|k| allowed.contains(&k.as_str()))
}

pub(crate) fn yaml(v: &Value) -> String {
    serde_yaml::to_string(v).unwrap_or_default().trim_end().to_owned()
}

#[derive(Default)]
pub(crate) struct Builder {
    pub(crate) next_id: usize,
    pub(crate) raw: usize,
    pub(crate) vars: Vec<(String, String)>,
}

impl Builder {
    pub(crate) fn id(&mut self) -> String {
        self.next_id += 1;
        format!("imp{}", self.next_id)
    }

    pub(crate) fn block(
        &mut self,
        ty: &str,
        fields: Value,
        inputs: Vec<(&str, Option<Value>)>,
        extra: Option<Value>,
    ) -> Value {
        let mut b = json!({"type": ty, "id": self.id()});
        if fields.as_object().is_some_and(|f| !f.is_empty()) {
            b["fields"] = fields;
        }
        let mut ins = Map::new();
        for (k, v) in inputs {
            if let Some(v) = v {
                ins.insert(k.to_owned(), json!({"block": v}));
            }
        }
        if !ins.is_empty() {
            b["inputs"] = Value::Object(ins);
        }
        if let Some(e) = extra {
            b["extraState"] = e;
        }
        b
    }

    pub(crate) fn var_id(&mut self, name: &str) -> String {
        if let Some((_, id)) = self.vars.iter().find(|(n, _)| n == name) {
            return id.clone();
        }
        let id = format!("var_{}", self.vars.len() + 1);
        self.vars.push((name.to_owned(), id.clone()));
        id
    }

    /// A value block for a literal (templates are rendered by HA).
    pub(crate) fn value(&mut self, v: &Value) -> Value {
        match v {
            Value::Number(n) => self.block("math_number", json!({"NUM": n}), vec![], None),
            Value::Bool(b) => {
                self.block("logic_boolean", json!({"BOOL": if *b { "TRUE" } else { "FALSE" }}), vec![], None)
            }
            Value::Null => self.block("logic_null", json!({}), vec![], None),
            Value::String(s) if is_template(s) => self.template_value(s),
            Value::String(s) => self.block("text", json!({"TEXT": s}), vec![], None),
            Value::Object(m) => self.dict(m),
            Value::Array(items) => {
                let n = items.len();
                let inputs: Vec<(String, Value)> =
                    items.iter().enumerate().map(|(i, it)| (format!("ADD{i}"), self.value(it))).collect();
                let mut b = self.block("lists_create_with", json!({}), vec![], Some(json!({"itemCount": n})));
                let mut ins = Map::new();
                for (k, v) in inputs {
                    ins.insert(k, json!({"block": v}));
                }
                if !ins.is_empty() {
                    b["inputs"] = Value::Object(ins);
                }
                b
            }
        }
    }

    pub(crate) fn dict(&mut self, m: &Map<String, Value>) -> Value {
        let pairs: Vec<Value> = m
            .iter()
            .map(|(k, v)| {
                let val = self.value(v);
                self.block("dict_pair", json!({"KEY": k}), vec![("VALUE", Some(val))], None)
            })
            .collect();
        self.block("dict_create", json!({}), vec![("ITEMS", chain(pairs))], None)
    }

    pub(crate) fn variables(&mut self, vars: &Map<String, Value>) -> Vec<Value> {
        vars.iter()
            .map(|(name, v)| {
                let id = self.var_id(name);
                let val = self.value(v);
                self.block("variables_set", json!({"VAR": {"id": id}}), vec![("VALUE", Some(val))], None)
            })
            .collect()
    }
}

/// `"-00:30:00"` / `"01:00:00"` / seconds → signed seconds.
pub(crate) fn offset_seconds(v: &Value) -> Option<f64> {
    match v {
        Value::String(s) => {
            let (sign, rest) = match s.strip_prefix('-') {
                Some(r) => (-1.0, r),
                None => (1.0, s.strip_prefix('+').unwrap_or(s)),
            };
            seconds(&json!(rest)).map(|x| sign * x)
        }
        other => seconds(other),
    }
}
