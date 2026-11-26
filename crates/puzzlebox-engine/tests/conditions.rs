//! HA conditions that raise count as false (with a warning), like in HA's own scripts.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use puzzlebox_engine::{ConditionError, Engine, EngineEvent, HaApi, TriggerStream};
use puzzlebox_storage::{ProgramRecord, Store};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio::time::timeout;

/// Answers conditions by the entity: `light.on` is true, `sensor.gone` raises, `net.down` fails.
#[derive(Default)]
struct Ha {
    calls: Mutex<Vec<String>>,
}

#[async_trait]
impl HaApi for Ha {
    async fn call_service(&self, s: &str, _: Option<Value>, _: Option<Value>) -> Result<Value, String> {
        self.calls.lock().unwrap().push(s.into());
        Ok(Value::Null)
    }
    fn state(&self, _: &str) -> Option<Value> {
        None
    }
    async fn test_condition(&self, _: Value, _: Value) -> Result<bool, String> {
        unreachable!("the engine uses check_condition")
    }
    async fn check_condition(&self, c: Value, _: Value) -> Result<bool, ConditionError> {
        match c["entity_id"].as_str() {
            Some("light.on") => Ok(true),
            Some("sensor.gone") => {
                Err(ConditionError::Raised("In 'state' condition: unknown entity sensor.gone".into()))
            }
            _ => Err(ConditionError::Failed("not connected to Home Assistant".into())),
        }
    }
    async fn execute_script(&self, _: Value, _: Value) -> Result<Value, String> {
        Ok(Value::Null)
    }
    async fn render_template(&self, _: &str, _: Value) -> Result<Value, String> {
        Ok(Value::Null)
    }
    async fn fire_event(&self, _: &str, _: Option<Value>) -> Result<(), String> {
        Ok(())
    }
    async fn subscribe_trigger(&self, _: Value) -> Result<TriggerStream, String> {
        Ok(Box::pin(futures_util::stream::pending()))
    }
}

fn is_on(id: &str, entity: &str) -> Value {
    json!({"type": "ha_is_state", "id": id, "fields": {"STATE": "on"},
        "inputs": {"ENTITY": {"shadow": {"type": "ha_entity", "id": format!("{id}e"), "fields": {"ENTITY": entity}}}}})
}

/// if <cond> then perform `then` else `otherwise`.
fn if_else(id: &str, cond: Value, then: &str, otherwise: &str) -> Value {
    let call = |id: String, s: &str| json!({"block": {"type": "ha_call_service", "id": id, "fields": {"SERVICE": s}}});
    json!({"type": "controls_if", "id": id, "extraState": {"hasElse": true}, "inputs": {
        "IF0": {"block": cond}, "DO0": call(format!("{id}t"), then), "ELSE": call(format!("{id}f"), otherwise)}})
}

fn program(body: Value) -> ProgramRecord {
    ProgramRecord {
        folder: String::new(),
        id: "p".into(),
        name: "p".into(),
        description: String::new(),
        enabled: true,
        mode: "single".into(),
        max_runs: 10,
        workspace: json!({"blocks": {"blocks": [{"type": "ha_on_run", "id": "run", "inputs": {"DO": {"block": body}}}]}}),
        created_at: 0,
        updated_at: 0,
    }
}

/// Runs the program; returns its error (if any) and the warnings it logged.
async fn run(
    engine: &Engine,
    events: &mut broadcast::Receiver<EngineEvent>,
) -> (Option<String>, Vec<String>) {
    assert!(engine.run_now("p", None, None));
    let mut warnings = Vec::new();
    loop {
        match timeout(Duration::from_secs(5), events.recv()).await.expect("timed out").unwrap() {
            EngineEvent::Log { message, .. } => warnings.push(message),
            EngineEvent::RunFinished { error, .. } => return (error, warnings),
            _ => {}
        }
    }
}

#[tokio::test]
async fn raised_conditions_count_as_false() {
    let ha = Arc::new(Ha::default());
    let engine = Engine::new(ha.clone(), Store::open_in_memory().unwrap()).await.unwrap();
    let mut events = engine.subscribe();

    // A raising condition is false, also inside "or" and "not"; the run carries on.
    let or = json!({"type": "logic_operation", "id": "or", "fields": {"OP": "OR"},
        "inputs": {"A": {"block": is_on("a", "sensor.gone")}, "B": {"block": is_on("b", "light.on")}}});
    let not = json!({"type": "logic_negate", "id": "not", "inputs": {"BOOL": {"block": is_on("c", "sensor.gone")}}});
    let mut body = if_else("if1", is_on("s", "sensor.gone"), "x.then", "x.else");
    let mut second = if_else("if2", or, "or.then", "or.else");
    second["next"] = json!({"block": if_else("if3", not, "not.then", "not.else")});
    body["next"] = json!({"block": second});
    engine.save(program(body)).await.unwrap();
    let (error, warnings) = run(&engine, &mut events).await;
    assert_eq!(error, None);
    assert_eq!(*ha.calls.lock().unwrap(), ["x.else", "or.then", "not.then"]);
    assert_eq!(warnings.len(), 3, "{warnings:?}");
    assert!(warnings[0].contains("unknown entity sensor.gone"), "{warnings:?}");

    let runs = engine.store().list_runs("p", 1).await.unwrap();
    let trace = engine.store().get_run(&runs[0].id).await.unwrap().unwrap().trace.unwrap();
    let flagged: Vec<&Value> =
        trace["steps"].as_array().unwrap().iter().filter(|s| !s["error"].is_null()).collect();
    assert_eq!(flagged[0]["block"], "s");

    // Checked again and again (a loop, "wait until"): one warning per run.
    let repeat = json!({"type": "controls_repeat_ext", "id": "rep", "inputs": {
        "TIMES": {"shadow": {"type": "math_number", "id": "n", "fields": {"NUM": 3}}},
        "DO": {"block": if_else("if", is_on("s", "sensor.gone"), "x.then", "x.else")}}});
    engine.save(program(repeat)).await.unwrap();
    let (error, warnings) = run(&engine, &mut events).await;
    assert_eq!((error, warnings.len()), (None, 1));

    // Other failures (here: no connection) still stop the run.
    ha.calls.lock().unwrap().clear();
    engine.save(program(if_else("if", is_on("s", "net.down"), "x.then", "x.else"))).await.unwrap();
    let (error, _) = run(&engine, &mut events).await;
    assert!(error.is_some_and(|e| e.contains("not connected")));
    assert!(ha.calls.lock().unwrap().is_empty());
}
