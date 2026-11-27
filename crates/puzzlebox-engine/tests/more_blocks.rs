//! Lists changed in place, "run program" and fading lights.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use puzzlebox_engine::{Engine, EngineEvent, HaApi, TriggerStream};
use puzzlebox_storage::{ProgramRecord, Store};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio::time::timeout;

type Call = (String, Option<Value>, Option<Value>);

#[derive(Default)]
struct MockHa {
    calls: Mutex<Vec<Call>>,
    states: Mutex<HashMap<String, Value>>,
}

impl MockHa {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().iter().map(|c| c.0.clone()).collect()
    }
}

#[async_trait]
impl HaApi for MockHa {
    async fn call_service(
        &self,
        service: &str,
        data: Option<Value>,
        target: Option<Value>,
    ) -> Result<Value, String> {
        if service == "fail.now" {
            return Err("boom".into());
        }
        self.calls.lock().unwrap().push((service.into(), data, target));
        Ok(Value::Null)
    }
    fn state(&self, entity_id: &str) -> Option<Value> {
        self.states.lock().unwrap().get(entity_id).cloned()
    }
    async fn test_condition(&self, _c: Value, _v: Value) -> Result<bool, String> {
        Ok(false)
    }
    async fn execute_script(&self, _s: Value, _v: Value) -> Result<Value, String> {
        Ok(Value::Null)
    }
    async fn render_template(&self, t: &str, _v: Value) -> Result<Value, String> {
        Ok(json!(t))
    }
    async fn fire_event(&self, _t: &str, _d: Option<Value>) -> Result<(), String> {
        Ok(())
    }
    async fn subscribe_trigger(&self, _trigger: Value) -> Result<TriggerStream, String> {
        Ok(Box::pin(futures_util::stream::pending()))
    }
}

fn b(ty: &str, id: &str, fields: Value, inputs: Value) -> Value {
    json!({"type": ty, "id": id, "fields": fields, "inputs": inputs})
}
fn blk(v: Value) -> Value {
    json!({"block": v})
}
fn chain(mut blocks: Vec<Value>) -> Value {
    let mut next: Option<Value> = None;
    while let Some(mut cur) = blocks.pop() {
        if let Some(n) = next.take() {
            cur["next"] = blk(n);
        }
        next = Some(cur);
    }
    next.unwrap_or(Value::Null)
}
fn num(id: &str, n: f64) -> Value {
    b("math_number", id, json!({"NUM": n}), json!({}))
}
fn text(id: &str, s: &str) -> Value {
    b("text", id, json!({"TEXT": s}), json!({}))
}
fn call(id: &str, service: &str) -> Value {
    b(
        "ha_call_service",
        id,
        json!({"SERVICE": service}),
        json!({"TARGET": blk(text(&format!("{id}t"), "light.a"))}),
    )
}
fn on_run(body: Value) -> Value {
    b("ha_on_run", "run", json!({}), json!({"DO": blk(body)}))
}
fn run_program(id: &str, program: &str, wait: bool) -> Value {
    let ty = if wait { "program_run_wait" } else { "program_run" };
    b(ty, id, json!({"PROGRAM": program}), json!({}))
}
fn program(id: &str, body: Value, vars: Value, mode: &str) -> ProgramRecord {
    ProgramRecord {
        folder: String::new(),
        id: id.into(),
        name: format!("Program {id}"),
        description: String::new(),
        enabled: true,
        mode: mode.into(),
        max_runs: 50,
        workspace: json!({"blocks": {"languageVersion": 0, "blocks": [on_run(body)]}, "variables": vars}),
        created_at: 0,
        updated_at: 0,
    }
}

async fn setup() -> (Engine, Arc<MockHa>) {
    let ha = Arc::new(MockHa::default());
    let engine = Engine::new(ha.clone(), Store::open_in_memory().unwrap()).await.unwrap();
    (engine, ha)
}

/// The next run of `program` to finish: (status, error).
async fn finished(rx: &mut broadcast::Receiver<EngineEvent>, program: &str) -> (String, Option<String>) {
    loop {
        let ev = timeout(Duration::from_secs(10), rx.recv()).await.expect("timed out").unwrap();
        if let EngineEvent::RunFinished { program_id, status, error, .. } = ev
            && program_id == program
        {
            return (status, error);
        }
    }
}

#[tokio::test]
async fn lists_add_remove_and_contains() {
    let (engine, ha) = setup().await;
    let mut events = engine.subscribe();
    let add =
        |id: &str, item: Value| b("lists_add", id, json!({"VAR": {"id": "v"}}), json!({"ITEM": blk(item)}));
    let list = b("variables_get", "get", json!({"VAR": {"id": "v"}}), json!({}));
    let contains = |id: &str, item: Value| {
        b("lists_contains", id, json!({}), json!({"LIST": blk(list.clone()), "ITEM": blk(item)}))
    };
    let if_then = |id: &str, cond: Value, service: &str| {
        b(
            "controls_if",
            id,
            json!({}),
            json!({"IF0": blk(cond), "DO0": blk(call(&format!("{id}c"), service))}),
        )
    };
    let body = chain(vec![
        add("a1", text("t1", "kitchen")),
        add("a2", num("n1", 5.0)),
        add("a3", text("t2", "kitchen")),
        add("a4", text("t3", "hall")),
        if_then("i1", contains("c1", text("t4", "5")), "yes.five"),
        b(
            "lists_remove_value",
            "r1",
            json!({"VAR": {"id": "v"}}),
            json!({"ITEM": blk(text("t5", "kitchen"))}),
        ),
        if_then("i2", contains("c2", text("t6", "kitchen")), "still.kitchen"),
    ]);
    let vars = json!([{"name": "rooms", "id": "v", "type": "Program"}]);
    let (_, errors) = engine.save(program("l", body, vars, "single")).await.unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(engine.run_now("l", None, None));
    assert_eq!(finished(&mut events, "l").await, ("ok".into(), None));
    assert_eq!(ha.calls(), ["yes.five"]);
    assert_eq!(engine.store().kv_all("program:l").await.unwrap()["rooms"], json!([5, "hall"]));
}

