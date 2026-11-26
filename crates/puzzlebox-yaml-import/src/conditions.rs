//! Conditions: HA condition configs → true/false blocks.
//!
//! Leaf conditions only become blocks that compile back to the same HA condition config (HA
//! still evaluates them), or to an equivalent engine expression; everything else stays a raw
//! condition. and/or/not become logic blocks, which evaluate in the same order and short-circuit
//! like HA's.

use serde_json::{Map, Value, json};

use crate::*;

/// Keys every condition may have (`note` is only used by HA's frontend).
const BASE: [&str; 4] = ["condition", "alias", "enabled", "note"];

const WEEKDAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// HA skips disabled conditions (they return neither true nor false), so one counts as true
/// in an AND and as false in an OR (or a NOT, which is a NOR).
#[derive(Clone, Copy, PartialEq)]
enum Ctx {
    All,
    Any,
}

enum Enabled {
    Yes,
    No,
    /// Rendered by HA; falsy disables the condition.
    Template(String),
    Invalid,
}

fn with(extra: &[&'static str]) -> Vec<&'static str> {
    BASE.iter().chain(extra).copied().collect()
}

fn is_raw(b: &Value) -> bool {
    b["type"] == "ha_condition_raw"
}

/// `enabled:` as HA's `vol.Any(cv.boolean, cv.template)` reads it.
fn enabled(m: &Map<String, Value>) -> Enabled {
    match m.get("enabled") {
        None => Enabled::Yes,
        Some(Value::Bool(b)) => {
            if *b {
                Enabled::Yes
            } else {
                Enabled::No
            }
        }
        Some(Value::Number(n)) => {
            if n.as_f64() == Some(0.0) {
                Enabled::No
            } else {
                Enabled::Yes
            }
        }
        Some(Value::String(s)) => match s.trim().to_lowercase().as_str() {
            "1" | "true" | "yes" | "on" | "enable" => Enabled::Yes,
            "0" | "false" | "no" | "off" | "disable" => Enabled::No,
            _ => Enabled::Template(s.clone()),
        },
        Some(_) => Enabled::Invalid,
    }
}

/// `entity_id:` as a list of ids (HA splits comma-separated strings).
fn entity_ids(v: &Value) -> Option<Vec<String>> {
    let ids: Vec<String> = match v {
        Value::String(s) => s.split(',').map(|p| p.trim().to_owned()).collect(),
        Value::Array(a) => {
            a.iter().map(|x| x.as_str().map(|s| s.trim().to_owned())).collect::<Option<_>>()?
        }
        _ => return None,
    };
    let ok = !ids.is_empty() && ids.iter().all(|id| !id.is_empty() && !id.contains(',') && !is_template(id));
    ok.then_some(ids)
}

/// A `for:` duration in seconds, read like HA's `cv.positive_time_period` (no templates).
fn period_seconds(v: &Value) -> Option<f64> {
    let num = |v: &Value| match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) if !is_template(s) => s.trim().parse::<f64>().ok().filter(|f| f.is_finite()),
        _ => None,
    };
    let secs = match v {
        Value::Number(_) => num(v)?,
        Value::String(s) => {
            let s = s.strip_prefix('+').unwrap_or(s);
            let parts: Vec<&str> = s.split(':').collect();
            let int = |p: &str| {
                (!p.is_empty() && p.bytes().all(|c| c.is_ascii_digit())).then(|| p.parse::<f64>().ok())?
            };
            match parts.as_slice() {
                [h, m] => int(h)? * 3600.0 + int(m)? * 60.0,
                [h, m, sec] => {
                    let sec = sec
                        .parse::<f64>()
                        .ok()
                        .filter(|f| f.is_finite() && *f >= 0.0 && !sec.starts_with('+'))?;
                    int(h)? * 3600.0 + int(m)? * 60.0 + sec
                }
                _ => num(v)?,
            }
        }
        Value::Object(m) => {
            let units = [
                ("days", 86400.0),
                ("hours", 3600.0),
                ("minutes", 60.0),
                ("seconds", 1.0),
                ("milliseconds", 0.001),
            ];
            if m.is_empty() || !m.keys().all(|k| units.iter().any(|(u, _)| u == k)) {
                return None;
            }
            let mut total = 0.0;
            for (unit, factor) in units {
                if let Some(x) = m.get(unit) {
                    total += num(x)? * factor;
                }
            }
            total
        }
        _ => return None,
    };
    (secs >= 0.0).then_some(secs)
}

