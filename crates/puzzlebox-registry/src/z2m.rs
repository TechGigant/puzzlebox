//! Blocks generated from Zigbee2MQTT's `bridge/devices` and `bridge/groups` messages.
//!
//! Block types are keyed by IEEE address (devices) or group id, so renaming a device in
//! Zigbee2MQTT keeps saved programs working; only the topic in the lowering changes.

use serde_json::{Value, json};

use crate::autogen::humanize;
use crate::model::*;

const ACCESS_STATE: u64 = 1;
const ACCESS_SET: u64 = 2;
const ACCESS_GET: u64 = 4;

/// A leaf feature from `definition.exposes` (specific types like `light` are flattened).
struct Feature {
    property: String,
    label: String,
    access: u64,
    input: ParamInput,
    /// Values a binary or enum feature reports, with friendly names.
    values: Vec<SelectOption>,
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect()
}

/// Words for the true and false values of well-known binary features.
fn binary_words(name: &str) -> [&'static str; 2] {
    match name {
        // Zigbee2MQTT's contact is true while the magnet touches, i.e. the door is closed.
        "contact" => ["closed", "open"],
        "occupancy" | "presence" | "smoke" | "vibration" | "gas" | "carbon_monoxide" => ["detected", "clear"],
        "water_leak" => ["leak", "dry"],
        "tamper" => ["tampered", "ok"],
        "battery_low" => ["low", "ok"],
        _ => ["on", "off"],
    }
}

/// Shown name of a Zigbee2MQTT value ("ON" → "on", "brightness_move_up" → "brightness move up").
fn friendly(v: &str) -> String {
    humanize(v).to_lowercase()
}

fn value_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Bool(_) | Value::Number(_) => Some(v.to_string()),
        _ => None,
    }
}

/// Multi-endpoint devices (2-gang switches…) expose `state_l1`, `state_left`; Zigbee2MQTT
/// labels them all "State", so the endpoint is added ("state (l1)").
fn endpoint_label(label: String, e: &Value, inherited: Option<&str>, name: &str, property: &str) -> String {
    let endpoint = e
        .get("endpoint")
        .and_then(Value::as_str)
        .or(inherited)
        .map(str::to_owned)
        .or_else(|| property.strip_prefix(name).and_then(|r| r.strip_prefix('_')).map(str::to_owned))
        .filter(|ep| !ep.is_empty());
    match endpoint {
        Some(ep) if !label.contains(&ep) => format!("{label} ({ep})"),
        _ => label,
    }
}

