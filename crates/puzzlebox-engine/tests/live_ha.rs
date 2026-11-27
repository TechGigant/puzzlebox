//! The engine's Home Assistant calls against a real Home Assistant with the `demo` integration.
//! Skipped unless HA_WS_URL and HA_TOKEN are set.

use std::time::Duration;

use futures_util::StreamExt;
use puzzlebox_engine::{ConditionError, HaApi, LiveHa, OnError};
use puzzlebox_ha_client::{ClientConfig, HaClient};
use serde_json::{Value, json};
use tokio::time::timeout;

const LIGHT: &str = "light.bed_light";

async fn live() -> Option<std::sync::Arc<LiveHa>> {
    let url = std::env::var("HA_WS_URL").ok()?;
    let token = std::env::var("HA_TOKEN").ok()?;
    let client = HaClient::spawn(ClientConfig::new(url, token));
    timeout(Duration::from_secs(10), client.wait_connected()).await.unwrap().unwrap();
    Some(LiveHa::new(client))
}

async fn light(ha: &LiveHa, on: bool) {
    let svc = if on { "light.turn_on" } else { "light.turn_off" };
    ha.call_service(svc, None, Some(json!({"entity_id": LIGHT}))).await.unwrap();
}

#[tokio::test]
async fn script_features_work_against_home_assistant() {
    let Some(ha) = live().await else {
        eprintln!("skipping: HA_WS_URL/HA_TOKEN not set");
        return;
    };
    light(&ha, false).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // A condition that raises while HA checks it counts as false; a bad config still fails.
    let raised = ha
        .check_condition(
            json!({"condition": "numeric_state", "entity_id": "sensor.does_not_exist", "above": 1}),
            json!({}),
        )
        .await;
    assert!(matches!(raised, Err(ConditionError::Raised(_))), "{raised:?}");
    let invalid = ha.check_condition(json!({"condition": "no_such_condition"}), json!({})).await;
    assert!(matches!(invalid, Err(ConditionError::Failed(_))), "{invalid:?}");

    // Action responses.
    let response =
        ha.call_service_response("conversation.process", Some(json!({"text": "hello"})), None).await.unwrap();
    assert!(response.get("response").is_some(), "{response}");

    // Trigger templates see the run's variables, and HA numbers a list of triggers.
    let mut trig = ha
        .subscribe_trigger_with(
            json!([
                {"trigger": "event", "event_type": "puzzlebox_never_fired"},
                {"trigger": "template", "value_template": "{{ is_state(lamp, 'on') }}"},
            ]),
            json!({"lamp": LIGHT}),
        )
        .await
        .unwrap();
    // `wait_template`: the result again each time it changes.
    let mut tpl = ha
        .subscribe_template("{{ is_state(lamp, 'on') }}", json!({"lamp": LIGHT}))
        .await
        .expect("templates can be followed")
        .unwrap();
    assert_eq!(timeout(Duration::from_secs(5), tpl.next()).await.unwrap().unwrap().unwrap(), json!(false));

    light(&ha, true).await;
    let fired: Value = timeout(Duration::from_secs(5), trig.next()).await.unwrap().unwrap();
    assert_eq!(fired["idx"], "1", "{fired}");
    assert_eq!(fired["platform"], "template");
    assert_eq!(timeout(Duration::from_secs(5), tpl.next()).await.unwrap().unwrap().unwrap(), json!(true));

    light(&ha, false).await;
}

