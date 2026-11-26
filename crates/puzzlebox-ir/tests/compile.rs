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

    let every =
        |n: u32, unit: &str| json!({"type": "ha_trigger_every", "id": "e", "fields": {"N": n, "UNIT": unit}});
    let config = |t: serde_json::Value| {
        let p = compile(&ws(t), &NoExternals).unwrap();
        let HandlerKind::Triggers { triggers } = &p.handlers[0].kind else { panic!() };
        let Trigger::Ha { config, .. } = &triggers[0] else { panic!() };
        config.clone()
    };
    assert_eq!(config(every(60, "minutes"))["hours"], "/1");
    assert_eq!(config(every(24, "hours"))["at"], "00:00:00");
    assert_eq!(config(every(7, "minutes"))["minutes"], "/7");
    let errors = compile(&ws(every(90, "minutes")), &NoExternals).unwrap_err();
    assert!(errors[0].message.contains("can't repeat"), "{}", errors[0].message);
}

/// The HA config a single trigger block compiles to, without the `id` the compiler adds.
fn trigger_config(trigger: serde_json::Value) -> Result<serde_json::Value, Vec<CompileError>> {
    let ws = json!({"blocks": {"blocks": [{"type": "ha_when", "id": "w", "inputs": {"TRIGGERS": {"block": trigger}}}]}});
    let p = compile(&ws, &NoExternals)?;
    let HandlerKind::Triggers { triggers } = &p.handlers[0].kind else { panic!() };
    let Trigger::Ha { config, .. } = &triggers[0] else { panic!("not an HA trigger") };
    let mut config = config.clone();
    config.as_object_mut().unwrap().remove("id");
    Ok(config)
}

fn dur(n: f64, unit: &str) -> serde_json::Value {
    json!({"shadow": {"type": "duration", "fields": {"UNIT": unit},
        "inputs": {"N": {"shadow": {"type": "math_number", "fields": {"NUM": n}}}}}})
}

fn tb(ty: &str, fields: serde_json::Value) -> serde_json::Value {
    json!({"type": ty, "id": "t", "fields": fields})
}

#[test]
fn state_trigger_options() {
    let state = |fields: serde_json::Value| trigger_config(tb("ha_trigger_state", fields)).unwrap();
    // Old shape: unchanged.
    assert_eq!(
        state(json!({"ENTITY": "light.a", "FROM": "off", "TO": "on"})),
        json!({"trigger": "state", "entity_id": "light.a", "from": "off", "to": "on"})
    );
    assert_eq!(
        state(json!({"ENTITY": "light.a", "FROM": "", "TO": ""})),
        json!({"trigger": "state", "entity_id": "light.a", "to": null})
    );
    // Several states become a list.
    assert_eq!(
        state(json!({"ENTITY": "cover.a", "FROM": "", "TO": "open, opening"})),
        json!({"trigger": "state", "entity_id": "cover.a", "to": ["open", "opening"]})
    );
    assert_eq!(
        state(
            json!({"ENTITY": "light.a", "FROM_OP": "not", "FROM": "unavailable,unknown", "TO_OP": "not", "TO": "unavailable"})
        ),
        json!({"trigger": "state", "entity_id": "light.a", "not_from": ["unavailable", "unknown"], "not_to": "unavailable"})
    );
    // Any change, including details.
    assert_eq!(
        state(json!({"ENTITY": "sensor.x", "WHAT": "any", "FROM": "a", "TO": "b"})),
        json!({"trigger": "state", "entity_id": "sensor.x"})
    );
    // A detail: values keep their type.
    assert_eq!(
        state(
            json!({"ENTITY": "light.a", "WHAT": "attribute", "ATTRIBUTE": "brightness", "FROM": "", "TO": "255"})
        ),
        json!({"trigger": "state", "entity_id": "light.a", "attribute": "brightness", "to": 255})
    );
    assert_eq!(
        state(
            json!({"ENTITY": "climate.a", "WHAT": "attribute", "ATTRIBUTE": "hvac_action", "TO": "heating, true"})
        ),
        json!({"trigger": "state", "entity_id": "climate.a", "attribute": "hvac_action", "to": ["heating", true]})
    );
    assert_eq!(
        state(json!({"ENTITY": "light.a", "WHAT": "attribute", "ATTRIBUTE": "brightness", "TO": ""})),
        json!({"trigger": "state", "entity_id": "light.a", "attribute": "brightness", "to": null})
    );
    assert!(
        trigger_config(tb("ha_trigger_state", json!({"ENTITY": "light.a", "WHAT": "attribute"}))).is_err()
    );

    // "becomes" takes several states too.
    assert_eq!(
        trigger_config(tb("ha_trigger_becomes", json!({"ENTITY": "cover.a", "STATE": "open,closed"})))
            .unwrap(),
        json!({"trigger": "state", "entity_id": "cover.a", "to": ["open", "closed"]})
    );
}