/// A time for the time block: `H:MM` / `HH:MM[:SS]` (normalised the way the block's field
/// does), or a time/input_datetime/sensor entity.
fn time_value(v: &Value) -> Option<String> {
    let s = v.as_str()?.trim();
    if let Some((domain, object)) = s.split_once('.') {
        let slug = !object.is_empty()
            && object.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_');
        let ok = ["input_datetime", "time", "sensor"].contains(&domain) && slug;
        return ok.then(|| s.to_owned());
    }
    let parts: Vec<&str> = s.split(':').collect();
    let n = |p: &str, len: std::ops::RangeInclusive<usize>, max: u32| {
        (len.contains(&p.len()) && p.bytes().all(|c| c.is_ascii_digit()))
            .then(|| p.parse::<u32>().ok().filter(|x| *x <= max))
            .flatten()
    };
    match parts.as_slice() {
        [h, m] => Some(format!("{:02}:{:02}", n(h, 1..=2, 23)?, n(m, 2..=2, 59)?)),
        [h, m, sec] => {
            Some(format!("{:02}:{:02}:{:02}", n(h, 1..=2, 23)?, n(m, 2..=2, 59)?, n(sec, 2..=2, 59)?))
        }
        _ => None,
    }
}

/// A trigger id that the engine's `==` compares as plain text (it compares numbers numerically),
/// and that "triggered by" can't take for a block id.
fn plain_id(v: &Value) -> Option<String> {
    let s = v.as_str()?;
    (s.trim().parse::<f64>().is_err() && !crate::triggers::looks_like_block_id(s)).then(|| s.to_owned())
}

/// A state for the state box. Attribute values keep their type when the block compiles
/// (numbers and true/false), so a text that looks like one can't be written in the box.
fn state_text(v: &Value, attribute: bool) -> Option<String> {
    let looks_typed =
        |s: &str| matches!(serde_json::from_str::<Value>(s.trim()), Ok(Value::Number(_) | Value::Bool(_)));
    match v {
        Value::String(s) if !attribute || !looks_typed(s) => Some(s.clone()),
        Value::Number(n) if attribute => Some(n.to_string()),
        Value::Bool(b) if attribute => Some(b.to_string()),
        _ => None,
    }
}

/// A numeric_state limit: a number, or a number helper/sensor entity (HA's
/// `NUMERIC_STATE_THRESHOLD_SCHEMA`).
fn threshold_text(v: &Value) -> Option<String> {
    match v {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) if s.trim().parse::<f64>().is_ok_and(f64::is_finite) => Some(s.trim().to_owned()),
        Value::String(s) => {
            let (domain, object) = s.split_once('.')?;
            let slug = !object.is_empty()
                && object.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_');
            (["input_number", "number", "sensor", "zone"].contains(&domain) && slug).then(|| s.clone())
        }
        _ => None,
    }
}

/// A sun offset in seconds, read like HA's `cv.time_period` (which allows negative ones).
fn signed_seconds(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) if !is_template(s) => match s.trim().strip_prefix('-') {
            Some(rest) => period_seconds(&json!(rest)).map(|x| -x),
            None => period_seconds(&json!(s.trim())),
        },
        other => period_seconds(other),
    }
}

