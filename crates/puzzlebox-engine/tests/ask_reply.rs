//! "Send notification … and wait for a reply": the buttons' action ids, the wait for their
//! `mobile_app_notification_action` event, and "the reply".

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use puzzlebox_engine::{Engine, EngineEvent, HaApi, TriggerStream};
use puzzlebox_storage::{ProgramRecord, Store};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use tokio::time::timeout;

#[derive(Default)]
struct MockHa {
    /// (action, data) of each call.
    calls: Mutex<Vec<(String, Value)>>,
    /// (config, variables) of each trigger subscription.
    triggers: Mutex<Vec<(Value, Value, mpsc::UnboundedSender<Value>)>>,
}

#[async_trait]
impl HaApi for MockHa {
    async fn call_service(
        &self,
        service: &str,
        data: Option<Value>,
        _: Option<Value>,
    ) -> Result<Value, String> {
        self.calls.lock().unwrap().push((service.into(), data.unwrap_or_default()));
        Ok(json!({"context": {}}))
    }
    async fn call_service_response(
        &self,
        s: &str,
        d: Option<Value>,
        t: Option<Value>,
    ) -> Result<Value, String> {
        self.call_service(s, d, t).await
    }
    fn state(&self, _: &str) -> Option<Value> {
        None
    }
    async fn test_condition(&self, _: Value, _: Value) -> Result<bool, String> {
        Ok(true)
    }
    async fn execute_script(&self, _: Value, _: Value) -> Result<Value, String> {
        Ok(Value::Null)
    }
    async fn render_template(&self, t: &str, _: Value) -> Result<Value, String> {
        Ok(json!(t))
    }
    async fn fire_event(&self, _: &str, _: Option<Value>) -> Result<(), String> {
        Ok(())
    }
    async fn subscribe_trigger(&self, trigger: Value) -> Result<TriggerStream, String> {
        self.subscribe_trigger_with(trigger, Value::Null).await
    }
    async fn subscribe_trigger_with(
        &self,
        trigger: Value,
        variables: Value,
    ) -> Result<TriggerStream, String> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.triggers.lock().unwrap().push((trigger, variables, tx));
        Ok(Box::pin(futures_util::stream::unfold(
            rx,
            |mut rx| async move { rx.recv().await.map(|v| (v, rx)) },
        )))
    }
}

fn b(ty: &str, id: &str, fields: Value, inputs: Value) -> Value {
    json!({"type": ty, "id": id, "fields": fields, "inputs": inputs})
}
fn shadow(ty: &str, fields: Value) -> Value {
    json!({"shadow": {"type": ty, "id": format!("s_{ty}"), "fields": fields}})
}

/// Asks, then copies "the reply" into the variable `r`.
fn program(id: &str, seconds: f64) -> ProgramRecord {
    let ask = b(
        "notify_ask",
        "q",
        json!({"TO": "mobile_app_pixel", "BUTTONS": "Yes, No", "LIMIT": "limit"}),
        json!({
            "MESSAGE": shadow("text", json!({"TEXT": "Close the garage?"})),
            "TIMEOUT": shadow("math_number", json!({"NUM": seconds})),
        }),
    );
    let mut body = ask;
    body["next"] = json!({"block": b("variables_set", "s", json!({"VAR": {"id": "r"}}),
        json!({"VALUE": {"block": b("notify_reply", "rep", json!({}), json!({}))}}))});
    let workspace = json!({
        "blocks": {"languageVersion": 0, "blocks": [b("ha_on_run", "run", json!({}), json!({"DO": {"block": body}}))]},
        "variables": [{"name": "r", "id": "r"}],
    });
    ProgramRecord {
        folder: String::new(),
        id: id.into(),
        name: id.into(),
        description: String::new(),
        enabled: true,
        mode: "parallel".into(),
        max_runs: 10,
        workspace,
        created_at: 0,
        updated_at: 0,
    }
}

async fn setup(seconds: f64) -> (Engine, Arc<MockHa>, broadcast::Receiver<EngineEvent>) {
    let ha = Arc::new(MockHa::default());
    let engine = Engine::new(ha.clone(), Store::open_in_memory().unwrap()).await.unwrap();
    let events = engine.subscribe();
    let (_, errors) = engine.save(program("ask", seconds)).await.unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    (engine, ha, events)
}

async fn subscribed(ha: &MockHa, n: usize) {
    timeout(Duration::from_secs(5), async {
        while ha.triggers.lock().unwrap().len() < n {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("never subscribed");
}

/// The final value of `r` of each finished run, in the order they finish.
async fn replies(engine: &Engine, events: &mut broadcast::Receiver<EngineEvent>, n: usize) -> Vec<Value> {
    let mut out = Vec::new();
    while out.len() < n {
        let ev = timeout(Duration::from_secs(5), events.recv()).await.expect("timed out").unwrap();
        if let EngineEvent::RunFinished { run_id, status, error, .. } = ev {
            assert_eq!(status, "ok", "{error:?}");
            let run = engine.store().get_run(&run_id).await.unwrap().unwrap();
            out.push(run.trace.unwrap()["variables"]["r"].clone());
        }
    }
    out
}

/// What HA sends when the trigger with this index fires.
fn tapped(idx: usize, action: &str) -> Value {
    json!({"platform": "event", "idx": idx.to_string(), "id": idx.to_string(),
        "event": {"event_type": "mobile_app_notification_action", "data": {"action": action}}})
}

#[tokio::test]
async fn each_run_asks_with_its_own_ids_and_gets_its_own_reply() {
    let (engine, ha, mut events) = setup(60.0).await;
    assert!(engine.run_now("ask", None, None));
    assert!(engine.run_now("ask", None, None));
    subscribed(&ha, 2).await;

    let calls = ha.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    let subs: Vec<(Value, Value, _)> =
        ha.triggers.lock().unwrap().iter().map(|(t, v, tx)| (t.clone(), v.clone(), tx.clone())).collect();
    let mut ids = Vec::new();
    for ((service, data), (config, vars, _)) in calls.iter().zip(&subs) {
        assert_eq!(service, "notify.mobile_app_pixel");
        assert_eq!(data["message"], "Close the garage?");
        let id = vars["__reply_id"].as_str().unwrap().to_owned();
        assert!(id.starts_with("PUZZLEBOX_") && id.len() > 20, "{id}");
        assert_eq!(
            data["data"]["actions"],
            json!([{"action": format!("{id}_0"), "title": "Yes"}, {"action": format!("{id}_1"), "title": "No"}])
        );
        assert_eq!(config[1]["event_data"]["action"], "{{ __reply_id }}_1");
        ids.push(id);
    }
    assert_ne!(ids[0], ids[1], "concurrent runs must not share action ids");

    // HA only fires a run's own trigger: answer the second run "No", then the first "Yes".
    subs[1].2.send(tapped(1, &format!("{}_1", ids[1]))).unwrap();
    assert_eq!(replies(&engine, &mut events, 1).await, [json!("No")]);
    subs[0].2.send(tapped(0, &format!("{}_0", ids[0]))).unwrap();
    assert_eq!(replies(&engine, &mut events, 1).await, [json!("Yes")]);
}

#[tokio::test]
async fn no_answer_in_time_means_no_reply() {
    let (engine, ha, mut events) = setup(0.05).await;
    assert!(engine.run_now("ask", None, None));
    subscribed(&ha, 1).await;
    assert_eq!(replies(&engine, &mut events, 1).await, [Value::Null]);
}
