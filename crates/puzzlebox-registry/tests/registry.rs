use puzzlebox_ir::{ExprKind, HandlerKind, StmtKind, Trigger, compile};
use puzzlebox_registry::autogen::{HaDescriptions, generate};
use puzzlebox_registry::*;
use serde_json::{Value, json};

fn descriptions() -> (Value, Value, Value, serde_json::Map<String, Value>) {
    let services = json!({"light": {"turn_on": {
        "fields": {
            "brightness_pct": {"selector": {"number": {"min": 0, "max": 100, "unit_of_measurement": "%"}}},
            "flash": {"selector": {"select": {"options": ["long", "short"], "translation_key": "flash"}}},
            "advanced_fields": {"collapsed": true, "fields": {
                "rgb_color": {"selector": {"color_rgb": {}}}
            }}
        },
        "target": {"entity": [{"domain": ["light"]}]}
    }}, "weather": {"get_forecasts": {
        "fields": {"type": {"required": true, "selector": {"select": {"options": ["daily", "hourly"]}}}},
        "target": {"entity": [{"domain": "weather"}]},
        "response": {"optional": false}
    }}});
    let triggers = json!({"light.turned_on": {
        "fields": {"behavior": {"required": true, "default": "each", "selector": {"automation_behavior": {"mode": "trigger"}}},
                   "for": {"required": true, "default": "00:00:00", "selector": {"duration": {}}}},
        "target": {"entity": [{"domain": ["light"]}]}}});
    let conditions = json!({"light.is_on": {
        "fields": {"behavior": {"required": true, "default": "any", "selector": {"automation_behavior": {"mode": "condition"}}}},
        "target": {"entity": [{"domain": ["light"]}]}}});
    let tr = json!({
        "component.light.title": "Light",
        "component.light.services.turn_on.name": "Turn on",
        "component.light.services.turn_on.fields.brightness_pct.name": "Brightness",
        "component.light.selector.flash.options.long": "Long",
        "component.light.triggers.turned_on.name": "Light turned on",
    });
    (services, triggers, conditions, tr.as_object().unwrap().clone())
}

fn registry() -> Registry {
    let (s, t, c, tr) = descriptions();
    let r = Registry::default();
    r.set_source(
        "ha",
        generate(&HaDescriptions {
            services: &s,
            triggers: &t,
            conditions: &c,
            translations: &tr,
            config_entries: &Value::Null,
        }),
    );
    r
}

#[test]
fn autogen_maps_selectors_and_translations() {
    let all = registry().all();
    let turn_on = all.iter().find(|d| d.block_type == "ha_svc__light__turn_on").unwrap();
    assert_eq!(turn_on.title, "Turn on");
    assert_eq!(turn_on.category, ["Light"]);
    assert_eq!(turn_on.target.as_ref().unwrap().domains, ["light"]);
    let names: Vec<_> = turn_on.params.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"rgb_color"), "collapsible sections are flattened: {names:?}");
    let flash = turn_on.params.iter().find(|p| p.name == "flash").unwrap();
    let ParamInput::Select { options, .. } = &flash.input else { panic!() };
    assert_eq!(options[0].label, "Long");

    // Response-only services become value blocks only.
    assert!(
        all.iter().any(|d| d.block_type == "ha_svcr__weather__get_forecasts" && d.kind == BlockKind::Value)
    );
    assert!(!all.iter().any(|d| d.block_type == "ha_svc__weather__get_forecasts"));
}

fn blk(v: Value) -> Value {
    json!({"block": v})
}

