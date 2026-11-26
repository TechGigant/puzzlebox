use puzzlebox_ir::{HandlerKind, NoExternals, Trigger, compile};
use puzzlebox_yaml_import::import_yaml;
use serde_json::{Value, json};

struct Out {
    types: Vec<String>,
    /// Compiled HA configs, without the `id` the compiler adds.
    configs: Vec<Value>,
    raw: usize,
    blocks: Vec<Value>,
}

fn import(triggers: &str) -> Out {
    let list: Value = serde_yaml::from_str(triggers).unwrap();
    let text = serde_yaml::to_string(&json!({"triggers": list, "actions": []})).unwrap();
    import_triggers(&text)
}

fn import_triggers(text: &str) -> Out {
    let imp = import_yaml(text).unwrap().remove(0);
    let mut blocks = Vec::new();
    let mut cur = imp.workspace["blocks"]["blocks"][0]["inputs"]["TRIGGERS"].get("block").cloned();
    while let Some(b) = cur {
        cur = b.get("next").and_then(|n| n.get("block")).cloned();
        blocks.push(b);
    }
    let program = compile(&imp.workspace, &NoExternals).expect("imported triggers compile");
    let HandlerKind::Triggers { triggers } = &program.handlers[0].kind else { panic!() };
    let configs = triggers
        .iter()
        .map(|t| match t {
            Trigger::Ha { id, config } => {
                let mut c = config.clone();
                if c["id"] == json!(id) {
                    c.as_object_mut().unwrap().remove("id");
                }
                c
            }
            other => panic!("not an HA trigger: {other:?}"),
        })
        .collect();
    Out {
        types: blocks.iter().map(|b| b["type"].as_str().unwrap().to_owned()).collect(),
        configs,
        raw: imp.raw_blocks,
        blocks,
    }
}

/// Imports `yaml`, expecting these block types and compiled configs.
fn check(yaml: &str, types: &[&str], configs: Value) {
    let out = import(yaml);
    assert_eq!(out.types, types, "{yaml}");
    assert_eq!(Value::Array(out.configs), configs, "{yaml}");
}

/// Imports `yaml`, expecting one raw block that compiles to the original config.
fn check_raw(yaml: &str) {
    let out = import(yaml);
    assert_eq!(out.types, ["ha_trigger_raw"], "{yaml}");
    assert_eq!(out.raw, 1);
    let original: Value = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(Value::Array(out.configs), original, "{yaml}");
}

#[test]
fn state() {
    check(
        "- {trigger: state, entity_id: binary_sensor.door, to: 'on'}",
        &["ha_trigger_becomes"],
        json!([{"trigger": "state", "entity_id": "binary_sensor.door", "to": "on"}]),
    );
    // "HH:MM" is hours and minutes; lists of entities become a comma list, which HA splits.
    check(
        "- {trigger: state, entity_id: [light.a, light.b], from: 'off', to: 'on', for: '00:05'}",
        &["ha_trigger_state"],
        json!([{"trigger": "state", "entity_id": "light.a, light.b", "from": "off", "to": "on",
                "for": {"hours": 0, "minutes": 5, "seconds": 0}}]),
    );
    check(
        "- {platform: state, entity_id: light.a, to: ['on'], for: {minutes: 2, seconds: 30.5}}",
        &["ha_trigger_becomes"],
        json!([{"trigger": "state", "entity_id": "light.a", "to": "on", "for": {"seconds": 150.5}}]),
    );
    check(
        "- {trigger: state, entity_id: light.a, from: 'on', to: null}",
        &["ha_trigger_state"],
        json!([{"trigger": "state", "entity_id": "light.a", "from": "on"}]),
    );
    check(
        "- {trigger: state, entity_id: sensor.x, to: null, for: 90}",
        &["ha_trigger_state"],
        json!([{"trigger": "state", "entity_id": "sensor.x", "to": null,
                "for": {"hours": 0, "minutes": 1, "seconds": 30}}]),
    );
    check(
        "- {trigger: state, entity_id: sensor.x, from: null}",
        &["ha_trigger_state"],
        json!([{"trigger": "state", "entity_id": "sensor.x", "to": null}]),
    );
    // Durations use the current socket shape: a duration shadow in FOR.
    let out = import("- {trigger: state, entity_id: light.a, to: 'on', for: {minutes: 5}}");
    let dur = &out.blocks[0]["inputs"]["FOR"]["shadow"];
    assert_eq!(dur["type"], "duration");
    assert_eq!(dur["fields"]["UNIT"], "minutes");
    assert_eq!(dur["inputs"]["N"]["shadow"]["fields"]["NUM"], 5);

    for raw in [
        // Fires on attribute changes too.
        "- {trigger: state, entity_id: sensor.x}",
        "- {trigger: state, entity_id: sensor.x, for: 5}",
        "- {trigger: state, entity_id: light.a, to: ['on', 'off']}",
        "- {trigger: state, entity_id: light.a, not_to: [unavailable, unknown]}",
        "- {trigger: state, entity_id: light.a, not_from: unavailable, to: 'on'}",
        "- {trigger: state, entity_id: climate.x, attribute: hvac_action, to: heating}",
        "- {trigger: state, entity_id: light.a, to: 'on', for: \"{{ 5 }}\"}",
        "- {trigger: state, entity_id: light.a, to: 'on', for: {minutes: \"{{ 2 }}\"}}",
        // Sets trigger.for to zero instead of none.
        "- {trigger: state, entity_id: light.a, to: 'on', for: 0}",
        "- {trigger: state, entity_id: light.a, from: 'off', to: 'on', for: 1.5}",
        "- {trigger: state, entity_id: light.a, to: ''}",
        "- {trigger: state, entity_id: light.a, to: ' on'}",
        "- {trigger: state, entity_id: light.a, to: true}",
        "- {trigger: state, platform: state, entity_id: light.a, to: 'on'}",
    ] {
        check_raw(raw);
    }
}

