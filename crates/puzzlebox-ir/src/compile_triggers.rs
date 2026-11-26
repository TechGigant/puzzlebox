//! Home Assistant trigger blocks. Each builds the config HA's own trigger schema takes, so it
//! behaves exactly like the same trigger written in YAML.

use serde_json::{Map, Value, json};

use super::{Block, Compiler, duration_or_none, number};

/// HA's `WEEKDAYS`, in its order.
const WEEKDAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// A signed "HH:MM:SS" offset, as HA's `cv.time_period` reads it.
pub(super) fn offset_text(secs: f64) -> String {
    let sign = if secs < 0.0 { "-" } else { "" };
    let s = secs.abs() as u64;
    format!("{sign}{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// One value, or a list when the text has several separated by commas.
pub(super) fn one_or_list(items: Vec<Value>) -> Option<Value> {
    match items.len() {
        0 => None,
        1 => items.into_iter().next(),
        _ => Some(Value::Array(items)),
    }
}

pub(super) fn comma_list(raw: &str) -> Vec<&str> {
    raw.split(',').map(str::trim).filter(|s| !s.is_empty()).collect()
}

/// Attributes keep their type (brightness is 255, not "255"), so attribute values that read as
/// numbers or true/false are sent as such.
fn attribute_value(s: &str) -> Value {
    match s {
        "true" => json!(true),
        "false" => json!(false),
        _ => match s.parse::<f64>() {
            Ok(n) if n.is_finite() => number(n),
            _ => json!(s),
        },
    }
}

fn checked(b: &Block<'_>, field: &str, default: bool) -> bool {
    match b.field_str(field) {
        Some(v) => v == "TRUE",
        None => default,
    }
}

impl Compiler<'_> {
    /// HA trigger blocks; `None` if `b` isn't one, `Some(None)` if it has errors.
    pub(super) fn ha_trigger(&mut self, b: &Block<'_>) -> Option<Option<Value>> {
        Some(match b.ty() {
            "ha_trigger_state" => Some(self.state_trigger(b)),
            "ha_trigger_numeric" => Some(self.numeric_trigger(b)),
            "ha_trigger_time" => self.time_trigger(b),
            "ha_trigger_time_pattern" => self.time_pattern_trigger(b),
            "ha_trigger_ha_start" => {
                let event = if b.field_str("EVENT") == Some("shutdown") { "shutdown" } else { "start" };
                Some(json!({"trigger": "homeassistant", "event": event}))
            }
            "ha_trigger_template" => {
                let mut cfg =
                    json!({"trigger": "template", "value_template": self.required_str(b, "VALUE_TEMPLATE")});
                if let Some(d) = duration_or_none(self.fixed_number(b, "FOR")) {
                    cfg["for"] = d;
                }
                Some(cfg)
            }
            "ha_trigger_mqtt" => {
                let mut cfg = json!({"trigger": "mqtt", "topic": self.required_str(b, "TOPIC")});
                for (field, key) in [("PAYLOAD", "payload"), ("VALUE_TEMPLATE", "value_template")] {
                    if let Some(v) = b.field_str(field).map(str::trim).filter(|s| !s.is_empty()) {
                        cfg[key] = json!(v);
                    }
                }
                if let Some(q @ ("1" | "2")) = b.field_str("QOS") {
                    cfg["qos"] = json!(q.parse::<u8>().unwrap_or(0));
                }
                Some(cfg)
            }
            "ha_trigger_webhook" => {
                let id = self.required_str(b, "WEBHOOK_ID");
                let methods: Vec<&str> = [("GET", false), ("HEAD", false), ("POST", true), ("PUT", true)]
                    .into_iter()
                    .filter(|(m, default)| checked(b, m, *default))
                    .map(|(m, _)| m)
                    .collect();
                if methods.is_empty() {
                    self.err(b, "tick at least one kind of request");
                    return Some(None);
                }
                Some(json!({
                    "trigger": "webhook",
                    "webhook_id": id,
                    "allowed_methods": methods,
                    "local_only": checked(b, "LOCAL_ONLY", true),
                }))
            }
            "ha_trigger_calendar" => {
                let event = if b.field_str("EVENT") == Some("end") { "end" } else { "start" };
                let mut cfg = json!({"trigger": "calendar", "event": event, "entity_id": self.required_str(b, "ENTITY")});
                if let Some(off) = self.fixed_number(b, "OFFSET").filter(|o| *o != 0.0) {
                    cfg["offset"] = json!(offset_text(off));
                }
                Some(cfg)
            }
            "ha_trigger_tag" => {
                let tags =
                    comma_list(b.field_str("TAG_ID").unwrap_or("")).into_iter().map(Value::from).collect();
                let Some(tags) = one_or_list(tags) else {
                    self.err(b, "fill in \u{201c}tag_id\u{201d}");
                    return Some(None);
                };
                let mut cfg = json!({"trigger": "tag", "tag_id": tags});
                let devices =
                    comma_list(b.field_str("DEVICE").unwrap_or("")).into_iter().map(Value::from).collect();
                if let Some(d) = one_or_list(devices) {
                    cfg["device_id"] = d;
                }
                Some(cfg)
            }
            "ha_trigger_conversation" => {
                let sentences: Vec<&str> = b
                    .field_str("COMMAND")
                    .unwrap_or("")
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect();
                if sentences.is_empty() {
                    self.err(b, "add at least one sentence");
                    return Some(None);
                }
                Some(json!({"trigger": "conversation", "command": sentences}))
            }
            _ => return None,
        })
    }

    fn state_trigger(&mut self, b: &Block<'_>) -> Value {
        let mut cfg = Map::new();
        cfg.insert("trigger".into(), json!("state"));
        cfg.insert("entity_id".into(), json!(self.required_str(b, "ENTITY")));
        let what = b.field_str("WHAT").unwrap_or("state");
        let attribute = what == "attribute";
        if attribute {
            cfg.insert("attribute".into(), json!(self.required_str(b, "ATTRIBUTE")));
        }
        // "state or any detail" fires on every change: HA does that when from/to are all unset.
        if what != "any" {
            for (field, op, key, not_key) in
                [("FROM", "FROM_OP", "from", "not_from"), ("TO", "TO_OP", "to", "not_to")]
            {
                let items = comma_list(b.field_str(field).unwrap_or(""))
                    .into_iter()
                    .map(|s| if attribute { attribute_value(s) } else { json!(s) })
                    .collect();
                if let Some(v) = one_or_list(items) {
                    let key = if b.field_str(op) == Some("not") { not_key } else { key };
                    cfg.insert(key.into(), v);
                }
            }
        }
        if let Some(d) = duration_or_none(self.fixed_number(b, "FOR")) {
            cfg.insert("for".into(), d);
        }
        // "changes" means the state; without from/to HA would also fire on attribute changes.
        if what != "any" && !["from", "to", "not_from", "not_to"].iter().any(|k| cfg.contains_key(*k)) {
            cfg.insert("to".into(), Value::Null);
        }
        Value::Object(cfg)
    }

    fn numeric_trigger(&mut self, b: &Block<'_>) -> Value {
        let mut cfg = json!({"trigger": "numeric_state", "entity_id": self.required_str(b, "ENTITY")});
        match b.field_str("WHAT") {
            Some("attribute") => cfg["attribute"] = json!(self.required_str(b, "ATTRIBUTE")),
            Some("template") => cfg["value_template"] = json!(self.required_str(b, "VALUE_TEMPLATE")),
            _ => {}
        }
        // A number, or an entity id (e.g. input_number.limit), as HA accepts both.
        for (field, key) in [("ABOVE", "above"), ("BELOW", "below")] {
            match b.field_str(field).map(str::trim).filter(|s| !s.is_empty()) {
                Some(raw) => match raw.parse::<f64>() {
                    Ok(n) => cfg[key] = number(n),
                    Err(_) if raw.contains('.') => cfg[key] = json!(raw),
                    Err(_) => self.err(b, format!("'{}' must be a number or an entity id", key)),
                },
                None => {
                    if let Some(n) = b.field_f64(field) {
                        cfg[key] = number(n);
                    }
                }
            }
        }
        if cfg.get("above").is_none() && cfg.get("below").is_none() {
            self.err(b, "set 'above' and/or 'below'");
        }
        if let Some(d) = duration_or_none(self.fixed_number(b, "FOR")) {
            cfg["for"] = d;
        }
        cfg
    }

    fn time_trigger(&mut self, b: &Block<'_>) -> Option<Value> {
        let at = self.required_str(b, "AT");
        let at = match self.fixed_number(b, "OFFSET").filter(|o| *o != 0.0) {
            Some(off) if at.contains('.') => json!({"entity_id": at, "offset": offset_text(off)}),
            Some(_) => {
                self.err(
                    b,
                    "a shift only works with a date/time helper or a timestamp sensor, not a fixed time",
                );
                return None;
            }
            None => json!(at),
        };
        let mut cfg = json!({"trigger": "time", "at": at});
        let days: Vec<&str> = WEEKDAYS.into_iter().filter(|d| checked(b, &d.to_uppercase(), true)).collect();
        match days.len() {
            7 => {}
            0 => {
                self.err(b, "tick at least one day");
                return None;
            }
            1 => cfg["weekday"] = json!(days[0]),
            _ => cfg["weekday"] = json!(days),
        }
        Some(cfg)
    }

    fn time_pattern_trigger(&mut self, b: &Block<'_>) -> Option<Value> {
        let mut parts: Vec<(&str, Value)> = Vec::new();
        for (field, key, max) in
            [("HOURS", "hours", 23), ("MINUTES", "minutes", 59), ("SECONDS", "seconds", 59)]
        {
            let n = b.field_f64(field).unwrap_or(0.0);
            let whole = n.fract() == 0.0;
            let v = match b.field_str(&format!("{field}_MODE")).unwrap_or("any") {
                "every" if whole && (1.0..=max as f64).contains(&n) => json!(format!("/{}", n as u64)),
                "at" if whole && (0.0..=max as f64).contains(&n) => json!(n as u64),
                "every" | "at" => {
                    self.err(b, format!("{key} must be a whole number up to {max}"));
                    return None;
                }
                _ => json!("*"),
            };
            parts.push((key, v));
        }
        // Unset units are "any" until a larger unit is set, then 0 (as in HA's time_pattern),
        // so leading "any" and trailing zeros can be left out.
        let first = parts.iter().position(|(_, v)| v != "*").unwrap_or(parts.len() - 1);
        let mut parts = parts.split_off(first);
        while parts.len() > 1 && parts.last().is_some_and(|(_, v)| *v == json!(0)) {
            parts.pop();
        }
        let mut cfg = Map::new();
        cfg.insert("trigger".into(), json!("time_pattern"));
        cfg.extend(parts.into_iter().map(|(k, v)| (k.to_owned(), v)));
        Some(Value::Object(cfg))
    }
}
