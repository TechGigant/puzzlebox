//! Presence, remember/restore, alarm, button and "ask on the phone" blocks: the exact Home
//! Assistant configs they send.

use puzzlebox_ir::*;
use serde_json::{Value, json};

fn block(ty: &str, id: &str, fields: Value, inputs: Value) -> Value {
    json!({"type": ty, "id": id, "fields": fields, "inputs": inputs})
}

fn shadow(ty: &str, fields: Value) -> Value {
    json!({"shadow": {"type": ty, "id": format!("s_{ty}"), "fields": fields}})
}

fn entity(e: &str) -> Value {
    shadow("ha_entity", json!({"ENTITY": e}))
}

fn text(s: &str) -> Value {
    shadow("text", json!({"TEXT": s}))
}

fn num(n: f64) -> Value {
    shadow("math_number", json!({"NUM": n}))
}

fn run(body: Value) -> Value {
    json!({"blocks": {"blocks": [{"type": "ha_on_run", "id": "r", "inputs": {"DO": {"block": body}}}]}})
}

fn when(trigger: Value) -> Value {
    json!({"blocks": {"blocks": [{"type": "ha_when", "id": "w", "inputs": {"TRIGGERS": {"block": trigger}}}]}})
}

fn errors(ws: &Value) -> Vec<String> {
    compile(ws, &NoExternals).unwrap_err().into_iter().map(|e| e.message).collect()
}

fn body(ws: &Value) -> Vec<Stmt> {
    let p = compile(ws, &NoExternals).unwrap_or_else(|e| panic!("{e:?}"));
    p.handlers[0].body.clone()
}

/// Literals, lists and dicts of them as JSON; anything else as `{"expr": kind}`.
fn plain(e: &Expr) -> Value {
    match &e.kind {
        ExprKind::Lit { value } => value.clone(),
        ExprKind::List { items } => items.iter().map(plain).collect(),
        ExprKind::Dict { entries } => entries.iter().map(|(k, v)| (k.clone(), plain(v))).collect(),
        other => json!({"expr": other}),
    }
}

/// (service, target, data) of a single action call.
fn call(s: &Stmt) -> (String, Value, Value) {
    let StmtKind::CallService { service, target, data, .. } = &s.kind else { panic!("not a call: {s:?}") };
    let p = |e: &Option<Expr>| e.as_ref().map_or(Value::Null, plain);
    (service.clone(), p(target), p(data))
}

#[test]
fn first_and_last_person_use_the_zone_occupancy_triggers() {
    let p = compile(
        &when(block(
            "ha_trigger_occupancy",
            "t",
            json!({"EVENT": "enter", "ZONE": "zone.home"}),
            json!({"FOR": num(0.0)}),
        )),
        &NoExternals,
    )
    .unwrap();
    let HandlerKind::Triggers { triggers } = &p.handlers[0].kind else { panic!() };
    let Trigger::Ha { config, .. } = &triggers[0] else { panic!() };
    assert_eq!(
        config,
        &json!({"trigger": "zone.occupancy_detected", "options": {"zone": "zone.home"}, "id": "t"})
    );

    let p = compile(
        &when(block(
            "ha_trigger_occupancy",
            "t",
            json!({"EVENT": "leave", "ZONE": "zone.work"}),
            json!({"FOR": num(600.0)}),
        )),
        &NoExternals,
    )
    .unwrap();
    let HandlerKind::Triggers { triggers } = &p.handlers[0].kind else { panic!() };
    let Trigger::Ha { config, .. } = &triggers[0] else { panic!() };
    assert_eq!(
        config,
        &json!({"trigger": "zone.occupancy_cleared", "id": "t",
            "options": {"zone": "zone.work", "for": {"hours": 0, "minutes": 10, "seconds": 0}}})
    );
}

#[test]
fn remember_and_restore_are_a_scene_snapshot() {
    let list = json!({"block": {"type": "lists_create_with", "id": "l", "extraState": {"itemCount": 2},
        "inputs": {"ADD0": entity("light.a"), "ADD1": entity("light.b")}}});
    let mut remember =
        block("scene_remember", "m", json!({"NAME": " Before the Movie! "}), json!({"TARGET": list}));
    remember["next"] =
        json!({"block": block("scene_restore", "x", json!({"NAME": "before the movie"}), json!({}))});
    let b = body(&run(remember));
    assert_eq!(
        call(&b[0]),
        (
            "scene.create".into(),
            Value::Null,
            json!({"scene_id": "before_the_movie", "snapshot_entities": ["light.a", "light.b"]})
        )
    );
    assert_eq!(
        call(&b[1]),
        ("scene.turn_on".into(), json!({"entity_id": "scene.before_the_movie"}), Value::Null)
    );

    let area = json!({"block": {"type": "ha_area", "id": "a", "fields": {"ID": "kitchen"}}});
    let ws = run(block("scene_remember", "m", json!({"NAME": "x"}), json!({"TARGET": area})));
    assert_eq!(
        errors(&ws),
        ["areas, floors, labels and devices can't be remembered; choose entities (or use \"entities in …\")"]
    );
    let ws = run(block("scene_remember", "m", json!({"NAME": "!!"}), json!({"TARGET": entity("light.a")})));
    assert_eq!(errors(&ws), ["give the remembered state a name"]);
}