#[test]
fn numeric_state() {
    check(
        "- {trigger: numeric_state, entity_id: sensor.t, above: 25}",
        &["ha_trigger_numeric"],
        json!([{"trigger": "numeric_state", "entity_id": "sensor.t", "above": 25}]),
    );
    check(
        "- {trigger: numeric_state, entity_id: [sensor.a, sensor.b], above: input_number.low, below: '30.5'}",
        &["ha_trigger_numeric"],
        json!([{"trigger": "numeric_state", "entity_id": "sensor.a, sensor.b", "above": "input_number.low", "below": 30.5}]),
    );
    for raw in [
        "- {trigger: numeric_state, entity_id: sensor.t, below: 10, for: '00:10:00'}",
        "- {trigger: numeric_state, entity_id: climate.x, attribute: current_temperature, above: 20}",
        "- {trigger: numeric_state, entity_id: sensor.x, value_template: \"{{ state.state | float }}\", above: 5}",
        "- {trigger: numeric_state, entity_id: sensor.x, above: \"{{ 5 }}\"}",
        "- {trigger: numeric_state, entity_id: sensor.x, above: lots}",
    ] {
        check_raw(raw);
    }
}

#[test]
fn time() {
    check(
        "- {trigger: time, at: '07:30'}",
        &["ha_trigger_time"],
        json!([{"trigger": "time", "at": "07:30"}]),
    );
    // A list fires like separate triggers, one per entry.
    check(
        "- {trigger: time, at: ['06:00', input_datetime.wake, sensor.next_alarm]}",
        &["ha_trigger_time", "ha_trigger_time", "ha_trigger_time"],
        json!([
            {"trigger": "time", "at": "06:00"},
            {"trigger": "time", "at": "input_datetime.wake"},
            {"trigger": "time", "at": "sensor.next_alarm"},
        ]),
    );
    let out = import("- {trigger: time, at: ['22:00:00', {entity_id: sensor.alarm, offset: '-00:10:00'}]}");
    assert_eq!(out.types, ["ha_trigger_time", "ha_trigger_raw"]);
    assert_eq!(
        out.configs,
        [
            json!({"trigger": "time", "at": "22:00:00"}),
            json!({"trigger": "time", "at": {"entity_id": "sensor.alarm", "offset": "-00:10:00"}}),
        ]
    );
    for raw in [
        "- {trigger: time, at: '08:00', weekday: [mon, tue]}",
        "- {trigger: time, at: \"{{ '09:00' }}\"}",
        "- {trigger: time, at: {entity_id: sensor.alarm, offset: '00:05'}}",
    ] {
        check_raw(raw);
    }
}

