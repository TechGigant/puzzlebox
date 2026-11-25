//! Home Assistant automation/script YAML → Blockly workspace JSON.
//!
//! Common constructs become native blocks; anything that cannot be expressed exactly
//! becomes a raw "HA trigger/condition/action (YAML)" block, so imports are lossless.

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
    let doc: Value = serde_yaml::from_str(text).map_err(|e| format!("invalid YAML: {e}"))?;
    let items: Vec<(Option<String>, Value)> = match doc {
        Value::Array(list) => list.into_iter().map(|v| (None, v)).collect(),
        Value::Object(ref m) if is_config(&doc) => vec![(None, Value::Object(m.clone()))],
        Value::Object(m) => m.into_iter().map(|(k, v)| (Some(k), v)).collect(),
        _ => return Err("expected an automation, a script, or a list of them".into()),
    };
    let out: Vec<Imported> = items
        .into_iter()
        .filter(|(_, v)| is_config(v))
        .map(|(key, v)| import_one(&v, key.as_deref()))
        .collect();
    if out.is_empty() {
        return Err("no automations or scripts found".into());
    }
    Ok(out)
}

fn is_config(v: &Value) -> bool {
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

fn list(v: Option<&Value>) -> Vec<Value> {
    match v {
        Some(Value::Array(a)) => a.clone(),
        Some(Value::Null) | None => vec![],
        Some(other) => vec![other.clone()],
    }
}

/// Links statement blocks via `next` and returns the first.
fn chain(mut blocks: Vec<Value>) -> Option<Value> {
    let mut next: Option<Value> = None;
    while let Some(mut cur) = blocks.pop() {
        if let Some(n) = next.take() {
            cur["next"] = json!({"block": n});
        }
        next = Some(cur);
    }
    next
}

fn is_template(s: &str) -> bool {
    s.contains("{{") || s.contains("{%")
}

/// Seconds from `"HH:MM:SS"`, `{hours, minutes, seconds, milliseconds}` or a number.
fn seconds(v: &Value) -> Option<f64> {
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

fn only_keys(m: &Map<String, Value>, allowed: &[&str]) -> bool {
    m.keys().all(|k| allowed.contains(&k.as_str()))
}

fn yaml(v: &Value) -> String {
    serde_yaml::to_string(v).unwrap_or_default().trim_end().to_owned()
}

#[derive(Default)]
struct Builder {
    next_id: usize,
    raw: usize,
    vars: Vec<(String, String)>,
}

impl Builder {
    fn id(&mut self) -> String {
        self.next_id += 1;
        format!("imp{}", self.next_id)
    }

    fn block(
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

    fn var_id(&mut self, name: &str) -> String {
        if let Some((_, id)) = self.vars.iter().find(|(n, _)| n == name) {
            return id.clone();
        }
        let id = format!("var_{}", self.vars.len() + 1);
        self.vars.push((name.to_owned(), id.clone()));
        id
    }

    /// A value block for a literal (templates are rendered by HA).
    fn value(&mut self, v: &Value) -> Value {
        match v {
            Value::Number(n) => self.block("math_number", json!({"NUM": n}), vec![], None),
            Value::Bool(b) => {
                self.block("logic_boolean", json!({"BOOL": if *b { "TRUE" } else { "FALSE" }}), vec![], None)
            }
            Value::Null => self.block("logic_null", json!({}), vec![], None),
            Value::String(s) if is_template(s) => {
                self.block("ha_template", json!({"TEMPLATE": s}), vec![], None)
            }
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

    fn dict(&mut self, m: &Map<String, Value>) -> Value {
        let pairs: Vec<Value> = m
            .iter()
            .map(|(k, v)| {
                let val = self.value(v);
                self.block("dict_pair", json!({"KEY": k}), vec![("VALUE", Some(val))], None)
            })
            .collect();
        self.block("dict_create", json!({}), vec![("ITEMS", chain(pairs))], None)
    }

    fn variables(&mut self, vars: &Map<String, Value>) -> Vec<Value> {
        vars.iter()
            .map(|(name, v)| {
                let id = self.var_id(name);
                let val = self.value(v);
                self.block("variables_set", json!({"VAR": {"id": id}}), vec![("VALUE", Some(val))], None)
            })
            .collect()
    }

    // --- triggers -----------------------------------------------------------

    fn triggers(&mut self, list: &[Value]) -> Vec<Value> {
        list.iter().map(|t| self.trigger(t)).collect()
    }

    fn raw_trigger(&mut self, t: &Value) -> Value {
        self.raw += 1;
        self.block("ha_trigger_raw", json!({"CONFIG": yaml(t)}), vec![], None)
    }

    fn trigger(&mut self, t: &Value) -> Value {
        let Some(m) = t.as_object() else { return self.raw_trigger(t) };
        let platform = m.get("trigger").or_else(|| m.get("platform")).and_then(Value::as_str).unwrap_or("");
        let base = ["trigger", "platform"];
        let with = |extra: &[&'static str]| -> Vec<&'static str> {
            base.iter().copied().chain(extra.iter().copied()).collect()
        };
        let s = |k: &str| m.get(k).and_then(Value::as_str);
        match platform {
            "state"
                if only_keys(m, &with(&["entity_id", "from", "to", "for"]))
                    && s("entity_id").is_some()
                    && m.get("from").is_none_or(Value::is_string)
                    && m.get("to").is_none_or(Value::is_string)
                    && m.get("for").is_none_or(|f| seconds(f).is_some()) =>
            {
                let fields = json!({
                    "ENTITY": s("entity_id"), "FROM": s("from").unwrap_or(""), "TO": s("to").unwrap_or(""),
                    "FOR": m.get("for").and_then(seconds).unwrap_or(0.0),
                });
                self.block("ha_trigger_state", fields, vec![], None)
            }
            "numeric_state"
                if only_keys(m, &with(&["entity_id", "above", "below"]))
                    && s("entity_id").is_some()
                    && m.get("above").is_none_or(Value::is_number)
                    && m.get("below").is_none_or(Value::is_number) =>
            {
                let n = |k: &str| m.get(k).map(|v| v.to_string()).unwrap_or_default();
                self.block(
                    "ha_trigger_numeric",
                    json!({"ENTITY": s("entity_id"), "ABOVE": n("above"), "BELOW": n("below")}),
                    vec![],
                    None,
                )
            }
            "time" if only_keys(m, &with(&["at"])) && s("at").is_some() => {
                self.block("ha_trigger_time", json!({"AT": s("at")}), vec![], None)
            }
            "event" if only_keys(m, &with(&["event_type"])) && s("event_type").is_some() => {
                self.block("ha_trigger_event", json!({"EVENT_TYPE": s("event_type")}), vec![], None)
            }
            "sun"
                if only_keys(m, &with(&["event", "offset"]))
                    && m.get("offset").is_none_or(|o| offset_seconds(o).is_some()) =>
            {
                let off = m.get("offset").and_then(offset_seconds).unwrap_or(0.0);
                self.block(
                    "ha_trigger_sun",
                    json!({"EVENT": s("event").unwrap_or("sunset"), "OFFSET": off}),
                    vec![],
                    None,
                )
            }
            "homeassistant" if only_keys(m, &with(&["event"])) && s("event") == Some("start") => {
                self.block("ha_trigger_ha_start", json!({}), vec![], None)
            }
            _ => self.raw_trigger(t),
        }
    }

    // --- conditions -----------------------------------------------------------

    fn and_all(&mut self, conds: &[Value]) -> Value {
        let mut blocks: Vec<Value> = conds.iter().map(|c| self.condition(c)).collect();
        let mut acc = blocks.remove(0);
        for next in blocks {
            acc = self.block(
                "logic_operation",
                json!({"OP": "AND"}),
                vec![("A", Some(acc)), ("B", Some(next))],
                None,
            );
        }
        acc
    }

    fn or_all(&mut self, conds: &[Value]) -> Value {
        let mut blocks: Vec<Value> = conds.iter().map(|c| self.condition(c)).collect();
        let mut acc = blocks.remove(0);
        for next in blocks {
            acc = self.block(
                "logic_operation",
                json!({"OP": "OR"}),
                vec![("A", Some(acc)), ("B", Some(next))],
                None,
            );
        }
        acc
    }

    fn raw_condition(&mut self, c: &Value) -> Value {
        self.raw += 1;
        let cfg = match c {
            // Template shorthand: `- "{{ is_state('x', 'on') }}"`.
            Value::String(t) => json!({"condition": "template", "value_template": t}),
            other => other.clone(),
        };
        self.block("ha_condition_raw", json!({"CONFIG": yaml(&cfg)}), vec![], None)
    }

    fn condition(&mut self, c: &Value) -> Value {
        let Some(m) = c.as_object() else { return self.raw_condition(c) };
        let kind = m.get("condition").and_then(Value::as_str).unwrap_or("");
        let sub = list(m.get("conditions"));
        match kind {
            "and" if !sub.is_empty() && only_keys(m, &["condition", "conditions"]) => self.and_all(&sub),
            "or" if !sub.is_empty() && only_keys(m, &["condition", "conditions"]) => self.or_all(&sub),
            "not" if !sub.is_empty() && only_keys(m, &["condition", "conditions"]) => {
                let inner = self.or_all(&sub);
                self.block("logic_negate", json!({}), vec![("BOOL", Some(inner))], None)
            }
            "state"
                if only_keys(m, &["condition", "entity_id", "state"])
                    && m.get("entity_id").is_some_and(Value::is_string)
                    && m.get("state").is_some_and(Value::is_string) =>
            {
                self.block(
                    "ha_is_state",
                    json!({"ENTITY": m["entity_id"], "STATE": m["state"]}),
                    vec![],
                    None,
                )
            }
            _ => self.raw_condition(c),
        }
    }

    // --- actions --------------------------------------------------------------

    fn actions(&mut self, list: &[Value]) -> Vec<Value> {
        list.iter().map(|a| self.action(a)).collect()
    }

    fn raw_action(&mut self, a: &Value) -> Value {
        self.raw += 1;
        self.block("ha_action_raw", json!({"CONFIG": yaml(a)}), vec![], None)
    }

    fn action(&mut self, a: &Value) -> Value {
        let Some(m) = a.as_object() else { return self.raw_action(a) };
        // Options the blocks don't model keep the action raw so behaviour is unchanged.
        if m.contains_key("continue_on_error") || m.get("enabled") == Some(&json!(false)) {
            return self.raw_action(a);
        }
        let common = ["alias"];
        let ok = |allowed: &[&str]| {
            m.keys().all(|k| allowed.contains(&k.as_str()) || common.contains(&k.as_str()))
        };

        if let Some(svc) = m.get("action").or_else(|| m.get("service")).and_then(Value::as_str) {
            if !ok(&["action", "service", "target", "data", "entity_id", "service_data"]) || is_template(svc)
            {
                return self.raw_action(a);
            }
            let mut target = m.get("target").cloned();
            if let Some(e) = m.get("entity_id") {
                target = Some(json!({"entity_id": e}));
            }
            let target_block = target.map(|t| self.target(&t));
            let data = m.get("data").or_else(|| m.get("service_data")).and_then(Value::as_object).cloned();
            let data_block = data.filter(|d| !d.is_empty()).map(|d| self.dict(&d));
            return self.block(
                "ha_call_service",
                json!({"SERVICE": svc}),
                vec![("TARGET", target_block), ("DATA", data_block)],
                None,
            );
        }
        if let Some(d) = m.get("delay") {
            if !ok(&["delay"]) {
                return self.raw_action(a);
            }
            return match seconds(d) {
                Some(s) => {
                    let n = self.block("math_number", json!({"NUM": s}), vec![], None);
                    self.block("ha_wait", json!({}), vec![("SECONDS", Some(n))], None)
                }
                None => self.raw_action(a),
            };
        }
        if let Some(t) = m.get("wait_for_trigger") {
            if !ok(&["wait_for_trigger", "timeout", "continue_on_timeout"])
                || m.get("continue_on_timeout") == Some(&json!(false))
            {
                return self.raw_action(a);
            }
            let trig = self.triggers(&list(Some(t)));
            let timeout = match m.get("timeout") {
                Some(t) => match seconds(t) {
                    Some(s) => Some(self.block("math_number", json!({"NUM": s}), vec![], None)),
                    None => return self.raw_action(a),
                },
                None => None,
            };
            return self.block(
                "ha_wait_for_trigger",
                json!({}),
                vec![("TRIGGERS", chain(trig)), ("TIMEOUT", timeout)],
                None,
            );
        }
        if m.contains_key("if") && ok(&["if", "then", "else"]) {
            let cond = self.and_all(&list(m.get("if")));
            let then = self.actions(&list(m.get("then")));
            let els = m.get("else").map(|e| self.actions(&list(Some(e))));
            let has_else = els.is_some();
            return self.block(
                "controls_if",
                json!({}),
                vec![("IF0", Some(cond)), ("DO0", chain(then)), ("ELSE", els.and_then(chain))],
                has_else.then(|| json!({"hasElse": true})),
            );
        }
        if let Some(options) = m.get("choose").and_then(Value::as_array) {
            if !ok(&["choose", "default"]) || options.is_empty() {
                return self.raw_action(a);
            }
            let mut inputs: Vec<(String, Option<Value>)> = Vec::new();
            for (i, opt) in options.iter().enumerate() {
                let conds = list(opt.get("conditions"));
                if conds.is_empty() {
                    return self.raw_action(a);
                }
                inputs.push((format!("IF{i}"), Some(self.and_all(&conds))));
                let seq = self.actions(&list(opt.get("sequence")));
                inputs.push((format!("DO{i}"), chain(seq)));
            }
            let default = m.get("default").map(|d| self.actions(&list(Some(d))));
            let has_else = default.is_some();
            if let Some(d) = default {
                inputs.push(("ELSE".into(), chain(d)));
            }
            let mut extra = json!({"elseIfCount": options.len() - 1});
            if has_else {
                extra["hasElse"] = json!(true);
            }
            let refs: Vec<(&str, Option<Value>)> =
                inputs.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
            return self.block("controls_if", json!({}), refs, Some(extra));
        }
        if let Some(r) = m.get("repeat").and_then(Value::as_object)
            && ok(&["repeat"])
        {
            let seq = self.actions(&list(r.get("sequence")));
            if let Some(count) = r.get("count").and_then(Value::as_f64) {
                let n = self.block("math_number", json!({"NUM": count}), vec![], None);
                return self.block(
                    "controls_repeat_ext",
                    json!({}),
                    vec![("TIMES", Some(n)), ("DO", chain(seq))],
                    None,
                );
            }
            if let Some(w) = r.get("while") {
                let cond = self.and_all(&list(Some(w)));
                return self.block(
                    "controls_whileUntil",
                    json!({"MODE": "WHILE"}),
                    vec![("BOOL", Some(cond)), ("DO", chain(seq))],
                    None,
                );
            }
            // `until` checks after the body (do-while) and `for_each` exposes `repeat.item`
            // to templates; neither maps exactly, so keep them native.
            return self.raw_action(a);
        }
        if let Some(vars) = m.get("variables").and_then(Value::as_object)
            && ok(&["variables"])
        {
            let sets = self.variables(vars);
            return self.sequence_block(sets, a);
        }
        if m.contains_key("stop") && ok(&["stop", "error"]) {
            if m.get("error") == Some(&json!(true)) {
                let msg = self.value(&m["stop"]);
                return self.block("control_throw", json!({}), vec![("MESSAGE", Some(msg))], None);
            }
            return self.block("control_stop", json!({}), vec![], None);
        }
        if let Some(ev) = m.get("event").and_then(Value::as_str)
            && ok(&["event", "event_data"])
        {
            let t = self.block("text", json!({"TEXT": ev}), vec![], None);
            let data = m.get("event_data").and_then(Value::as_object).cloned().map(|d| self.dict(&d));
            return self.block(
                "ha_fire_event",
                json!({}),
                vec![("EVENT_TYPE", Some(t)), ("DATA", data)],
                None,
            );
        }
        if let Some(branches) = m.get("parallel").and_then(Value::as_array)
            && ok(&["parallel"])
        {
            let bs: Vec<Value> = branches
                .iter()
                .map(|br| {
                    let seq = match br.get("sequence") {
                        Some(s) if br.as_object().is_some_and(|o| o.len() == 1) => {
                            self.actions(&list(Some(s)))
                        }
                        _ => vec![self.action(br)],
                    };
                    self.block("control_branch", json!({}), vec![("DO", chain(seq))], None)
                })
                .collect();
            return self.block("control_parallel", json!({}), vec![("BRANCHES", chain(bs))], None);
        }
        if m.contains_key("condition") {
            // Inline condition action: stop the run when false.
            let cond = self.condition(a);
            let not = self.block("logic_negate", json!({}), vec![("BOOL", Some(cond))], None);
            let stop = self.block("control_stop", json!({}), vec![], None);
            return self.block("controls_if", json!({}), vec![("IF0", Some(not)), ("DO0", Some(stop))], None);
        }
        if let Some(scene) = m.get("scene").and_then(Value::as_str)
            && ok(&["scene"])
        {
            let t = self.target(&json!({"entity_id": scene}));
            return self.block(
                "ha_call_service",
                json!({"SERVICE": "scene.turn_on"}),
                vec![("TARGET", Some(t))],
                None,
            );
        }
        self.raw_action(a)
    }

    /// Wraps several statements where one is expected.
    fn sequence_block(&mut self, mut blocks: Vec<Value>, original: &Value) -> Value {
        match blocks.len() {
            0 => self.raw_action(original),
            1 => blocks.remove(0),
            _ => {
                let br = self.block("control_branch", json!({}), vec![("DO", chain(blocks))], None);
                // A single parallel branch runs sequentially.
                self.block("control_parallel", json!({}), vec![("BRANCHES", Some(br))], None)
            }
        }
    }

    fn target(&mut self, t: &Value) -> Value {
        let entities = |v: &Value| -> Option<String> {
            match v {
                Value::String(s) if !is_template(s) => Some(s.clone()),
                Value::Array(a) if a.iter().all(|x| x.as_str().is_some_and(|s| !is_template(s))) => {
                    Some(a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(","))
                }
                _ => None,
            }
        };
        if let Some(m) = t.as_object()
            && m.len() == 1
            && let Some(ids) = m.get("entity_id").and_then(entities)
        {
            return self.block("ha_entity", json!({"ENTITY": ids}), vec![], None);
        }
        self.value(t)
    }
}

/// `"-00:30:00"` / `"01:00:00"` / seconds → signed seconds.
fn offset_seconds(v: &Value) -> Option<f64> {
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