#[test]
fn alarm_actions_and_condition() {
    let arm = block(
        "alarm_arm",
        "a",
        json!({"MODE": "night"}),
        json!({"TARGET": entity("alarm_control_panel.house"), "CODE": text("1234")}),
    );
    assert_eq!(
        call(&body(&run(arm))[0]),
        (
            "alarm_control_panel.alarm_arm_night".into(),
            json!("alarm_control_panel.house"),
            json!({"code": "1234"})
        )
    );
    let disarm = block(
        "alarm_disarm",
        "d",
        json!({}),
        json!({"TARGET": entity("alarm_control_panel.house"), "CODE": text("")}),
    );
    assert_eq!(
        call(&body(&run(disarm))[0]),
        ("alarm_control_panel.alarm_disarm".into(), json!("alarm_control_panel.house"), Value::Null)
    );

    let cond = |state: &str| {
        let is = block(
            "alarm_is",
            "c",
            json!({"STATE": state}),
            json!({"ENTITY": entity("alarm_control_panel.house")}),
        );
        let ws = run(block("controls_if", "if", json!({}), json!({"IF0": {"block": is}})));
        let StmtKind::If { branches, .. } = &body(&ws)[0].kind else { panic!() };
        let ExprKind::Condition { config, .. } = &branches[0].0.kind else { panic!() };
        config["state"].clone()
    };
    assert_eq!(
        cond("armed"),
        json!(["armed_away", "armed_home", "armed_night", "armed_vacation", "armed_custom_bypass"])
    );
    assert_eq!(cond("triggered"), json!("triggered"));
}

#[test]
fn press_picks_the_action_from_the_domain() {
    let press =
        |e: &str| call(&body(&run(block("button_press", "p", json!({}), json!({"TARGET": entity(e)}))))[0]).0;
    assert_eq!(press("button.doorbell"), "button.press");
    assert_eq!(press("input_button.reset, input_button.go"), "input_button.press");
    let ws =
        run(block("button_press", "p", json!({}), json!({"TARGET": entity("button.a, input_button.b")})));
    assert_eq!(errors(&ws), ["press buttons and button helpers (input_button) with separate blocks"]);
}

#[test]
fn ask_sends_actions_and_waits_for_their_event() {
    let ask = block(
        "notify_ask",
        "q",
        json!({"TO": "mobile_app_pixel", "BUTTONS": "Close it, Leave it open,", "LIMIT": "limit"}),
        json!({"MESSAGE": text("Garage open"), "TITLE": text("Home"), "TIMEOUT": num(600.0)}),
    );
    let b = body(&run(ask));
    let StmtKind::If { branches, .. } = &b[0].kind else { panic!() };
    let steps = &branches[0].1;
    let StmtKind::Set { name, value, .. } = &steps[0].kind else { panic!() };
    assert_eq!(name, "__reply_id");
    assert!(matches!(&value.kind, ExprKind::Builtin { name, .. } if name == "text_join"));

    let (service, target, data) = call(&steps[1]);
    assert_eq!((service.as_str(), target), ("notify.mobile_app_pixel", Value::Null));
    assert_eq!((&data["message"], &data["title"]), (&json!("Garage open"), &json!("Home")));
    let actions = data["data"]["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[1]["title"], "Leave it open");

    let StmtKind::WaitForTrigger { triggers, timeout, .. } = &steps[2].kind else { panic!() };
    assert_eq!(timeout.as_ref().map(plain), Some(json!(600)));
    assert_eq!(
        triggers,
        &[
            json!({"trigger": "event", "event_type": "mobile_app_notification_action",
                "event_data": {"action": "{{ __reply_id }}_0"}}),
            json!({"trigger": "event", "event_type": "mobile_app_notification_action",
                "event_data": {"action": "{{ __reply_id }}_1"}}),
        ]
    );
    let StmtKind::Set { name, .. } = &steps[3].kind else { panic!() };
    assert_eq!(name, "__reply");

    let no_phone = block(
        "notify_ask",
        "q",
        json!({"TO": "persistent", "BUTTONS": " , "}),
        json!({"MESSAGE": text("x")}),
    );
    assert_eq!(
        errors(&run(no_phone)),
        [
            "choose a phone with the Home Assistant app (notify.mobile_app_…); only it can show buttons",
            "add at least one button, e.g. \"Yes, No\""
        ]
    );
}
