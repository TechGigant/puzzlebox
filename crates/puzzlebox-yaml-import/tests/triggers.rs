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

    // Without from/to it fires on attribute changes too: "state or any detail".
    check(
        "- {trigger: state, entity_id: sensor.x}",
        &["ha_trigger_state"],
        json!([{"trigger": "state", "entity_id": "sensor.x"}]),
    );
    assert_eq!(import("- {trigger: state, entity_id: sensor.x}").blocks[0]["fields"]["WHAT"], "any");
    check(
        "- {trigger: state, entity_id: sensor.x, for: 5}",
        &["ha_trigger_state"],
        json!([{"trigger": "state", "entity_id": "sensor.x", "for": {"hours": 0, "minutes": 0, "seconds": 5}}]),
    );
    // Several states and "not".
    check(
        "- {trigger: state, entity_id: light.a, to: ['on', 'off']}",
        &["ha_trigger_becomes"],
        json!([{"trigger": "state", "entity_id": "light.a", "to": ["on", "off"]}]),
    );
    check(
        "- {trigger: state, entity_id: light.a, not_to: [unavailable, unknown]}",
        &["ha_trigger_state"],
        json!([{"trigger": "state", "entity_id": "light.a", "not_to": ["unavailable", "unknown"]}]),
    );
    check(
        "- {trigger: state, entity_id: light.a, not_from: unavailable, to: 'on'}",
        &["ha_trigger_state"],
        json!([{"trigger": "state", "entity_id": "light.a", "not_from": "unavailable", "to": "on"}]),
    );
    // One detail; its values keep their type.
    check(
        "- {trigger: state, entity_id: climate.x, attribute: hvac_action, to: heating}",
        &["ha_trigger_state"],
        json!([{"trigger": "state", "entity_id": "climate.x", "attribute": "hvac_action", "to": "heating"}]),
    );
    check(
        "- {trigger: state, entity_id: light.a, attribute: brightness, from: [0, 1.5], not_to: 255}",
        &["ha_trigger_state"],
        json!([{"trigger": "state", "entity_id": "light.a", "attribute": "brightness", "from": [0, 1.5], "not_to": 255}]),
    );
    // Only that detail's changes, as with `to: null`.
    check(
        "- {trigger: state, entity_id: light.a, attribute: brightness}",
        &["ha_trigger_state"],
        json!([{"trigger": "state", "entity_id": "light.a", "attribute": "brightness", "to": null}]),
    );

    for raw in [
        "- {trigger: state, entity_id: light.a, from: 'on', not_from: 'off'}",
        "- {trigger: state, entity_id: light.a, to: 'on,off'}",
        "- {trigger: state, entity_id: light.a, to: []}",
        "- {trigger: state, entity_id: light.a, to: 5}",
        // The compiler would send these as numbers or booleans.
        "- {trigger: state, entity_id: light.a, attribute: brightness, to: '255'}",
        "- {trigger: state, entity_id: light.a, attribute: on_off, to: 'true'}",
        "- {trigger: state, entity_id: light.a, attribute: \"{{ 'x' }}\", to: 'a'}",
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
    check(
        "- {trigger: numeric_state, entity_id: sensor.t, below: 10, for: '00:10:00'}",
        &["ha_trigger_numeric"],
        json!([{"trigger": "numeric_state", "entity_id": "sensor.t", "below": 10,
                "for": {"hours": 0, "minutes": 10, "seconds": 0}}]),
    );
    check(
        "- {trigger: numeric_state, entity_id: climate.x, attribute: current_temperature, above: 20}",
        &["ha_trigger_numeric"],
        json!([{"trigger": "numeric_state", "entity_id": "climate.x", "attribute": "current_temperature", "above": 20}]),
    );
    check(
        "- {trigger: numeric_state, entity_id: sensor.x, value_template: \"{{ state.state | float }}\", above: 5}",
        &["ha_trigger_numeric"],
        json!([{"trigger": "numeric_state", "entity_id": "sensor.x", "value_template": "{{ state.state | float }}", "above": 5}]),
    );
    for raw in [
        "- {trigger: numeric_state, entity_id: sensor.x, attribute: a, value_template: \"{{ 1 }}\", above: 5}",
        "- {trigger: numeric_state, entity_id: sensor.t, below: 10, for: 0}",
        "- {trigger: numeric_state, entity_id: sensor.t, below: 10, for: 1.5}",
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
    check(
        "- {trigger: time, at: ['22:00:00', {entity_id: sensor.alarm, offset: '-00:10:00'}]}",
        &["ha_trigger_time", "ha_trigger_time"],
        json!([
            {"trigger": "time", "at": "22:00:00"},
            {"trigger": "time", "at": {"entity_id": "sensor.alarm", "offset": "-00:10:00"}},
        ]),
    );
    // "HH:MM" is hours and minutes.
    check(
        "- {trigger: time, at: {entity_id: input_datetime.wake, offset: '00:05'}}",
        &["ha_trigger_time"],
        json!([{"trigger": "time", "at": {"entity_id": "input_datetime.wake", "offset": "00:05:00"}}]),
    );
    check(
        "- {trigger: time, at: {entity_id: sensor.alarm}}",
        &["ha_trigger_time"],
        json!([{"trigger": "time", "at": "sensor.alarm"}]),
    );
    check(
        "- {trigger: time, at: '08:00', weekday: [mon, tue]}",
        &["ha_trigger_time"],
        json!([{"trigger": "time", "at": "08:00", "weekday": ["mon", "tue"]}]),
    );
    check(
        "- {trigger: time, at: '08:00', weekday: sat}",
        &["ha_trigger_time"],
        json!([{"trigger": "time", "at": "08:00", "weekday": "sat"}]),
    );
    // Every day is the same as no days.
    check(
        "- {trigger: time, at: '08:00', weekday: [mon, tue, wed, thu, fri, sat, sun]}",
        &["ha_trigger_time"],
        json!([{"trigger": "time", "at": "08:00"}]),
    );
    // The days go with each entry of a split list, raw ones too.
    let out = import("- {trigger: time, at: ['06:00', \"{{ '09:00' }}\"], weekday: [sun]}");
    assert_eq!(out.types, ["ha_trigger_time", "ha_trigger_raw"]);
    assert_eq!(out.configs[0], json!({"trigger": "time", "at": "06:00", "weekday": "sun"}));
    assert_eq!(out.configs[1], json!({"trigger": "time", "at": "{{ '09:00' }}", "weekday": ["sun"]}));
    for raw in [
        "- {trigger: time, at: \"{{ '09:00' }}\"}",
        "- {trigger: time, at: '08:00', weekday: []}",
        "- {trigger: time, at: '08:00', weekday: monday}",
        "- {trigger: time, at: {entity_id: sensor.alarm, offset: 0.5}}",
        "- {trigger: time, at: {entity_id: calendar.x, offset: '00:05'}}",
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
    // Other patterns use the clock block, with HA's defaults made visible.
    let clock = |yaml: &str, fields: Value, config: Value| {
        let out = import(yaml);
        assert_eq!(out.types, ["ha_trigger_time_pattern"], "{yaml}");
        assert_eq!(out.blocks[0]["fields"], fields, "{yaml}");
        assert_eq!(out.configs, [config], "{yaml}");
    };
    let fields = |h: (&str, u64), m: (&str, u64), s: (&str, u64)| json!({"HOURS_MODE": h.0, "HOURS": h.1, "MINUTES_MODE": m.0, "MINUTES": m.1, "SECONDS_MODE": s.0, "SECONDS": s.1});
    clock(
        "- {trigger: time_pattern, minutes: 30}",
        fields(("any", 0), ("at", 30), ("at", 0)),
        json!({"trigger": "time_pattern", "minutes": 30}),
    );
    clock(
        "- {trigger: time_pattern, hours: 7, minutes: '/5'}",
        fields(("at", 7), ("every", 5), ("at", 0)),
        json!({"trigger": "time_pattern", "hours": 7, "minutes": "/5"}),
    );
    clock(
        "- {trigger: time_pattern, minutes: '/5', seconds: 30}",
        fields(("any", 0), ("every", 5), ("at", 30)),
        json!({"trigger": "time_pattern", "minutes": "/5", "seconds": 30}),
    );
    clock(
        "- {trigger: time_pattern, hours: 0}",
        fields(("at", 0), ("at", 0), ("at", 0)),
        json!({"trigger": "time_pattern", "hours": 0}),
    );
    clock(
        "- {trigger: time_pattern, minutes: '*'}",
        fields(("any", 0), ("any", 0), ("at", 0)),
        json!({"trigger": "time_pattern", "seconds": 0}),
    );
    clock(
        "- {trigger: time_pattern, hours: 7, minutes: '*'}",
        fields(("at", 7), ("any", 0), ("at", 0)),
        json!({"trigger": "time_pattern", "hours": 7, "minutes": "*"}),
    );
    for raw in [
        "- {trigger: time_pattern, minutes: '/0'}",
        "- {trigger: time_pattern, minutes: '/75'}",
        "- {trigger: time_pattern, hours: 24}",
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
    check(
        "- {trigger: homeassistant, event: shutdown}",
        &["ha_trigger_ha_start"],
        json!([{"trigger": "homeassistant", "event": "shutdown"}]),
    );
    check_raw("- {trigger: homeassistant, event: restart}");
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
fn template_mqtt_webhook() {
    check(
        "- {trigger: template, value_template: \"{{ is_state('light.a', 'on') }}\", for: '00:01:00'}",
        &["ha_trigger_template"],
        json!([{"trigger": "template", "value_template": "{{ is_state('light.a', 'on') }}",
                "for": {"hours": 0, "minutes": 1, "seconds": 0}}]),
    );
    check(
        "- {trigger: template, value_template: \"{{ true }}\"}",
        &["ha_trigger_template"],
        json!([{"trigger": "template", "value_template": "{{ true }}"}]),
    );
    check(
        "- {trigger: mqtt, topic: home/door, payload: open, value_template: \"{{ value_json.state }}\"}",
        &["ha_trigger_mqtt"],
        json!([{"trigger": "mqtt", "topic": "home/door", "payload": "open", "value_template": "{{ value_json.state }}"}]),
    );
    check(
        "- {trigger: mqtt, topic: home/door, qos: '2'}",
        &["ha_trigger_mqtt"],
        json!([{"trigger": "mqtt", "topic": "home/door", "qos": 2}]),
    );
    // QoS 0 is HA's default.
    check(
        "- {trigger: mqtt, topic: home/door, qos: 0}",
        &["ha_trigger_mqtt"],
        json!([{"trigger": "mqtt", "topic": "home/door"}]),
    );
    check(
        "- {trigger: webhook, webhook_id: my-hook, allowed_methods: [POST, PUT], local_only: true}",
        &["ha_trigger_webhook"],
        json!([{"trigger": "webhook", "webhook_id": "my-hook", "allowed_methods": ["POST", "PUT"], "local_only": true}]),
    );
    // HA's defaults: POST and PUT, local only.
    check(
        "- {trigger: webhook, webhook_id: hook}",
        &["ha_trigger_webhook"],
        json!([{"trigger": "webhook", "webhook_id": "hook", "allowed_methods": ["POST", "PUT"], "local_only": true}]),
    );
    check(
        "- {trigger: webhook, webhook_id: hook, allowed_methods: get, local_only: false}",
        &["ha_trigger_webhook"],
        json!([{"trigger": "webhook", "webhook_id": "hook", "allowed_methods": ["GET"], "local_only": false}]),
    );
    for raw in [
        "- {trigger: template, value_template: \"{{ true }}\", for: 0}",
        "- {trigger: mqtt, topic: home/door, encoding: ''}",
        "- {trigger: mqtt, topic: home/door, qos: 3}",
        "- {trigger: mqtt, topic: home/door, payload: ''}",
        "- {trigger: webhook, webhook_id: hook, allowed_methods: [POST, POST]}",
        "- {trigger: webhook, webhook_id: hook, allowed_methods: [DELETE]}",
        "- {trigger: webhook, webhook_id: hook, allowed_methods: []}",
        "- {trigger: webhook, webhook_id: hook, local_only: 'yes'}",
    ] {
        check_raw(raw);
    }
}

#[test]
fn calendar_tag_conversation() {
    check(
        "- {trigger: calendar, event: start, entity_id: calendar.work, offset: '-0:15:0'}",
        &["ha_trigger_calendar"],
        json!([{"trigger": "calendar", "event": "start", "entity_id": "calendar.work", "offset": "-00:15:00"}]),
    );
    check(
        "- {trigger: calendar, entity_id: calendar.work}",
        &["ha_trigger_calendar"],
        json!([{"trigger": "calendar", "event": "start", "entity_id": "calendar.work"}]),
    );
    check(
        "- {trigger: tag, tag_id: A7-6B, device_id: [abc]}",
        &["ha_trigger_tag"],
        json!([{"trigger": "tag", "tag_id": "A7-6B", "device_id": "abc"}]),
    );
    check(
        "- {trigger: tag, tag_id: [a, b]}",
        &["ha_trigger_tag"],
        json!([{"trigger": "tag", "tag_id": ["a", "b"]}]),
    );
    check(
        "- {trigger: conversation, command: ['turn on the [living room] lights', lights on]}",
        &["ha_trigger_conversation"],
        json!([{"trigger": "conversation", "command": ["turn on the [living room] lights", "lights on"]}]),
    );
    check(
        "- {trigger: conversation, command: good night}",
        &["ha_trigger_conversation"],
        json!([{"trigger": "conversation", "command": ["good night"]}]),
    );
    for raw in [
        "- {trigger: calendar, event: start, entity_id: calendar.work, offset: 0.5}",
        "- {trigger: tag, tag_id: 'a,b'}",
        "- {trigger: tag, tag_id: []}",
        "- {trigger: conversation, command: []}",
        "- {trigger: conversation, command: [' padded']}",
    ] {
        check_raw(raw);
    }
}

#[test]
fn triggers_without_blocks_stay_raw() {
    for raw in [
        "- {trigger: geo_location, source: feed, zone: zone.alert, event: enter}",
        "- {trigger: persistent_notification, update_type: [added, removed], notification_id: x}",
        "- {trigger: some_custom.thing, foo: bar}",
        "- {trigger: light.turned_on, target: {entity_id: all}}",
        "- {trigger: light.turned_on, target: {area_id: '{{ room }}'}}",
        "- {trigger: light.turned_on, target: {entity_id: light.a, exclude: {entity_id: light.b}}}",
        "- {trigger: light.turned_on, target: {}}",
        "- {trigger: light.turned_on, options: on}",
    ] {
        check_raw(raw);
    }
}

#[test]
fn integration_triggers() {
    check(
        "- {trigger: light.turned_on, target: {area_id: kitchen}, options: {behavior: any}}",
        &["ha_trigger_integration"],
        json!([{"trigger": "light.turned_on", "target": {"area_id": ["kitchen"]}, "options": {"behavior": "any"}}]),
    );
    check(
        "- {trigger: sun.dusk, options: {type: civil}}",
        &["ha_trigger_integration"],
        json!([{"trigger": "sun.dusk", "options": {"type": "civil"}}]),
    );
    check(
        "- {trigger: calendar.event_started, target: {entity_id: [calendar.work, calendar.home]}, options: {offset: '00:05'}}",
        &["ha_trigger_integration"],
        json!([{"trigger": "calendar.event_started", "target": {"entity_id": ["calendar.work", "calendar.home"]},
            "options": {"offset": "00:05"}}]),
    );
    check(
        "- {trigger: switch.turned_off, target: {label_id: [a, b], device_id: d}, options: {}, id: off, alias: Switched off}",
        &["ha_trigger_integration"],
        json!([{"trigger": "switch.turned_off", "target": {"label_id": ["a", "b"], "device_id": ["d"]}, "options": {},
            "id": "off", "alias": "Switched off"}]),
    );
    // Options a dictionary block can't keep exactly stay as YAML in the options socket.
    let out = import("- {trigger: x.y, options: {for: 5.0, value: '{{ 1 }}'}}");
    assert_eq!(out.blocks[0]["inputs"]["OPTIONS"]["block"]["type"], "yaml_value");
    assert_eq!(out.configs, [json!({"trigger": "x.y", "options": {"for": 5.0, "value": "{{ 1 }}"}})]);
}

#[test]
fn base_options() {
    // Ids (also repeated ones) and aliases become the block's trigger id and name.
    check(
        "- {trigger: state, entity_id: light.a, to: 'on', id: x, alias: Light on}\n- {trigger: state, entity_id: light.b, to: 'on', id: x}",
        &["ha_trigger_becomes", "ha_trigger_becomes"],
        json!([{"trigger": "state", "entity_id": "light.a", "to": "on", "id": "x", "alias": "Light on"},
            {"trigger": "state", "entity_id": "light.b", "to": "on", "id": "x"}]),
    );
    let out = import("- {trigger: state, entity_id: light.a, to: 'on', id: light_on}");
    assert!(out.blocks[0]["id"].as_str().unwrap().starts_with("imp"));
    assert_eq!(out.blocks[0]["extraState"], json!({"triggerSettings": ["TRIGGER_ID"]}));
    // HA only takes text; empty ones can't be set.
    check_raw("- {trigger: state, entity_id: light.a, to: 'on', id: ''}");
    check_raw("- {trigger: state, entity_id: light.a, to: 'on', id: 5}");
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
    assert_eq!((native, out.raw), (57, 10), "{:?}", out.types);
}

/// Every block in the workspace, depth first.
fn all_blocks(v: &Value, out: &mut Vec<Value>) {
    match v {
        Value::Object(m) => {
            if m.contains_key("type") && m.contains_key("id") {
                out.push(v.clone());
            }
            m.values().for_each(|x| all_blocks(x, out));
        }
        Value::Array(a) => a.iter().for_each(|x| all_blocks(x, out)),
        _ => {}
    }
}

#[test]
fn trigger_ids_reach_conditions_and_waits() {
    let yaml = r#"
triggers:
  - {trigger: state, entity_id: binary_sensor.door, to: "on", id: door}
  - {trigger: state, entity_id: binary_sensor.back_door, to: "on", id: door}
  - {trigger: time, at: "07:00", id: morning}
conditions:
  - condition: trigger
    id: [door, raw_only]
  - condition: trigger
    id: imp1
actions:
  - wait_for_trigger:
      - {trigger: event, event_type: confirm, id: confirmed, alias: Confirmed}
"#;
    let imp = import_yaml(yaml).unwrap().remove(0);
    let mut blocks = Vec::new();
    all_blocks(&imp.workspace, &mut blocks);
    let of = |ty: &str| blocks.iter().filter(|b| b["type"] == ty).cloned().collect::<Vec<_>>();
    let doors: Vec<Value> =
        of("ha_trigger_becomes").into_iter().filter(|b| b["fields"]["TRIGGER_ID"] == "door").collect();
    assert_eq!(doors.len(), 2);
    // "triggered by" points at the first trigger with the id; ids no trigger block has stay as
    // they are, and one that looks like a block id keeps the condition raw.
    let by: Vec<Value> = of("triggered_by").iter().map(|b| b["fields"]["TRIGGER"].clone()).collect();
    assert_eq!(by, [doors[0]["id"].clone(), json!("raw_only")]);
    assert_eq!(of("ha_condition_raw").len(), 1);
    assert_eq!(imp.raw_blocks, 1);

    let t = &of("ha_wait_for_trigger")[0]["inputs"]["TRIGGERS"]["block"];
    assert_eq!(t["type"], "ha_trigger_event");
    assert_eq!(
        (&t["fields"]["TRIGGER_ID"], &t["fields"]["TRIGGER_ALIAS"]),
        (&json!("confirmed"), &json!("Confirmed"))
    );

    let p = compile(&imp.workspace, &NoExternals).unwrap();
    let HandlerKind::Triggers { triggers } = &p.handlers[0].kind else { panic!() };
    let ids: Vec<&Value> = triggers
        .iter()
        .map(|t| match t {
            Trigger::Ha { config, .. } => &config["id"],
            _ => panic!(),
        })
        .collect();
    assert_eq!(ids, [&json!("door"), &json!("door"), &json!("morning")]);
    // The comparisons use the ids, and the wait sends its id and alias.
    let body = serde_json::to_string(&p.handlers[0].body).unwrap();
    for want in [r#"{"op":"lit","value":"door"}"#, r#""id":"confirmed""#, r#""alias":"Confirmed""#] {
        assert!(body.contains(want), "{want} in {body}");
    }
}
