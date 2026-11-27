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
    /// The trigger id and name (alias), as the block's settings (field, value).
    settings: Vec<(&'static str, String)>,
}

impl Builder {
    /// Triggers anywhere (automations and `wait_for_trigger`). An `id` or `alias` becomes the
    /// block's trigger id or name setting, which the compiler sends as is.
    pub(crate) fn triggers(&mut self, list: &[Value]) -> Vec<Value> {
        flatten(list).iter().flat_map(|t| self.trigger_blocks(t, true)).collect()
    }

    pub(crate) fn raw_trigger(&mut self, t: &Value) -> Value {
        self.raw += 1;
        self.block("ha_trigger_raw", json!({"CONFIG": yaml(t)}), vec![], None)
    }

    /// Blocks for one trigger. `split` allows one block per list item (`at: [...]`,
    /// `event_type: [...]`), which HA treats exactly like separate triggers (with the same id).
    fn trigger_blocks(&mut self, t: &Value, split: bool) -> Vec<Value> {
        let Some(m) = t.as_object() else { return vec![self.raw_trigger(t)] };
        let Some(base) = base(m) else { return vec![self.raw_trigger(t)] };
        let Some(mut blocks) = self.native(m, split) else {
            return vec![self.raw_trigger(t)];
        };
        for b in &mut blocks {
            if b["type"] == "ha_trigger_raw" {
                continue;
            }
            if base.disabled {
                b["disabledReasons"] = json!(["MANUALLY_DISABLED"]);
            }
            if !base.settings.is_empty() {
                for (field, v) in &base.settings {
                    b["fields"][*field] = json!(v);
                }
                let names: Vec<&str> = base.settings.iter().map(|(f, _)| *f).collect();
                if b.get("extraState").is_none() {
                    b["extraState"] = json!({});
                }
                b["extraState"]["triggerSettings"] = json!(names);
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
            "homeassistant" if only_keys(&rest, &["event"]) => {
                let event = rest.get("event")?.as_str().filter(|e| ["start", "shutdown"].contains(e))?;
                one(self.block("ha_trigger_ha_start", json!({"EVENT": event}), vec![], None))
            }
            "template" => one(self.template(&rest)?),
            "mqtt" => one(self.mqtt(&rest)?),
            "webhook" => one(self.webhook(&rest)?),
            "calendar" => one(self.calendar(&rest)?),
            "tag" => one(self.tag(&rest)?),
            "conversation" => one(self.conversation(&rest)?),
            "device" => one(self.device(m, &rest)?),
            p @ ("zone.occupancy_detected" | "zone.occupancy_cleared") if self.is_occupancy(&rest) => {
                one(self.occupancy(p, &rest))
            }
            p if p.contains('.') => one(self.integration_trigger(p, &rest)?),
            _ => None,
        }
    }

    fn state(&mut self, m: &Map<String, Value>) -> Option<Value> {
        if !only_keys(m, &["entity_id", "from", "to", "not_from", "not_to", "for", "attribute"]) {
            return None;
        }
        let entity = entities(m.get("entity_id")?)?;
        let attribute = match m.get("attribute") {
            None => None,
            Some(Value::String(s)) if plain(s) => Some(s.clone()),
            _ => return None,
        };
        let typed = attribute.is_some();
        let (from_not, from) = side(m, "from", "not_from", typed)?;
        let (to_not, to) = side(m, "to", "not_to", typed)?;
        // Without any of from/to/not_from/not_to, HA fires on every change, details included.
        let every_change = ["from", "to", "not_from", "not_to"].iter().all(|k| !m.contains_key(*k));
        let duration = match m.get("for") {
            None => None,
            // `for: 0` still sets `trigger.for` to 0 seconds instead of none.
            Some(f) => Some(period(f).filter(|s| *s > 0.0)?),
        };
        if !typed && from.is_none() && !to_not && to.is_some() {
            let mut b = self.block(
                "ha_trigger_becomes",
                json!({"ENTITY": entity, "STATE": to.unwrap_or_default()}),
                vec![],
                None,
            );
            let d = self.duration(duration.unwrap_or(0.0));
            shadow(&mut b, "FOR", d);
            return Some(b);
        }
        // This block's duration is whole seconds.
        if duration.is_some_and(|d| d.fract() != 0.0) {
            return None;
        }
        let what = match (&attribute, every_change) {
            (Some(_), _) => "attribute",
            (None, true) => "any",
            (None, false) => "state",
        };
        let op = |not: bool| if not { "not" } else { "is" };
        let fields = json!({
            "WHAT": what,
            "ATTRIBUTE": attribute.unwrap_or_default(),
            "ENTITY": entity,
            "FROM_OP": op(from_not),
            "FROM": from.unwrap_or_default(),
            "TO_OP": op(to_not),
            "TO": to.unwrap_or_default(),
        });
        let mut b = self.block("ha_trigger_state", fields, vec![], None);
        let d = self.duration(duration.unwrap_or(0.0));
        shadow(&mut b, "FOR", d);
        Some(b)
    }

    fn numeric_state(&mut self, m: &Map<String, Value>) -> Option<Value> {
        if !only_keys(m, &["entity_id", "above", "below", "attribute", "value_template", "for"])
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
        // The block reads either a detail or a template, not both.
        let (what, attribute, template) = match (m.get("attribute"), m.get("value_template")) {
            (None, None) => ("state", "", ""),
            (Some(Value::String(a)), None) if plain(a) => ("attribute", a.as_str(), ""),
            (None, Some(Value::String(t))) if !t.is_empty() && t.trim() == t => ("template", "", t.as_str()),
            _ => return None,
        };
        let duration = match m.get("for") {
            None => 0.0,
            Some(f) => period(f).filter(|s| *s > 0.0 && s.fract() == 0.0)?,
        };
        let mut fields = json!({"WHAT": what, "ENTITY": entity, "ABOVE": above, "BELOW": below});
        if !attribute.is_empty() {
            fields["ATTRIBUTE"] = json!(attribute);
        }
        if !template.is_empty() {
            fields["VALUE_TEMPLATE"] = json!(template);
        }
        let mut b = self.block("ha_trigger_numeric", fields, vec![], None);
        let d = self.duration(duration);
        shadow(&mut b, "FOR", d);
        Some(b)
    }

    /// One block per plain time or entity (with its offset) in `at`; the other entries
    /// (templates) stay raw on their own.
    fn time(&mut self, all: &Map<String, Value>, m: &Map<String, Value>, split: bool) -> Option<Vec<Value>> {
        if !only_keys(m, &["at", "weekday"]) {
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
        let days = match m.get("weekday") {
            None => None,
            Some(v) => Some(weekdays(v)?),
        };
        let blocks = items
            .iter()
            .map(|at| match time_at(at) {
                Some((at, offset)) => {
                    let mut fields = json!({"AT": at});
                    if let Some(days) = &days {
                        for d in WEEKDAYS {
                            fields[d.to_uppercase()] =
                                json!(if days.contains(&d) { "TRUE" } else { "FALSE" });
                        }
                    }
                    let mut b = self.block("ha_trigger_time", fields, vec![], None);
                    let d = self.duration(offset);
                    shadow(&mut b, "OFFSET", d);
                    b
                }
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
        // "every" blocks are exact intervals from when the program starts; HA's "/n" counts
        // from the top of the hour (or minute, or day), which only the clock block does.
        let mut fields = Map::new();
        for (key, p) in [("HOURS", h), ("MINUTES", min), ("SECONDS", s)] {
            let (mode, n) = match p {
                None | Some(Pattern::Any) => ("any", 0),
                Some(Pattern::At(n)) => ("at", n),
                Some(Pattern::Every(n)) => ("every", n),
            };
            fields.insert(format!("{key}_MODE"), json!(mode));
            fields.insert(key.into(), json!(n));
        }
        Some(self.block("ha_trigger_time_pattern", Value::Object(fields), vec![], None))
    }

    fn template(&mut self, m: &Map<String, Value>) -> Option<Value> {
        if !only_keys(m, &["value_template", "for"]) {
            return None;
        }
        let tpl = m.get("value_template")?.as_str().filter(|t| !t.is_empty() && t.trim() == *t)?;
        let duration = match m.get("for") {
            None => 0.0,
            Some(f) => period(f).filter(|s| *s > 0.0 && s.fract() == 0.0)?,
        };
        let mut b = self.block("ha_trigger_template", json!({"VALUE_TEMPLATE": tpl}), vec![], None);
        let d = self.duration(duration);
        shadow(&mut b, "FOR", d);
        Some(b)
    }

    fn mqtt(&mut self, m: &Map<String, Value>) -> Option<Value> {
        if !only_keys(m, &["topic", "payload", "value_template", "qos"]) {
            return None;
        }
        let text = |k: &str| match m.get(k) {
            None => Some(""),
            Some(Value::String(s)) if !s.is_empty() && s.trim() == s => Some(s.as_str()),
            _ => None,
        };
        let topic = text("topic").filter(|t| !t.is_empty())?;
        let (payload, template) = (text("payload")?, text("value_template")?);
        let qos = match m.get("qos") {
            None => 0,
            Some(Value::Number(n)) => n.as_u64().filter(|q| *q <= 2)?,
            Some(Value::String(s)) => s.parse::<u64>().ok().filter(|q| *q <= 2)?,
            _ => return None,
        };
        Some(self.block(
            "ha_trigger_mqtt",
            json!({"TOPIC": topic, "PAYLOAD": payload, "VALUE_TEMPLATE": template, "QOS": qos.to_string()}),
            vec![],
            None,
        ))
    }

    fn webhook(&mut self, m: &Map<String, Value>) -> Option<Value> {
        if !only_keys(m, &["webhook_id", "allowed_methods", "local_only"]) {
            return None;
        }
        let id = m.get("webhook_id")?.as_str().filter(|s| !s.is_empty() && s.trim() == *s)?;
        let methods: Vec<String> = match m.get("allowed_methods") {
            None => vec!["POST".into(), "PUT".into()],
            Some(Value::String(s)) => vec![s.to_uppercase()],
            Some(Value::Array(a)) => {
                a.iter().map(|v| v.as_str().map(str::to_uppercase)).collect::<Option<_>>()?
            }
            _ => return None,
        };
        let known = ["GET", "HEAD", "POST", "PUT"];
        // HA rejects other methods and repeats; no method at all can't be ticked.
        let unique = methods.iter().enumerate().all(|(i, x)| !methods[..i].contains(x));
        if methods.is_empty() || !unique || !methods.iter().all(|x| known.contains(&x.as_str())) {
            return None;
        }
        let local_only = match m.get("local_only") {
            None => true,
            Some(Value::Bool(b)) => *b,
            _ => return None,
        };
        let tick = |b: bool| if b { "TRUE" } else { "FALSE" };
        let mut fields = json!({"WEBHOOK_ID": id, "LOCAL_ONLY": tick(local_only)});
        for k in known {
            fields[k] = json!(tick(methods.iter().any(|x| x == k)));
        }
        Some(self.block("ha_trigger_webhook", fields, vec![], None))
    }

    /// The single-calendar form; `calendar.event_started` and friends take targets and stay raw.
    fn calendar(&mut self, m: &Map<String, Value>) -> Option<Value> {
        if !only_keys(m, &["event", "entity_id", "offset"]) {
            return None;
        }
        let entity = m.get("entity_id")?.as_str().filter(|e| plain(e) && !e.contains(','))?;
        let event = match m.get("event") {
            None => "start",
            Some(e) => e.as_str().filter(|e| ["start", "end"].contains(e))?,
        };
        let offset = match m.get("offset") {
            None => 0.0,
            Some(o) => period(o).filter(|s| s.fract() == 0.0)?,
        };
        let mut b =
            self.block("ha_trigger_calendar", json!({"ENTITY": entity, "EVENT": event}), vec![], None);
        let d = self.duration(offset);
        shadow(&mut b, "OFFSET", d);
        Some(b)
    }

    fn tag(&mut self, m: &Map<String, Value>) -> Option<Value> {
        if !only_keys(m, &["tag_id", "device_id"]) {
            return None;
        }
        let tags = comma_field(m.get("tag_id")?)?;
        let device = match m.get("device_id") {
            None => String::new(),
            Some(d) => comma_field(d)?,
        };
        Some(self.block("ha_trigger_tag", json!({"TAG_ID": tags, "DEVICE": device}), vec![], None))
    }

    fn conversation(&mut self, m: &Map<String, Value>) -> Option<Value> {
        if !only_keys(m, &["command"]) {
            return None;
        }
        let sentences: Vec<&str> = match m.get("command")? {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) if !a.is_empty() => a.iter().map(Value::as_str).collect::<Option<_>>()?,
            _ => return None,
        };
        // The block holds one sentence per line, trimmed.
        if !sentences.iter().all(|s| !s.is_empty() && s.trim() == *s && !s.contains('\n')) {
            return None;
        }
        Some(self.block("ha_trigger_conversation", json!({"COMMAND": sentences.join("\n")}), vec![], None))
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

    /// `zone.occupancy_*` with just a fixed zone and a whole number of seconds (more than 0:
    /// `for: 0` still sets `trigger.for`), as the "first person arrives / last person leaves" block sends it.
    fn is_occupancy(&self, m: &Map<String, Value>) -> bool {
        let Some(Value::Object(o)) = m.get("options") else { return false };
        let zone = o.get("zone").and_then(Value::as_str).is_some_and(|z| plain(z) && !z.contains(','));
        let wait = o.get("for").is_none_or(|f| period(f).is_some_and(|s| s > 0.0 && s.fract() == 0.0));
        only_keys(m, &["options"]) && only_keys(o, &["zone", "for"]) && zone && wait
    }

    fn occupancy(&mut self, platform: &str, m: &Map<String, Value>) -> Value {
        let o = &m["options"];
        let event = if platform.ends_with("cleared") { "leave" } else { "enter" };
        let mut b =
            self.block("ha_trigger_occupancy", json!({"EVENT": event, "ZONE": o["zone"]}), vec![], None);
        let d = self.duration(o.get("for").and_then(period).unwrap_or(0.0));
        shadow(&mut b, "FOR", d);
        b
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

    /// The newer form of integration triggers (`trigger: light.turned_on`, with `target` and
    /// `options`).
    fn integration_trigger(&mut self, name: &str, m: &Map<String, Value>) -> Option<Value> {
        let (target, options) = self.integration_parts(name, m)?;
        Some(self.block(
            "ha_trigger_integration",
            json!({"NAME": name}),
            vec![("TARGET", target), ("OPTIONS", options)],
            None,
        ))
    }

    /// The target and options blocks of an integration trigger or condition; `None` for what
    /// the blocks can't send exactly.
    pub(crate) fn integration_parts(
        &mut self,
        name: &str,
        m: &Map<String, Value>,
    ) -> Option<(Option<Value>, Option<Value>)> {
        let options = match m.get("options") {
            None => None,
            Some(Value::Object(o)) => Some(o),
            Some(_) => return None,
        };
        if !plain(name) || !only_keys(m, &["target", "options"]) {
            return None;
        }
        let snapshot = self.snapshot();
        let target = match m.get("target").map(|t| self.target_block(t)) {
            None => None,
            Some(Some(t)) => Some(t),
            Some(None) => {
                self.restore(snapshot);
                return None;
            }
        };
        Some((target, options.map(|o| self.options_block(o))))
    }

    /// Entity, area, floor, label and device blocks (in a list when there are several kinds).
    fn target_block(&mut self, t: &Value) -> Option<Value> {
        let m = t.as_object().filter(|m| !m.is_empty())?;
        let kinds = [
            ("entity_id", "ha_entity", "ENTITY"),
            ("area_id", "ha_area", "ID"),
            ("floor_id", "ha_floor", "ID"),
            ("label_id", "ha_label", "ID"),
            ("device_id", "ha_device", "ID"),
        ];
        if !only_keys(m, &kinds.map(|(k, _, _)| k)) {
            return None;
        }
        let mut blocks = Vec::new();
        for (key, ty, field) in kinds {
            let Some(v) = m.get(key) else { continue };
            let ids = comma_field(v)?;
            // "all" and "none" are HA keywords, not ids.
            if ids.split(", ").any(|id| ["all", "none"].contains(&id.to_lowercase().as_str())) {
                return None;
            }
            blocks.push(self.block(ty, json!({field: ids}), vec![], None));
        }
        if blocks.len() == 1 {
            return blocks.pop();
        }
        let n = blocks.len();
        let inputs = blocks.into_iter().enumerate().map(|(i, b)| (format!("ADD{i}"), json!({"block": b})));
        let mut list = self.block("lists_create_with", json!({}), vec![], Some(json!({"itemCount": n})));
        list["inputs"] = Value::Object(inputs.collect());
        Some(list)
    }

    /// A dictionary block when every value is one the blocks keep exactly (text without
    /// templates, numbers, true/false, nothing, lists, dictionaries), else the options as YAML.
    fn options_block(&mut self, o: &Map<String, Value>) -> Value {
        fn exact(v: &Value) -> bool {
            match v {
                Value::String(s) => !is_tpl(s),
                // Whole numbers written as 5.0 would be sent as 5.
                Value::Number(n) => {
                    n.is_i64() || n.as_f64().is_some_and(|f| f.is_finite() && f.fract() != 0.0)
                }
                Value::Bool(_) | Value::Null => true,
                Value::Array(a) => a.iter().all(exact),
                Value::Object(m) => m.values().all(exact),
            }
        }
        if o.values().all(exact) {
            return self.dict(o);
        }
        let json = serde_json::to_string(o).unwrap_or_default();
        self.block("yaml_value", json!({"CONFIG": json}), vec![], None)
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
fn base(m: &Map<String, Value>) -> Option<Base> {
    if m.contains_key("trigger") == m.contains_key("platform") {
        return None;
    }
    // Trigger variables have no block.
    if m.contains_key("variables") {
        return None;
    }
    let mut settings = Vec::new();
    for (key, field) in [("id", "TRIGGER_ID"), ("alias", "TRIGGER_ALIAS")] {
        match m.get(key) {
            None => {}
            // HA takes text only; the settings leave out empty ones.
            Some(Value::String(s)) if !s.trim().is_empty() => settings.push((field, s.clone())),
            Some(_) => return None,
        }
    }
    let disabled = match m.get("enabled") {
        None => false,
        Some(v) => !boolean(v)?,
    };
    Some(Base { disabled, settings })
}

/// Points the `triggered_by` blocks at the first of the automation's triggers with that id
/// (they compare with its id), so the editor shows which trigger they mean.
pub(crate) fn link_triggered_by(top: &mut Value) {
    let mut first: HashMap<String, String> = HashMap::new();
    let mut cur = top.pointer("/inputs/TRIGGERS/block");
    while let Some(b) = cur {
        if let (Some(own), Some(id)) =
            (b.pointer("/fields/TRIGGER_ID").and_then(Value::as_str), b["id"].as_str())
        {
            first.entry(own.to_owned()).or_insert_with(|| id.to_owned());
        }
        cur = b.pointer("/next/block");
    }
    fn walk(v: &mut Value, first: &HashMap<String, String>) {
        match v {
            Value::Object(m) => {
                if m.get("type").and_then(Value::as_str) == Some("triggered_by")
                    && let Some(t) = m.get_mut("fields").and_then(|f| f.get_mut("TRIGGER"))
                    && let Some(id) = t.as_str().and_then(|t| first.get(t))
                {
                    *t = json!(id);
                }
                m.values_mut().for_each(|x| walk(x, first));
            }
            Value::Array(a) => a.iter_mut().for_each(|x| walk(x, first)),
            _ => {}
        }
    }
    walk(top, &first);
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

/// Ids like the importer's own block ids (`imp<N>`), which "triggered by" would take for one.
pub(crate) fn looks_like_block_id(id: &str) -> bool {
    id.strip_prefix("imp").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
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

/// `from`/`not_from` (or `to`/`not_to`) of a state trigger: whether it's the "not" form, and
/// the values as the field holds them (`None` for any). `None` for what the field can't hold.
fn side(m: &Map<String, Value>, key: &str, not_key: &str, typed: bool) -> Option<(bool, Option<String>)> {
    match (m.get(key), m.get(not_key)) {
        // HA rejects both.
        (Some(_), Some(_)) => None,
        (v, None) => Some((false, state_values(v, typed)?)),
        (None, v) => Some((true, state_values(v, typed)?)),
    }
}

/// State values: `Some(None)` for any (absent or null), else the comma-separated text the
/// compiler splits again. With `typed` (an attribute), numbers and booleans are allowed and
/// strings that the compiler would turn into one stay raw.
fn state_values(v: Option<&Value>, typed: bool) -> Option<Option<String>> {
    let item = |v: &Value| -> Option<String> {
        match v {
            Value::String(s)
                if plain(s)
                    && !s.contains(',')
                    && !(typed && (s.parse::<f64>().is_ok() || s == "true" || s == "false")) =>
            {
                Some(s.clone())
            }
            Value::Number(n) if typed && n.as_f64().is_some_and(f64::is_finite) => Some(n.to_string()),
            Value::Bool(b) if typed => Some(b.to_string()),
            _ => None,
        }
    };
    match v {
        None | Some(Value::Null) => Some(None),
        Some(Value::Array(a)) if !a.is_empty() => {
            Some(Some(a.iter().map(item).collect::<Option<Vec<_>>>()?.join(", ")))
        }
        Some(Value::Array(_)) => None,
        Some(v) => Some(Some(item(v)?)),
    }
}

/// A text field from one string or a list of them (the compiler splits it at commas again).
fn comma_field(v: &Value) -> Option<String> {
    let items: Vec<&str> = match v {
        Value::String(s) => vec![s.as_str()],
        Value::Array(a) if !a.is_empty() => a.iter().map(Value::as_str).collect::<Option<_>>()?,
        _ => return None,
    };
    items.iter().all(|s| plain(s) && !s.contains(',')).then(|| items.join(", "))
}

/// HA's `WEEKDAYS`; the time block has a box for each.
const WEEKDAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

fn weekdays(v: &Value) -> Option<Vec<&'static str>> {
    let items: Vec<&str> = match v {
        Value::String(s) => vec![s.as_str()],
        Value::Array(a) if !a.is_empty() => a.iter().map(Value::as_str).collect::<Option<_>>()?,
        _ => return None,
    };
    items.iter().map(|d| WEEKDAYS.into_iter().find(|w| w == d)).collect()
}

/// One entry of a time trigger's `at`: a time or entity, and its offset in whole seconds.
fn time_at(at: &Value) -> Option<(String, f64)> {
    match at {
        Value::String(s) if plain(s) => Some((s.clone(), 0.0)),
        Value::Object(o) if only_keys(o, &["entity_id", "offset"]) => {
            let entity = o.get("entity_id")?.as_str().filter(|e| {
                plain(e) && !e.contains(',') && (e.starts_with("input_datetime.") || e.starts_with("sensor."))
            })?;
            let offset = match o.get("offset") {
                None => 0.0,
                Some(v) => period(v).filter(|s| s.fract() == 0.0)?,
            };
            Some((entity.to_owned(), offset))
        }
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
    fn ids_and_aliases_become_settings() {
        let list: Vec<Value> = serde_yaml::from_str(
            r#"
- {trigger: state, entity_id: light.a, to: "on", id: twice, alias: Light on}
- {trigger: state, entity_id: light.b, to: "on", id: twice}
- {trigger: time, at: ["07:00", "08:00"], id: split}
- {trigger: state, entity_id: light.c, to: "on", id: ""}
- {trigger: state, entity_id: light.c, to: "on", alias: [x]}
"#,
        )
        .unwrap();
        let mut b = Builder::default();
        let blocks = b.triggers(&list);
        assert_eq!(
            types(&blocks),
            [
                "ha_trigger_becomes",
                "ha_trigger_becomes",
                "ha_trigger_time",
                "ha_trigger_time",
                "ha_trigger_raw",
                "ha_trigger_raw"
            ]
        );
        assert_eq!(blocks[0]["fields"]["TRIGGER_ID"], "twice");
        assert_eq!(blocks[0]["fields"]["TRIGGER_ALIAS"], "Light on");
        assert_eq!(blocks[0]["extraState"], json!({"triggerSettings": ["TRIGGER_ID", "TRIGGER_ALIAS"]}));
        assert_eq!(blocks[1]["extraState"], json!({"triggerSettings": ["TRIGGER_ID"]}));
        assert_eq!(blocks[2]["fields"]["TRIGGER_ID"], "split");
        assert_eq!(blocks[3]["fields"]["TRIGGER_ID"], "split");
    }
}
