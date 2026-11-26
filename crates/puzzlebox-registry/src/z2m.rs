//! Blocks generated from Zigbee2MQTT's `bridge/devices` and `bridge/groups` messages.
//!
//! Block types are keyed by IEEE address (devices) or group id, so renaming a device in
//! Zigbee2MQTT keeps saved programs working; only the topic in the lowering changes.

use serde_json::{Value, json};

use crate::autogen::humanize;
use crate::model::*;

const ACCESS_STATE: u64 = 1;
const ACCESS_SET: u64 = 2;

/// A leaf feature from `definition.exposes` (specific types like `light` are flattened).
struct Feature {
    property: String,
    label: String,
    access: u64,
    input: ParamInput,
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect()
}

fn features(exposes: &Value, out: &mut Vec<Feature>) {
    for e in exposes.as_array().into_iter().flatten() {
        let ty = e.get("type").and_then(Value::as_str).unwrap_or("");
        let property = e.get("property").and_then(Value::as_str).map(str::to_owned);
        let name = e.get("name").and_then(Value::as_str).unwrap_or("");
        // Colour composites (color_xy / color_hs) become one colour field.
        if ty == "composite" && name.starts_with("color") {
            if let Some(property) = property {
                let access = e.get("access").and_then(Value::as_u64).unwrap_or(0);
                out.push(Feature { property, label: "color".into(), access, input: ParamInput::Color });
            }
            continue;
        }
        if let Some(inner) = e.get("features")
            && property.is_none()
        {
            features(inner, out);
            continue;
        }
        let Some(property) = property else { continue };
        let access = e.get("access").and_then(Value::as_u64).unwrap_or(0);
        let label = e
            .get("label")
            .and_then(Value::as_str)
            .map(str::to_lowercase)
            .unwrap_or_else(|| humanize(name).to_lowercase());
        let input = match ty {
            "binary" => {
                let on = e.get("value_on").cloned().unwrap_or(json!(true));
                if on.is_boolean() {
                    ParamInput::Boolean
                } else {
                    let options = ["value_on", "value_off", "value_toggle"]
                        .iter()
                        .filter_map(|k| e.get(*k).and_then(Value::as_str))
                        .map(|v| SelectOption { label: v.to_owned(), value: v.to_owned() })
                        .collect();
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
                let options: Vec<SelectOption> = e
                    .get("values")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|v| SelectOption { label: humanize(v).to_lowercase(), value: v.to_owned() })
                    .collect();
                if options.is_empty() {
                    ParamInput::Text { multiline: false }
                } else {
                    ParamInput::Select { options, multiple: false }
                }
            }
            "text" => ParamInput::Text { multiline: false },
            _ => ParamInput::Any,
        };
        out.push(Feature { property, label, access, input });
    }
}

