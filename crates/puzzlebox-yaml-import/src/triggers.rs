//! Triggers: HA trigger configs → trigger blocks (raw YAML block when no block fits).
//!
//! A native block is only used when its compiled config behaves exactly like the original
//! (see `puzzlebox_ir::compile`); everything else keeps its YAML in an `ha_trigger_raw` block.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use crate::*;

/// Keys every trigger may have (HA's `TRIGGER_BASE_SCHEMA`), handled before the platform.
const BASE_KEYS: [&str; 7] = ["trigger", "platform", "id", "alias", "variables", "enabled", "note"];

/// What a trigger's base keys mean for the blocks made from it.
struct Base {
    disabled: bool,
}

impl Builder {
    /// Triggers anywhere (also `wait_for_trigger`): a trigger with an `id` stays raw, because
    /// only the compiler's automation triggers report the block id as `trigger.id`.
    pub(crate) fn triggers(&mut self, list: &[Value]) -> Vec<Value> {
        flatten(list).iter().flat_map(|t| self.trigger_blocks(t, None, true)).collect()
    }

    /// The automation's own triggers. A native block made from a trigger with `id: X` gets the
    /// Blockly block id `X`: the compiler injects the block id as the HA config's `id` (and the
    /// engine reports it for its own triggers), so `trigger.id` stays exactly `X`, and a
    /// `triggered_by` block with `TRIGGER: X` matches it. Raw triggers keep `id: X` in their
    /// YAML, so `triggered_by` with `X` matches them too. Ids that are used more than once, or
    /// that look like the importer's own block (`imp<N>`) or variable (`var_<N>`) ids, stay raw
    /// so block ids remain unique; so does a trigger with an id that would need several blocks.
    pub(crate) fn automation_triggers(&mut self, list: &[Value]) -> Vec<Value> {
        let flat = flatten(list);
        let mut counts: HashMap<String, usize> = HashMap::new();
        for t in &flat {
            if let Some(id) = t.get("id").and_then(Value::as_str) {
                *counts.entry(id.to_owned()).or_default() += 1;
            }
        }
        flat.iter()
            .flat_map(|t| {
                let id = t
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| counts.get(*id) == Some(&1) && usable_block_id(id));
                self.trigger_blocks(t, id, true)
            })
            .collect()
    }

    pub(crate) fn raw_trigger(&mut self, t: &Value) -> Value {
        self.raw += 1;
        self.block("ha_trigger_raw", json!({"CONFIG": yaml(t)}), vec![], None)
    }

    /// Blocks for one trigger. `id` is the user's trigger id when it may become the block id;
    /// `split` allows one block per list item (`at: [...]`, `event_type: [...]`), which HA
    /// treats exactly like separate triggers.
    fn trigger_blocks(&mut self, t: &Value, id: Option<&str>, split: bool) -> Vec<Value> {
        let Some(m) = t.as_object() else { return vec![self.raw_trigger(t)] };
        let Some(base) = base(m, id.is_some()) else { return vec![self.raw_trigger(t)] };
        let Some(mut blocks) = self.native(m, split && id.is_none()) else {
            return vec![self.raw_trigger(t)];
        };
        for b in &mut blocks {
            if b["type"] == "ha_trigger_raw" {
                continue;
            }
            if base.disabled {
                b["disabledReasons"] = json!(["MANUALLY_DISABLED"]);
            }
            if let Some(id) = id {
                b["id"] = json!(id);
            }
        }
        blocks
    }

    /// Native blocks for the platform part of a trigger (`None` → keep it raw).
    fn native(&mut self, m: &Map<String, Value>, split: bool) -> Option<Vec<Value>> {
        let platform = m.get("trigger").or_else(|| m.get("platform"))?.as_str()?;
        let rest: Map<String, Value> = m
            .iter()
            .filter(|(k, _)| !BASE_KEYS.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let one = |b: Value| Some(vec![b]);
        match platform {
            "state" => one(self.state(&rest)?),
            "numeric_state" => one(self.numeric_state(&rest)?),
            "time" => self.time(m, &rest, split),
            "time_pattern" => one(self.time_pattern(&rest)?),
            "event" => self.event(m, &rest, split),
            "sun" => one(self.sun(&rest)?),
            "zone" => one(self.zone(&rest)?),
            "homeassistant" if only_keys(&rest, &["event"]) && rest.get("event")? == "start" => {
                one(self.block("ha_trigger_ha_start", json!({}), vec![], None))
            }
            "device" => one(self.device(m, &rest)?),
            _ => None,
        }
    }

    fn state(&mut self, m: &Map<String, Value>) -> Option<Value> {
        // Without from/to/not_from/not_to HA also fires on attribute changes; no block does.
        if !only_keys(m, &["entity_id", "from", "to", "for"])
            || !(m.contains_key("from") || m.contains_key("to"))
        {
            return None;
        }
        let entity = entities(m.get("entity_id")?)?;
        let from = state_match(m.get("from"))?;
        let to = state_match(m.get("to"))?;
        let duration = match m.get("for") {
            None => None,
            // `for: 0` still sets `trigger.for` to 0 seconds instead of none.
            Some(f) => Some(period(f).filter(|s| *s > 0.0)?),
        };
        match (from, to) {
            (None, Some(to)) => {
                let mut b =
                    self.block("ha_trigger_becomes", json!({"ENTITY": entity, "STATE": to}), vec![], None);
                let d = self.duration(duration.unwrap_or(0.0));
                shadow(&mut b, "FOR", d);
                Some(b)
            }
            (from, to) => {
                // This block's duration is whole seconds.
                if duration.is_some_and(|d| d.fract() != 0.0) {
                    return None;
                }
                let fields =
                    json!({"ENTITY": entity, "FROM": from.unwrap_or_default(), "TO": to.unwrap_or_default()});
                let mut b = self.block("ha_trigger_state", fields, vec![], None);
                let d = self.duration(duration.unwrap_or(0.0));
                shadow(&mut b, "FOR", d);
                Some(b)
            }
        }
    }

    fn numeric_state(&mut self, m: &Map<String, Value>) -> Option<Value> {
        if !only_keys(m, &["entity_id", "above", "below"])
            || !(m.contains_key("above") || m.contains_key("below"))
        {
            return None;
        }
        let entity = entities(m.get("entity_id")?)?;
        let threshold = |k: &str| -> Option<String> {
            match m.get(k) {
                None => Some(String::new()),
                Some(Value::Number(n)) => n.as_f64().filter(|f| f.is_finite()).map(|_| n.to_string()),
                // HA takes a numeric string as a number and anything else as an entity id; the
                // compiler does the same for numbers and dotted ids.
                Some(Value::String(s))
                    if s.trim() == s
                        && !s.is_empty()
                        && !is_tpl(s)
                        && (s.parse::<f64>().is_ok_and(f64::is_finite)
                            || s.contains('.') && s.parse::<f64>().is_err()) =>
                {
                    Some(s.clone())
                }
                _ => None,
            }
        };
        let (above, below) = (threshold("above")?, threshold("below")?);
        Some(self.block(
            "ha_trigger_numeric",
            json!({"ENTITY": entity, "ABOVE": above, "BELOW": below}),
            vec![],
            None,
        ))
    }

    /// One block per plain time or entity in `at`; the other entries (offsets, templates)
    /// stay raw on their own.
    fn time(&mut self, all: &Map<String, Value>, m: &Map<String, Value>, split: bool) -> Option<Vec<Value>> {
        if !only_keys(m, &["at"]) {
            return None;
        }
        let items = match m.get("at")? {
            Value::Array(a) if !a.is_empty() => a.clone(),
            Value::Array(_) => return None,
            other => vec![other.clone()],
        };
        if items.len() > 1 && !split {
            return None;
        }
        let blocks = items
            .iter()
            .map(|at| match at.as_str().filter(|s| plain(s)) {
                Some(s) => self.block("ha_trigger_time", json!({"AT": s}), vec![], None),
                None => self.raw_part(all, "at", at),
            })
            .collect::<Vec<_>>();
        Some(blocks)
    }

    fn time_pattern(&mut self, m: &Map<String, Value>) -> Option<Value> {
        if !only_keys(m, &["hours", "minutes", "seconds"]) || m.is_empty() {
            return None;
        }
        let part = |k: &str, max: u64| -> Option<Option<Pattern>> {
            m.get(k).map(|v| pattern(v, max)).map_or(Some(None), |p| p.map(Some))
        };
        let (h, mut min, mut s) = (part("hours", 23)?, part("minutes", 59)?, part("seconds", 59)?);
        // As HA does: a larger unit set defaults the smaller ones to zero.
        if min.is_none() && h.is_some() {
            min = Some(Pattern::At(0));
        }
        if s.is_none() && min.is_some() {
            s = Some(Pattern::At(0));
        }
        let any = |p: &Option<Pattern>| matches!(p, None | Some(Pattern::Any));
        let (n, unit) = match (h, min, s) {
            (h, m, Some(Pattern::Every(n))) if any(&h) && any(&m) => (n, "seconds"),
            (h, Some(Pattern::Every(n)), Some(Pattern::At(0))) if any(&h) => (n, "minutes"),
            (Some(Pattern::Every(n)), Some(Pattern::At(0)), Some(Pattern::At(0))) => (n, "hours"),
            _ => return None,
        };
        Some(self.block("ha_trigger_every", json!({"N": n, "UNIT": unit}), vec![], None))
    }

    fn event(&mut self, all: &Map<String, Value>, m: &Map<String, Value>, split: bool) -> Option<Vec<Value>> {
        if !only_keys(m, &["event_type"]) {
            return None;
        }
        let items = match m.get("event_type")? {
            Value::Array(a) if !a.is_empty() => a.clone(),
            Value::Array(_) => return None,
            other => vec![other.clone()],
        };
        if items.len() > 1 && !split {
            return None;
        }
        let blocks = items
            .iter()
            .map(|ev| match ev.as_str().filter(|s| plain(s)) {
                Some(s) => self.block("ha_trigger_event", json!({"EVENT_TYPE": s}), vec![], None),
                None => self.raw_part(all, "event_type", ev),
            })
            .collect();
        Some(blocks)
    }

    fn sun(&mut self, m: &Map<String, Value>) -> Option<Value> {
        if !only_keys(m, &["event", "offset"]) {
            return None;
        }
        let event = m.get("event")?.as_str().filter(|e| ["sunrise", "sunset"].contains(e))?;
        let offset = match m.get("offset") {
            None => 0.0,
            Some(o) => period(o)?,
        };
        // The block writes the offset as HH:MM:SS.
        if offset.fract() != 0.0 {
            return None;
        }
        let mut b = self.block("ha_trigger_sun", json!({"EVENT": event}), vec![], None);
        let d = self.duration(offset);
        shadow(&mut b, "OFFSET", d);
        Some(b)
    }

    fn zone(&mut self, m: &Map<String, Value>) -> Option<Value> {
        if !only_keys(m, &["entity_id", "zone", "event"]) {
            return None;
        }
        let who = entities(m.get("entity_id")?)?;
        let zone = m.get("zone")?.as_str().filter(|z| plain(z) && !z.contains(','))?;
        let event = match m.get("event") {
            None => "enter",
            Some(e) => e.as_str().filter(|e| ["enter", "leave"].contains(e))?,
        };
        Some(self.block(
            "ha_trigger_zone",
            json!({"PERSON": who, "ZONE": zone, "EVENT": event}),
            vec![],
            None,
        ))
    }

    /// The device block sends its stored config as is, so every device trigger fits; its
    /// options stay in the config instead of becoming editable fields.
    fn device(&mut self, all: &Map<String, Value>, m: &Map<String, Value>) -> Option<Value> {
        let device = m.get("device_id")?.as_str()?;
        let domain = m.get("domain")?.as_str()?;
        let mut config = Map::new();
        let key = if all.contains_key("trigger") { "trigger" } else { "platform" };
        config.insert(key.into(), json!("device"));
        config.extend(m.clone());
        let what = [m.get("type"), m.get("subtype")]
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|s| s.replace('_', " "))
            .collect::<Vec<_>>()
            .join(" ");
        let label =
            if what.is_empty() { format!("{domain} device {device}") } else { format!("{domain}: {what}") };
        Some(self.block(
            "device_trigger",
            json!({}),
            vec![],
            Some(json!({"config": config, "label": label, "params": []})),
        ))
    }

    /// A raw block for one entry of a split list: the original trigger with only that entry.
    fn raw_part(&mut self, all: &Map<String, Value>, key: &str, item: &Value) -> Value {
        let mut cfg = all.clone();
        cfg.insert(key.into(), item.clone());
        self.raw_trigger(&Value::Object(cfg))
    }
}