/// Each action, whether HA's own `continue_on_error` skips its error, and how the engine sorts
/// the error the WebSocket API gives for it.
#[tokio::test]
async fn action_errors_are_sorted_like_continue_on_error() {
    let Some(ha) = live().await else {
        eprintln!("skipping: HA_WS_URL/HA_TOKEN not set");
        return;
    };
    let cases = [
        // ServiceValidationError
        (
            json!({"action": "climate.set_temperature", "data": {"temperature": 1000}, "target": {"entity_id": "climate.hvac"}}),
            true,
        ),
        // ServiceNotSupported
        (
            json!({"action": "climate.set_humidity", "data": {"humidity": 50}, "target": {"entity_id": "climate.ecobee"}}),
            true,
        ),
        // HomeAssistantError
        (
            json!({"action": "homeassistant.reload_config_entry", "data": {"entry_id": "puzzlebox_none"}}),
            true,
        ),
        // ServiceNotFound
        (json!({"action": "light.puzzlebox_none", "target": {"entity_id": LIGHT}}), false),
        // vol.Invalid
        (
            json!({"action": "light.turn_on", "data": {"brightness": "abc"}, "target": {"entity_id": LIGHT}}),
            false,
        ),
    ];
    for (action, skipped) in cases {
        let mut flagged = action.clone();
        flagged["continue_on_error"] = json!(true);
        let native = ha.execute_script(json!([flagged]), json!({})).await;
        assert_eq!(native.is_ok(), skipped, "HA itself: {action} {native:?}");

        let service = action["action"].as_str().unwrap();
        let called = ha
            .call_service_checked(service, action.get("data").cloned(), action.get("target").cloned(), false)
            .await;
        let want = if skipped { OnError::Skip } else { OnError::Stop };
        assert_eq!(called.unwrap_err().on_error, want, "call_service: {action}");
        let script = ha.execute_script_checked(json!([action]), json!({})).await;
        assert_eq!(script.unwrap_err().on_error, want, "execute_script: {action}");
    }

    // A response asked for wrongly: the script refuses it as invalid.
    let native = ha
        .execute_script(json!([{"action": "light.turn_on", "target": {"entity_id": LIGHT}, "response_variable": "r", "continue_on_error": true}]), json!({}))
        .await;
    assert!(native.is_err());
    let called =
        ha.call_service_checked("light.turn_on", None, Some(json!({"entity_id": LIGHT})), true).await;
    assert_eq!(called.unwrap_err().on_error, OnError::Stop);
    let called = ha
        .call_service_checked(
            "weather.get_forecasts",
            Some(json!({"type": "daily"})),
            Some(json!({"entity_id": "weather.demo_weather_south"})),
            false,
        )
        .await;
    assert_eq!(called.unwrap_err().on_error, OnError::Stop);

    // A data template that fails is a HomeAssistantError to the script, skipped too.
    let native = ha
        .execute_script(
            json!([{"action": "light.turn_on", "data": {"brightness": "{{ x.y }}"}, "target": {"entity_id": LIGHT}, "continue_on_error": true}]),
            json!({}),
        )
        .await;
    assert!(native.is_ok(), "{native:?}");
    light(&ha, false).await;
}

/// What HA's waits set `wait` to with a timeout of 0.
#[tokio::test]
async fn waits_with_a_timeout_of_zero() {
    let Some(ha) = live().await else {
        eprintln!("skipping: HA_WS_URL/HA_TOKEN not set");
        return;
    };
    let wait = |step: Value| {
        let ha = ha.clone();
        async move {
            let res = ha
                .execute_script(json!([step, {"stop": "done", "response_variable": "wait"}]), json!({}))
                .await;
            res.unwrap()["response"].clone()
        }
    };
    let trig = json!({"wait_for_trigger": [{"trigger": "event", "event_type": "puzzlebox_never_fired"}], "timeout": 0});
    assert_eq!(wait(trig).await, json!({"remaining": 0.0, "completed": false, "trigger": null}));
    let yes = json!({"wait_template": "{{ true }}", "timeout": 0});
    assert_eq!(wait(yes).await, json!({"remaining": 0.0, "completed": true}));
    let no = json!({"wait_template": "{{ false }}", "timeout": 0});
    assert_eq!(wait(no).await, json!({"remaining": 0.0, "completed": false}));
}

/// The statements a block compiles to, run in `ha_on_run`.
fn compiled(body: Value) -> Vec<puzzlebox_ir::Stmt> {
    let ws =
        json!({"blocks": {"blocks": [{"type": "ha_on_run", "id": "r", "inputs": {"DO": {"block": body}}}]}});
    puzzlebox_ir::compile(&ws, &puzzlebox_ir::NoExternals).unwrap().handlers.remove(0).body
}

/// (action, data, target) of a block that compiles to one fixed action call.
fn compiled_call(body: Value) -> (String, Option<Value>, Option<Value>) {
    let stmts = compiled(body);
    let puzzlebox_ir::StmtKind::CallService { service, target, data, .. } = &stmts[0].kind else { panic!() };
    let target = target.as_ref().map(|t| puzzlebox_ir::normalize_target(&fixed(t)).unwrap());
    (service.clone(), data.as_ref().map(fixed), target)
}

fn fixed(e: &puzzlebox_ir::Expr) -> Value {
    use puzzlebox_ir::ExprKind;
    match &e.kind {
        ExprKind::Lit { value } => value.clone(),
        ExprKind::List { items } => items.iter().map(fixed).collect(),
        ExprKind::Dict { entries } => entries.iter().map(|(k, v)| (k.clone(), fixed(v))).collect(),
        other => panic!("not fixed: {other:?}"),
    }
}