#[tokio::test]
async fn run_program_and_wait() {
    let (engine, ha) = setup().await;
    let mut events = engine.subscribe();
    let child = chain(vec![
        b("ha_wait", "w", json!({}), json!({"SECONDS": blk(num("n", 0.2))})),
        call("c", "child.done"),
    ]);
    engine.save(program("child", child, json!([]), "single")).await.unwrap();
    let parent = chain(vec![run_program("r", "child", true), call("p", "parent.after")]);
    engine.save(program("parent", parent, json!([]), "single")).await.unwrap();
    assert!(engine.run_now("parent", None, None));
    assert_eq!(finished(&mut events, "parent").await, ("ok".into(), None));
    assert_eq!(ha.calls(), ["child.done", "parent.after"]);

    // Without waiting, the caller carries on at once.
    let starter = chain(vec![run_program("r", "child", false), call("p", "starter.after")]);
    engine.save(program("starter", starter, json!([]), "single")).await.unwrap();
    assert!(engine.run_now("starter", None, None));
    finished(&mut events, "starter").await;
    finished(&mut events, "child").await;
    assert_eq!(ha.calls()[2..], ["starter.after", "child.done"]);
}

#[tokio::test]
async fn run_program_errors_say_why() {
    let (engine, _ha) = setup().await;
    let mut events = engine.subscribe();
    engine.save(program("bad", call("c", "fail.now"), json!([]), "single")).await.unwrap();
    let mut off = program("off", call("c", "x.y"), json!([]), "single");
    off.enabled = false;
    engine.save(off).await.unwrap();

    for (target, expected) in [
        ("bad", "“Program bad” failed: boom"),
        ("off", "“Program off” is turned off"),
        ("gone", "the program to run was deleted"),
        ("caller", "can't wait for “Program caller”"),
    ] {
        let body = run_program("r", target, true);
        engine.save(program("caller", body, json!([]), "single")).await.unwrap();
        assert!(engine.run_now("caller", None, None));
        let (status, error) = finished(&mut events, "caller").await;
        assert_eq!(status, "error");
        assert!(error.as_deref().unwrap_or("").contains(expected), "{target}: {error:?}");
    }
}

#[tokio::test]
async fn a_program_starting_itself_stops_after_ten() {
    let (engine, ha) = setup().await;
    let mut events = engine.subscribe();
    let body = chain(vec![call("c", "loop.round"), run_program("r", "loop", false)]);
    engine.save(program("loop", body, json!([]), "parallel")).await.unwrap();
    assert!(engine.run_now("loop", None, None));
    let mut errors = vec![];
    for _ in 0..11 {
        if let (s, Some(e)) = finished(&mut events, "loop").await {
            assert_eq!(s, "error");
            errors.push(e);
        }
    }
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("more than 10 deep"), "{errors:?}");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(ha.calls().len(), 11);
}

fn fade(seconds: f64) -> Value {
    b(
        "light_fade",
        "f",
        json!({}),
        json!({
            "TARGET": blk(text("ft", "light.bed")),
            "FROM": blk(num("n1", 10.0)),
            "TO": blk(num("n2", 100.0)),
            "SECONDS": blk(num("n3", seconds)),
        }),
    )
}

#[tokio::test]
async fn fade_steps_and_stops_when_changed_by_hand() {
    let (engine, ha) = setup().await;
    let mut events = engine.subscribe();
    engine
        .save(program("fade", chain(vec![fade(4.0), call("c", "after.fade")]), json!([]), "single"))
        .await
        .unwrap();
    assert!(engine.run_now("fade", None, None));
    assert_eq!(finished(&mut events, "fade").await, ("ok".into(), None));
    let calls = ha.calls.lock().unwrap().clone();
    let pct: Vec<Value> =
        calls.iter().filter_map(|c| c.1.as_ref().map(|d| d["brightness_pct"].clone())).collect();
    assert_eq!(pct, [json!(10.0), json!(55.0), json!(100.0)]);
    assert_eq!(calls[1].1.as_ref().unwrap()["transition"], 2.0);
    assert_eq!(calls[3].0, "after.fade");

    // Someone turns the light off during the fade: it stops, and the run carries on.
    ha.calls.lock().unwrap().clear();
    assert!(engine.run_now("fade", None, None));
    tokio::time::sleep(Duration::from_millis(300)).await;
    ha.states.lock().unwrap().insert("light.bed".into(), json!({"state": "off", "attributes": {}}));
    assert_eq!(finished(&mut events, "fade").await, ("ok".into(), None));
    assert_eq!(ha.calls(), ["light.turn_on", "after.fade"]);
}
