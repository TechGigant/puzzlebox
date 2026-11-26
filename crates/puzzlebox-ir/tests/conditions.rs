use std::collections::BTreeMap;

use puzzlebox_ir::*;
use serde_json::{Value, json};

/// Compiles `cond` inside an "if" and returns the HA config it sends (literal args filled in).
fn config(cond: Value) -> Value {
    let p = compile_one(cond).unwrap();
    let StmtKind::If { branches, .. } = &p.handlers[0].body[0].kind else { panic!() };
    let ExprKind::Condition { config, args } = &branches[0].0.kind else { panic!("{:?}", branches[0].0) };
    let vals: BTreeMap<String, Value> = args
        .iter()
        .map(|(k, a)| match &a.kind {
            ExprKind::Lit { value } => (k.clone(), value.clone()),
            other => panic!("argument {k} is not a literal: {other:?}"),
        })
        .collect();
    substitute_args(config, &vals)
}

fn compile_one(cond: Value) -> Result<Program, Vec<CompileError>> {
    let ws = json!({"blocks": {"blocks": [{"type": "ha_on_run", "id": "r", "inputs": {"DO": {"block": {
        "type": "controls_if", "id": "if", "inputs": {"IF0": {"block": cond}}}}}}]}});
    compile(&ws, &NoExternals)
}

fn entity(ids: &str) -> Value {
    json!({"shadow": {"type": "ha_entity", "id": "e", "fields": {"ENTITY": ids}}})
}

fn minutes(n: i64) -> Value {
    json!({"shadow": {"type": "duration", "id": "d", "fields": {"UNIT": "minutes"},
        "inputs": {"N": {"shadow": {"type": "math_number", "id": "n", "fields": {"NUM": n}}}}}})
}

fn is_state(fields: Value) -> Value {
    json!({"type": "ha_is_state", "id": "s", "fields": fields, "inputs": {"ENTITY": entity("light.a")}})
}

#[test]
fn state_condition_keeps_the_old_config() {
    assert_eq!(
        config(is_state(json!({"STATE": "on"}))),
        json!({"condition": "state", "entity_id": "light.a", "state": "on"})
    );
    // Without a comma the state is sent as typed, spaces included.
    assert_eq!(config(is_state(json!({"STATE": " on "})))["state"], " on ");
    let old = json!({"type": "ha_state_for", "id": "s", "fields": {"STATE": "off"},
        "inputs": {"ENTITY": entity("light.a"), "SECONDS": minutes(5)}});
    assert_eq!(
        config(old),
        json!({"condition": "state", "entity_id": "light.a", "state": "off", "for": {"seconds": 300}})
    );
}

#[test]
fn state_condition_options() {
    assert_eq!(config(is_state(json!({"STATE": "on, unavailable"})))["state"], json!(["on", "unavailable"]));
    let any = json!({"type": "ha_is_state", "id": "s", "extraState": {"options": ["MATCH"]},
        "fields": {"STATE": "on", "MATCH": "any"}, "inputs": {"ENTITY": entity("light.a,light.b")}});
    assert_eq!(
        config(any),
        json!({"condition": "state", "entity_id": "light.a,light.b", "state": "on", "match": "any"})
    );
    assert!(config(is_state(json!({"STATE": "on", "MATCH": "all"}))).get("match").is_none());

    // Details keep their type: HA compares attribute values with ==.
    let attr = |state: &str| config(is_state(json!({"STATE": state, "ATTRIBUTE": "brightness"})));
    assert_eq!(
        attr("255"),
        json!({"condition": "state", "entity_id": "light.a", "attribute": "brightness", "state": 255})
    );
    assert_eq!(attr("true")["state"], true);
    assert_eq!(attr("0.5, 1")["state"], json!([0.5, 1]));
    assert_eq!(attr("heat")["state"], "heat");
    assert_eq!(attr("01")["state"], "01");
    assert_eq!(config(is_state(json!({"STATE": "255", "ATTRIBUTE": " "})))["state"], "255");

    let for_any = json!({"type": "ha_state_for", "id": "s", "fields": {"STATE": "off,unavailable", "MATCH": "any"},
        "inputs": {"ENTITY": entity("light.a,light.b"), "SECONDS": minutes(1)}});
    assert_eq!(
        config(for_any),
        json!({"condition": "state", "entity_id": "light.a,light.b", "state": ["off", "unavailable"],
            "match": "any", "for": {"seconds": 60}})
    );
}

