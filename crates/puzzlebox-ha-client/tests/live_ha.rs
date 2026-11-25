//! Runs against a real Home Assistant with the `demo` integration loaded.
//! Skipped unless HA_WS_URL and HA_TOKEN are set.

use std::time::Duration;

use puzzlebox_ha_client::{ClientConfig, HaClient, Target};
use serde_json::json;
use tokio::time::timeout;

fn live_client() -> Option<HaClient> {
    let url = std::env::var("HA_WS_URL").ok()?;
    let token = std::env::var("HA_TOKEN").ok()?;
    Some(HaClient::spawn(ClientConfig::new(url, token)))
}

#[tokio::test]
async fn delegates_triggers_conditions_and_actions() {
    let Some(ha) = live_client() else {
        eprintln!("skipping: HA_WS_URL/HA_TOKEN not set");
        return;
    };
    timeout(Duration::from_secs(10), ha.wait_connected()).await.unwrap().unwrap();

    let light = "light.bed_light";
    ha.call_service("light", "turn_off", None, Some(Target::entities([light])), false).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut trig = ha
        .subscribe_trigger(json!({"trigger": "state", "entity_id": light, "to": "on"}), None)
        .await
        .unwrap();
    ha.call_service(
        "light",
        "turn_on",
        Some(json!({"brightness": 120})),
        Some(Target::entities([light])),
        false,
    )
    .await
    .unwrap();
    let ev = timeout(Duration::from_secs(5), trig.next()).await.unwrap().unwrap();
    assert_eq!(ev["variables"]["trigger"]["to_state"]["state"], "on");

    let is_on = ha
        .test_condition(json!({"condition": "state", "entity_id": light, "state": "on"}), None)
        .await
        .unwrap();
    assert!(is_on);

    ha.execute_script(json!([{"action": "light.turn_off", "target": {"entity_id": light}}]), None)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let rendered = ha.render_template(&format!("{{{{ states('{light}') }}}}"), None).await.unwrap();
    assert_eq!(rendered, "off");
}
