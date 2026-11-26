#![recursion_limit = "512"]

use puzzlebox_ir::*;
use serde_json::json;

fn motion_light() -> serde_json::Value {
    json!({
      "blocks": {"languageVersion": 0, "blocks": [{
        "type": "ha_when", "id": "when1", "x": 10, "y": 10,
        "inputs": {
          "TRIGGERS": {"block": {"type": "ha_trigger_state", "id": "t1",
            "fields": {"ENTITY": "binary_sensor.motion", "FROM": "", "TO": "on", "FOR": 0}}},
          "DO": {"block": {"type": "ha_call_service", "id": "c1",
            "fields": {"SERVICE": "light.turn_on"},
            "inputs": {"TARGET": {"block": {"type": "ha_entity", "id": "e1", "fields": {"ENTITY": "light.hall"}}}},
            "next": {"block": {"type": "ha_wait", "id": "w1",
              "inputs": {"SECONDS": {"shadow": {"type": "math_number", "id": "n1", "fields": {"NUM": 300}}}},
              "next": {"block": {"type": "variables_set", "id": "s1",
                "fields": {"VAR": {"id": "v1"}},
                "inputs": {"VALUE": {"block": {"type": "math_arithmetic", "id": "a1", "fields": {"OP": "ADD"},
                  "inputs": {
                    "A": {"block": {"type": "variables_get", "id": "g1", "fields": {"VAR": {"id": "v1"}}}},
                    "B": {"shadow": {"type": "math_number", "id": "n2", "fields": {"NUM": 1}}}}}}}}}}}}}
        }
      }]},
      "variables": [{"name": "count", "id": "v1", "type": "Program"}]
    })
}

#[test]
fn compiles_motion_light() {
    let p = compile(&motion_light(), &NoExternals).unwrap();
    assert_eq!(p.persistent, vec!["count"]);
    let h = &p.handlers[0];
    let HandlerKind::Triggers { triggers } = &h.kind else { panic!() };
    assert_eq!(
        triggers[0],
        Trigger::Ha {
            id: "t1".into(),
            config: json!({"trigger": "state", "entity_id": "binary_sensor.motion", "to": "on", "id": "t1"})
        }
    );
    assert_eq!(h.body.len(), 3);
    assert!(
        matches!(&h.body[0].kind, StmtKind::CallService { service, target: Some(_), .. } if service == "light.turn_on")
    );
    assert!(matches!(&h.body[1].kind, StmtKind::Wait { .. }));
    let StmtKind::Set { name, scope, .. } = &h.body[2].kind else { panic!() };
    assert_eq!((name.as_str(), *scope), ("count", VarScope::Program));
}

#[test]
fn reports_errors_with_block_ids() {
    let ws = json!({"blocks": {"blocks": [{
        "type": "ha_when", "id": "w",
        "inputs": {"DO": {"block": {"type": "ha_call_service", "id": "c", "fields": {"SERVICE": "nodot"}}}}
    }]}});
    let errs = compile(&ws, &NoExternals).unwrap_err();
    let ids: Vec<_> = errs.iter().map(|e| e.block_id.as_deref().unwrap()).collect();
    assert_eq!(ids, vec!["w", "c"], "{errs:?}");
}

#[test]
fn skips_disabled_blocks_and_compiles_if_else_and_functions() {
    let ws = json!({"blocks": {"blocks": [
      {"type": "ha_on_start", "id": "s",
       "inputs": {"DO": {"block": {"type": "controls_if", "id": "if", "extraState": {"elseIfCount": 1, "hasElse": true},
         "inputs": {
           "IF0": {"block": {"type": "logic_boolean", "id": "b0", "fields": {"BOOL": "TRUE"}}},
           "DO0": {"block": {"type": "procedures_callnoreturn", "id": "call", "extraState": {"name": "greet", "params": []}}},
           "IF1": {"block": {"type": "logic_boolean", "id": "b1", "fields": {"BOOL": "FALSE"}}},
           "ELSE": {"block": {"type": "ha_call_service", "id": "dis", "enabled": false, "fields": {"SERVICE": "x"}}}
         }}}}},
      {"type": "procedures_defnoreturn", "id": "f", "fields": {"NAME": "greet"}, "extraState": {"params": []}}
    ]}});
    let p = compile(&ws, &NoExternals).unwrap();
    let StmtKind::If { branches, otherwise } = &p.handlers[0].body[0].kind else { panic!() };
    assert_eq!(branches.len(), 2);
    assert_eq!(otherwise.as_deref(), Some(&[][..]));
    assert!(p.functions.contains_key("greet"));
}

#[test]
fn unknown_function_is_an_error() {
    let ws = json!({"blocks": {"blocks": [{"type": "ha_on_start", "id": "s",
       "inputs": {"DO": {"block": {"type": "procedures_callnoreturn", "id": "call", "extraState": {"name": "nope", "params": []}}}}}]}});
    let errs = compile(&ws, &NoExternals).unwrap_err();
    assert_eq!(errs[0].block_id.as_deref(), Some("call"));
}

