//! Remember/restore, alarm, button and first/last person blocks: imported only where they send
//! exactly what the YAML does.

use puzzlebox_ir::{
    Expr, ExprKind, HandlerKind, NoExternals, Program, StmtKind, Trigger, compile, normalize_target,
};
use puzzlebox_yaml_import::import_yaml;
use serde_json::{Value, json};

fn import(yaml: &str) -> (Value, Program) {
    let imp = import_yaml(yaml).expect("imports").remove(0);
    let p = compile(&imp.workspace, &NoExternals).unwrap_or_else(|e| panic!("compiles: {e:?}\n{yaml}"));
    (imp.workspace, p)
}

/// The block type an action became, and the call it compiles to (target normalised).
fn action(yaml: &str) -> (String, Value) {
    let (ws, p) = import(&format!("alias: t\nsequence:\n  - {yaml}\n"));
    let ty = ws.pointer("/blocks/blocks/0/inputs/DO/block/type").and_then(Value::as_str).unwrap().to_owned();
    let StmtKind::CallService { service, target, data, .. } = &p.handlers[0].body[0].kind else {
        return (ty, Value::Null);
    };
    let target = target.as_ref().map(|t| normalize_target(&plain(t)).unwrap());
    (ty, json!({"action": service, "target": target, "data": data.as_ref().map(plain)}))
}

fn plain(e: &Expr) -> Value {
    match &e.kind {
        ExprKind::Lit { value } => value.clone(),
        ExprKind::List { items } => items.iter().map(plain).collect(),
        ExprKind::Dict { entries } => entries.iter().map(|(k, v)| (k.clone(), plain(v))).collect(),
        other => json!({"expr": other}),
    }
}

#[test]
fn alarm_actions() {
    assert_eq!(
        action(
            "action: alarm_control_panel.alarm_arm_night\n    target: {entity_id: alarm_control_panel.house}\n    data: {code: 1234}"
        ),
        (
            "alarm_arm".into(),
            json!({"action": "alarm_control_panel.alarm_arm_night",
                "target": {"entity_id": ["alarm_control_panel.house"]}, "data": {"code": "1234"}})
        )
    );
    assert_eq!(
        action(
            "action: alarm_control_panel.alarm_disarm\n    target: {entity_id: alarm_control_panel.house}"
        ),
        (
            "alarm_disarm".into(),
            json!({"action": "alarm_control_panel.alarm_disarm",
                "target": {"entity_id": ["alarm_control_panel.house"]}, "data": null})
        )
    );
    // Not the block's: an empty code, other data, or an action it doesn't offer.
    for yaml in [
        "action: alarm_control_panel.alarm_disarm\n    target: {entity_id: alarm_control_panel.house}\n    data: {code: ''}",
        "action: alarm_control_panel.alarm_arm_away\n    target: {entity_id: alarm_control_panel.house}\n    data: {code: '1', x: 1}",
        "action: alarm_control_panel.alarm_trigger\n    target: {entity_id: alarm_control_panel.house}",
    ] {
        assert_eq!(action(yaml).0, "ha_call_service", "{yaml}");
    }
}

#[test]
fn button_presses() {
    let press = |yaml: &str| action(yaml).0;
    assert_eq!(press("action: button.press\n    target: {entity_id: button.doorbell}"), "button_press");
    assert_eq!(
        press("action: input_button.press\n    target: {entity_id: [input_button.a, input_button.b]}"),
        "button_press"
    );
    assert_eq!(press("action: button.press\n    target: {area_id: hall}"), "button_press");
    // The block would send these with the other action.
    assert_eq!(press("action: button.press\n    target: {entity_id: input_button.a}"), "ha_call_service");
    assert_eq!(press("action: input_button.press\n    target: {area_id: hall}"), "ha_call_service");
    let (_, call) =
        action("action: input_button.press\n    target: {entity_id: [input_button.a, input_button.b]}");
    assert_eq!(call["action"], "input_button.press");
}

#[test]
fn scene_snapshots() {
    // One entity picker with both: HA splits the text at the commas, as for a list.
    assert_eq!(
        action(
            "action: scene.create\n    data: {scene_id: before_tv, snapshot_entities: [light.a, light.b]}"
        ),
        (
            "scene_remember".into(),
            json!({"action": "scene.create", "target": null,
                "data": {"scene_id": "before_tv", "snapshot_entities": "light.a,light.b"}})
        )
    );
    for yaml in [
        "action: scene.create\n    data: {scene_id: Before TV, snapshot_entities: light.a}",
        "action: scene.create\n    data: {scene_id: x, snapshot_entities: \"{{ lights }}\"}",
        "action: scene.create\n    data: {scene_id: x, entities: {light.a: 'on'}}",
    ] {
        assert_eq!(action(yaml).0, "ha_call_service", "{yaml}");
    }
}

#[test]
fn zone_occupancy_triggers() {
    let trigger = |t: &str| {
        let (ws, p) = import(&format!("alias: t\ntriggers:\n  - {t}\nactions: []\n"));
        let ty = ws
            .pointer("/blocks/blocks/0/inputs/TRIGGERS/block/type")
            .and_then(Value::as_str)
            .unwrap()
            .to_owned();
        let HandlerKind::Triggers { triggers } = &p.handlers[0].kind else { panic!() };
        let Trigger::Ha { config, .. } = &triggers[0] else { panic!() };
        let mut config = config.clone();
        config.as_object_mut().unwrap().remove("id");
        (ty, config)
    };
    assert_eq!(
        trigger("trigger: zone.occupancy_detected\n    options: {zone: zone.home}"),
        (
            "ha_trigger_occupancy".into(),
            json!({"trigger": "zone.occupancy_detected", "options": {"zone": "zone.home"}})
        )
    );
    assert_eq!(
        trigger("trigger: zone.occupancy_cleared\n    options: {zone: zone.home, for: '00:10:00'}"),
        (
            "ha_trigger_occupancy".into(),
            json!({"trigger": "zone.occupancy_cleared",
                "options": {"zone": "zone.home", "for": {"hours": 0, "minutes": 10, "seconds": 0}}})
        )
    );
    // `for: 0` still sets trigger.for; the generic block keeps it.
    assert_eq!(
        trigger("trigger: zone.occupancy_cleared\n    options: {zone: zone.home, for: 0}").0,
        "ha_trigger_integration"
    );
}