#[test]
fn numeric_trigger_options() {
    let mut t = tb(
        "ha_trigger_numeric",
        json!({"ENTITY": "climate.a", "WHAT": "attribute", "ATTRIBUTE": "current_temperature", "ABOVE": "20", "BELOW": ""}),
    );
    t["inputs"] = json!({"FOR": dur(10.0, "minutes")});
    assert_eq!(
        trigger_config(t).unwrap(),
        json!({"trigger": "numeric_state", "entity_id": "climate.a", "attribute": "current_temperature", "above": 20,
               "for": {"hours": 0, "minutes": 10, "seconds": 0}})
    );
    let t = tb(
        "ha_trigger_numeric",
        json!({"ENTITY": "sensor.a", "WHAT": "template", "VALUE_TEMPLATE": "{{ state.state | float * 2 }}", "ATTRIBUTE": "x", "BELOW": "5"}),
    );
    assert_eq!(
        trigger_config(t).unwrap(),
        json!({"trigger": "numeric_state", "entity_id": "sensor.a", "value_template": "{{ state.state | float * 2 }}", "below": 5})
    );
}

#[test]
fn time_trigger_options() {
    let days = |on: &[&str]| {
        let mut f = json!({"AT": "08:00"});
        for d in ["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"] {
            f[d] = json!(if on.contains(&d) { "TRUE" } else { "FALSE" });
        }
        tb("ha_trigger_time", f)
    };
    assert_eq!(
        trigger_config(tb("ha_trigger_time", json!({"AT": "07:00"}))).unwrap(),
        json!({"trigger": "time", "at": "07:00"})
    );
    assert_eq!(
        trigger_config(days(&["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"])).unwrap(),
        json!({"trigger": "time", "at": "08:00"})
    );
    assert_eq!(
        trigger_config(days(&["SAT", "SUN"])).unwrap(),
        json!({"trigger": "time", "at": "08:00", "weekday": ["sat", "sun"]})
    );
    assert_eq!(
        trigger_config(days(&["MON"])).unwrap(),
        json!({"trigger": "time", "at": "08:00", "weekday": "mon"})
    );
    assert!(trigger_config(days(&[])).is_err());

    let mut t = tb("ha_trigger_time", json!({"AT": "sensor.alarm"}));
    t["inputs"] = json!({"OFFSET": dur(-10.0, "minutes")});
    assert_eq!(
        trigger_config(t).unwrap(),
        json!({"trigger": "time", "at": {"entity_id": "sensor.alarm", "offset": "-00:10:00"}})
    );
    let mut t = tb("ha_trigger_time", json!({"AT": "07:00"}));
    t["inputs"] = json!({"OFFSET": dur(5.0, "minutes")});
    assert!(trigger_config(t).is_err(), "a fixed time can't be shifted");
}

#[test]
fn time_pattern_trigger() {
    let pattern = |h: (&str, u32), m: (&str, u32), s: (&str, u32)| {
        trigger_config(tb(
            "ha_trigger_time_pattern",
            json!({"HOURS_MODE": h.0, "HOURS": h.1, "MINUTES_MODE": m.0, "MINUTES": m.1, "SECONDS_MODE": s.0, "SECONDS": s.1}),
        ))
    };
    assert_eq!(
        pattern(("any", 0), ("every", 15), ("at", 0)).unwrap(),
        json!({"trigger": "time_pattern", "minutes": "/15"})
    );
    assert_eq!(
        pattern(("at", 7), ("at", 30), ("at", 0)).unwrap(),
        json!({"trigger": "time_pattern", "hours": 7, "minutes": 30})
    );
    // "any" minute under a set hour must stay, or HA would use minute 0.
    assert_eq!(
        pattern(("at", 7), ("any", 0), ("at", 0)).unwrap(),
        json!({"trigger": "time_pattern", "hours": 7, "minutes": "*"})
    );
    assert_eq!(
        pattern(("every", 2), ("at", 0), ("at", 30)).unwrap(),
        json!({"trigger": "time_pattern", "hours": "/2", "minutes": 0, "seconds": 30})
    );
    assert_eq!(
        pattern(("any", 0), ("any", 0), ("any", 0)).unwrap(),
        json!({"trigger": "time_pattern", "seconds": "*"})
    );
    assert!(pattern(("every", 0), ("at", 0), ("at", 0)).is_err());
    assert!(pattern(("at", 24), ("at", 0), ("at", 0)).is_err());
}

