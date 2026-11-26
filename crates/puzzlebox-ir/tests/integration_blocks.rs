//! Integration triggers and conditions (`light.turned_on`, `light.is_on`) and the id and name
//! settings of trigger blocks.

use std::collections::BTreeMap;

use puzzlebox_ir::*;
use serde_json::{Value, json};

fn when(triggers: Value, body: Option<Value>) -> Value {
    let mut w = json!({"type": "ha_when", "id": "w", "inputs": {"TRIGGERS": {"block": triggers}}});
    if let Some(b) = body {
        w["inputs"]["DO"] = json!({"block": b});
    }
    json!({"blocks": {"blocks": [w]}})
}

/// The HA configs of the "when" block's triggers, ids included.
fn configs(ws: &Value) -> Vec<Value> {
    let p = compile(ws, &NoExternals).unwrap();
    let HandlerKind::Triggers { triggers } = &p.handlers[0].kind else { panic!() };
    triggers
        .iter()
        .map(|t| match t {
            Trigger::Ha { config, .. } => config.clone(),
            other => panic!("not an HA trigger: {other:?}"),
        })
        .collect()
}

fn errors(ws: &Value) -> Vec<String> {
    compile(ws, &NoExternals).unwrap_err().into_iter().map(|e| e.message).collect()
}

fn block(ty: &str, id: &str, fields: Value, inputs: Value) -> Value {
    json!({"type": ty, "id": id, "fields": fields, "inputs": inputs})
}

fn shadow(ty: &str, fields: Value) -> Value {
    json!({"shadow": {"type": ty, "id": format!("s_{ty}"), "fields": fields}})
}

fn dict(pairs: &[(&str, Value)]) -> Value {
    let mut next: Option<Value> = None;
    for (k, v) in pairs.iter().rev() {
        let mut p = json!({"type": "dict_pair", "id": format!("p_{k}"), "fields": {"KEY": k},
            "inputs": {"VALUE": {"block": v}}});
        if let Some(n) = next.take() {
            p["next"] = json!({"block": n});
        }
        next = Some(p);
    }
    let mut d = json!({"type": "dict_create", "id": "d"});
    if let Some(n) = next {
        d["inputs"] = json!({"ITEMS": {"block": n}});
    }
    json!({"block": d})
}

fn text(s: &str) -> Value {
    json!({"type": "text", "id": format!("txt_{s}"), "fields": {"TEXT": s}})
}

#[test]
fn integration_trigger() {
    let t = block(
        "ha_trigger_integration",
        "t",
        json!({"NAME": "light.turned_on"}),
        json!({
            "TARGET": {"block": {"type": "lists_create_with", "id": "l", "extraState": {"itemCount": 2}, "inputs": {
                "ADD0": shadow("ha_area", json!({"ID": "kitchen"})),
                "ADD1": shadow("ha_entity", json!({"ENTITY": "light.a, light.b"})),
            }}},
            "OPTIONS": dict(&[("behavior", text("any")), ("for", json!({"type": "math_number", "id": "n", "fields": {"NUM": 5}}))]),
        }),
    );
    assert_eq!(
        configs(&when(t, None)),
        [json!({"trigger": "light.turned_on", "id": "t",
            "target": {"area_id": ["kitchen"], "entity_id": ["light.a", "light.b"]},
            "options": {"behavior": "any", "for": 5}})]
    );

    // No target (an empty entity picker) and no options, or empty options.
    let bare = block(
        "ha_trigger_integration",
        "t",
        json!({"NAME": "sun.elevation_crossed_threshold"}),
        json!({"TARGET": shadow("ha_entity", json!({"ENTITY": ""}))}),
    );
    assert_eq!(
        configs(&when(bare, None)),
        [json!({"trigger": "sun.elevation_crossed_threshold", "id": "t"})]
    );
    let empty = block(
        "ha_trigger_integration",
        "t",
        json!({"NAME": "x.y"}),
        json!({"OPTIONS": shadow("yaml_value", json!({"CONFIG": "{}"}))}),
    );
    assert_eq!(configs(&when(empty, None)), [json!({"trigger": "x.y", "id": "t", "options": {}})]);

    let var = json!({"block": {"type": "variables_get", "id": "g", "fields": {"VAR": {"id": "v"}}}});
    let mut ws =
        when(block("ha_trigger_integration", "t", json!({"NAME": "x.y"}), json!({"OPTIONS": var})), None);
    ws["variables"] = json!([{"name": "o", "id": "v"}]);
    assert_eq!(errors(&ws), ["a trigger needs fixed options, not a variable or calculation"]);
    let ws = when(block("ha_trigger_integration", "t", json!({"NAME": " "}), json!({})), None);
    assert_eq!(errors(&ws), ["fill in \u{201c}name\u{201d}"]);
}

/// The config a condition sends, with fixed arguments filled in.
fn condition(cond: Value) -> (Value, BTreeMap<String, Expr>) {
    let ws = json!({"blocks": {"blocks": [{"type": "ha_on_run", "id": "r", "inputs": {"DO": {"block": {
        "type": "controls_if", "id": "if", "inputs": {"IF0": {"block": cond}}}}}}]}});
    let p = compile(&ws, &NoExternals).unwrap();
    let StmtKind::If { branches, .. } = &p.handlers[0].body[0].kind else { panic!() };
    let ExprKind::Condition { config, args } = &branches[0].0.kind else { panic!() };
    (config.clone(), args.clone())
}