fn is_input_entity(s: &str) -> bool {
    ["input_select.", "input_text.", "input_number.", "input_boolean.", "input_datetime."]
        .iter()
        .any(|p| s.starts_with(p))
}

impl Builder {
    pub(crate) fn and_all(&mut self, conds: &[Value]) -> Value {
        match conds {
            [one] => self.condition(one),
            _ => self.combinator("and", conds, &json!({"condition": "and", "conditions": conds})),
        }
    }

    // Part of the interface other modules may use.

    pub(crate) fn raw_condition(&mut self, c: &Value) -> Value {
        self.raw += 1;
        let cfg = match c {
            // Template shorthand: `- "{{ is_state('x', 'on') }}"`.
            Value::String(t) => json!({"condition": "template", "value_template": t}),
            other => other.clone(),
        };
        self.block("ha_condition_raw", json!({"CONFIG": yaml(&cfg)}), vec![], None)
    }

    pub(crate) fn condition(&mut self, c: &Value) -> Value {
        self.condition_in(c, Ctx::All).unwrap_or_else(|| self.boolean(true))
    }

    /// `None` when the condition is disabled.
    fn condition_in(&mut self, c: &Value, ctx: Ctx) -> Option<Value> {
        let Some(m) = c.as_object() else {
            return Some(match c {
                Value::String(t) => self.template_cond(t),
                _ => self.raw_condition(c),
            });
        };
        match enabled(m) {
            Enabled::Yes => Some(self.enabled_condition(c, m)),
            Enabled::No => None,
            Enabled::Invalid => Some(self.raw_condition(c)),
            Enabled::Template(t) => {
                let mut rest = m.clone();
                rest.remove("enabled");
                let inner = self.enabled_condition(&Value::Object(rest.clone()), &rest);
                let on = self.block("ha_template", json!({"TEMPLATE": t}), vec![], None);
                Some(match ctx {
                    Ctx::All => {
                        let off = self.negate(on);
                        self.logic("OR", off, inner)
                    }
                    Ctx::Any => self.logic("AND", on, inner),
                })
            }
        }
    }

    fn enabled_condition(&mut self, c: &Value, m: &Map<String, Value>) -> Value {
        let b = match m.get("condition") {
            Some(Value::String(kind)) => match kind.as_str() {
                "and" | "or" | "not"
                    if m.contains_key("conditions") && only_keys(m, &with(&["conditions"])) =>
                {
                    self.combinator(kind, &list(m.get("conditions")), c)
                }
                "state" => self.state_condition(c, m),
                "numeric_state" => self.numeric_condition(c, m),
                "sun" => self.sun_condition(c, m),
                "zone" => self.zone_condition(c, m),
                "time" => self.time_condition(c, m),
                "trigger" => self.trigger_condition(c, m),
                "device" => self.device_condition(c, m),
                "template" => match m.get("value_template") {
                    Some(Value::String(t)) if only_keys(m, &with(&["value_template"])) => {
                        self.template_cond(t)
                    }
                    _ => self.raw_condition(c),
                },
                // Shorthand of the condition action: `- condition: "{{ ... }}"`.
                t if is_template(t) => {
                    let mut cfg = m.clone();
                    cfg.insert("condition".into(), json!("template"));
                    cfg.insert("value_template".into(), json!(t));
                    if only_keys(m, &BASE) {
                        self.template_cond(t)
                    } else {
                        self.raw_condition(&Value::Object(cfg))
                    }
                }
                t if t.contains('.') => self.integration_condition(c, t, m),
                _ => self.raw_condition(c),
            },
            // `condition: [...]` is a shorthand AND.
            Some(Value::Array(items)) if only_keys(m, &BASE) => self.combinator("and", items, c),
            None => match ["and", "or", "not"].into_iter().find(|k| m.contains_key(*k)) {
                Some(k) if only_keys(m, &["alias", "enabled", "note", k]) => {
                    self.combinator(k, &list(m.get(k)), c)
                }
                _ => self.raw_condition(c),
            },
            _ => self.raw_condition(c),
        };
        self.annotate(b, m)
    }

