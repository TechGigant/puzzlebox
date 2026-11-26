//! Actions and script flow: HA action sequences → statement blocks.
//!
//! An action becomes native blocks only when they run exactly like Home Assistant would;
//! otherwise it stays a raw YAML block, which HA runs as a script of its own. Such a script
//! can't change the program's variables or stop the program, so when later actions depend on
//! that (a variable it sets, the `wait` result, a `stop` inside it), the raw block grows to
//! include them: the rest of the sequence, or the enclosing loop/parallel/sequence.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use crate::*;

/// Effects of an imported action that the program won't see, because they happen inside a
/// raw block (or a native block that differs there). Harmless unless later actions read them.
#[derive(Default, Clone, Debug)]
struct Lost {
    /// Variables assigned inside a raw block.
    names: BTreeSet<String>,
    /// A wait inside a raw block: `wait` isn't set at all.
    wait_lost: bool,
    /// A native "wait for trigger": `wait` has only `trigger` (no `remaining`/`completed`).
    wait_partial: bool,
    /// ...with several triggers, which HA numbers 0, 1… but Puzzlebox subscribes one by one.
    wait_multi: bool,
    /// A native wait in a parallel branch: HA keeps that `wait` inside the branch.
    wait_leak: bool,
    /// A stop/abort inside a raw block, which only ends that block.
    halts: bool,
}

impl Lost {
    fn merge(&mut self, o: Lost) {
        self.names.extend(o.names);
        self.wait_lost |= o.wait_lost;
        self.wait_partial |= o.wait_partial;
        self.wait_multi |= o.wait_multi;
        self.wait_leak |= o.wait_leak;
        self.halts |= o.halts;
    }

    /// Whether running `later` (actions or conditions) after this could tell the difference.
    fn observable(&self, later: &[Value]) -> bool {
        (self.halts && later.iter().any(enabled))
            || self.names.iter().any(|n| later.iter().any(|v| !uses(v, n).is_empty()))
            || ((self.wait_lost || self.wait_leak) && later.iter().any(|v| !uses(v, "wait").is_empty()))
            || (self.wait_partial
                && later.iter().any(|v| uses(v, "wait").iter().any(|u| !wait_ok(u, self.wait_multi))))
    }
}

/// Value sockets of a helper block: (socket, YAML value, kind: `n` Number, `s` String, `a` any).
type Inputs<'a> = Vec<(&'a str, Value, char)>;

/// Action keys in the order HA uses to tell what an action is (`cv.ACTIONS_MAP`).
const KINDS: [&str; 21] = [
    "delay",
    "wait_template",
    "condition",
    "and",
    "or",
    "not",
    "event",
    "device_id",
    "scene",
    "repeat",
    "choose",
    "wait_for_trigger",
    "variables",
    "if",
    "action",
    "service",
    "service_template",
    "stop",
    "parallel",
    "sequence",
    "set_conversation_response",
];

fn kind(m: &Map<String, Value>) -> &'static str {
    match KINDS.iter().find(|k| m.contains_key(**k)) {
        Some(&("and" | "or" | "not")) => "condition",
        Some(&("service" | "action")) => "action",
        Some(k) => k,
        None => "",
    }
}

fn enabled(v: &Value) -> bool {
    v.get("enabled") != Some(&json!(false))
}

/// HA's `is_template_string`.
fn is_tpl(s: &str) -> bool {
    s.contains('{') && (s.contains("{{") || s.contains("{%") || s.contains("{#"))
}

fn has_tpl(v: &Value) -> bool {
    match v {
        Value::String(s) => is_tpl(s),
        Value::Array(a) => a.iter().any(has_tpl),
        Value::Object(m) => m.iter().any(|(k, v)| is_tpl(k) || has_tpl(v)),
        _ => false,
    }
}

/// HA's `valid_entity_id` (also used for action names), after lower-casing like HA does.
fn entity_id(s: &str) -> Option<String> {
    let s = s.trim().to_lowercase();
    let (a, b) = s.split_once('.')?;
    let part = |p: &str| {
        !p.is_empty()
            && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            && !p.starts_with('_')
            && !p.ends_with('_')
    };
    (part(a) && part(b) && !s.contains("__")).then_some(s)
}

/// A time period as HA's `cv.positive_time_period` reads it: `"HH:MM[:SS.f]"`, seconds (number
/// or numeric string) or `{days, hours, minutes, seconds, milliseconds}`. Templates: `None`.
fn period(v: &Value) -> Option<f64> {
    let secs = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) if is_tpl(s) => return None,
        Value::String(s) => {
            let (sign, rest) = match s.strip_prefix('-') {
                Some(r) => (-1.0, r),
                None => (1.0, s.strip_prefix('+').unwrap_or(s)),
            };
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() == 2 || parts.len() == 3 {
                let h: i64 = parts[0].trim().parse().ok()?;
                let m: i64 = parts[1].trim().parse().ok()?;
                let sec: f64 = match parts.get(2) {
                    Some(p) => p.trim().parse().ok()?,
                    None => 0.0,
                };
                sign * (h as f64 * 3600.0 + m as f64 * 60.0 + sec)
            } else {
                s.trim().parse().ok()?
            }
        }
        Value::Object(m) if !m.is_empty() => {
            let mut total = 0.0;
            for (k, x) in m {
                let factor = match k.as_str() {
                    "days" => 86400.0,
                    "hours" => 3600.0,
                    "minutes" => 60.0,
                    "seconds" => 1.0,
                    "milliseconds" => 0.001,
                    _ => return None,
                };
                let n: f64 = match x {
                    Value::Number(n) => n.as_f64()?,
                    Value::String(s) if !is_tpl(s) => s.trim().parse().ok()?,
                    _ => return None,
                };
                total += n * factor;
            }
            total
        }
        _ => return None,
    };
    (secs.is_finite() && secs >= 0.0).then_some(secs)
}