async fn render(ha: &LiveHa, t: &str) -> String {
    ha.render_template(t, json!({})).await.unwrap().as_str().map(str::to_owned).unwrap_or_default()
}

/// "remember the state of … as" and "restore" put a light back the way it was, and the
/// alarm blocks arm and disarm the demo alarm with its code.
#[tokio::test]
async fn remember_restore_and_alarm_blocks() {
    let Some(ha) = live().await else {
        eprintln!("skipping: HA_WS_URL/HA_TOKEN not set");
        return;
    };
    let entity = |e: &str| json!({"shadow": {"type": "ha_entity", "id": "e", "fields": {"ENTITY": e}}});
    // Not the light the other tests switch.
    let lamp = "light.office_rgbw_lights";
    let text = |s: &str| json!({"shadow": {"type": "text", "id": "t", "fields": {"TEXT": s}}});

    ha.call_service("light.turn_on", Some(json!({"brightness": 100})), Some(json!({"entity_id": lamp})))
        .await
        .unwrap();
    let (svc, data, target) = compiled_call(json!({"type": "scene_remember", "id": "m",
        "fields": {"NAME": "Puzzlebox live test"}, "inputs": {"TARGET": entity(lamp)}}));
    ha.call_service_checked(&svc, data, target, false).await.unwrap();
    ha.call_service("light.turn_off", None, Some(json!({"entity_id": lamp}))).await.unwrap();
    let (svc, data, target) =
        compiled_call(json!({"type": "scene_restore", "id": "x", "fields": {"NAME": "puzzlebox live test"}}));
    ha.call_service_checked(&svc, data, target, false).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let now = render(
        &ha,
        "{{ states('light.office_rgbw_lights') }} {{ state_attr('light.office_rgbw_lights', 'brightness') }}",
    )
    .await;
    assert_eq!(now, "on 100");

    let alarm = |ty: &str, code: &str| {
        compiled_call(json!({"type": ty, "id": "a", "fields": {"MODE": "away"},
            "inputs": {"TARGET": entity("alarm_control_panel.security"), "CODE": text(code)}}))
    };
    let state = || render(&ha, "{{ states('alarm_control_panel.security') }}");
    let (svc, data, target) = alarm("alarm_arm", "1234");
    ha.call_service_checked(&svc, data, target, false).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(state().await, "arming");
    let (svc, data, target) = alarm("alarm_disarm", "0000");
    assert!(ha.call_service_checked(&svc, data, target, false).await.is_err(), "wrong code");
    let (svc, data, target) = alarm("alarm_disarm", "1234");
    ha.call_service_checked(&svc, data, target, false).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(state().await, "disarmed");
}

/// The reply wait only ends for this question's buttons, and says which one.
#[tokio::test]
async fn reply_wait_matches_only_its_own_action_ids() {
    let Some(ha) = live().await else {
        eprintln!("skipping: HA_WS_URL/HA_TOKEN not set");
        return;
    };
    let stmts = compiled(json!({"type": "notify_ask", "id": "q",
        "fields": {"TO": "mobile_app_x", "BUTTONS": "Yes, No", "LIMIT": "none"},
        "inputs": {"MESSAGE": {"shadow": {"type": "text", "id": "m", "fields": {"TEXT": "?"}}}}}));
    let puzzlebox_ir::StmtKind::If { branches, .. } = &stmts[0].kind else { panic!() };
    let puzzlebox_ir::StmtKind::WaitForTrigger { triggers, .. } = &branches[0].1[2].kind else { panic!() };
    let mut fired = ha
        .subscribe_trigger_with(Value::Array(triggers.clone()), json!({"__reply_id": "PUZZLEBOX_00ff"}))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let tap = |action: &str| Some(json!({"action": action}));
    ha.fire_event("mobile_app_notification_action", tap("PUZZLEBOX_abcd_1")).await.unwrap();
    ha.fire_event("mobile_app_notification_action", tap("PUZZLEBOX_00ff_1")).await.unwrap();
    let got: Value = timeout(Duration::from_secs(5), fired.next()).await.unwrap().unwrap();
    assert_eq!(got["idx"], "1", "{got}");
    assert_eq!(got["event"]["data"]["action"], "PUZZLEBOX_00ff_1");
}