    /// The YAML's alias/note as the block's comment (raw blocks keep them in their YAML).
    fn annotate(&mut self, mut b: Value, m: &Map<String, Value>) -> Value {
        if is_raw(&b) {
            return b;
        }
        let mut texts: Vec<String> = ["alias", "note"]
            .iter()
            .filter_map(|k| m.get(*k).and_then(Value::as_str))
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .collect();
        if texts.is_empty() {
            return b;
        }
        if let Some(old) = b.pointer("/icons/comment/text").and_then(Value::as_str) {
            texts.push(old.to_owned());
        }
        b["icons"] = json!({"comment": {"text": texts.join("\n"), "pinned": false}});
        b
    }

    /// and / or / not. When every part would be raw, the whole condition stays one raw block,
    /// which also keeps HA's handling of errors in the parts.
    fn combinator(&mut self, kind: &str, items: &[Value], original: &Value) -> Value {
        let snap = self.snapshot();
        let ctx = if kind == "and" { Ctx::All } else { Ctx::Any };
        let parts: Vec<Value> = items.iter().filter_map(|c| self.condition_in(c, ctx)).collect();
        if !parts.is_empty() && parts.iter().all(is_raw) {
            self.restore(snap);
            return self.raw_condition(original);
        }
        let op = if kind == "and" { "AND" } else { "OR" };
        let joined = match self.join(op, parts) {
            Some(b) => b,
            // HA: an empty (or all-disabled) AND is true, OR false, NOT true.
            None => self.boolean(kind == "and"),
        };
        if kind == "not" { self.negate(joined) } else { joined }
    }

    fn state_condition(&mut self, c: &Value, m: &Map<String, Value>) -> Value {
        let keys_ok = only_keys(m, &with(&["entity_id", "state", "for", "match", "attribute"]));
        let ents = m.get("entity_id").and_then(entity_ids);
        let attribute = match m.get("attribute") {
            None => Some(None),
            Some(Value::String(a)) if !a.is_empty() && a.trim() == a => Some(Some(a.clone())),
            _ => None,
        };
        let typed = matches!(attribute, Some(Some(_)));
        let states: Option<Vec<String>> = match m.get("state") {
            Some(Value::Array(a)) if !a.is_empty() => {
                a.iter().map(|s| state_text(s, typed)).collect::<Option<_>>()
            }
            Some(s) if !s.is_array() => state_text(s, typed).map(|t| vec![t]),
            _ => None,
        };
        let any = match m.get("match").map(|v| v.as_str().map(str::to_lowercase)) {
            None => Some(false),
            Some(Some(s)) if s == "all" => Some(false),
            Some(Some(s)) if s == "any" => Some(true),
            _ => None,
        };
        let secs = match m.get("for") {
            None => Some(None),
            Some(f) => period_seconds(f).map(Some),
        };
        let (true, Some(ents), Some(attribute), Some(states), Some(any), Some(secs)) =
            (keys_ok, ents, attribute, states, any, secs)
        else {
            return self.raw_condition(c);
        };
        // The block's box splits states at commas (and trims them when there are several).
        let box_ok = states.iter().all(|s| !s.contains(','))
            && (states.len() == 1 || states.iter().all(|s| !s.is_empty() && s.trim() == s));
        // HA rejects `for` with an attribute or an entity state.
        if !box_ok || (secs.is_some() && (attribute.is_some() || states.iter().any(|s| is_input_entity(s)))) {
            return self.raw_condition(c);
        }
        let ids = ents.join(",");
        if let ([state], None, false) = (states.as_slice(), &attribute, any)
            && let Some(b) = self.friendlier_state(&ids, state, secs)
        {
            return b;
        }
        let mut fields = json!({"STATE": states.join(", ")});
        let mut options = Vec::new();
        if let Some(a) = attribute {
            fields["ATTRIBUTE"] = json!(a);
            options.push("ATTRIBUTE");
        }
        if any {
            fields["MATCH"] = json!("any");
            options.push("MATCH");
        }
        let extra = (!options.is_empty()).then(|| json!({"options": options}));
        let ent = self.entity(&ids);
        let mut inputs = json!({"ENTITY": {"shadow": ent}});
        let ty = match secs {
            Some(secs) => {
                inputs["SECONDS"] = json!({"shadow": self.duration(secs)});
                "ha_state_for"
            }
            None => "ha_is_state",
        };
        let mut b = self.block(ty, fields, vec![], extra);
        b["inputs"] = inputs;
        b
    }