#[test]
fn more_ha_triggers() {
    let cfg = |t: serde_json::Value| trigger_config(t).unwrap();
    assert_eq!(
        cfg(tb("ha_trigger_ha_start", json!({}))),
        json!({"trigger": "homeassistant", "event": "start"})
    );
    assert_eq!(
        cfg(tb("ha_trigger_ha_start", json!({"EVENT": "shutdown"}))),
        json!({"trigger": "homeassistant", "event": "shutdown"})
    );

    let mut t = tb("ha_trigger_template", json!({"VALUE_TEMPLATE": "{{ is_state('light.a', 'on') }}"}));
    t["inputs"] = json!({"FOR": dur(90.0, "seconds")});
    assert_eq!(
        cfg(t),
        json!({"trigger": "template", "value_template": "{{ is_state('light.a', 'on') }}",
               "for": {"hours": 0, "minutes": 1, "seconds": 30}})
    );

    assert_eq!(
        cfg(tb(
            "ha_trigger_mqtt",
            json!({"TOPIC": "home/door", "PAYLOAD": "", "VALUE_TEMPLATE": "", "QOS": "0"})
        )),
        json!({"trigger": "mqtt", "topic": "home/door"})
    );
    assert_eq!(
        cfg(tb(
            "ha_trigger_mqtt",
            json!({"TOPIC": "home/door", "PAYLOAD": "open", "VALUE_TEMPLATE": "{{ value_json.state }}", "QOS": "1"})
        )),
        json!({"trigger": "mqtt", "topic": "home/door", "payload": "open", "value_template": "{{ value_json.state }}", "qos": 1})
    );

    assert_eq!(
        cfg(tb("ha_trigger_webhook", json!({"WEBHOOK_ID": "my-hook"}))),
        json!({"trigger": "webhook", "webhook_id": "my-hook", "allowed_methods": ["POST", "PUT"], "local_only": true})
    );
    assert_eq!(
        cfg(tb(
            "ha_trigger_webhook",
            json!({"WEBHOOK_ID": "h", "GET": "TRUE", "HEAD": "FALSE", "POST": "FALSE", "PUT": "FALSE", "LOCAL_ONLY": "FALSE"})
        )),
        json!({"trigger": "webhook", "webhook_id": "h", "allowed_methods": ["GET"], "local_only": false})
    );
    assert!(
        trigger_config(tb("ha_trigger_webhook", json!({"WEBHOOK_ID": "h", "POST": "FALSE", "PUT": "FALSE"})))
            .is_err()
    );

    let mut t = tb("ha_trigger_calendar", json!({"ENTITY": "calendar.work", "EVENT": "start"}));
    t["inputs"] = json!({"OFFSET": dur(-15.0, "minutes")});
    assert_eq!(
        cfg(t),
        json!({"trigger": "calendar", "event": "start", "entity_id": "calendar.work", "offset": "-00:15:00"})
    );
    assert_eq!(
        cfg(tb("ha_trigger_calendar", json!({"ENTITY": "calendar.work", "EVENT": "end"}))),
        json!({"trigger": "calendar", "event": "end", "entity_id": "calendar.work"})
    );

    assert_eq!(
        cfg(tb("ha_trigger_tag", json!({"TAG_ID": "A7-6B", "DEVICE": ""}))),
        json!({"trigger": "tag", "tag_id": "A7-6B"})
    );
    assert_eq!(
        cfg(tb("ha_trigger_tag", json!({"TAG_ID": "a, b", "DEVICE": "dev1"}))),
        json!({"trigger": "tag", "tag_id": ["a", "b"], "device_id": "dev1"})
    );
    assert!(trigger_config(tb("ha_trigger_tag", json!({"TAG_ID": " "}))).is_err());

    assert_eq!(
        cfg(tb("ha_trigger_conversation", json!({"COMMAND": "good night\n\n  [turn on] night mode \n"}))),
        json!({"trigger": "conversation", "command": ["good night", "[turn on] night mode"]})
    );
    assert!(trigger_config(tb("ha_trigger_conversation", json!({"COMMAND": ""}))).is_err());
}

