//! The engine's Home Assistant calls against a real Home Assistant with the `demo` integration.
//! Skipped unless HA_WS_URL and HA_TOKEN are set.

use std::time::Duration;

use futures_util::StreamExt;
use puzzlebox_engine::{ConditionError, HaApi, LiveHa};
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