fn features(exposes: &Value, endpoint: Option<&str>, out: &mut Vec<Feature>) {
    for e in exposes.as_array().into_iter().flatten() {
        let ty = e.get("type").and_then(Value::as_str).unwrap_or("");
        let property = e.get("property").and_then(Value::as_str).map(str::to_owned);
        let name = e.get("name").and_then(Value::as_str).unwrap_or("");
        let access = e.get("access").and_then(Value::as_u64).unwrap_or(0);
        // Colour composites (color_xy / color_hs) become one colour field.
        if ty == "composite" && name.starts_with("color") {
            if let Some(property) = property {
                let label = endpoint_label("color".into(), e, endpoint, "color", &property);
                out.push(Feature { property, label, access, input: ParamInput::Color, values: Vec::new() });
            }
            continue;
        }
        if let Some(inner) = e.get("features")
            && property.is_none()
        {
            features(inner, e.get("endpoint").and_then(Value::as_str).or(endpoint), out);
            continue;
        }
        let Some(property) = property else { continue };
        let label = e
            .get("label")
            .and_then(Value::as_str)
            .map(str::to_lowercase)
            .unwrap_or_else(|| humanize(name).to_lowercase());
        let label = endpoint_label(label, e, endpoint, name, &property);
        let mut values = Vec::new();
        let input = match ty {
            "binary" => {
                let on = e.get("value_on").cloned().unwrap_or(json!(true));
                let off = e.get("value_off").cloned().unwrap_or(json!(false));
                for v in [&on, &off] {
                    let Some(text) = value_text(v) else { continue };
                    let label = match (binary_words(name), v) {
                        ([yes, no], Value::Bool(b)) => (if *b { yes } else { no }).to_owned(),
                        _ => friendly(&text),
                    };
                    values.push(SelectOption { label, value: text });
                }
                if on.is_boolean() {
                    ParamInput::Boolean
                } else {
                    let mut options = values.clone();
                    if let Some(t) = e.get("value_toggle").and_then(Value::as_str) {
                        options.push(SelectOption { label: friendly(t), value: t.to_owned() });
                    }
                    ParamInput::Select { options, multiple: false }
                }
            }
            "numeric" => ParamInput::Number {
                min: e.get("value_min").and_then(Value::as_f64),
                max: e.get("value_max").and_then(Value::as_f64),
                step: e.get("value_step").and_then(Value::as_f64),
                unit: e.get("unit").and_then(Value::as_str).map(str::to_owned),
            },
            "enum" => {
                values = e
                    .get("values")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|v| SelectOption { label: friendly(v), value: v.to_owned() })
                    .collect();
                if values.is_empty() {
                    ParamInput::Text { multiline: false }
                } else {
                    ParamInput::Select { options: values.clone(), multiple: false }
                }
            }
            "text" => ParamInput::Text { multiline: false },
            _ => ParamInput::Any,
        };
        out.push(Feature { property, label, access, input, values });
    }
}

/// The toolbox group a device goes in, from what it exposes.
fn device_kind(exposes: &Value, feats: &[Feature]) -> &'static str {
    let has = |t: &str| {
        exposes.as_array().into_iter().flatten().any(|e| e.get("type").and_then(Value::as_str) == Some(t))
    };
    if has("light") {
        "Lights"
    } else if has("switch") {
        "Switches and plugs"
    } else if has("cover") {
        "Covers"
    } else if has("climate") || has("fan") {
        "Climate"
    } else if has("lock") {
        "Locks"
    } else if feats.iter().any(|f| f.property == "action") {
        "Buttons and remotes"
    } else if feats.iter().any(|f| {
        // Sensors may have settings (sensitivity, LED), but report something besides upkeep.
        f.access & ACCESS_SET == 0 && !matches!(f.property.as_str(), "linkquality" | "battery" | "voltage")
    }) {
        "Sensors"
    } else {
        "Other devices"
    }
}

fn device_features(d: &Value) -> Vec<Feature> {
    let mut feats = Vec::new();
    features(d.pointer("/definition/exposes").unwrap_or(&Value::Null), None, &mut feats);
    feats
}

/// `(device topic, payload)` for a `<topic>/get` request (`{"state": "", "brightness": ""}`)
/// asking for the features each device answers on request, so value blocks know them
/// without waiting for the device to report.
pub fn get_requests(base: &str, devices: &Value) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    for d in devices.as_array().into_iter().flatten() {
        if d.get("type").and_then(Value::as_str) == Some("Coordinator") {
            continue;
        }
        let Some(name) = d.get("friendly_name").and_then(Value::as_str) else { continue };
        let payload: serde_json::Map<String, Value> = device_features(d)
            .into_iter()
            .filter(|f| f.access & ACCESS_GET != 0 && f.access & ACCESS_STATE != 0)
            .map(|f| (f.property, json!("")))
            .collect();
        if !payload.is_empty() {
            out.push((format!("{base}/{name}"), Value::Object(payload)));
        }
    }
    out
}