    /// Blocks that compile to the same state condition: "the sun is up", "… is home".
    fn friendlier_state(&mut self, ids: &str, state: &str, secs: Option<f64>) -> Option<Value> {
        if secs.is_some() || ids.contains(',') {
            return None;
        }
        if ids == "sun.sun" && (state == "above_horizon" || state == "below_horizon") {
            let place = if state == "above_horizon" { "up" } else { "down" };
            return Some(self.block("sun_is", json!({"WHERE": place}), vec![], None));
        }
        if (ids.starts_with("person.") || ids.starts_with("device_tracker."))
            && (state == "home" || state == "not_home")
        {
            let ent = self.entity(ids);
            let place = if state == "home" { "home" } else { "away" };
            let mut b = self.block("person_is_home", json!({"WHERE": place}), vec![], None);
            b["inputs"] = json!({"ENTITY": {"shadow": ent}});
            return Some(b);
        }
        None
    }

    fn numeric_condition(&mut self, c: &Value, m: &Map<String, Value>) -> Value {
        let keys_ok = only_keys(m, &with(&["entity_id", "above", "below", "attribute", "value_template"]));
        let ents = m.get("entity_id").and_then(entity_ids);
        let bound = |k: &str| match m.get(k) {
            None => Some(String::new()),
            Some(v) => threshold_text(v),
        };
        let (above, below) = (bound("above"), bound("below"));
        let text = |k: &str, trimmed: bool| match m.get(k) {
            None => Some(None),
            Some(Value::String(s)) if !s.trim().is_empty() && (!trimmed || s.trim() == s) => {
                Some(Some(s.clone()))
            }
            _ => None,
        };
        let (true, Some(ents), Some(above), Some(below), Some(attribute), Some(template)) =
            (keys_ok, ents, above, below, text("attribute", true), text("value_template", false))
        else {
            return self.raw_condition(c);
        };
        if above.is_empty() && below.is_empty() {
            return self.raw_condition(c);
        }
        let mut fields = json!({"ABOVE": above, "BELOW": below});
        let mut options = Vec::new();
        if let Some(a) = attribute {
            fields["ATTRIBUTE"] = json!(a);
            options.push("ATTRIBUTE");
        }
        if let Some(t) = template {
            fields["VALUE_TEMPLATE"] = json!(t);
            options.push("VALUE_TEMPLATE");
        }
        let extra = (!options.is_empty()).then(|| json!({"options": options}));
        let ent = self.entity(&ents.join(","));
        let mut b = self.block("ha_numeric_state", fields, vec![], extra);
        b["inputs"] = json!({"ENTITY": {"shadow": ent}});
        b
    }