fn param(f: &Feature, required: bool) -> Param {
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
    let block = |ty: String, kind, cat: &str, title: String, tooltip: String, params, lowering| BlockDef {
        block_type: ty,
        kind,
        category: vec!["Zigbee2MQTT".into(), cat.to_owned()],
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
        let mut feats = Vec::new();
        features(d.pointer("/definition/exposes").unwrap_or(&Value::Null), &mut feats);
        if feats.is_empty() {
            continue;
        }
        let model = d.pointer("/definition/description").and_then(Value::as_str).unwrap_or("");
        let id = format!("{prefix}__{}", sanitize(ieee));
        let topic = format!("{base}/{name}");

        if let Some(action) = feats.iter().find(|f| f.property == "action")
            && let ParamInput::Select { options, .. } = &action.input
        {
            out.push(block(
                format!("{id}__action"),
                BlockKind::Trigger,
                name,
                format!("{name} action"),
                format!("Fires when {name} sends the chosen action. {model}"),
                vec![select_param("action", "is", options.clone(), true)],
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
        let props: Vec<SelectOption> = published
            .iter()
            .map(|f| SelectOption { label: f.label.clone(), value: f.property.clone() })
            .collect();
        if !props.is_empty() {
            let to = Param {
                name: "to".into(),
                label: "to".into(),
                description: "Only fire for this value (e.g. ON, true or 21). Leave empty for any change."
                    .into(),
                required: false,
                input: ParamInput::Text { multiline: false },
                default: None,
            };
            out.push(block(
                format!("{id}__changed"),
                BlockKind::Trigger,
                name,
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
                name,
                name.to_string(),
                format!("The last reported value from {name}."),
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
                name,
                format!("Set {name}"),
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
        let cat = format!("Group: {name}");
        let num = |n: &str, l: &str, min: f64, max: f64| Param {
            name: n.into(),
            label: l.into(),
            description: String::new(),
            required: false,
            input: ParamInput::Number { min: Some(min), max: Some(max), step: Some(1.0), unit: None },
            default: None,
        };
        let state_opts = ["ON", "OFF", "TOGGLE"]
            .iter()
            .map(|v| SelectOption { label: (*v).into(), value: (*v).into() })
            .collect();
        out.push(block(
            format!("{id}__set"),
            BlockKind::Action,
            &cat,
            format!("Set group {name}"),
            format!("Sends one command to every member of the Zigbee group {name}."),
            vec![
                select_param("state", "state", state_opts, false),
                num("brightness", "brightness", 0.0, 254.0),
                num("color_temp", "color temperature (mired)", 150.0, 500.0),
                Param {
                    name: "color".into(),
                    label: "color".into(),
                    description: String::new(),
                    required: false,
                    input: ParamInput::Color,
                    default: None,
                },
                num("transition", "transition (s)", 0.0, 300.0),
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
                &cat,
                format!("Recall scene in {name}"),
                "Recalls a scene stored in the group's devices.".into(),
                vec![select_param("scene_recall", "scene", scenes, true)],
                Lowering::MqttPublish { topic: topic.clone(), fixed: Value::Null },
            ));
        }
        out.push(block(
            format!("{id}__store"),
            BlockKind::Action,
            &cat,
            format!("Store scene in {name}"),
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

    #[test]
    fn generates_device_and_group_blocks() {
        let devices = json!([
            {"type": "Coordinator", "friendly_name": "Coordinator", "ieee_address": "0x00"},
            {"type": "EndDevice", "friendly_name": "Hall/Button", "ieee_address": "0x00158d0001a2b3c4",
             "definition": {"description": "Wireless switch", "exposes": [
                {"type": "enum", "name": "action", "property": "action", "access": 1, "values": ["single", "double", "hold"]},
                {"type": "numeric", "name": "battery", "property": "battery", "access": 1, "unit": "%"}]}},
            {"type": "Router", "friendly_name": "Desk Lamp", "ieee_address": "0x0017880100aabbcc",
             "definition": {"description": "Bulb", "exposes": [
                {"type": "light", "features": [
                    {"type": "binary", "name": "state", "property": "state", "access": 7, "value_on": "ON", "value_off": "OFF", "value_toggle": "TOGGLE"},
                    {"type": "numeric", "name": "brightness", "property": "brightness", "access": 7, "value_min": 0, "value_max": 254},
                    {"type": "composite", "name": "color_xy", "property": "color", "access": 7, "features": []}]}]}}
        ]);
        let groups = json!([{"id": 3, "friendly_name": "Living", "scenes": [{"id": 1, "name": "Movie"}]}]);
        let blocks = generate("zigbee2mqtt", &devices, &groups);
        let ty: Vec<&str> = blocks.iter().map(|b| b.block_type.as_str()).collect();
        assert!(ty.contains(&"z2m__zigbee2mqtt__0x00158d0001a2b3c4__action"), "{ty:?}");
        assert!(!ty.iter().any(|t| t.contains("0x00__")), "coordinator skipped");
        let action = &blocks[0];
        assert_eq!(action.category, ["Zigbee2MQTT", "Hall/Button"]);
        let Lowering::MqttTrigger { topic, .. } = &action.lowering else { panic!() };
        assert_eq!(topic, "zigbee2mqtt/Hall/Button");
        let set = blocks.iter().find(|b| b.block_type.ends_with("0x0017880100aabbcc__set")).unwrap();
        let names: Vec<&str> = set.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["state", "brightness", "color"]);
        assert!(set.params[0].required);
        assert!(matches!(set.params[2].input, ParamInput::Color));
        assert!(ty.contains(&"z2m__zigbee2mqtt__group3__scene"));
    }
}