#[test]
fn raw_yaml_trigger() {
    let ws = json!({"blocks": {"blocks": [{"type": "ha_when", "id": "w",
       "inputs": {"TRIGGERS": {"block": {"type": "ha_trigger_raw", "id": "r",
         "fields": {"CONFIG": "trigger: zone\nentity_id: person.me\nzone: zone.home\nevent: enter"}}}}}]}});
    let p = compile(&ws, &NoExternals).unwrap();
    let HandlerKind::Triggers { triggers } = &p.handlers[0].kind else { panic!() };
    assert_eq!(
        triggers[0],
        Trigger::Ha {
            id: "r".into(),
            config: json!({"trigger": "zone", "entity_id": "person.me", "zone": "zone.home", "event": "enter", "id": "r"})
        }
    );
}

#[test]
fn numeric_trigger_accepts_numbers_and_entities() {
    let ws = json!({"blocks": {"blocks": [{"type": "ha_when", "id": "w", "inputs": {"TRIGGERS": {"block": {
        "type": "ha_trigger_numeric", "id": "n",
        "fields": {"ENTITY": "sensor.t", "ABOVE": "input_number.limit", "BELOW": "30"}}}}}]}});
    let p = compile(&ws, &NoExternals).unwrap();
    let HandlerKind::Triggers { triggers } = &p.handlers[0].kind else { panic!() };
    assert_eq!(
        triggers[0],
        Trigger::Ha {
            id: "n".into(),
            config: json!({
        "trigger": "numeric_state", "entity_id": "sensor.t", "above": "input_number.limit", "below": 30, "id": "n"})
        }
    );
    let bad = json!({"blocks": {"blocks": [{"type": "ha_when", "id": "w", "inputs": {"TRIGGERS": {"block": {
        "type": "ha_trigger_numeric", "id": "n", "fields": {"ENTITY": "sensor.t", "ABOVE": "warm"}}}}}]}});
    assert!(compile(&bad, &NoExternals).is_err());
}

#[test]
fn every_trigger_and_area_targets() {
    let ws = json!({"blocks": {"languageVersion": 0, "blocks": [{
        "type": "ha_when", "id": "w",
        "inputs": {
            "TRIGGERS": {"block": {"type": "ha_trigger_every", "id": "e", "fields": {"N": 10, "UNIT": "minutes"}}},
            "DO": {"block": {"type": "ha_call_service", "id": "c", "fields": {"SERVICE": "light.turn_off"},
                "inputs": {"TARGET": {"block": {"type": "ha_area", "id": "a", "fields": {"ID": "kitchen, hall"}}}}}}
        }
    }]}});
    let p = compile(&ws, &NoExternals).unwrap();
    let HandlerKind::Triggers { triggers } = &p.handlers[0].kind else { panic!() };
    let Trigger::Ha { config, .. } = &triggers[0] else { panic!() };
    assert_eq!(config["minutes"], "/10");
    assert_eq!(config["trigger"], "time_pattern");
    let StmtKind::CallService { target: Some(t), .. } = &p.handlers[0].body[0].kind else { panic!() };
    assert_eq!(t.kind, ExprKind::Lit { value: json!({"area_id": ["kitchen", "hall"]}) });
}

#[test]
fn trigger_durations_are_sockets_and_every_is_checked() {
    let ws = |trigger: serde_json::Value| {
        json!({"blocks": {"languageVersion": 0, "blocks": [{
            "type": "ha_when", "id": "w", "inputs": {"TRIGGERS": {"block": trigger}}
        }]}})
    };
    let state = json!({"type": "ha_trigger_state", "id": "s", "fields": {"ENTITY": "light.a", "FROM": "", "TO": ""},
        "inputs": {"FOR": {"shadow": {"type": "duration", "id": "d", "fields": {"UNIT": "minutes"},
            "inputs": {"N": {"shadow": {"type": "math_number", "id": "n", "fields": {"NUM": 5}}}}}}}});
    let p = compile(&ws(state), &NoExternals).unwrap();
    let HandlerKind::Triggers { triggers } = &p.handlers[0].kind else { panic!() };
    let Trigger::Ha { config, .. } = &triggers[0] else { panic!() };
    assert_eq!(config["for"], json!({"hours": 0, "minutes": 5, "seconds": 0}));
    assert_eq!(config["to"], serde_json::Value::Null, "only state changes, not attribute changes");
    assert!(config.as_object().unwrap().contains_key("to"));

    let every = json!({"type": "ha_trigger_every", "id": "e", "fields": {"N": 7, "UNIT": "minutes"}});
    let errors = compile(&ws(every), &NoExternals).unwrap_err();
    assert!(errors[0].message.contains("doesn't fit evenly"), "{}", errors[0].message);
}