#[test]
fn generated_blocks_compile_to_native_configs() {
    let r = registry();
    let lowerer = r.lowerer();
    let ws = json!({"blocks": {"blocks": [{
        "type": "ha_when", "id": "w",
        "inputs": {
            "TRIGGERS": blk(json!({"type": "ha_trg__light__turned_on", "id": "t",
                "fields": {"P_behavior": "first"},
                "inputs": {"TARGET": {"shadow": {"type": "ha_entity", "id": "te", "fields": {"ENTITY": "light.a,light.b"}}},
                           "P_for": {"shadow": {"type": "math_number", "id": "tf", "fields": {"NUM": 5}}}}})),
            "DO": blk(json!({"type": "controls_if", "id": "if",
                "inputs": {
                    "IF0": blk(json!({"type": "ha_cnd__light__is_on", "id": "c", "fields": {"P_behavior": "all"},
                        "inputs": {"TARGET": blk(json!({"type": "ha_entity", "id": "ce", "fields": {"ENTITY": "light.a"}}))}})),
                    "DO0": blk(json!({"type": "ha_svc__light__turn_on", "id": "s",
                        "fields": {"P_rgb_color": "#ff8000"},
                        "inputs": {"TARGET": blk(json!({"type": "ha_entity", "id": "se", "fields": {"ENTITY": "light.c"}})),
                                   "P_brightness_pct": {"shadow": {"type": "math_number", "id": "b", "fields": {"NUM": 40}}}},
                        "extraState": {"optional": ["brightness_pct", "rgb_color"]}}))
                }}))
        }
    }]}});
    let p = compile(&ws, lowerer.as_ref()).unwrap();
    let HandlerKind::Triggers { triggers } = &p.handlers[0].kind else { panic!() };
    let Trigger::Ha { config, .. } = &triggers[0] else { panic!() };
    assert_eq!(
        config,
        &json!({"trigger": "light.turned_on", "target": {"entity_id": ["light.a", "light.b"]},
        "options": {"behavior": "first", "for": 5}, "id": "t"})
    );

    let StmtKind::If { branches, .. } = &p.handlers[0].body[0].kind else { panic!() };
    let ExprKind::Condition { config, args } = &branches[0].0.kind else { panic!() };
    assert_eq!(config["condition"], "light.is_on");
    assert!(args.contains_key("target"));
    let StmtKind::CallService { service, data: Some(data), target: Some(_), .. } = &branches[0].1[0].kind
    else {
        panic!()
    };
    assert_eq!(service, "light.turn_on");
    let ExprKind::Dict { entries } = &data.kind else { panic!() };
    let rgb = entries.iter().find(|(k, _)| k == "rgb_color").unwrap();
    assert_eq!(rgb.1.kind, ExprKind::Lit { value: json!([255, 128, 0]) });
}

#[test]
fn trigger_params_must_be_constant() {
    let r = registry();
    let ws = json!({"blocks": {"blocks": [{"type": "ha_when", "id": "w", "inputs": {
        "TRIGGERS": blk(json!({"type": "ha_trg__light__turned_on", "id": "t",
            "inputs": {"TARGET": blk(json!({"type": "variables_get", "id": "v", "fields": {"VAR": {"id": "x"}}}))}}))}}]},
        "variables": [{"name": "x", "id": "x"}]});
    let errs = compile(&ws, r.lowerer().as_ref()).unwrap_err();
    assert!(errs.iter().any(|e| e.message.contains("fixed")), "{errs:?}");
}

#[test]
fn packs_validate() {
    let ok = r#"
pack: phone
name: My phone
blocks:
  - id: notify
    kind: action
    title: Notify phone
    params:
      - {name: message, label: Message, required: true, input: text}
    lowering:
      type: ha_action
      sequence:
        - action: notify.mobile_app_phone
          data: {message: {"$arg": message}}
  - id: low_battery
    kind: trigger
    title: Phone battery low
    lowering:
      type: ha_trigger
      config: {trigger: numeric_state, entity_id: sensor.phone_battery, below: 20}
"#;
    let pack = parse_pack(ok, "pack").unwrap();
    assert_eq!(pack.blocks[0].block_type, "pack__phone__notify");
    assert_eq!(pack.blocks[0].category, ["My phone"]);

    let bad_kind = ok.replace("kind: trigger", "kind: action");
    assert!(parse_pack(&bad_kind, "pack").unwrap_err().contains("does not fit"));
    let plugin = "pack: p\nname: P\nblocks:\n  - {id: x, kind: action, title: X, lowering: {type: plugin, plugin: p, function: f}}";
    assert!(parse_pack(plugin, "pack").is_err());
    assert!(parse_pack(plugin, "wasm").is_ok());
}