fn param(f: &Feature, required: bool) -> Param {
    // White temperature is set in kelvin like everywhere else in the app; the lowering turns
    // `color_temp_kelvin` back into mired for Zigbee2MQTT.
    if f.property == "color_temp"
        && let ParamInput::Number { min, max, .. } = &f.input
    {
        let kelvin = |m: Option<f64>| m.filter(|m| *m > 0.0).map(|m| (1_000_000.0 / m).round());
        return Param {
            name: "color_temp_kelvin".into(),
            label: "white temperature".into(),
            description: String::new(),
            required,
            input: ParamInput::Number {
                min: kelvin(*max).or(Some(2000.0)),
                max: kelvin(*min).or(Some(6500.0)),
                step: Some(1.0),
                unit: Some("K".into()),
            },
            default: None,
        };
    }
    Param {
        name: f.property.clone(),
        label: f.label.clone(),
        description: String::new(),
        required,
        input: f.input.clone(),
        default: match (&f.input, required) {
            (ParamInput::Select { options, .. }, true) => options.first().map(|o| json!(o.value)),
            _ => None,
        },
    }
}

fn select_param(name: &str, label: &str, options: Vec<SelectOption>, required: bool) -> Param {
    let default = options.first().map(|o| json!(o.value));
    Param {
        name: name.into(),
        label: label.into(),
        description: String::new(),
        required,
        input: ParamInput::Select { options, multiple: false },
        default,
    }
}