    fn sun_condition(&mut self, c: &Value, m: &Map<String, Value>) -> Value {
        let event = |k: &str| match m.get(k) {
            None => Some(None),
            Some(Value::String(e)) => {
                let e = e.to_lowercase();
                (e == "sunrise" || e == "sunset").then_some(Some(e))
            }
            _ => None,
        };
        let offset = |k: &str| match m.get(k) {
            None => Some(0.0),
            Some(v) => signed_seconds(v).filter(|s| s.fract() == 0.0),
        };
        let (true, Some(after), Some(before), Some(after_off), Some(before_off)) = (
            only_keys(m, &with(&["after", "before", "after_offset", "before_offset"])),
            event("after"),
            event("before"),
            offset("after_offset"),
            offset("before_offset"),
        ) else {
            return self.raw_condition(c);
        };
        // An offset without its event does nothing in HA; there's no block for that.
        let lone_offset = (after.is_none() && m.contains_key("after_offset"))
            || (before.is_none() && m.contains_key("before_offset"));
        let mut limits: Vec<(&str, String, f64)> = Vec::new();
        if let Some(e) = after {
            limits.push(("after", e, after_off));
        }
        if let Some(e) = before {
            limits.push(("before", e, before_off));
        }
        if limits.is_empty() || lone_offset {
            return self.raw_condition(c);
        }
        let (when, event, secs) = &limits[0];
        let mut fields = json!({"WHEN": when, "EVENT": event});
        let mut inputs = json!({"OFFSET": {"shadow": self.duration(*secs)}});
        let mut extra = None;
        if let Some((when, event, secs)) = limits.get(1) {
            fields["WHEN2"] = json!(when);
            fields["EVENT2"] = json!(event);
            inputs["SECOND"] = json!({"shadow": self.duration(*secs)});
            extra = Some(json!({"options": ["SECOND"]}));
        }
        let mut b = self.block("sun_condition", fields, vec![], extra);
        b["inputs"] = inputs;
        b
    }

    fn zone_condition(&mut self, c: &Value, m: &Map<String, Value>) -> Value {
        let (true, Some(ents), Some(zones)) = (
            only_keys(m, &with(&["entity_id", "zone"])),
            m.get("entity_id").and_then(entity_ids),
            m.get("zone").and_then(entity_ids),
        ) else {
            return self.raw_condition(c);
        };
        let ent = self.entity(&ents.join(","));
        let mut b = self.block("zone_condition", json!({"ZONE": zones.join(", ")}), vec![], None);
        b["inputs"] = json!({"ENTITY": {"shadow": ent}});
        b
    }

    fn time_condition(&mut self, c: &Value, m: &Map<String, Value>) -> Value {
        let bound = |k: &str| match m.get(k) {
            None => Some(String::new()),
            Some(v) => time_value(v),
        };
        let days: Option<Vec<&str>> = match m.get("weekday") {
            None => Some(WEEKDAYS.to_vec()),
            Some(Value::String(d)) => WEEKDAYS.iter().find(|w| *w == d).map(|w| vec![*w]),
            Some(Value::Array(a)) if !a.is_empty() => {
                a.iter().map(|d| WEEKDAYS.iter().find(|w| Some(**w) == d.as_str()).copied()).collect()
            }
            _ => None,
        };
        let (true, Some(after), Some(before), Some(days)) =
            (only_keys(m, &with(&["after", "before", "weekday"])), bound("after"), bound("before"), days)
        else {
            return self.raw_condition(c);
        };
        if !["after", "before", "weekday"].iter().any(|k| m.contains_key(*k)) {
            return self.raw_condition(c);
        }
        let has = |d: &str| days.contains(&d);
        if after.is_empty() && before.is_empty() {
            let when = match WEEKDAYS.iter().filter(|d| has(d)).count() {
                // Every day: the block would compile to a time condition without options.
                7 => return self.boolean(true),
                5 if WEEKDAYS[..5].iter().all(|d| has(d)) => Some("weekday"),
                2 if has("sat") && has("sun") => Some("weekend"),
                1 => WEEKDAYS.iter().find(|d| has(d)).copied(),
                _ => None,
            };
            if let Some(when) = when {
                return self.block("weekday_is", json!({"WHEN": when}), vec![], None);
            }
        }
        let mut fields = json!({"AFTER": after, "BEFORE": before});
        for d in WEEKDAYS {
            fields[d.to_uppercase()] = json!(if has(d) { "TRUE" } else { "FALSE" });
        }
        self.block("time_is_between", fields, vec![], None)
    }