/// The parts of a template HA evaluates (`{{ … }}`, `{% … %}`).
fn segments(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    loop {
        let open = [rest.find("{{"), rest.find("{%")].into_iter().flatten().min();
        let Some(i) = open else { break };
        let close = if rest[i..].starts_with("{{") { "}}" } else { "%}" };
        let body = &rest[i + 2..];
        match body.find(close) {
            Some(j) => {
                out.push(&body[..j]);
                rest = &body[j + 2..];
            }
            None => {
                out.push(body);
                break;
            }
        }
    }
    out
}

fn ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// What follows each use of the variable `name` in the templates (and `response_variable`s)
/// of `v`. Conservative: anything that looks like the name counts.
fn uses<'a>(v: &'a Value, name: &str) -> Vec<&'a str> {
    fn walk<'a>(v: &'a Value, name: &str, out: &mut Vec<&'a str>) {
        match v {
            Value::String(s) if is_tpl(s) => {
                for seg in segments(s) {
                    let mut from = 0;
                    while let Some(i) = seg[from..].find(name) {
                        let at = from + i;
                        let end = at + name.len();
                        let before = seg[..at].chars().next_back();
                        let after = seg[end..].chars().next();
                        if !before.is_some_and(|c| ident_char(c) || c == '.')
                            && !after.is_some_and(ident_char)
                        {
                            out.push(&seg[end..]);
                        }
                        from = end;
                    }
                }
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, name, out)),
            Value::Object(m) => {
                for (k, x) in m {
                    if k == "response_variable" && x.as_str() == Some(name) {
                        out.push("");
                    }
                    walk(x, name, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(v, name, &mut out);
    out
}

/// A use of `wait` that a native "wait for trigger" gets right: `wait.trigger…` (with several
/// triggers, not its `id`/`idx`, which HA numbers by position).
fn wait_ok(after: &str, multi: bool) -> bool {
    let rest = [".trigger", "['trigger']", "[\"trigger\"]"].iter().find_map(|p| after.strip_prefix(p));
    let Some(rest) = rest else { return false };
    if rest.chars().next().is_some_and(ident_char) {
        return false;
    }
    let field = [".", "['", "[\""]
        .iter()
        .find_map(|p| rest.strip_prefix(p))
        .map(|f| f.chars().take_while(|c| ident_char(*c)).collect::<String>());
    !(multi && matches!(field.as_deref(), Some("id" | "idx")))
}

/// Whether `repeat` (the loop variable HA sets) is used where this loop's value would be seen.
fn uses_repeat(r: &Map<String, Value>) -> bool {
    fn walk(v: &Value) -> bool {
        match v {
            Value::Object(m) => {
                if let Some(inner) = m.get("repeat").and_then(Value::as_object) {
                    // A nested loop's body and conditions see its own `repeat`.
                    let own = ["count", "for_each"].iter().filter_map(|k| inner.get(*k)).any(walk);
                    return own || m.iter().filter(|(k, _)| *k != "repeat").any(|(_, x)| walk(x));
                }
                m.values().any(walk)
            }
            Value::Array(a) => a.iter().any(walk),
            other => !uses(other, "repeat").is_empty(),
        }
    }
    ["sequence", "while", "until"].iter().filter_map(|k| r.get(*k)).any(walk)
}

/// What a raw block hides from the rest of the program.
fn raw_lost(v: &Value) -> Lost {
    fn walk(v: &Value, l: &mut Lost) {
        match v {
            Value::Array(a) => a.iter().for_each(|x| walk(x, l)),
            Value::Object(m) if enabled(v) => {
                if let Some(vars) = m.get("variables").and_then(Value::as_object) {
                    l.names.extend(vars.keys().cloned());
                }
                if let Some(r) = m.get("response_variable").and_then(Value::as_str) {
                    l.names.insert(r.to_owned());
                }
                if m.contains_key("wait_template") || m.contains_key("wait_for_trigger") {
                    l.wait_lost = true;
                }
                if m.contains_key("stop") || m.get("continue_on_timeout") == Some(&json!(false)) {
                    l.halts = true;
                }
                m.values().for_each(|x| walk(x, l));
            }
            _ => {}
        }
    }
    let mut l = Lost::default();
    walk(v, &mut l);
    l
}

fn disable(b: &mut Value) {
    b["disabledReasons"] = json!(["MANUALLY_DISABLED"]);
}

fn is_whole(f: f64) -> bool {
    f.fract() == 0.0 && f.abs() <= 9_007_199_254_740_992.0
}

impl Builder {
    pub(crate) fn actions(&mut self, list: &[Value]) -> Vec<Value> {
        self.seq(list).0
    }

    pub(crate) fn raw_action(&mut self, a: &Value) -> Value {
        self.raw += 1;
        self.block("ha_action_raw", json!({"CONFIG": yaml(a)}), vec![], None)
    }

    fn raw_list(&mut self, list: &[Value]) -> Value {
        match list {
            [one] => self.raw_action(one),
            many => self.raw_action(&Value::Array(many.to_vec())),
        }
    }

    /// A sequence of actions (one HA script scope) → statements, and what its raw parts hide.
    fn seq(&mut self, list: &[Value]) -> (Vec<Value>, Lost) {
        // Nothing after an (always) enabled stop runs, and the stop block can't have a next.
        let end = list
            .iter()
            .position(|a| {
                a.as_object().is_some_and(|m| {
                    kind(m) == "stop" && matches!(m.get("enabled"), None | Some(Value::Bool(true)))
                })
            })
            .map_or(list.len(), |i| i + 1);
        let list = &list[..end];
        let mut out = Vec::new();
        let mut lost = Lost::default();
        for (i, a) in list.iter().enumerate() {
            let rest = &list[i + 1..];
            let snap = self.snapshot();
            // A failing condition ends its own sequence only (the enclosing one carries on),
            // so the rest of this sequence goes inside an "if".
            if let Some(m) = a.as_object()
                && kind(m) == "condition"
                && enabled(a)
            {
                match self.condition_action(m) {
                    Some(cond) => {
                        let (inner, l) = self.seq(rest);
                        let mut b = self.block(
                            "controls_if",
                            json!({}),
                            vec![("IF0", Some(cond)), ("DO0", chain(inner))],
                            None,
                        );
                        self.comment(&mut b, m);
                        out.push(b);
                        lost.merge(l);
                    }
                    None => {
                        self.restore(snap);
                        out.push(self.raw_list(&list[i..]));
                        lost.merge(raw_lost(&Value::Array(list[i..].to_vec())));
                    }
                }
                return (out, lost);
            }
            let (blocks, l) = self.act(a);
            if l.observable(rest) {
                self.restore(snap);
                out.push(self.raw_list(&list[i..]));
                lost.merge(raw_lost(&Value::Array(list[i..].to_vec())));
                break;
            }
            out.extend(blocks);
            lost.merge(l);
        }
        (out, lost)
    }

    fn raw_one(&mut self, a: &Value) -> (Vec<Value>, Lost) {
        let b = self.raw_action(a);
        (vec![b], raw_lost(a))
    }

    fn comment(&self, b: &mut Value, m: &Map<String, Value>) {
        if let Some(alias) = m.get("alias").and_then(Value::as_str) {
            b["icons"] = json!({"comment": {"text": alias, "pinned": false}});
        }
    }

    /// One action → statements (usually one), and what its raw parts hide.
    fn act(&mut self, a: &Value) -> (Vec<Value>, Lost) {
        let Some(m) = a.as_object() else { return self.raw_one(a) };
        let k = kind(m);
        let mut m2 = m.clone();
        m2.remove("alias");
        let disabled = match m2.remove("enabled") {
            None | Some(Value::Bool(true)) => false,
            Some(Value::Bool(false)) => true,
            Some(_) => return self.raw_one(a),
        };
        if disabled {
            // A disabled stop would still end the chain in the editor.
            let mut blocks = if k == "stop" {
                vec![self.raw_action(a)]
            } else {
                let mut c = m.clone();
                c.remove("enabled");
                if k == "condition" {
                    match self.condition_action(&c) {
                        Some(cond) => {
                            vec![self.block("controls_if", json!({}), vec![("IF0", Some(cond))], None)]
                        }
                        None => vec![self.raw_action(a)],
                    }
                } else {
                    self.act(&Value::Object(c)).0
                }
            };
            blocks.iter_mut().for_each(disable);
            return (blocks, Lost::default());
        }
        match m2.remove("continue_on_error") {
            None | Some(Value::Bool(false)) => {}
            // Errors these raise stop the script even with continue_on_error.
            Some(Value::Bool(true)) if matches!(k, "stop" | "variables" | "delay" | "condition") => {}
            Some(_) => return self.raw_one(a),
        }
        let snap = self.snapshot();
        let res = match k {
            "delay" => self.delay(&m2),
            "event" => self.fire_event(&m2),
            "device_id" => self.device_action(&m2),
            "scene" => self.scene(&m2),
            "repeat" => self.repeat(&m2),
            "choose" => self.choose(&m2),
            "wait_for_trigger" => self.wait_for_trigger(&m2),
            "variables" => self.set_variables(&m2),
            "if" => self.if_then(&m2),
            "action" => self.service(&m2),
            "stop" => self.stop(&m2),
            "parallel" => self.parallel(&m2),
            "sequence" if only_keys(&m2, &["sequence", "metadata"]) => {
                Some(self.seq(&list(m2.get("sequence"))))
            }
            "condition" => {
                // Only reached through `action()` on a lone condition: nothing follows it.
                self.condition_action(&m2).map(|c| {
                    let b = self.block("controls_if", json!({}), vec![("IF0", Some(c))], None);
                    (vec![b], Lost::default())
                })
            }
            // wait_template: HA reacts to every state change and sets `wait.remaining`; the
            // "wait until" block polls once a second. set_conversation_response, templated
            // action names and the rest have no block.
            _ => None,
        };
        match res {
            Some((mut blocks, lost)) => {
                if let Some(first) = blocks.first_mut() {
                    self.comment(first, m);
                }
                (blocks, lost)
            }
            None => {
                self.restore(snap);
                self.raw_one(a)
            }
        }
    }

    /// The condition of a condition action (shorthands expanded), for the conditions module.
    fn condition_action(&mut self, m: &Map<String, Value>) -> Option<Value> {
        let mut c = m.clone();
        c.remove("alias");
        c.remove("continue_on_error");
        if c.contains_key("enabled") {
            return None;
        }
        let cfg = match c.get("condition") {
            Some(Value::String(t)) if is_tpl(t) => {
                c.insert("value_template".into(), json!(t));
                c.insert("condition".into(), json!("template"));
                Value::Object(c)
            }
            Some(_) => Value::Object(c),
            None => {
                let key = ["and", "or", "not"].into_iter().find(|k| c.contains_key(*k))?;
                if c.len() != 1 {
                    return None;
                }
                json!({"condition": key, "conditions": c[key]})
            }
        };
        Some(self.condition(&cfg))
    }

    /// Conditions that must all hold (an empty list holds).
    fn conds(&mut self, list: &[Value]) -> Value {
        if list.is_empty() {
            self.block("logic_boolean", json!({"BOOL": "TRUE"}), vec![], None)
        } else {
            self.and_all(list)
        }
    }

    /// A value block giving exactly the JSON HA would get (templates: rendered by HA).
    fn lit(&mut self, v: &Value) -> Option<Value> {
        Some(match v {
            Value::String(s) if is_tpl(s) => self.template_value(s),
            Value::String(s) => self.block("text", json!({"TEXT": s}), vec![], None),
            Value::Number(n) => match n.as_f64() {
                // The number block turns 2.0 into 2 and loses digits past 2^53.
                Some(f) if f.fract() != 0.0 || (is_whole(f) && (n.is_i64() || n.is_u64())) => {
                    self.block("math_number", json!({"NUM": n}), vec![], None)
                }
                _ => self.block("yaml_value", json!({"CONFIG": n.to_string()}), vec![], None),
            },
            Value::Bool(b) => {
                self.block("logic_boolean", json!({"BOOL": if *b { "TRUE" } else { "FALSE" }}), vec![], None)
            }
            Value::Null => self.block("logic_null", json!({}), vec![], None),
            Value::Array(items) => {
                let blocks = items.iter().map(|i| self.lit(i)).collect::<Option<Vec<_>>>()?;
                let mut b = self.block(
                    "lists_create_with",
                    json!({}),
                    vec![],
                    Some(json!({"itemCount": blocks.len()})),
                );
                let inputs: Map<String, Value> = blocks
                    .into_iter()
                    .enumerate()
                    .map(|(i, x)| (format!("ADD{i}"), json!({"block": x})))
                    .collect();
                if !inputs.is_empty() {
                    b["inputs"] = Value::Object(inputs);
                }
                b
            }
            Value::Object(m) => {
                let mut pairs = Vec::new();
                for (k, x) in m {
                    // HA renders keys too.
                    if is_tpl(k) {
                        return None;
                    }
                    let val = self.lit(x)?;
                    pairs.push(self.block("dict_pair", json!({"KEY": k}), vec![("VALUE", Some(val))], None));
                }
                self.block("dict_create", json!({}), vec![("ITEMS", chain(pairs))], None)
            }
        })
    }

    fn num(&mut self, n: f64) -> Value {
        let v = if is_whole(n) { json!(n as i64) } else { json!(n) };
        self.block("math_number", json!({"NUM": v}), vec![], None)
    }

    fn delay(&mut self, m: &Map<String, Value>) -> Option<(Vec<Value>, Lost)> {
        if !only_keys(m, &["delay"]) {
            return None;
        }
        let s = period(&m["delay"])?;
        let d = self.duration(s);
        Some((vec![self.block("ha_wait", json!({}), vec![("SECONDS", Some(d))], None)], Lost::default()))
    }

    fn fire_event(&mut self, m: &Map<String, Value>) -> Option<(Vec<Value>, Lost)> {
        if !only_keys(m, &["event", "event_data", "event_data_template"]) {
            return None;
        }
        let ty = m["event"].as_str()?;
        let mut data = Map::new();
        for k in ["event_data", "event_data_template"] {
            match m.get(k) {
                None => {}
                Some(Value::Object(d)) => data.extend(d.clone()),
                Some(_) => return None,
            }
        }
        // HA leaves out data whose template fails and fires anyway; the block would fail.
        if has_tpl(&Value::Object(data.clone())) {
            return None;
        }
        let t = self.block("text", json!({"TEXT": ty}), vec![], None);
        let d = if data.is_empty() { None } else { Some(self.lit(&Value::Object(data))?) };
        let b = self.block("ha_fire_event", json!({}), vec![("EVENT_TYPE", Some(t)), ("DATA", d)], None);
        Some((vec![b], Lost::default()))
    }

    /// Device actions run through HA exactly as written; the block just shows them.
    fn device_action(&mut self, m: &Map<String, Value>) -> Option<(Vec<Value>, Lost)> {
        let domain = m.get("domain")?.as_str()?;
        m.get("device_id")?.as_str()?;
        let what = m.get("type").and_then(Value::as_str).unwrap_or("action").replace('_', " ");
        let config = Value::Object(m.clone());
        let label = format!("{domain}: {what}");
        let b = self.block(
            "device_action",
            json!({}),
            vec![],
            Some(json!({"config": config, "label": label, "params": []})),
        );
        Some((vec![b], Lost::default()))
    }

    fn scene(&mut self, m: &Map<String, Value>) -> Option<(Vec<Value>, Lost)> {
        if !only_keys(m, &["scene"]) {
            return None;
        }
        let scene = entity_id(m["scene"].as_str()?).filter(|s| s.starts_with("scene."))?;
        let t = self.block("ha_entity", json!({"ENTITY": scene}), vec![], None);
        Some((
            vec![self.block("scene_activate", json!({}), vec![("TARGET", Some(t))], None)],
            Lost::default(),
        ))
    }

    fn repeat(&mut self, m: &Map<String, Value>) -> Option<(Vec<Value>, Lost)> {
        let r = m.get("repeat")?.as_object()?;
        let modes = ["count", "for_each", "while", "until"];
        if !only_keys(m, &["repeat"])
            || !only_keys(r, &["count", "for_each", "while", "until", "sequence"])
            || modes.iter().filter(|k| r.contains_key(**k)).count() != 1
            || !r.contains_key("sequence")
            // HA sets `repeat` (index, first, last, item) for the body; no block does.
            || uses_repeat(r)
        {
            return None;
        }
        let body_yaml = list(r.get("sequence"));
        let cond_yaml = list(r.get("while").or_else(|| r.get("until")));
        let times = match (r.get("count"), r.get("for_each")) {
            (Some(Value::Number(n)), _) => Some(n.as_f64()?.trunc()),
            (Some(Value::String(s)), _) if !is_tpl(s) => Some(s.trim().parse::<i64>().ok()? as f64),
            (Some(_), _) => return None,
            // Only the item count matters when the body doesn't use `repeat.item`.
            (_, Some(Value::Array(items))) => Some(items.len() as f64),
            (_, Some(_)) => return None,
            _ => None,
        };
        let (mut body, lost) = self.seq(&body_yaml);
        // What the body hides is seen by its next round and by the loop condition.
        let mut again = body_yaml.clone();
        again.extend(cond_yaml.iter().cloned());
        if lost.observable(&again) {
            return None;
        }
        let b = if let Some(n) = times {
            let n = self.num(n);
            self.block("controls_repeat_ext", json!({}), vec![("TIMES", Some(n)), ("DO", chain(body))], None)
        } else if r.contains_key("while") {
            let c = self.conds(&cond_yaml);
            self.block(
                "controls_whileUntil",
                json!({"MODE": "WHILE"}),
                vec![("BOOL", Some(c)), ("DO", chain(body))],
                None,
            )
        } else {
            // HA checks `until` after each round: loop forever, leave when it holds.
            if body.last().is_none_or(|l| l["type"] != "control_stop") {
                let c = self.conds(&cond_yaml);
                let brk = self.block("controls_flow_statements", json!({"FLOW": "BREAK"}), vec![], None);
                body.push(self.block(
                    "controls_if",
                    json!({}),
                    vec![("IF0", Some(c)), ("DO0", Some(brk))],
                    None,
                ));
            }
            let t = self.block("logic_boolean", json!({"BOOL": "TRUE"}), vec![], None);
            self.block(
                "controls_whileUntil",
                json!({"MODE": "WHILE"}),
                vec![("BOOL", Some(t)), ("DO", chain(body))],
                None,
            )
        };
        Some((vec![b], lost))
    }

    fn choose(&mut self, m: &Map<String, Value>) -> Option<(Vec<Value>, Lost)> {
        if !only_keys(m, &["choose", "default"]) {
            return None;
        }
        let options = list(m.get("choose"));
        if options.is_empty() {
            return None;
        }
        let mut lost = Lost::default();
        let mut inputs: Vec<(String, Option<Value>)> = Vec::new();
        for (i, opt) in options.iter().enumerate() {
            let o = opt.as_object()?;
            if !only_keys(o, &["conditions", "sequence", "alias"])
                || !o.contains_key("conditions")
                || !o.contains_key("sequence")
            {
                return None;
            }
            let c = self.conds(&list(o.get("conditions")));
            inputs.push((format!("IF{i}"), Some(c)));
            let (seq, l) = self.seq(&list(o.get("sequence")));
            lost.merge(l);
            inputs.push((format!("DO{i}"), chain(seq)));
        }
        let mut extra = json!({"elseIfCount": options.len() - 1});
        if let Some(d) = m.get("default") {
            let (seq, l) = self.seq(&list(Some(d)));
            lost.merge(l);
            inputs.push(("ELSE".into(), chain(seq)));
            extra["hasElse"] = json!(true);
        }
        let refs: Vec<(&str, Option<Value>)> = inputs.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        Some((vec![self.block("controls_if", json!({}), refs, Some(extra))], lost))
    }

    fn if_then(&mut self, m: &Map<String, Value>) -> Option<(Vec<Value>, Lost)> {
        if !only_keys(m, &["if", "then", "else"]) || !m.contains_key("then") {
            return None;
        }
        let c = self.conds(&list(m.get("if")));
        let (then, mut lost) = self.seq(&list(m.get("then")));
        let mut inputs = vec![("IF0", Some(c)), ("DO0", chain(then))];
        let mut extra = None;
        if let Some(e) = m.get("else") {
            let (els, l) = self.seq(&list(Some(e)));
            lost.merge(l);
            inputs.push(("ELSE", chain(els)));
            extra = Some(json!({"hasElse": true}));
        }
        Some((vec![self.block("controls_if", json!({}), inputs, extra)], lost))
    }

    fn wait_for_trigger(&mut self, m: &Map<String, Value>) -> Option<(Vec<Value>, Lost)> {
        if !only_keys(m, &["wait_for_trigger", "timeout", "continue_on_timeout"]) {
            return None;
        }
        let triggers = list(m.get("wait_for_trigger"));
        // HA renders trigger templates with the script's variables; Puzzlebox subscribes without.
        if triggers.is_empty() || triggers.iter().any(has_tpl) {
            return None;
        }
        // A zero timeout means "time out at once" in HA but "no timeout" for the block.
        let timeout = match m.get("timeout") {
            Some(t) => Some(period(t).filter(|s| *s > 0.0)?),
            None => None,
        };
        let stop_on_timeout = match m.get("continue_on_timeout") {
            None | Some(Value::Bool(true)) => false,
            Some(Value::Bool(false)) => true,
            Some(_) => return None,
        };
        let multi = triggers.len() > 1 || triggers.iter().any(|t| t.get("triggers").is_some());
        let trig = self.triggers(&triggers);
        let timeout = timeout.map(|s| self.duration(s));
        let mut out = vec![self.block(
            "ha_wait_for_trigger",
            json!({}),
            vec![("TRIGGERS", chain(trig)), ("TIMEOUT", timeout)],
            None,
        )];
        if stop_on_timeout {
            let timed_out = self.block("wait_timed_out", json!({}), vec![], None);
            let stop = self.block("control_stop", json!({}), vec![], None);
            out.push(self.block(
                "controls_if",
                json!({}),
                vec![("IF0", Some(timed_out)), ("DO0", Some(stop))],
                None,
            ));
        }
        Some((out, Lost { wait_partial: true, wait_multi: multi, ..Lost::default() }))
    }

    fn set_variables(&mut self, m: &Map<String, Value>) -> Option<(Vec<Value>, Lost)> {
        if !only_keys(m, &["variables"]) {
            return None;
        }
        let vars = m["variables"].as_object()?;
        // Puzzlebox always hands HA its own `trigger`.
        if vars.contains_key("trigger") {
            return None;
        }
        let mut out = Vec::new();
        for (name, v) in vars {
            let val = self.lit(v)?;
            let id = self.var_id(name);
            out.push(self.block(
                "variables_set",
                json!({"VAR": {"id": id}}),
                vec![("VALUE", Some(val))],
                None,
            ));
        }
        Some((out, Lost::default()))
    }

    fn stop(&mut self, m: &Map<String, Value>) -> Option<(Vec<Value>, Lost)> {
        if !only_keys(m, &["stop", "error", "response_variable"]) {
            return None;
        }
        let msg = match &m["stop"] {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            _ => return None,
        };
        let b = match m.get("error") {
            None | Some(Value::Bool(false)) => self.block("control_stop", json!({}), vec![], None),
            Some(Value::Bool(true)) => {
                // The message is plain text in HA, never a template.
                let t = self.block("text", json!({"TEXT": msg}), vec![], None);
                self.block("control_throw", json!({}), vec![("MESSAGE", Some(t))], None)
            }
            Some(_) => return None,
        };
        Some((vec![b], Lost::default()))
    }

    fn parallel(&mut self, m: &Map<String, Value>) -> Option<(Vec<Value>, Lost)> {
        if !only_keys(m, &["parallel"]) {
            return None;
        }
        let branches = list(m.get("parallel"));
        if branches.is_empty() {
            return None;
        }
        let mut lost = Lost::default();
        let mut blocks = Vec::new();
        for (i, br) in branches.iter().enumerate() {
            let seq = match br {
                Value::Array(a) => a.clone(),
                other => vec![other.clone()],
            };
            let (body, l) = self.seq(&seq);
            let others: Vec<Value> =
                branches.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, b)| b.clone()).collect();
            let seen =
                Lost { names: l.names.clone(), wait_leak: l.wait_partial || l.wait_leak, ..Lost::default() };
            if seen.observable(&others) {
                return None;
            }
            // HA keeps each branch's `wait` to itself: a lost one is not missed outside.
            lost.merge(Lost { wait_lost: false, wait_partial: false, ..seen });
            lost.halts |= l.halts;
            blocks.push(self.block("control_branch", json!({}), vec![("DO", chain(body))], None));
        }
        Some((vec![self.block("control_parallel", json!({}), vec![("BRANCHES", chain(blocks))], None)], lost))
    }

    fn service(&mut self, m: &Map<String, Value>) -> Option<(Vec<Value>, Lost)> {
        if m.contains_key("action") && m.contains_key("service") {
            return None;
        }
        // `response_variable` needs an action block that keeps the response.
        if !only_keys(m, &["action", "service", "target", "data", "data_template", "entity_id", "metadata"]) {
            return None;
        }
        let svc = m.get("action").or_else(|| m.get("service"))?.as_str()?;
        if is_tpl(svc) {
            return None;
        }
        let svc = entity_id(svc)?;
        let mut data = Map::new();
        for k in ["data", "data_template"] {
            match m.get(k) {
                None => {}
                Some(Value::Object(d)) => data.extend(d.clone()),
                Some(_) => return None,
            }
        }
        let mut target = match m.get("target") {
            None => Map::new(),
            Some(Value::Object(t)) => t.clone(),
            Some(_) => return None,
        };
        if let Some(e) = m.get("entity_id") {
            if has_tpl(e) {
                return None;
            }
            target.insert("entity_id".into(), e.clone());
        }
        let snap = self.snapshot();
        if let Some(b) = self.helper(&svc, &target, &data) {
            return Some((vec![b], Lost::default()));
        }
        self.restore(snap);
        let t = if target.is_empty() { None } else { Some(self.target_from(&target)?) };
        let d = if data.is_empty() { None } else { Some(self.lit(&Value::Object(data))?) };
        let b =
            self.block("ha_call_service", json!({"SERVICE": svc}), vec![("TARGET", t), ("DATA", d)], None);
        Some((vec![b], Lost::default()))
    }

    fn target_from(&mut self, t: &Map<String, Value>) -> Option<Value> {
        const KEYS: [&str; 5] = ["entity_id", "area_id", "device_id", "floor_id", "label_id"];
        if t.is_empty() || !only_keys(t, &KEYS) {
            return None;
        }
        let strings = |v: &Value| -> Option<Vec<String>> {
            match v {
                Value::String(s) => Some(vec![s.clone()]),
                Value::Array(a) if !a.is_empty() => a.iter().map(|x| x.as_str().map(str::to_owned)).collect(),
                _ => None,
            }
        };
        let mut ids: Vec<(&str, Vec<String>)> = Vec::new();
        for k in KEYS {
            let Some(v) = t.get(k) else { continue };
            let items = strings(v)?;
            let mut out = Vec::new();
            for s in items {
                if is_tpl(&s) {
                    out.push(s);
                } else if k == "entity_id" {
                    // HA splits comma lists; "all"/"none" and registry ids don't survive the block.
                    let parts: Vec<&str> = if v.is_string() { s.split(',').collect() } else { vec![&s] };
                    for p in parts {
                        out.push(entity_id(p)?);
                    }
                } else if !s.trim().is_empty() && !s.contains(',') && s != "none" {
                    out.push(s);
                } else {
                    return None;
                }
            }
            ids.push((k, out));
        }
        if ids.iter().any(|(_, v)| v.iter().any(|s| is_tpl(s))) {
            let mut m = Map::new();
            for (k, v) in &ids {
                let val = match t[*k] {
                    Value::String(_) if v.len() == 1 && is_tpl(&v[0]) => json!(v[0]),
                    _ => json!(v),
                };
                m.insert((*k).to_owned(), val);
            }
            return self.lit(&Value::Object(m));
        }
        let mut parts: Vec<Value> = ids
            .into_iter()
            .map(|(k, v)| {
                let joined = v.join(",");
                match k {
                    "entity_id" => self.block("ha_entity", json!({"ENTITY": joined}), vec![], None),
                    other => {
                        let ty = format!("ha_{}", other.trim_end_matches("_id"));
                        self.block(&ty, json!({"ID": joined}), vec![], None)
                    }
                }
            })
            .collect();
        if parts.len() == 1 {
            return parts.pop();
        }
        let n = parts.len();
        let mut b = self.block("lists_create_with", json!({}), vec![], Some(json!({"itemCount": n})));
        b["inputs"] = Value::Object(
            parts.into_iter().enumerate().map(|(i, x)| (format!("ADD{i}"), json!({"block": x}))).collect(),
        );
        Some(b)
    }

    /// A value for a Number socket: a number or a template.
    fn num_input(&mut self, v: &Value) -> Option<Value> {
        match v {
            Value::Number(n) => {
                let f = n.as_f64()?;
                (f.fract() != 0.0 || is_whole(f)).then(|| self.num(f))
            }
            Value::String(s) if is_tpl(s) => Some(self.template_value(s)),
            _ => None,
        }
    }

    /// A value for a String socket: text or a template.
    fn text_input(&mut self, v: &Value) -> Option<Value> {
        match v.as_str()? {
            s if is_tpl(s) => Some(self.template_value(s)),
            s => Some(self.block("text", json!({"TEXT": s}), vec![], None)),
        }
    }

    /// The dedicated block for an action call, when it sends exactly the same call.
    fn helper(&mut self, svc: &str, target: &Map<String, Value>, data: &Map<String, Value>) -> Option<Value> {
        let (domain, name) = svc.split_once('.')?;
        let keys: BTreeSet<&str> = data.keys().map(String::as_str).collect();
        let exactly = |want: &[&str]| keys.len() == want.len() && want.iter().all(|k| keys.contains(k));
        let within = |allowed: &[&str]| keys.iter().all(|k| allowed.contains(k));

        if target.is_empty() {
            let to = match (domain, name) {
                ("persistent_notification", "create") => "persistent".to_owned(),
                ("notify", "persistent") => "notify.persistent".to_owned(),
                ("notify", "send_message") => return None,
                ("notify", n) => n.to_owned(),
                _ => return None,
            };
            if !keys.contains("message") || !within(&["message", "title"]) {
                return None;
            }
            let msg = self.lit(&data["message"])?;
            let title = match data.get("title") {
                Some(t) => Some(self.lit(t)?),
                None => None,
            };
            return Some(self.block(
                "notify_send",
                json!({"TO": to}),
                vec![("MESSAGE", Some(msg)), ("TITLE", title)],
                None,
            ));
        }

        let single = || -> Option<String> {
            if target.len() != 1 {
                return None;
            }
            let e = match &target["entity_id"] {
                Value::String(s) => s.clone(),
                Value::Array(a) if a.len() == 1 => a[0].as_str()?.to_owned(),
                _ => return None,
            };
            entity_id(&e)
        };
        let lit_num = |k: &str| data.get(k).and_then(Value::as_f64);
        let int_in =
            |k: &str, lo: f64, hi: f64| lit_num(k).filter(|f| f.fract() == 0.0 && *f >= lo && *f <= hi);

        // helper_set picks the action from the entity's domain.
        let set_key = match svc {
            "input_number.set_value"
            | "number.set_value"
            | "input_text.set_value"
            | "text.set_value"
            | "counter.set_value" => Some("value"),
            "input_select.select_option" | "select.select_option" => Some("option"),
            _ => None,
        };
        if let Some(key) = set_key {
            let e = single().filter(|e| e.split('.').next() == Some(domain))?;
            if !exactly(&[key]) {
                return None;
            }
            let v = self.lit(&data[key])?;
            return Some(self.block("helper_set", json!({"ENTITY": e}), vec![("VALUE", Some(v))], None));
        }

        let tts = if svc == "tts.speak" {
            let e = single().filter(|e| e.starts_with("tts."))?;
            if !exactly(&["message", "media_player_entity_id"]) {
                return None;
            }
            Some(e)
        } else {
            None
        };

        let mode = |pairs: &[(&str, &'static str)]| pairs.iter().find(|(n, _)| *n == name).map(|(_, m)| *m);
        let no_data = data.is_empty();
        let mut spec: Option<(&str, Value, Inputs)> = None;
        match domain {
            "homeassistant" if no_data => {
                spec = mode(&[("turn_on", "on"), ("turn_off", "off"), ("toggle", "toggle")])
                    .map(|m| ("device_power", json!({"MODE": m}), vec![]));
            }
            "cover" => {
                let cmd = mode(&[
                    ("open_cover", "open"),
                    ("close_cover", "close"),
                    ("stop_cover", "stop"),
                    ("open_cover_tilt", "open_tilt"),
                    ("close_cover_tilt", "close_tilt"),
                ]);
                if let Some(m) = cmd.filter(|_| no_data) {
                    spec = Some(("cover_command", json!({"MODE": m}), vec![]));
                } else if name == "set_cover_position" && exactly(&["position"]) {
                    spec = Some((
                        "cover_set_position",
                        json!({"WHAT": "position"}),
                        vec![("POSITION", data["position"].clone(), 'n')],
                    ));
                } else if name == "set_cover_tilt_position" && exactly(&["tilt_position"]) {
                    spec = Some((
                        "cover_set_position",
                        json!({"WHAT": "tilt"}),
                        vec![("POSITION", data["tilt_position"].clone(), 'n')],
                    ));
                }
            }
            "climate" => {
                if name == "set_temperature" && exactly(&["temperature"]) {
                    spec = Some((
                        "climate_set_temperature",
                        json!({}),
                        vec![("TEMPERATURE", data["temperature"].clone(), 'n')],
                    ));
                } else if name == "set_hvac_mode" && exactly(&["hvac_mode"]) {
                    let modes = ["heat", "cool", "heat_cool", "auto", "dry", "fan_only", "off"];
                    spec = data["hvac_mode"]
                        .as_str()
                        .filter(|m| modes.contains(m))
                        .map(|m| ("climate_set_mode", json!({"MODE": m}), vec![]));
                } else if name == "set_preset_mode" && exactly(&["preset_mode"]) {
                    spec = Some((
                        "climate_set_preset",
                        json!({}),
                        vec![("PRESET", data["preset_mode"].clone(), 's')],
                    ));
                }
            }
            "media_player" => {
                let cmd = mode(&[
                    ("media_play", "play"),
                    ("media_pause", "pause"),
                    ("media_play_pause", "play_pause"),
                    ("media_stop", "stop"),
                    ("media_next_track", "next"),
                    ("media_previous_track", "previous"),
                    ("turn_on", "on"),
                    ("turn_off", "off"),
                ]);
                if let Some(m) = cmd.filter(|_| no_data) {
                    spec = Some(("media_command", json!({"MODE": m}), vec![]));
                } else if name == "volume_mute" && exactly(&["is_volume_muted"]) {
                    spec = data["is_volume_muted"].as_bool().map(|b| {
                        ("media_command", json!({"MODE": if b { "mute" } else { "unmute" }}), vec![])
                    });
                } else if name == "volume_set" && exactly(&["volume_level"]) {
                    // The block takes percent and divides by 100: only when that gives the same number.
                    spec = lit_num("volume_level")
                        .filter(|v| (v * 100.0) / 100.0 == *v && (0.0..=1.0).contains(v))
                        .map(|v| ("media_set_volume", json!({}), vec![("VOLUME", json!(v * 100.0), 'n')]));
                } else if name == "play_media" && exactly(&["media_content_id", "media_content_type"]) {
                    let types = ["music", "video", "playlist", "url", "channel"];
                    spec = data["media_content_type"].as_str().filter(|t| types.contains(t)).map(|t| {
                        (
                            "media_play",
                            json!({"TYPE": t}),
                            vec![("MEDIA", data["media_content_id"].clone(), 's')],
                        )
                    });
                }
            }
            "fan" if name == "set_percentage" && exactly(&["percentage"]) => {
                spec = Some(("fan_set_speed", json!({}), vec![("SPEED", data["percentage"].clone(), 'n')]));
            }
            "lock" if no_data => {
                spec = mode(&[("lock", "lock"), ("unlock", "unlock"), ("open", "open")])
                    .map(|m| ("lock_command", json!({"MODE": m}), vec![]));
            }
            "vacuum" if no_data => {
                spec = mode(&[
                    ("start", "start"),
                    ("pause", "pause"),
                    ("stop", "stop"),
                    ("return_to_base", "return_to_base"),
                    ("locate", "locate"),
                ])
                .map(|m| ("vacuum_command", json!({"MODE": m}), vec![]));
            }
            "counter" if no_data => {
                spec = mode(&[("increment", "increment"), ("decrement", "decrement"), ("reset", "reset")])
                    .map(|m| ("counter_command", json!({"MODE": m}), vec![]));
            }
            "scene" if name == "turn_on" && no_data => spec = Some(("scene_activate", json!({}), vec![])),
            "script" if name == "turn_on" => {
                if no_data {
                    spec = Some(("script_run", json!({}), vec![]));
                } else if exactly(&["variables"]) && data["variables"].is_object() {
                    spec = Some(("script_run", json!({}), vec![("DATA", data["variables"].clone(), 'a')]));
                }
            }
            "tts" => {
                if let Some(e) = &tts {
                    let msg = self.lit(&data["message"])?;
                    let players = self.lit(&data["media_player_entity_id"])?;
                    return Some(self.block(
                        "media_say",
                        json!({"TTS": e}),
                        vec![("MESSAGE", Some(msg)), ("TARGET", Some(players))],
                        None,
                    ));
                }
            }
            "light" => {
                // light_data: brightness_pct rounded to 0–100, kelvin to 1000–12000, transition only if > 0.
                let transition = match data.get("transition") {
                    None => Some(None),
                    Some(t) => t.as_f64().filter(|t| *t > 0.0).map(|t| Some(json!(t))),
                };
                let transition = transition?;
                let power = mode(&[("turn_on", "on"), ("turn_off", "off"), ("toggle", "toggle")]);
                let mut inputs: Inputs = Vec::new();
                if let Some(t) = &transition {
                    inputs.push(("TRANSITION", t.clone(), 'n'));
                }
                let bright = data.get("brightness_pct").map(|_| int_in("brightness_pct", 0.0, 100.0));
                if let Some(b) = &bright {
                    inputs.push(("BRIGHTNESS", json!((*b)?), 'n'));
                }
                if within(&["transition"]) {
                    spec = power.map(|m| ("light_power", json!({"MODE": m}), inputs));
                } else if name == "turn_on" && within(&["transition", "brightness_pct"]) {
                    spec = Some(("light_set_brightness", json!({}), inputs));
                } else if name == "turn_on" && within(&["transition", "brightness_pct", "color_temp_kelvin"])
                {
                    let k = int_in("color_temp_kelvin", 1000.0, 12000.0)?;
                    inputs.push(("KELVIN", json!(k), 'n'));
                    spec = Some(("light_set_white", json!({}), inputs));
                } else if name == "turn_on" && within(&["transition", "brightness_pct", "rgb_color"]) {
                    let rgb = data["rgb_color"].as_array().filter(|a| a.len() == 3)?;
                    let mut hex = String::from("#");
                    for c in rgb {
                        let c = c.as_u64().filter(|c| *c <= 255)?;
                        hex.push_str(&format!("{c:02x}"));
                    }
                    let colour = self.block("colour_picker", json!({"COLOUR": hex}), vec![], None);
                    let mut b = self.light_block("light_set_colour", json!({}), inputs, target)?;
                    b["inputs"]["COLOUR"] = json!({"block": colour});
                    return Some(b);
                } else if name == "turn_on" && within(&["transition", "brightness_step_pct"]) {
                    let s = int_in("brightness_step_pct", -100.0, 100.0)?;
                    let dir = if s < 0.0 { "dim" } else { "brighten" };
                    inputs.push(("STEP", json!(s.abs()), 'n'));
                    spec = Some(("light_step", json!({"DIRECTION": dir}), inputs));
                }
            }
            _ => {}
        }
        let (ty, fields, inputs) = spec?;
        self.light_block(ty, fields, inputs, target)
    }

    /// A helper block with its value sockets and TARGET.
    fn light_block(
        &mut self,
        ty: &str,
        fields: Value,
        inputs: Inputs,
        target: &Map<String, Value>,
    ) -> Option<Value> {
        let mut built = Vec::new();
        for (socket, v, kind) in inputs {
            let b = match kind {
                'n' => self.num_input(&v)?,
                's' => self.text_input(&v)?,
                _ => self.lit(&v)?,
            };
            built.push((socket, Some(b)));
        }
        let t = self.target_from(target)?;
        built.push(("TARGET", Some(t)));
        Some(self.block(ty, fields, built, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periods_like_home_assistant() {
        assert_eq!(period(&json!("00:05")), Some(300.0));
        assert_eq!(period(&json!("01:02:03.5")), Some(3723.5));
        assert_eq!(period(&json!("30")), Some(30.0));
        assert_eq!(period(&json!(2.5)), Some(2.5));
        assert_eq!(period(&json!({"minutes": 1, "milliseconds": 500})), Some(60.5));
        assert_eq!(period(&json!("-00:00:05")), None);
        assert_eq!(period(&json!("{{ 5 }}")), None);
        assert_eq!(period(&json!({"minutes": "{{ x }}"})), None);
        assert_eq!(period(&json!("1:2:3:4")), None);
    }

    #[test]
    fn finds_variable_uses() {
        let v = json!({"data": {"m": "Please wait {{ wait.remaining }} {{ x.wait }} {{ waiting }}"}});
        assert_eq!(uses(&v, "wait"), vec![".remaining "]);
        assert!(wait_ok(".trigger.entity_id", true));
        assert!(!wait_ok(" is none", false));
        assert!(!wait_ok(".trigger.id", true));
        assert!(wait_ok(".trigger.id", false));
        assert_eq!(uses(&json!({"stop": "", "response_variable": "r"}), "r").len(), 1);
    }
}