fn shadow(block: &mut Value, input: &str, s: Value) {
    if block.get("inputs").is_none() {
        block["inputs"] = json!({});
    }
    block["inputs"][input] = json!({"shadow": s});
}

/// `- triggers: [...]` entries are spliced into the list, as HA does.
fn flatten(list: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for t in list {
        match t.as_object() {
            Some(m) if m.len() == 1 && m.contains_key("triggers") => {
                out.extend(crate::list(m.get("triggers")))
            }
            _ => out.push(t.clone()),
        }
    }
    out
}

/// Checks the base keys; `None` if one of them needs the raw block.
fn base(m: &Map<String, Value>, id_ok: bool) -> Option<Base> {
    if m.contains_key("trigger") == m.contains_key("platform") {
        return None;
    }
    // `alias` is visible to templates as `trigger.alias`; variables and options have no block.
    if ["alias", "variables", "options"].iter().any(|k| m.contains_key(*k)) {
        return None;
    }
    if m.contains_key("id") && !id_ok {
        return None;
    }
    let disabled = match m.get("enabled") {
        None => false,
        Some(v) => !boolean(v)?,
    };
    Some(Base { disabled })
}

/// HA's `cv.boolean` (templates are rendered by HA, so they stay raw).
fn boolean(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_f64().map(|f| f != 0.0),
        Value::String(s) => match s.trim().to_lowercase().as_str() {
            "1" | "true" | "yes" | "on" | "enable" => Some(true),
            "0" | "false" | "no" | "off" | "disable" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn usable_block_id(id: &str) -> bool {
    let generated = |prefix: &str| {
        id.strip_prefix(prefix).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    };
    !id.is_empty() && !generated("imp") && !generated("var_")
}

/// HA's template check (`is_template_string`).
fn is_tpl(s: &str) -> bool {
    s.contains('{') && (s.contains("{%") || s.contains("{{") || s.contains("{#"))
}

/// A literal the compiler passes on unchanged (it trims fields and drops empty ones).
fn plain(s: &str) -> bool {
    !s.is_empty() && s.trim() == s && !is_tpl(s)
}

/// An entity field from an entity id, a comma-separated string or a list of ids (HA splits
/// the string form at commas, so a list joined with commas means the same).
fn entities(v: &Value) -> Option<String> {
    let ids: Vec<&str> = match v {
        Value::String(s) => vec![s.as_str()],
        Value::Array(a) if !a.is_empty() => a.iter().map(Value::as_str).collect::<Option<_>>()?,
        _ => return None,
    };
    ids.iter().all(|s| plain(s)).then(|| ids.join(", "))
}

/// `from`/`to` of a state trigger: `Some(None)` for any state (absent or null), `Some(Some(s))`
/// for one state; `None` for what the fields can't hold (several states, non-strings, "").
fn state_match(v: Option<&Value>) -> Option<Option<String>> {
    match v {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(s)) if plain(s) => Some(Some(s.clone())),
        Some(Value::Array(a)) if a.len() == 1 => match &a[0] {
            Value::String(s) if plain(s) => Some(Some(s.clone())),
            _ => None,
        },
        _ => None,
    }
}

/// Seconds of a fixed HA time period (`cv.time_period`): "HH:MM", "HH:MM:SS" (optionally
/// signed), seconds as a number or string, or a dict of days…milliseconds.
fn period(v: &Value) -> Option<f64> {
    let secs = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) if is_tpl(s) => return None,
        Value::String(s) => {
            let (sign, body) = match (s.strip_prefix('-'), s.strip_prefix('+')) {
                (Some(r), _) => (-1.0, r),
                (_, Some(r)) => (1.0, r),
                _ => (1.0, s.as_str()),
            };
            let parts: Vec<&str> = body.split(':').collect();
            if parts.len() == 2 || parts.len() == 3 {
                let h: i64 = parts[0].trim().parse().ok()?;
                let m: i64 = parts[1].trim().parse().ok()?;
                let sec: f64 = parts.get(2).map_or(Some(0.0), |p| p.trim().parse().ok())?;
                sign * (h as f64 * 3600.0 + m as f64 * 60.0 + sec)
            } else {
                s.trim().parse().ok()?
            }
        }
        Value::Object(m) if !m.is_empty() => {
            let mut total = 0.0;
            for (k, v) in m {
                let factor = match k.as_str() {
                    "days" => 86400.0,
                    "hours" => 3600.0,
                    "minutes" => 60.0,
                    "seconds" => 1.0,
                    "milliseconds" => 0.001,
                    _ => return None,
                };
                let n = match v {
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
    secs.is_finite().then_some(secs)
}

#[derive(Clone, Copy, PartialEq)]
enum Pattern {
    Any,
    Every(u64),
    At(u64),
}

/// One field of a time pattern, validated like HA's `TimePattern`.
fn pattern(v: &Value, max: u64) -> Option<Pattern> {
    let digits = |s: &str| {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse::<u64>().ok()).flatten()
    };
    match v {
        Value::String(s) if s == "*" => Some(Pattern::Any),
        Value::String(s) if s.starts_with('/') => {
            digits(&s[1..]).filter(|n| (1..=max).contains(n)).map(Pattern::Every)
        }
        Value::String(s) => digits(s).filter(|n| *n <= max).map(Pattern::At),
        Value::Number(n) => n.as_u64().filter(|n| *n <= max).map(Pattern::At),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn types(blocks: &[Value]) -> Vec<&str> {
        blocks.iter().map(|b| b["type"].as_str().unwrap()).collect()
    }

    #[test]
    fn automation_triggers_keep_unique_ids_as_block_ids() {
        let list: Vec<Value> = serde_yaml::from_str(
            r#"
- {trigger: state, entity_id: light.a, to: "on", id: arrived}
- {trigger: state, entity_id: light.b, to: "on", id: twice}
- {trigger: state, entity_id: light.c, to: "on", id: twice}
- {trigger: state, entity_id: light.d, to: "on", id: imp7}
- {trigger: state, entity_id: light.e, to: "on", id: var_1}
- {trigger: time, at: ["07:00", "08:00"], id: split}
- {trigger: event, event_type: x}
"#,
        )
        .unwrap();
        let mut b = Builder::default();
        let blocks = b.automation_triggers(&list);
        assert_eq!(
            types(&blocks),
            [
                "ha_trigger_becomes",
                "ha_trigger_raw",
                "ha_trigger_raw",
                "ha_trigger_raw",
                "ha_trigger_raw",
                "ha_trigger_raw",
                "ha_trigger_event"
            ]
        );
        assert_eq!(blocks[0]["id"], "arrived");
        assert!(blocks[1]["fields"]["CONFIG"].as_str().unwrap().contains("id: twice"));
        assert!(blocks[6]["id"].as_str().unwrap().starts_with("imp"));

        let ws = json!({"blocks": {"blocks": [{
            "type": "ha_when", "id": "w",
            "inputs": {"TRIGGERS": {"block": chain(blocks).unwrap()}},
        }]}});
        let p = puzzlebox_ir::compile(&ws, &puzzlebox_ir::NoExternals).unwrap();
        let puzzlebox_ir::HandlerKind::Triggers { triggers } = &p.handlers[0].kind else { panic!() };
        let ids: Vec<&Value> = triggers
            .iter()
            .map(|t| match t {
                puzzlebox_ir::Trigger::Ha { config, .. } => &config["id"],
                _ => panic!(),
            })
            .collect();
        assert_eq!(
            ids[..6],
            [
                &json!("arrived"),
                &json!("twice"),
                &json!("twice"),
                &json!("imp7"),
                &json!("var_1"),
                &json!("split")
            ]
        );
    }

    #[test]
    fn plain_triggers_keep_ids_raw() {
        let mut b = Builder::default();
        let t = json!({"trigger": "state", "entity_id": "light.a", "to": "on", "id": "x"});
        assert_eq!(types(&b.triggers(&[t])), ["ha_trigger_raw"]);
    }
}
