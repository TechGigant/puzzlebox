//! Actions: HA action configs (scripts and automation actions) → statement blocks.

use serde_json::{Value, json};

use crate::*;

impl Builder {
    pub(crate) fn actions(&mut self, list: &[Value]) -> Vec<Value> {
        list.iter().map(|a| self.action(a)).collect()
    }

    pub(crate) fn raw_action(&mut self, a: &Value) -> Value {
        self.raw += 1;
        self.block("ha_action_raw", json!({"CONFIG": yaml(a)}), vec![], None)
    }

    pub(crate) fn action(&mut self, a: &Value) -> Value {
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
    pub(crate) fn sequence_block(&mut self, mut blocks: Vec<Value>, original: &Value) -> Value {
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

    pub(crate) fn target(&mut self, t: &Value) -> Value {
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