#[test]
fn time_pattern() {
    let every = |yaml: &str, n: u64, unit: &str, config: Value| {
        let out = import(yaml);
        assert_eq!(out.types, ["ha_trigger_every"], "{yaml}");
        assert_eq!(out.blocks[0]["fields"], json!({"N": n, "UNIT": unit}));
        assert_eq!(out.configs, [config], "{yaml}");
    };
    every(
        "- {trigger: time_pattern, minutes: '/5'}",
        5,
        "minutes",
        json!({"trigger": "time_pattern", "minutes": "/5"}),
    );
    every(
        "- {trigger: time_pattern, hours: '*', minutes: '/15', seconds: 0}",
        15,
        "minutes",
        json!({"trigger": "time_pattern", "minutes": "/15"}),
    );
    every(
        "- {trigger: time_pattern, hours: '/2'}",
        2,
        "hours",
        json!({"trigger": "time_pattern", "hours": "/2"}),
    );
    every(
        "- {trigger: time_pattern, hours: '/3', minutes: 0, seconds: '0'}",
        3,
        "hours",
        json!({"trigger": "time_pattern", "hours": "/3"}),
    );
    every(
        "- {trigger: time_pattern, seconds: '/10'}",
        10,
        "seconds",
        json!({"trigger": "time_pattern", "seconds": "/10"}),
    );
    every(
        "- {trigger: time_pattern, minutes: '*', seconds: '/30'}",
        30,
        "seconds",
        json!({"trigger": "time_pattern", "seconds": "/30"}),
    );
    for raw in [
        "- {trigger: time_pattern, minutes: 30}",
        "- {trigger: time_pattern, hours: 7, minutes: '/5'}",
        "- {trigger: time_pattern, minutes: '/5', seconds: 30}",
        "- {trigger: time_pattern, hours: 0}",
        "- {trigger: time_pattern, minutes: '/0'}",
        "- {trigger: time_pattern, minutes: '/75'}",
        "- {trigger: time_pattern, minutes: '*'}",
    ] {
        check_raw(raw);
    }
}

#[test]
fn sun_zone_event_homeassistant() {
    check(
        "- {trigger: sun, event: sunset, offset: '-00:30:00'}",
        &["ha_trigger_sun"],
        json!([{"trigger": "sun", "event": "sunset", "offset": "-00:30:00"}]),
    );
    check(
        "- {trigger: sun, event: sunrise, offset: {hours: 1}}",
        &["ha_trigger_sun"],
        json!([{"trigger": "sun", "event": "sunrise", "offset": "01:00:00"}]),
    );
    check(
        "- {trigger: sun, event: sunrise}",
        &["ha_trigger_sun"],
        json!([{"trigger": "sun", "event": "sunrise"}]),
    );
    check_raw("- {trigger: sun, event: sunrise, offset: 0.5}");
    check_raw("- {trigger: sun.dusk, options: {type: civil}}");

    check(
        "- {trigger: zone, entity_id: person.me, zone: zone.home, event: leave}",
        &["ha_trigger_zone"],
        json!([{"trigger": "zone", "entity_id": "person.me", "zone": "zone.home", "event": "leave"}]),
    );
    check(
        "- {trigger: zone, entity_id: [person.a, device_tracker.b], zone: zone.work}",
        &["ha_trigger_zone"],
        json!([{"trigger": "zone", "entity_id": "person.a, device_tracker.b", "zone": "zone.work", "event": "enter"}]),
    );

    check(
        "- {trigger: event, event_type: my_event}",
        &["ha_trigger_event"],
        json!([{"trigger": "event", "event_type": "my_event"}]),
    );
    let out = import("- {trigger: event, event_type: [a_event, \"{{ 'b' }}_event\"]}");
    assert_eq!(out.types, ["ha_trigger_event", "ha_trigger_raw"]);
    assert_eq!(out.configs[1], json!({"trigger": "event", "event_type": "{{ 'b' }}_event"}));
    check_raw("- {trigger: event, event_type: call_service, event_data: {domain: light}}");
    check_raw("- {trigger: event, event_type: x, context: {user_id: [abc]}}");

    check(
        "- {trigger: homeassistant, event: start}",
        &["ha_trigger_ha_start"],
        json!([{"trigger": "homeassistant", "event": "start"}]),
    );
    check_raw("- {trigger: homeassistant, event: shutdown}");
}