#[test]
fn action_responses_and_loop_info() {
    let ask = |id: &str, var: &str| {
        json!({"type": "ha_call_service_response", "id": id,
            "fields": {"SERVICE": "weather.get_forecasts", "RESPONSE": {"id": var}}})
    };
    let mut first = ask("a", "r");
    first["next"] = json!({"block": ask("b", "p")});
    let info = json!({"type": "loop_info", "id": "li", "fields": {"WHAT": "item"}});
    first["next"]["block"]["next"] = json!({"block": {"type": "controls_repeat_ext", "id": "rep",
        "inputs": {"TIMES": {"block": {"type": "math_number", "id": "n", "fields": {"NUM": 2}}},
            "DO": {"block": {"type": "variables_set", "id": "s", "fields": {"VAR": {"id": "r"}},
                "inputs": {"VALUE": {"block": info}}}}}}});
    let ws = json!({"blocks": {"blocks": [{"type": "ha_on_run", "id": "run", "inputs": {"DO": {"block": first}}}]},
        "variables": [{"name": "r", "id": "r"}, {"name": "p", "id": "p", "type": "Program"}]});
    let p = compile(&ws, &NoExternals).unwrap();
    let body = &p.handlers[0].body;
    assert!(matches!(&body[0].kind,
        StmtKind::CallService { service, response_var: Some(v), .. } if service == "weather.get_forecasts" && v == "r"));
    // A program variable gets the response copied in.
    let StmtKind::If { branches, .. } = &body[1].kind else { panic!("{:?}", body[1]) };
    let StmtKind::CallService { response_var: Some(tmp), .. } = &branches[0].1[0].kind else { panic!() };
    let StmtKind::Set { name, scope, value } = &branches[0].1[1].kind else { panic!() };
    assert_eq!((name.as_str(), *scope), ("p", VarScope::Program));
    assert_eq!(value.kind, ExprKind::Var { name: tmp.clone(), scope: VarScope::Local });
    let StmtKind::Repeat { body: inner, .. } = &body[2].kind else { panic!() };
    let StmtKind::Set { value, .. } = &inner[0].kind else { panic!() };
    assert_eq!(
        value.kind,
        ExprKind::Builtin { name: "loop_info".into(), args: vec![Expr::lit("li", json!("item"))] }
    );

    // Outside a loop (also in a timer started inside one), it is an error.
    let set = json!({"type": "variables_set", "id": "s", "fields": {"VAR": {"id": "r"}},
        "inputs": {"VALUE": {"block": {"type": "loop_info", "id": "li", "fields": {"WHAT": "index"}}}}});
    let timer = json!({"type": "timer_start", "id": "tm", "fields": {"NAME": "t"}, "inputs": {
        "SECONDS": {"block": {"type": "math_number", "id": "n2", "fields": {"NUM": 2}}}, "DO": {"block": set}}});
    let rep = json!({"type": "controls_repeat_ext", "id": "rep", "inputs": {
        "TIMES": {"block": {"type": "math_number", "id": "n", "fields": {"NUM": 2}}}, "DO": {"block": timer}}});
    let ws = json!({"blocks": {"blocks": [{"type": "ha_on_run", "id": "run", "inputs": {"DO": {"block": rep}}}]},
        "variables": [{"name": "r", "id": "r"}]});
    let errs = compile(&ws, &NoExternals).unwrap_err();
    assert_eq!(errs[0].block_id.as_deref(), Some("li"), "{errs:?}");
}

#[test]
fn waits_keep_their_trigger_configs_as_written() {
    let ws = json!({"blocks": {"blocks": [{"type": "ha_on_run", "id": "run", "inputs": {"DO": {"block":
        {"type": "ha_wait_for_trigger", "id": "w", "inputs": {"TRIGGERS": {"block":
            {"type": "ha_trigger_raw", "id": "t1", "fields": {"CONFIG": "trigger: event\nevent_type: a"},
             "next": {"block": {"type": "ha_trigger_raw", "id": "t2", "fields": {"CONFIG": "trigger: event\nevent_type: b"}}}}}}}}}}]}});
    let p = compile(&ws, &NoExternals).unwrap();
    let StmtKind::WaitForTrigger { triggers, .. } = &p.handlers[0].body[0].kind else { panic!() };
    // No `id`, so HA numbers them like a script's `wait_for_trigger` ("0", "1").
    assert_eq!(
        triggers,
        &vec![json!({"trigger": "event", "event_type": "a"}), json!({"trigger": "event", "event_type": "b"})]
    );
}