/// All Zigbee2MQTT blocks for one instance (`base` topic).
pub fn generate(base: &str, devices: &Value, groups: &Value) -> Vec<BlockDef> {
    let source = format!("z2m:{base}");
    let prefix = format!("z2m__{}", sanitize(base));
    let mut out = Vec::new();
    let block =
        |ty: String, kind, cat: [&str; 2], title: String, tooltip: String, params, lowering| BlockDef {
            block_type: ty,
            kind,
            category: vec!["Zigbee2MQTT".into(), cat[0].to_owned(), cat[1].to_owned()],
            style: "zigbee".into(),
            title,
            tooltip,
            help_url: None,
            target: None,
            params,
            lowering,
            source: source.clone(),
        };

    for d in devices.as_array().into_iter().flatten() {
        if d.get("type").and_then(Value::as_str) == Some("Coordinator")
            || d.get("interview_completed") == Some(&json!(false))
        {
            continue;
        }
        let (Some(name), Some(ieee)) =
            (d.get("friendly_name").and_then(Value::as_str), d.get("ieee_address").and_then(Value::as_str))
        else {
            continue;
        };
        let feats = device_features(d);
        if feats.is_empty() {
            continue;
        }
        let cat = [device_kind(d.pointer("/definition/exposes").unwrap_or(&Value::Null), &feats), name];
        let model = d.pointer("/definition/description").and_then(Value::as_str).unwrap_or("");
        let id = format!("{prefix}__{}", sanitize(ieee));
        let topic = format!("{base}/{name}");

        if let Some(action) = feats.iter().find(|f| f.property == "action")
            && let ParamInput::Select { options, .. } = &action.input
        {
            out.push(block(
                format!("{id}__action"),
                BlockKind::Trigger,
                cat,
                format!("{name} sends"),
                format!("Fires when {name} sends the chosen action (a press, turn or gesture). {model}"),
                vec![select_param("action", "", options.clone(), true)],
                Lowering::MqttTrigger {
                    topic: topic.clone(),
                    key: Some("action".into()),
                    key_param: None,
                    equals_param: Some("action".into()),
                    changed: false,
                },
            ));
        }
        let published: Vec<&Feature> =
            feats.iter().filter(|f| f.access & ACCESS_STATE != 0 && f.property != "action").collect();
        // Features with a few known values get their own trigger with those values to pick
        // ("Front door contact becomes open").
        for f in published.iter().filter(|f| !f.values.is_empty()) {
            let mut options = vec![SelectOption { label: "changes".into(), value: String::new() }];
            options.extend(
                f.values
                    .iter()
                    .map(|v| SelectOption { label: format!("becomes {}", v.label), value: v.value.clone() }),
            );
            out.push(block(
                format!("{id}__becomes__{}", sanitize(&f.property)),
                BlockKind::Trigger,
                cat,
                format!("{name} {}", f.label),
                format!("Fires when the {} of {name} changes (to the chosen value). {model}", f.label),
                vec![select_param("to", "", options, true)],
                Lowering::MqttTrigger {
                    topic: topic.clone(),
                    key: Some(f.property.clone()),
                    key_param: None,
                    equals_param: Some("to".into()),
                    changed: true,
                },
            ));
        }
        let props: Vec<SelectOption> = published
            .iter()
            .map(|f| SelectOption { label: f.label.clone(), value: f.property.clone() })
            .collect();
        if !props.is_empty() {
            let to = Param {
                name: "to".into(),
                label: "to".into(),
                description: "Only fire for this value (e.g. 21). Leave empty for any change.".into(),
                required: false,
                input: ParamInput::Text { multiline: false },
                default: None,
            };
            out.push(block(
                format!("{id}__changed"),
                BlockKind::Trigger,
                cat,
                format!("{name} changes"),
                format!("Fires when the chosen value of {name} changes. {model}"),
                vec![select_param("property", "", props.clone(), true), to],
                Lowering::MqttTrigger {
                    topic: topic.clone(),
                    key: None,
                    key_param: Some("property".into()),
                    equals_param: Some("to".into()),
                    changed: true,
                },
            ));
            out.push(block(
                format!("{id}__value"),
                BlockKind::Value,
                cat,
                name.to_string(),
                format!(
                    "The value {name} last reported. Empty until it has reported since Puzzlebox started \
                     (values the device can be asked for are requested at start-up)."
                ),
                vec![select_param("property", "", props, true)],
                Lowering::MqttValue { topic: topic.clone(), key: None, key_param: Some("property".into()) },
            ));
        }
        let settable: Vec<&Feature> = feats.iter().filter(|f| f.access & ACCESS_SET != 0).collect();
        if !settable.is_empty() {
            let params = settable.iter().map(|f| param(f, f.property == "state")).collect();
            out.push(block(
                format!("{id}__set"),
                BlockKind::Action,
                cat,
                format!("set {name}"),
                format!("Sends a command to {name} through Zigbee2MQTT. {model}"),
                params,
                Lowering::MqttPublish { topic: format!("{topic}/set"), fixed: Value::Null },
            ));
        }
    }

    for g in groups.as_array().into_iter().flatten() {
        let (Some(name), Some(gid)) =
            (g.get("friendly_name").and_then(Value::as_str), g.get("id").and_then(Value::as_u64))
        else {
            continue;
        };
        let id = format!("{prefix}__group{gid}");
        let topic = format!("{base}/{name}/set");
        let cat = ["Groups", name];
        let num = |n: &str, l: &str, min: f64, max: f64, unit: Option<&str>| Param {
            name: n.into(),
            label: l.into(),
            description: String::new(),
            required: false,
            input: ParamInput::Number {
                min: Some(min),
                max: Some(max),
                step: Some(1.0),
                unit: unit.map(str::to_owned),
            },
            default: None,
        };
        let state_opts = ["ON", "OFF", "TOGGLE"]
            .iter()
            .map(|v| SelectOption { label: friendly(v), value: (*v).into() })
            .collect();
        out.push(block(
            format!("{id}__set"),
            BlockKind::Action,
            cat,
            format!("set group {name}"),
            format!("Sends one command to every member of the Zigbee group {name}."),
            vec![
                select_param("state", "state", state_opts, false),
                num("brightness_percent", "brightness", 0.0, 100.0, Some("%")),
                num("color_temp_kelvin", "white temperature", 2000.0, 6500.0, Some("K")),
                Param {
                    name: "color".into(),
                    label: "color".into(),
                    description: String::new(),
                    required: false,
                    input: ParamInput::Color,
                    default: None,
                },
                num("transition", "fade", 0.0, 300.0, Some("seconds")),
            ],
            Lowering::MqttPublish { topic: topic.clone(), fixed: Value::Null },
        ));
        let scenes: Vec<SelectOption> = g
            .get("scenes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|s| {
                let sid = s.get("id")?.as_u64()?;
                let label = s
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|n| !n.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("scene {sid}"));
                Some(SelectOption { label, value: sid.to_string() })
            })
            .collect();
        if !scenes.is_empty() {
            out.push(block(
                format!("{id}__scene"),
                BlockKind::Action,
                cat,
                format!("recall scene in {name}"),
                "Recalls a scene stored in the group's devices.".into(),
                vec![select_param("scene_recall", "scene", scenes, true)],
                Lowering::MqttPublish { topic: topic.clone(), fixed: Value::Null },
            ));
        }
        out.push(block(
            format!("{id}__store"),
            BlockKind::Action,
            cat,
            format!("store scene in {name}"),
            "Saves the group's current light states as a scene (stored in the devices).".into(),
            vec![Param {
                name: "scene_store".into(),
                label: "as scene number".into(),
                description: String::new(),
                required: true,
                input: ParamInput::Number { min: Some(0.0), max: Some(255.0), step: Some(1.0), unit: None },
                default: Some(json!(1)),
            }],
            Lowering::MqttPublish { topic, fixed: Value::Null },
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/z2m-devices.json")).unwrap()
    }

    fn find<'a>(blocks: &'a [BlockDef], suffix: &str) -> &'a BlockDef {
        blocks.iter().find(|b| b.block_type.ends_with(suffix)).unwrap_or_else(|| panic!("no block {suffix}"))
    }

    fn options(p: &Param) -> Vec<(&str, &str)> {
        let ParamInput::Select { options, .. } = &p.input else { panic!("{} is not a dropdown", p.name) };
        options.iter().map(|o| (o.label.as_str(), o.value.as_str())).collect()
    }

    #[test]
    fn generates_device_and_group_blocks() {
        let groups = json!([{"id": 3, "friendly_name": "Living", "scenes": [{"id": 1, "name": "Movie"}]}]);
        let blocks = generate("zigbee2mqtt", &fixture(), &groups);
        let ty: Vec<&str> = blocks.iter().map(|b| b.block_type.as_str()).collect();
        assert!(!ty.iter().any(|t| t.contains("0x00124b0022aabb01")), "coordinator skipped");
        assert!(!ty.iter().any(|t| t.contains("0x0017880100deadbe")), "unpaired device skipped");

        let action = find(&blocks, "0x000b57fffe445566__action");
        assert_eq!(action.title, "Living room remote sends");
        assert_eq!(action.category, ["Zigbee2MQTT", "Buttons and remotes", "Living room remote"]);
        assert!(options(&action.params[0]).contains(&("brightness up click", "brightness_up_click")));
        let Lowering::MqttTrigger { topic, .. } = &action.lowering else { panic!() };
        assert_eq!(topic, "zigbee2mqtt/Living room remote");

        let set = find(&blocks, "0x000d6ffffe112233__set");
        assert_eq!(set.title, "set Desk lamp");
        assert_eq!(set.category[1], "Lights");
        let names: Vec<&str> = set.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["state", "brightness", "color_temp_kelvin", "color_temp_startup", "effect", "power_on_behavior"]
        );
        assert!(set.params[0].required);
        assert_eq!(options(&set.params[0]), [("on", "ON"), ("off", "OFF"), ("toggle", "TOGGLE")]);

        let group = find(&blocks, "group3__set");
        assert_eq!(group.category, ["Zigbee2MQTT", "Groups", "Living"]);
        assert_eq!(options(&group.params[0])[0], ("on", "ON"));
        assert_eq!(find(&blocks, "group3__scene").title, "recall scene in Living");

        assert_eq!(find(&blocks, "0x00158d0001a2b3c4__changed").category[1], "Sensors");
        assert_eq!(find(&blocks, "0x0017880106f1e2d3__changed").category[1], "Sensors", "settings aside");
        assert_eq!(find(&blocks, "0xa4c138aabbccdd01__set").category[1], "Switches and plugs");
    }

    #[test]
    fn white_temperature_is_set_in_kelvin() {
        let blocks = generate("zigbee2mqtt", &fixture(), &json!([]));
        let set = find(&blocks, "0x000d6ffffe112233__set");
        let p = set.params.iter().find(|p| p.name == "color_temp_kelvin").unwrap();
        assert_eq!(p.label, "white temperature");
        // 454 mired is the warmest (2203 K), 250 the coolest (4000 K).
        assert!(matches!(
            p.input,
            ParamInput::Number { min: Some(2203.0), max: Some(4000.0), unit: Some(ref u), .. } if u == "K"
        ));
    }

    #[test]
    fn endpoints_tell_gangs_apart() {
        let blocks = generate("zigbee2mqtt", &fixture(), &json!([]));
        let set = find(&blocks, "0xa4c138aabbccdd01__set");
        let labels: Vec<&str> = set.params.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["state (left)", "state (right)", "power-on behavior"]);
        assert_eq!(
            find(&blocks, "0xa4c138aabbccdd01__becomes__state_left").title,
            "Kitchen switch state (left)"
        );

        // Without an `endpoint`, the suffix of the property is used.
        let devices = json!([{"type": "Router", "friendly_name": "Plug", "ieee_address": "0x01",
            "definition": {"exposes": [{"type": "binary", "name": "state", "property": "state_l2", "access": 7,
                "value_on": "ON", "value_off": "OFF", "label": "State"}]}}]);
        let blocks = generate("zigbee2mqtt", &devices, &json!([]));
        assert_eq!(find(&blocks, "0x01__set").params[0].label, "state (l2)");
    }

    #[test]
    fn binary_and_enum_triggers_offer_their_values() {
        let blocks = generate("zigbee2mqtt", &fixture(), &json!([]));
        let contact = find(&blocks, "0x00158d0001a2b3c4__becomes__contact");
        assert_eq!(contact.title, "Front door contact");
        assert_eq!(
            options(&contact.params[0]),
            [("changes", ""), ("becomes open", "false"), ("becomes closed", "true")]
        );
        let Lowering::MqttTrigger { key, equals_param, changed, .. } = &contact.lowering else { panic!() };
        assert_eq!((key.as_deref(), equals_param.as_deref(), changed), (Some("contact"), Some("to"), &true));

        let occupancy = find(&blocks, "0x0017880106f1e2d3__becomes__occupancy");
        assert_eq!(
            options(&occupancy.params[0])[1..],
            [("becomes detected", "true"), ("becomes clear", "false")]
        );
        let sensitivity = find(&blocks, "0x0017880106f1e2d3__becomes__motion_sensitivity");
        assert!(options(&sensitivity.params[0]).contains(&("becomes very high", "very_high")));
        // Numbers only get the general "changes" trigger.
        assert!(!blocks.iter().any(|b| b.block_type.ends_with("__becomes__battery")));
        // Set-only features (effect) can't be watched.
        assert!(!blocks.iter().any(|b| b.block_type.ends_with("__becomes__effect")));
    }

    #[test]
    fn asks_for_readable_values() {
        let gets = get_requests("zigbee2mqtt", &fixture());
        let find = |t: &str| gets.iter().find(|(topic, _)| topic == t).map(|(_, p)| p.clone());
        assert_eq!(find("zigbee2mqtt/Front door"), None, "sleeping sensor can't be asked");
        assert_eq!(
            find("zigbee2mqtt/Kitchen switch"),
            Some(json!({"state_left": "", "state_right": "", "power_on_behavior": ""}))
        );
        let lamp = find("zigbee2mqtt/Desk lamp").unwrap();
        assert!(lamp.get("color_temp").is_some() && lamp.get("effect").is_none(), "{lamp}");
        assert_eq!(find("zigbee2mqtt/Living room remote"), Some(json!({"battery": ""})));
    }
}