#[test]
fn example_pack_compiles() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/block-pack/comfort.yaml"
    ))
    .unwrap();
    let pack = parse_pack(&text, "pack").unwrap();
    let r = Registry::default();
    r.set_source(&pack.source, pack.blocks);
    let num = |id: &str, n: f64| json!({"shadow": {"type": "math_number", "id": id, "fields": {"NUM": n}}});
    let ws = json!({"blocks": {"blocks": [{"type": "ha_when", "id": "w", "inputs": {
        "TRIGGERS": blk(json!({"type": "pack__comfort__door_left_open", "id": "t",
            "inputs": {"P_door": {"shadow": {"type": "ha_entity", "id": "d", "fields": {"ENTITY": "binary_sensor.back"}}},
                       "P_minutes": num("m", 10.0)}})),
        "DO": blk(json!({"type": "controls_if", "id": "if", "inputs": {
            "IF0": blk(json!({"type": "pack__comfort__sun_down", "id": "c"})),
            "DO0": blk(json!({"type": "pack__comfort__set_room_temperature", "id": "a",
                "inputs": {"TARGET": blk(json!({"type": "ha_entity", "id": "e", "fields": {"ENTITY": "climate.lr"}})),
                           "P_temperature": num("tt", 19.5)}}))}}))}}]}});
    let p = compile(&ws, r.lowerer().as_ref()).unwrap();
    let HandlerKind::Triggers { triggers } = &p.handlers[0].kind else { panic!() };
    let Trigger::Ha { config, .. } = &triggers[0] else { panic!() };
    assert_eq!(
        config,
        &json!({"trigger": "state", "entity_id": "binary_sensor.back", "to": "on", "for": {"minutes": 10}, "id": "t"})
    );
    let StmtKind::If { branches, .. } = &p.handlers[0].body[0].kind else { panic!() };
    assert!(matches!(&branches[0].0.kind, ExprKind::Condition { .. }));
    let StmtKind::HaScript { sequence, args } = &branches[0].1[0].kind else { panic!() };
    assert_eq!(sequence[0]["data"]["temperature"], json!({"$arg": "temperature"}));
    assert!(args.contains_key("target") && args.contains_key("temperature"));
}

#[test]
fn thresholds_and_state_values_lower_to_ha_shapes() {
    let triggers = json!({
        "light.brightness_crossed_threshold": {
            "fields": {"threshold": {"required": true, "selector": {"numeric_threshold": {"mode": "crossed", "unit_of_measurement": ["%"]}}}},
            "target": {"entity": [{"domain": ["light"]}]}},
        "event.received": {
            "fields": {"event_type": {"required": true, "selector": {"state": {"attribute": "event_type", "multiple": true}}}},
            "target": {"entity": [{"domain": ["event"]}]}}
    });
    let r = Registry::default();
    r.set_source(
        "ha",
        generate(&HaDescriptions {
            services: &json!({}),
            triggers: &triggers,
            conditions: &json!({}),
            translations: &serde_json::Map::new(),
            config_entries: &Value::Null,
        }),
    );
    let ent = |id: &str, e: &str| json!({"shadow": {"type": "ha_entity", "id": id, "fields": {"ENTITY": e}}});
    let num = |id: &str, n: f64| json!({"shadow": {"type": "math_number", "id": id, "fields": {"NUM": n}}});
    let ws = json!({"blocks": {"blocks": [{"type": "ha_when", "id": "w", "inputs": {
        "TRIGGERS": blk(json!({"type": "ha_trg__light__brightness_crossed_threshold", "id": "t1",
            "fields": {"P_threshold": "between", "P_threshold__unit": "%"},
            "inputs": {"TARGET": ent("e1", "light.a"), "P_threshold__a": num("a", 20.0), "P_threshold__b": num("b", 80.0)},
            "next": {"block": {"type": "ha_trg__event__received", "id": "t2",
                "fields": {"P_event_type": "double_press, long_press"},
                "inputs": {"TARGET": ent("e2", "event.button")}}}}))}}]}});
    let p = compile(&ws, r.lowerer().as_ref()).unwrap();
    let HandlerKind::Triggers { triggers } = &p.handlers[0].kind else { panic!() };
    let Trigger::Ha { config, .. } = &triggers[0] else { panic!() };
    assert_eq!(
        config["options"]["threshold"],
        json!({"type": "between",
        "value_min": {"number": 20, "unit_of_measurement": "%"},
        "value_max": {"number": 80, "unit_of_measurement": "%"}})
    );
    let Trigger::Ha { config, .. } = &triggers[1] else { panic!() };
    assert_eq!(config["options"]["event_type"], json!(["double_press", "long_press"]));
}
