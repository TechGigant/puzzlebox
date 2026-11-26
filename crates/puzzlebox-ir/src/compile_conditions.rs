//! Condition blocks that compile to one native HA condition config: state (with its optional
//! detail, several states and "any" match), numeric state, sun, zone and template.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::triggers::fixed;
use super::{Block, Compiler, arg_placeholder, number};
use crate::ir::*;

/// A state the user typed, as HA compares it. Attribute values keep their YAML type (HA
/// compares them with `==`), so numbers and true/false are sent as such.
fn typed_state(s: &str, attribute: bool) -> Value {
    if attribute && let Ok(v @ (Value::Number(_) | Value::Bool(_))) = serde_json::from_str::<Value>(s.trim())
    {
        return v;
    }
    json!(s)
}

/// The state box: one state, or several separated with commas (an HA list).
fn states(text: &str, attribute: bool) -> Value {
    if !text.contains(',') {
        return typed_state(text, attribute);
    }
    let items: Vec<Value> =
        text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(|s| typed_state(s, attribute)).collect();
    Value::Array(items)
}

/// One id, or a list for several separated with commas.
fn ids(text: &str) -> Value {
    let list: Vec<&str> = text.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    match list.as_slice() {
        [one] => json!(one),
        _ => json!(list),
    }
}

/// `HH:MM:SS` (with a minus sign for earlier), as HA writes sun offsets.
fn offset_text(seconds: f64) -> String {
    let sign = if seconds < 0.0 { "-" } else { "" };
    let s = seconds.abs().round() as u64;
    format!("{sign}{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

impl Compiler<'_> {
    /// `ha_is_state` and (with `for`) `ha_state_for`.
    pub(super) fn state_condition(&mut self, b: &Block<'_>, with_for: bool) -> ExprKind {
        let mut args = BTreeMap::new();
        args.insert("entity".to_owned(), self.entity_expr(b));
        let mut config = json!({"condition": "state", "entity_id": arg_placeholder("entity")});
        // HA doesn't allow "for" with an attribute; ha_state_for has no detail setting.
        let attribute = b.field_str("ATTRIBUTE").map(str::trim).filter(|a| !a.is_empty() && !with_for);
        if let Some(a) = attribute {
            config["attribute"] = json!(a);
        }
        config["state"] = states(b.field_str("STATE").unwrap_or(""), attribute.is_some());
        if b.field_str("MATCH") == Some("any") {
            config["match"] = json!("any");
        }
        if with_for {
            args.insert("for".to_owned(), self.value(b, "SECONDS"));
            config["for"] = json!({"seconds": arg_placeholder("for")});
        }
        ExprKind::Condition { config, args }
    }

    /// `{"condition": "light.is_on", "target": …, "options": …}`, for any integration's
    /// condition. Fixed parts are written into the config; variables are filled in each time.
    fn integration_condition(&mut self, b: &Block<'_>) -> ExprKind {
        let mut config = json!({"condition": self.required_str(b, "NAME")});
        let mut args = BTreeMap::new();
        if let Some(t) = self.integration_target(b) {
            match fixed(&t).map(|v| normalize_target(&v)) {
                Some(Ok(v)) => config["target"] = v,
                Some(Err(e)) => self.err(b, e),
                None => {
                    config["target"] = arg_placeholder("target");
                    let id = t.id.clone();
                    args.insert(
                        "target".to_owned(),
                        Expr { id, kind: ExprKind::Builtin { name: "to_target".into(), args: vec![t] } },
                    );
                }
            }
        }
        if let Some(o) = self.value_opt(b, "OPTIONS") {
            match fixed(&o) {
                Some(Value::Null) => {}
                Some(v @ Value::Object(_)) => config["options"] = v,
                Some(_) => self.err(b, "the options must be a dictionary"),
                None => {
                    config["options"] = arg_placeholder("options");
                    args.insert("options".to_owned(), o);
                }
            }
        }
        ExprKind::Condition { config, args }
    }

    /// The condition blocks of this module; `None` if `b` isn't one.
    pub(super) fn condition_expr(&mut self, b: &Block<'_>) -> Option<ExprKind> {
        Some(match b.ty() {
            "ha_numeric_state" => {
                let mut args = BTreeMap::new();
                args.insert("entity".to_owned(), self.entity_expr(b));
                let mut config =
                    json!({"condition": "numeric_state", "entity_id": arg_placeholder("entity")});
                for (field, key) in [("ABOVE", "above"), ("BELOW", "below")] {
                    let Some(raw) = b.field_str(field).map(str::trim).filter(|s| !s.is_empty()) else {
                        continue;
                    };
                    match raw.parse::<f64>() {
                        Ok(n) if n.is_finite() => config[key] = number(n),
                        _ if raw.contains('.') && !raw.contains(' ') => config[key] = json!(raw),
                        _ => self.err(b, format!("'{key}' must be a number or an entity id")),
                    }
                }
                if config.get("above").is_none() && config.get("below").is_none() {
                    self.err(b, "set 'above' and/or 'below'");
                }
                if let Some(a) = b.field_str("ATTRIBUTE").map(str::trim).filter(|a| !a.is_empty()) {
                    config["attribute"] = json!(a);
                }
                if let Some(t) = b.field_str("VALUE_TEMPLATE").filter(|t| !t.trim().is_empty()) {
                    config["value_template"] = json!(t);
                }
                ExprKind::Condition { config, args }
            }
            "sun_condition" => {
                let mut config = json!({"condition": "sun"});
                let mut args = BTreeMap::new();
                let mut bound = |c: &mut Self, when: &str, event: &str, offset: &str| {
                    let when = if b.field_str(when) == Some("before") { "before" } else { "after" };
                    if config.get(when).is_some() {
                        c.err(
                            b,
                            "one limit must be \u{201c}before\u{201d} and the other \u{201c}after\u{201d}",
                        );
                        return;
                    }
                    let event = if b.field_str(event) == Some("sunrise") { "sunrise" } else { "sunset" };
                    config[when] = json!(event);
                    let key = format!("{when}_offset");
                    match c.value_opt(b, offset) {
                        Some(Expr { kind: ExprKind::Lit { value }, .. }) => {
                            if let Some(s) = value.as_f64().filter(|s| *s != 0.0) {
                                config[&key] = json!(offset_text(s));
                            }
                        }
                        // Seconds, which HA's time period also accepts.
                        Some(e) => {
                            config[&key] = arg_placeholder(&key);
                            args.insert(key, e);
                        }
                        None => {}
                    }
                };
                bound(self, "WHEN", "EVENT", "OFFSET");
                if b.field("WHEN2").is_some() {
                    bound(self, "WHEN2", "EVENT2", "SECOND");
                }
                ExprKind::Condition { config, args }
            }
            "zone_condition" => {
                let mut args = BTreeMap::new();
                args.insert("entity".to_owned(), self.entity_expr(b));
                let zone = self.required_str(b, "ZONE");
                ExprKind::Condition {
                    config: json!({"condition": "zone", "entity_id": arg_placeholder("entity"), "zone": ids(&zone)}),
                    args,
                }
            }
            "ha_condition_integration" => self.integration_condition(b),
            "ha_template_condition" => {
                let template = b.field_str("TEMPLATE").unwrap_or("").to_owned();
                if template.trim().is_empty() {
                    self.err(b, "fill in the template");
                }
                ExprKind::Condition {
                    config: json!({"condition": "template", "value_template": template}),
                    args: BTreeMap::new(),
                }
            }
            _ => return None,
        })
    }
}