#[test]
fn numeric_state_condition() {
    let numeric = |fields: Value| json!({"type": "ha_numeric_state", "id": "n", "fields": fields, "inputs": {"ENTITY": entity("sensor.t")}});
    assert_eq!(
        config(numeric(json!({"ABOVE": "20", "BELOW": ""}))),
        json!({"condition": "numeric_state", "entity_id": "sensor.t", "above": 20})
    );
    assert_eq!(
        config(numeric(json!({"ABOVE": "", "BELOW": "input_number.max", "ATTRIBUTE": "temperature",
            "VALUE_TEMPLATE": "{{ state.state | float * 2 }}"}))),
        json!({"condition": "numeric_state", "entity_id": "sensor.t", "below": "input_number.max",
            "attribute": "temperature", "value_template": "{{ state.state | float * 2 }}"})
    );
    assert_eq!(config(numeric(json!({"ABOVE": "-1.5", "BELOW": "30"})))["above"], -1.5);
    assert!(compile_one(numeric(json!({"ABOVE": "", "BELOW": ""}))).is_err());
    assert!(compile_one(numeric(json!({"ABOVE": "warm"}))).is_err());
}

#[test]
fn sun_condition() {
    let sun = |fields: Value, inputs: Value| json!({"type": "sun_condition", "id": "s", "fields": fields, "inputs": inputs});
    assert_eq!(
        config(sun(json!({"WHEN": "after", "EVENT": "sunset"}), json!({"OFFSET": minutes(0)}))),
        json!({"condition": "sun", "after": "sunset"})
    );
    assert_eq!(
        config(sun(
            json!({"WHEN": "after", "EVENT": "sunset", "WHEN2": "before", "EVENT2": "sunrise"}),
            json!({"OFFSET": minutes(-30), "SECOND": minutes(90)})
        )),
        json!({"condition": "sun", "after": "sunset", "after_offset": "-00:30:00",
            "before": "sunrise", "before_offset": "01:30:00"})
    );
    // An offset from a calculation is sent as seconds.
    let calc = json!({"block": {"type": "math_arithmetic", "id": "m", "fields": {"OP": "MULTIPLY"}, "inputs": {
        "A": {"shadow": {"type": "math_number", "id": "a", "fields": {"NUM": 2}}},
        "B": {"block": {"type": "variables_get", "id": "g", "fields": {"VAR": {"id": "v"}}}}}}});
    let ws = json!({"blocks": {"blocks": [{"type": "ha_on_run", "id": "r", "inputs": {"DO": {"block": {
        "type": "controls_if", "id": "if", "inputs": {"IF0": {"block":
            sun(json!({"WHEN": "before", "EVENT": "sunrise"}), json!({"OFFSET": calc}))}}}}}}]},
        "variables": [{"name": "x", "id": "v"}]});
    let p = compile(&ws, &NoExternals).unwrap();
    let StmtKind::If { branches, .. } = &p.handlers[0].body[0].kind else { panic!() };
    let ExprKind::Condition { config, args } = &branches[0].0.kind else { panic!() };
    assert_eq!(config["before_offset"], json!({"$arg": "before_offset"}));
    assert!(args.contains_key("before_offset"));

    let both_after = sun(
        json!({"WHEN": "after", "EVENT": "sunset", "WHEN2": "after", "EVENT2": "sunrise"}),
        json!({"OFFSET": minutes(0), "SECOND": minutes(0)}),
    );
    assert!(compile_one(both_after).is_err());
}

#[test]
fn zone_and_template_conditions() {
    let zone = |z: &str| json!({"type": "zone_condition", "id": "z", "fields": {"ZONE": z}, "inputs": {"ENTITY": entity("person.a")}});
    assert_eq!(
        config(zone("zone.home")),
        json!({"condition": "zone", "entity_id": "person.a", "zone": "zone.home"})
    );
    assert_eq!(config(zone("zone.home, zone.work"))["zone"], json!(["zone.home", "zone.work"]));
    assert!(compile_one(zone("")).is_err());

    let t = json!({"type": "ha_template_condition", "id": "t", "fields": {"TEMPLATE": "{{ is_state('a.b', 'on') }}"}});
    assert_eq!(config(t), json!({"condition": "template", "value_template": "{{ is_state('a.b', 'on') }}"}));
}