    fn trigger_condition(&mut self, c: &Value, m: &Map<String, Value>) -> Value {
        let ids: Option<Vec<String>> = match m.get("id") {
            Some(Value::Array(a)) if !a.is_empty() => a.iter().map(plain_id).collect(),
            Some(v @ Value::String(_)) => plain_id(v).map(|id| vec![id]),
            _ => None,
        };
        let (true, Some(ids)) = (only_keys(m, &with(&["id"])), ids) else { return self.raw_condition(c) };
        // Linked to the trigger with this id once the automation's triggers are built.
        let parts: Vec<Value> =
            ids.iter().map(|id| self.block("triggered_by", json!({"TRIGGER": id}), vec![], None)).collect();
        self.join("OR", parts).expect("ids is not empty")
    }

    /// The newer form of integration conditions (`condition: light.is_on`, with `target` and
    /// `options`).
    fn integration_condition(&mut self, c: &Value, name: &str, m: &Map<String, Value>) -> Value {
        let rest: Map<String, Value> = m
            .iter()
            .filter(|(k, _)| !BASE.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        match self.integration_parts(name, &rest) {
            Some((target, options)) => self.block(
                "ha_condition_integration",
                json!({"NAME": name}),
                vec![("TARGET", target), ("OPTIONS", options)],
                None,
            ),
            None => self.raw_condition(c),
        }
    }

    fn device_condition(&mut self, c: &Value, m: &Map<String, Value>) -> Value {
        let (true, Some(domain)) =
            (m.get("device_id").is_some_and(Value::is_string), m.get("domain").and_then(Value::as_str))
        else {
            return self.raw_condition(c);
        };
        let config: Map<String, Value> = m
            .iter()
            .filter(|(k, _)| !BASE.contains(&k.as_str()) && *k != "metadata")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let what = m.get("type").and_then(Value::as_str).unwrap_or("condition").replace('_', " ");
        let label = format!("{} {what}", domain.replace('_', " "));
        // No options: anything else in the YAML stays in the stored config, sent to HA as is.
        let extra = json!({"config": config, "label": label, "params": []});
        self.block("device_condition", json!({}), vec![], Some(extra))
    }

    /// Native when the template converts to a block that is always true or false (HA's check is
    /// "renders `true`"), otherwise a template condition block, which HA renders.
    fn template_cond(&mut self, template: &str) -> Value {
        match self.template_condition(template) {
            Some(b) => b,
            None => self.block("ha_template_condition", json!({"TEMPLATE": template}), vec![], None),
        }
    }

    fn entity(&mut self, ids: &str) -> Value {
        self.block("ha_entity", json!({"ENTITY": ids}), vec![], None)
    }

    fn boolean(&mut self, b: bool) -> Value {
        self.block("logic_boolean", json!({"BOOL": if b { "TRUE" } else { "FALSE" }}), vec![], None)
    }

    fn negate(&mut self, inner: Value) -> Value {
        self.block("logic_negate", json!({}), vec![("BOOL", Some(inner))], None)
    }

    /// Joins with AND/OR as a balanced tree: it evaluates (and short-circuits) left to right
    /// like a chain, without nesting long lists deeply enough to overflow the compiler's stack.
    fn join(&mut self, op: &str, mut parts: Vec<Value>) -> Option<Value> {
        if parts.len() <= 1 {
            return parts.pop();
        }
        let right = parts.split_off(parts.len().div_ceil(2));
        let a = self.join(op, parts)?;
        let b = self.join(op, right)?;
        Some(self.logic(op, a, b))
    }

    fn logic(&mut self, op: &str, a: Value, b: Value) -> Value {
        self.block("logic_operation", json!({"OP": op}), vec![("A", Some(a)), ("B", Some(b))], None)
    }
}
