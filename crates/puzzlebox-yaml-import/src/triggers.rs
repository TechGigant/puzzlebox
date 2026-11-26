//! Triggers: HA trigger configs → trigger blocks (raw YAML block when no block fits).

use serde_json::{Value, json};

use crate::*;

impl Builder {
    pub(crate) fn triggers(&mut self, list: &[Value]) -> Vec<Value> {
        list.iter().map(|t| self.trigger(t)).collect()
    }

    pub(crate) fn raw_trigger(&mut self, t: &Value) -> Value {
        self.raw += 1;
        self.block("ha_trigger_raw", json!({"CONFIG": yaml(t)}), vec![], None)
    }

    pub(crate) fn trigger(&mut self, t: &Value) -> Value {
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
}