#[test]
fn integration_condition() {
    let c = block(
        "ha_condition_integration",
        "c",
        json!({"NAME": "light.is_on"}),
        json!({"TARGET": shadow("ha_label", json!({"ID": "downstairs"})), "OPTIONS": dict(&[("behavior", text("all"))])}),
    );
    let (config, args) = condition(c);
    assert_eq!(
        config,
        json!({"condition": "light.is_on", "target": {"label_id": ["downstairs"]}, "options": {"behavior": "all"}})
    );
    assert!(args.is_empty());

    // A target from a variable is turned into an HA target each time.
    let var = json!({"block": {"type": "ha_state", "id": "st", "inputs": {"ENTITY": shadow("ha_entity", json!({"ENTITY": "input_text.which"}))}}});
    let (config, args) = condition(block(
        "ha_condition_integration",
        "c",
        json!({"NAME": "light.is_on"}),
        json!({"TARGET": var}),
    ));
    assert_eq!(config, json!({"condition": "light.is_on", "target": {"$arg": "target"}}));
    assert!(matches!(&args["target"].kind, ExprKind::Builtin { name, .. } if name == "to_target"));
}

fn state_trigger(id: &str, fields: Value) -> Value {
    let mut f = json!({"ENTITY": "light.a", "TO": "on"});
    f.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
    json!({"type": "ha_trigger_state", "id": id, "fields": f})
}

#[test]
fn trigger_id_and_name() {
    let mut first = state_trigger("t1", json!({"TRIGGER_ID": "arrived", "TRIGGER_ALIAS": "Light on"}));
    first["next"] = json!({"block": state_trigger("t2", json!({"TRIGGER_ID": "arrived"}))});
    first["next"]["block"]["next"] =
        json!({"block": state_trigger("t3", json!({"TRIGGER_ID": " ", "TRIGGER_ALIAS": ""}))});
    let cfg = |id: &str, extra: Value| {
        let mut c = json!({"trigger": "state", "entity_id": "light.a", "to": "on", "id": id});
        c.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        c
    };
    // Ids may repeat, as in HA; empty settings are left out.
    assert_eq!(
        configs(&when(first.clone(), None)),
        [cfg("arrived", json!({"alias": "Light on"})), cfg("arrived", json!({})), cfg("t3", json!({}))]
    );

    // "triggered by" compares with the trigger's own id; without one, with its block id.
    let by = |t: &str| json!({"type": "triggered_by", "id": format!("by_{t}"), "fields": {"TRIGGER": t}});
    let right = |e: &Expr| match &e.kind {
        ExprKind::Binary { right, .. } => right.kind.clone(),
        other => panic!("{other:?}"),
    };
    for (t, want) in [("t1", "arrived"), ("t3", "t3"), ("door", "door")] {
        let body = json!({"type": "controls_if", "id": "if", "inputs": {"IF0": {"block": by(t)}}});
        let p = compile(&when(first.clone(), Some(body)), &NoExternals).unwrap();
        let StmtKind::If { branches, .. } = &p.handlers[0].body[0].kind else { panic!() };
        assert_eq!(right(&branches[0].0), ExprKind::Lit { value: json!(want) });
    }

    // Waits send the id and name too (and no block id, as before).
    let wait = json!({"type": "ha_wait_for_trigger", "id": "wt", "inputs": {"TRIGGERS": {"block":
        state_trigger("t4", json!({"TRIGGER_ID": "back"}))}}});
    let ws =
        json!({"blocks": {"blocks": [{"type": "ha_on_run", "id": "r", "inputs": {"DO": {"block": wait}}}]}});
    let p = compile(&ws, &NoExternals).unwrap();
    let StmtKind::WaitForTrigger { triggers, .. } = &p.handlers[0].body[0].kind else { panic!() };
    assert_eq!(triggers, &[json!({"trigger": "state", "entity_id": "light.a", "to": "on", "id": "back"})]);
    let wait = json!({"type": "ha_wait_for_trigger", "id": "wt", "inputs": {"TRIGGERS": {"block": state_trigger("t4", json!({}))}}});
    let ws =
        json!({"blocks": {"blocks": [{"type": "ha_on_run", "id": "r", "inputs": {"DO": {"block": wait}}}]}});
    let p = compile(&ws, &NoExternals).unwrap();
    let StmtKind::WaitForTrigger { triggers, .. } = &p.handlers[0].body[0].kind else { panic!() };
    assert_eq!(triggers, &[json!({"trigger": "state", "entity_id": "light.a", "to": "on"})]);

    // Engine triggers report their block id and can't take one.
    let cron = json!({"type": "engine_trigger_cron", "id": "c", "fields": {"CRON": "0 * * * * *", "TRIGGER_ID": "x"}});
    assert_eq!(errors(&when(cron, None)), ["only Home Assistant triggers can have an id or a name"]);
}