#[test]
fn device() {
    let yaml = "- {trigger: device, device_id: abc, domain: binary_sensor, entity_id: 5f3e, type: opened, for: {seconds: 5}}";
    let out = import(yaml);
    assert_eq!(out.types, ["device_trigger"]);
    assert_eq!(out.raw, 0);
    assert_eq!(out.blocks[0]["extraState"]["label"], "binary_sensor: opened");
    let original: Value = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(Value::Array(out.configs), original);
    let out = import(
        "- {platform: device, device_id: abc, domain: zha, type: remote_button_short_press, subtype: turn_on}",
    );
    assert_eq!(out.blocks[0]["extraState"]["label"], "zha: remote button short press turn on");
    assert_eq!(out.configs[0]["platform"], "device");
}

#[test]
fn triggers_without_blocks_stay_raw() {
    for raw in [
        "- {trigger: mqtt, topic: home/door, payload: open, value_template: \"{{ value_json.state }}\"}",
        "- {trigger: mqtt, topic: home/door}",
        "- {trigger: webhook, webhook_id: my-hook, allowed_methods: [POST, PUT], local_only: true}",
        "- {trigger: template, value_template: \"{{ is_state('light.a', 'on') }}\", for: '00:01:00'}",
        "- {trigger: geo_location, source: feed, zone: zone.alert, event: enter}",
        "- {trigger: calendar, event: start, entity_id: calendar.work, offset: '-0:15:0'}",
        "- {trigger: tag, tag_id: A7-6B, device_id: [abc]}",
        "- {trigger: conversation, command: ['turn on the [living room] lights', lights on]}",
        "- {trigger: persistent_notification, update_type: [added, removed], notification_id: x}",
        "- {trigger: light.turned_on, target: {area_id: kitchen}, options: {behavior: any}}",
        "- {trigger: some_custom.thing, foo: bar}",
    ] {
        check_raw(raw);
    }
}

#[test]
fn base_options() {
    // Ids stay in the raw YAML (native blocks report their block id as trigger.id).
    check_raw("- {trigger: state, entity_id: light.a, to: 'on', id: light_on}");
    check_raw("- {trigger: state, entity_id: light.a, to: 'on', alias: Light on}");
    check_raw("- {trigger: state, entity_id: light.a, to: 'on', variables: {room: kitchen}}");
    check_raw("- {trigger: event, event_type: x, enabled: \"{{ true }}\"}");

    // Disabled triggers become disabled blocks, which aren't subscribed.
    let out = import(
        "- {trigger: state, entity_id: light.a, to: 'off', enabled: false}\n- {trigger: homeassistant, event: start, enabled: 'on'}",
    );
    assert_eq!(out.types, ["ha_trigger_becomes", "ha_trigger_ha_start"]);
    assert_eq!(out.blocks[0]["disabledReasons"], json!(["MANUALLY_DISABLED"]));
    assert!(out.blocks[1].get("disabledReasons").is_none());
    assert_eq!(out.configs, [json!({"trigger": "homeassistant", "event": "start"})]);

    // `note` is dropped by HA.
    check(
        "- {trigger: state, entity_id: light.a, to: 'on', note: frontend only}",
        &["ha_trigger_becomes"],
        json!([{"trigger": "state", "entity_id": "light.a", "to": "on"}]),
    );
    // `triggers:` sub-lists are spliced in.
    check(
        "- triggers: [{trigger: homeassistant, event: start}, {trigger: event, event_type: nested}]",
        &["ha_trigger_ha_start", "ha_trigger_event"],
        json!([{"trigger": "homeassistant", "event": "start"}, {"trigger": "event", "event_type": "nested"}]),
    );
}

#[test]
fn every_trigger_form_compiles() {
    let text =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/triggers/all.yaml"))
            .unwrap();
    let out = import_triggers(&text);
    let native = out.types.iter().filter(|t| *t != "ha_trigger_raw").count();
    assert_eq!(out.types.len(), native + out.raw);
    assert_eq!((native, out.raw), (30, 33), "{:?}", out.types);
}
