//! Conditions: HA condition configs → true/false blocks.

use serde_json::{Value, json};

use crate::*;

impl Builder {
    pub(crate) fn and_all(&mut self, conds: &[Value]) -> Value {
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

    pub(crate) fn or_all(&mut self, conds: &[Value]) -> Value {
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
}
