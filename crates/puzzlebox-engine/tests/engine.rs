#![recursion_limit = "256"]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use puzzlebox_engine::{Engine, EngineEvent, HaApi, TriggerStream};
use puzzlebox_storage::{ProgramRecord, Store};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use tokio::time::timeout;

type Call = (String, Option<Value>, Option<Value>);

#[derive(Default)]
struct MockHa {
    calls: Mutex<Vec<Call>>,
    states: Mutex<HashMap<String, Value>>,
    triggers: Mutex<Vec<(Value, mpsc::UnboundedSender<Value>)>>,
}

impl MockHa {
    fn fire(&self, payload: Value) {
        self.triggers.lock().unwrap().retain(|(_, tx)| tx.send(payload.clone()).is_ok());
    }
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
    async fn test_condition(&self, c: Value, _v: Value) -> Result<bool, String> {
        let st = self.state(c["entity_id"].as_str().unwrap_or(""));
        Ok(st.is_some_and(|s| s["state"] == c["state"]))
    }
    async fn execute_script(&self, _s: Value, _v: Value) -> Result<Value, String> {
        Ok(Value::Null)
    }
    async fn render_template(&self, t: &str, v: Value) -> Result<Value, String> {
        Ok(json!(format!("{t}|{}", v["trigger"]["platform"].as_str().unwrap_or(""))))
    }
    async fn fire_event(&self, _t: &str, _d: Option<Value>) -> Result<(), String> {
        Ok(())
    }
    async fn subscribe_trigger(&self, trigger: Value) -> Result<TriggerStream, String> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.triggers.lock().unwrap().push((trigger, tx));
        Ok(Box::pin(futures_util::stream::unfold(
            rx,
            |mut rx| async move { rx.recv().await.map(|v| (v, rx)) },
        )))
    }
}

// --- workspace builders -------------------------------------------------------

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
fn call(id: &str, service: &str, entity: &str) -> Value {
    b(
        "ha_call_service",
        id,
        json!({"SERVICE": service}),
        json!({"TARGET": blk(text(&format!("{id}t"), entity))}),
    )
}
fn when(id: &str, triggers: Value, body: Value) -> Value {
    b("ha_when", id, json!({}), json!({"TRIGGERS": blk(triggers), "DO": blk(body)}))
}
fn state_trigger(id: &str, entity: &str) -> Value {
    b("ha_trigger_state", id, json!({"ENTITY": entity, "TO": "on"}), json!({}))
}
fn workspace(tops: Vec<Value>, vars: Value) -> Value {
    json!({"blocks": {"languageVersion": 0, "blocks": tops}, "variables": vars})
}
fn program(id: &str, ws: Value, mode: &str) -> ProgramRecord {
    ProgramRecord {
        id: id.into(),
        name: id.into(),
        description: String::new(),
        enabled: true,
        mode: mode.into(),
        max_runs: 10,
        workspace: ws,
        created_at: 0,
        updated_at: 0,
    }
}

async fn setup() -> (Engine, Arc<MockHa>) {
    let ha = Arc::new(MockHa::default());
    let engine = Engine::new(ha.clone(), Store::open_in_memory().unwrap()).await.unwrap();
    (engine, ha)
}

async fn next_finished(rx: &mut broadcast::Receiver<EngineEvent>) -> (String, Option<String>) {
    loop {
        let ev = timeout(Duration::from_secs(5), rx.recv()).await.expect("timed out").unwrap();
        if let EngineEvent::RunFinished { status, error, .. } = ev {
            return (status, error);
        }
    }
}

#[tokio::test]
async fn subscribe_is_wired() {
    // Sanity check of the mock stream helper used below.
    let (engine, ha) = setup().await;
    let ws = workspace(
        vec![when("w", state_trigger("t", "binary_sensor.m"), call("c", "light.turn_on", "light.a"))],
        json!([]),
    );
    let (_, errors) = engine.save(program("p", ws, "single")).await.unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(ha.triggers.lock().unwrap()[0].0["entity_id"], "binary_sensor.m");
}

#[tokio::test]
async fn motion_light_runs_and_persists_variables() {
    let (engine, ha) = setup().await;
    let mut events = engine.subscribe();
    let body = chain(vec![
        call("c1", "light.turn_on", "light.hall"),
        b("ha_wait", "w1", json!({}), json!({"SECONDS": blk(num("n1", 0.05))})),
        call("c2", "light.turn_off", "light.hall"),
        b("math_change", "inc", json!({"VAR": {"id": "v"}}), json!({"DELTA": blk(num("n2", 1.0))})),
    ]);
    let ws = workspace(
        vec![when("w", state_trigger("t", "binary_sensor.motion"), body)],
        json!([{"name": "count", "id": "v", "type": "Program"}]),
    );
    engine.save(program("p", ws.clone(), "single")).await.unwrap();

    ha.fire(json!({"platform": "state", "to_state": {"state": "on"}}));
    assert_eq!(next_finished(&mut events).await, ("ok".into(), None));
    assert_eq!(ha.calls(), ["light.turn_on", "light.turn_off"]);
    assert_eq!(ha.calls.lock().unwrap()[0].2, Some(json!({"entity_id": ["light.hall"]})));

    // Persisted counter survives a restart (re-save) of the program.
    engine.save(program("p", ws, "single")).await.unwrap();
    ha.fire(json!({"platform": "state"}));
    next_finished(&mut events).await;
    assert_eq!(engine.store().kv_all("program:p").await.unwrap()["count"], 2);

    let runs = engine.store().list_runs("p", 10).await.unwrap();
    assert_eq!(runs.len(), 2);
    let full = engine.store().get_run(&runs[0].id).await.unwrap().unwrap();
    let steps = full.trace.unwrap()["steps"].as_array().unwrap().len();
    assert_eq!(steps, 4);
}

fn slow_program(mode: &str) -> ProgramRecord {
    let body = chain(vec![
        b("ha_wait", "w1", json!({}), json!({"SECONDS": blk(num("n", 0.3))})),
        call("c", "light.turn_on", "light.a"),
    ]);
    program("slow", workspace(vec![when("w", state_trigger("t", "x.y"), body)], json!([])), mode)
}

#[tokio::test]
async fn single_mode_skips_overlapping_runs() {
    let (engine, ha) = setup().await;
    let mut events = engine.subscribe();
    engine.save(slow_program("single")).await.unwrap();
    ha.fire(json!({}));
    tokio::time::sleep(Duration::from_millis(50)).await;
    ha.fire(json!({}));
    assert_eq!(next_finished(&mut events).await.0, "ok");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(ha.calls().len(), 1);
}

#[tokio::test]
async fn restart_mode_cancels_running() {
    let (engine, ha) = setup().await;
    let mut events = engine.subscribe();
    engine.save(slow_program("restart")).await.unwrap();
    ha.fire(json!({}));
    tokio::time::sleep(Duration::from_millis(50)).await;
    ha.fire(json!({}));
    assert_eq!(next_finished(&mut events).await.0, "cancelled");
    assert_eq!(next_finished(&mut events).await.0, "ok");
    assert_eq!(ha.calls().len(), 1);
}

#[tokio::test]
async fn queued_mode_runs_sequentially() {
    let (engine, ha) = setup().await;
    let mut events = engine.subscribe();
    engine.save(slow_program("queued")).await.unwrap();
    ha.fire(json!({}));
    ha.fire(json!({}));
    next_finished(&mut events).await;
    assert_eq!(ha.calls().len(), 1);
    next_finished(&mut events).await;
    assert_eq!(ha.calls().len(), 2);
}

#[tokio::test]
async fn control_flow_functions_and_errors() {
    let (engine, ha) = setup().await;
    let mut events = engine.subscribe();
    ha.states
        .lock()
        .unwrap()
        .insert("sensor.t".into(), json!({"state": "21.5", "attributes": {"unit": "C"}}));

    // fact(n) = n <= 1 ? 1 : n * fact(n - 1)
    let fact = json!({
        "type": "procedures_defreturn", "id": "f", "fields": {"NAME": "fact"},
        "extraState": {"params": [{"name": "n", "id": "vn"}]},
        "inputs": {"RETURN": blk(b("logic_ternary", "tern", json!({}), json!({
            "IF": blk(b("logic_compare", "cmp", json!({"OP": "LTE"}), json!({
                "A": blk(b("variables_get", "gn", json!({"VAR": {"id": "vn"}}), json!({}))),
                "B": blk(num("one", 1.0))}))),
            "THEN": blk(num("one2", 1.0)),
            "ELSE": blk(b("math_arithmetic", "mul", json!({"OP": "MULTIPLY"}), json!({
                "A": blk(b("variables_get", "gn2", json!({"VAR": {"id": "vn"}}), json!({}))),
                "B": blk(json!({"type": "procedures_callreturn", "id": "rec",
                    "extraState": {"name": "fact", "params": ["n"]},
                    "inputs": {"ARG0": blk(b("math_arithmetic", "sub", json!({"OP": "MINUS"}), json!({
                        "A": blk(b("variables_get", "gn3", json!({"VAR": {"id": "vn"}}), json!({}))),
                        "B": blk(num("one3", 1.0))})))}}))})))})))}
    });
    let set = |id: &str, var: &str, v: Value| {
        b("variables_set", id, json!({"VAR": {"id": var}}), json!({"VALUE": blk(v)}))
    };
    let body = chain(vec![
        set(
            "s1",
            "r",
            json!({"type": "procedures_callreturn", "id": "c5",
            "extraState": {"name": "fact", "params": ["n"]}, "inputs": {"ARG0": blk(num("five", 5.0))}}),
        ),
        // sum of a list via for-each
        set("s2", "sum", num("zero", 0.0)),
        b(
            "controls_forEach",
            "fe",
            json!({"VAR": {"id": "item"}}),
            json!({
            "LIST": blk(json!({"type": "lists_create_with", "id": "lst", "extraState": {"itemCount": 3},
                "inputs": {"ADD0": blk(num("a", 1.0)), "ADD1": blk(num("b2", 2.0)), "ADD2": blk(num("c3", 3.0))}})),
            "DO": blk(b("math_change", "ch", json!({"VAR": {"id": "sum"}}), json!({"DELTA": blk(
                b("variables_get", "gi", json!({"VAR": {"id": "item"}}), json!({}))) })))}),
        ),
        // numeric comparison against an HA state string
        set(
            "s3",
            "warm",
            b(
                "logic_compare",
                "cmp2",
                json!({"OP": "GT"}),
                json!({
            "A": blk(b("ha_state", "st", json!({"ENTITY": "sensor.t"}), json!({}))),
            "B": blk(num("twenty", 20.0))}),
            ),
        ),
        // try/catch around a failing action
        b(
            "control_try",
            "try",
            json!({"ERROR": {"id": "err"}}),
            json!({
            "TRY": blk(call("bad", "fail.now", "x.y")),
            "CATCH": blk(call("good", "notify.ok", "x.y"))}),
        ),
    ]);
    let vars = json!([
        {"name": "r", "id": "r"}, {"name": "sum", "id": "sum"}, {"name": "item", "id": "item"},
        {"name": "warm", "id": "warm"}, {"name": "err", "id": "err"}, {"name": "n", "id": "vn"}
    ]);
    let ws = workspace(vec![b("ha_on_start", "start", json!({}), json!({"DO": blk(body)})), fact], vars);
    let (_, errors) = engine.save(program("cf", ws, "single")).await.unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(next_finished(&mut events).await, ("ok".into(), None));
    assert_eq!(ha.calls(), ["notify.ok"]);

    let run = engine.store().list_runs("cf", 1).await.unwrap().remove(0);
    let vars = engine.store().get_run(&run.id).await.unwrap().unwrap().trace.unwrap()["variables"].clone();
    assert_eq!(vars["r"], 120);
    assert_eq!(vars["sum"], 6);
    assert_eq!(vars["warm"], true);
    assert_eq!(vars["err"], "boom");
}

#[tokio::test]
async fn runtime_error_reports_block() {
    let (engine, _ha) = setup().await;
    let mut events = engine.subscribe();
    let body = b(
        "ha_log",
        "log",
        json!({"LEVEL": "info"}),
        json!({"MESSAGE": blk(
        b("math_arithmetic", "div", json!({"OP": "DIVIDE"}), json!({"A": blk(num("a", 1.0)), "B": blk(num("z", 0.0))})))}),
    );
    let ws = workspace(vec![b("ha_on_start", "s", json!({}), json!({"DO": blk(body)}))], json!([]));
    engine.save(program("e", ws, "single")).await.unwrap();
    loop {
        if let EngineEvent::RunFinished { status, error, block_id, .. } = events.recv().await.unwrap() {
            assert_eq!(status, "error");
            assert_eq!(error.as_deref(), Some("division by zero"));
            assert_eq!(block_id.as_deref(), Some("div"));
            break;
        }
    }
    assert_eq!(engine.status("e").unwrap().last_error.as_deref(), Some("division by zero"));
}

#[tokio::test]
async fn infinite_loop_hits_step_limit() {
    let (engine, _ha) = setup().await;
    let mut events = engine.subscribe();
    let body = b(
        "controls_whileUntil",
        "loop",
        json!({"MODE": "WHILE"}),
        json!({
        "BOOL": blk(b("logic_boolean", "t", json!({"BOOL": "TRUE"}), json!({})))}),
    );
    let ws = workspace(vec![b("ha_on_start", "s", json!({}), json!({"DO": blk(body)}))], json!([]));
    engine.save(program("inf", ws, "single")).await.unwrap();
    let (status, error) = timeout(Duration::from_secs(60), next_finished(&mut events)).await.unwrap();
    assert_eq!(status, "error");
    assert!(error.unwrap().contains("step limit"));
}

#[tokio::test]
async fn debugger_pauses_at_breakpoint_and_steps() {
    let (engine, ha) = setup().await;
    let mut events = engine.subscribe();
    let body =
        chain(vec![call("c1", "a.one", "x.y"), call("c2", "a.two", "x.y"), call("c3", "a.three", "x.y")]);
    let ws = workspace(vec![when("w", state_trigger("t", "x.y"), body)], json!([]));
    engine.save(program("dbg", ws, "single")).await.unwrap();

    let dbg = engine.debugger();
    let _guard = dbg.watch("dbg");
    dbg.set_breakpoints("dbg", ["c2".to_owned()]);
    ha.fire(json!({}));

    let run_id = loop {
        if let EngineEvent::Paused { block_id, run_id, .. } = events.recv().await.unwrap() {
            assert_eq!(block_id, "c2");
            break run_id;
        }
    };
    assert_eq!(ha.calls(), ["a.one"]);
    assert!(dbg.step(&run_id));
    loop {
        if let EngineEvent::Paused { block_id, .. } = events.recv().await.unwrap() {
            assert_eq!(block_id, "c3");
            break;
        }
    }
    assert_eq!(ha.calls(), ["a.one", "a.two"]);
    assert!(dbg.resume(&run_id));
    assert_eq!(next_finished(&mut events).await.0, "ok");
    assert_eq!(ha.calls(), ["a.one", "a.two", "a.three"]);
}

#[tokio::test]
async fn invalid_program_is_saved_but_not_started() {
    let (engine, ha) = setup().await;
    let ws = workspace(vec![when("w", state_trigger("t", ""), call("c", "nodot", "x"))], json!([]));
    let (saved, errors) = engine.save(program("bad", ws, "single")).await.unwrap();
    assert_eq!(errors.len(), 2);
    assert_eq!(engine.status("bad").unwrap().state, "invalid");
    assert!(ha.triggers.lock().unwrap().is_empty());
    assert!(engine.store().get_program(&saved.id).await.unwrap().is_some());
}

/// Maps block `x_trg` to an HA trigger on a configurable entity, like a generated device block.
struct TestLowerer(&'static str);
impl puzzlebox_ir::BlockLowerer for TestLowerer {
    fn lower_trigger(
        &self,
        b: puzzlebox_ir::Block<'_>,
        _c: &mut puzzlebox_ir::Compiler<'_>,
    ) -> Option<puzzlebox_ir::Trigger> {
        (b.ty() == "x_trg").then(|| puzzlebox_ir::Trigger::Ha {
            id: b.id().into(),
            config: json!({"trigger": "state", "entity_id": self.0}),
        })
    }
}

#[tokio::test]
async fn registry_change_restarts_only_affected_programs() {
    let (engine, ha) = setup().await;
    engine.set_lowerer(Arc::new(TestLowerer("sensor.old")));
    let custom =
        workspace(vec![when("w", b("x_trg", "t", json!({}), json!({})), call("c", "a.b", "x.y"))], json!([]));
    let plain = workspace(vec![when("w2", state_trigger("t2", "x.z"), call("c2", "a.b", "x.y"))], json!([]));
    engine.save(program("custom", custom, "single")).await.unwrap();
    engine.save(program("plain", plain, "single")).await.unwrap();
    let subscribed = |ha: &MockHa| -> Vec<String> {
        ha.triggers
            .lock()
            .unwrap()
            .iter()
            .map(|(t, _)| t["entity_id"].as_str().unwrap_or("").to_owned())
            .collect()
    };
    assert_eq!(subscribed(&ha), ["sensor.old", "x.z"]);

    engine.registry_changed(Arc::new(TestLowerer("sensor.new"))).await.unwrap();
    // Only the affected program resubscribed (the old stream stays in the mock's list).
    assert_eq!(subscribed(&ha), ["sensor.old", "x.z", "sensor.new"]);
}

fn on_run(body: Value) -> Value {
    b("ha_on_run", "run", json!({}), json!({"DO": blk(body)}))
}

#[tokio::test]
async fn restart_timer_fires_once_after_last_start() {
    let (engine, ha) = setup().await;
    let mut events = engine.subscribe();
    let timer = b(
        "timer_start",
        "tm",
        json!({"NAME": "off", "MODE": "restart"}),
        json!({"SECONDS": blk(num("s", 0.3)), "DO": blk(call("fired", "light.turn_off", "light.a"))}),
    );
    let ws = workspace(vec![on_run(timer)], json!([]));
    engine.save(program("tmr", ws, "single")).await.unwrap();
    assert!(engine.run_now("tmr", None, None));
    next_finished(&mut events).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    // Second start restarts the countdown, so only one firing happens.
    assert!(engine.run_now("tmr", None, None));
    next_finished(&mut events).await;
    assert!(ha.calls().is_empty());
    // The timer's own run (single mode doesn't block it).
    let (status, _) = next_finished(&mut events).await;
    assert_eq!(status, "ok");
    assert_eq!(ha.calls(), ["light.turn_off"]);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(ha.calls().len(), 1);
}

#[tokio::test]
async fn throttle_wait_until_and_triggered_by() {
    let (engine, ha) = setup().await;
    let mut events = engine.subscribe();
    let throttled = b(
        "controls_if",
        "if",
        json!({}),
        json!({
        "IF0": blk(b("throttle", "th", json!({"KEY": "k"}), json!({"SECONDS": blk(num("ts", 60.0))}))),
        "DO0": blk(call("c", "notify.x", "x.y"))}),
    );
    let wait = b(
        "wait_until",
        "w",
        json!({}),
        json!({
        "COND": blk(b("logic_boolean", "f", json!({"BOOL": "FALSE"}), json!({}))),
        "TIMEOUT": blk(num("to", 0.2))}),
    );
    let set = b(
        "variables_set",
        "s",
        json!({"VAR": {"id": "v"}}),
        json!({"VALUE": blk(
        b("triggered_by", "tb", json!({"TRIGGER": "trg"}), json!({})))}),
    );
    let body = chain(vec![throttled, wait, set]);
    let ws =
        workspace(vec![when("wh", state_trigger("trg", "x.y"), body)], json!([{"name": "hit", "id": "v"}]));
    engine.save(program("th", ws, "parallel")).await.unwrap();
    for _ in 0..3 {
        ha.fire(json!({"platform": "state", "id": "trg"}));
        next_finished(&mut events).await;
    }
    assert_eq!(ha.calls(), ["notify.x"], "throttle lets only the first run through");
    let run = engine.store().list_runs("th", 1).await.unwrap().remove(0);
    let vars = engine.store().get_run(&run.id).await.unwrap().unwrap().trace.unwrap()["variables"].clone();
    assert_eq!(vars["wait"], json!({"completed": false}));
    assert_eq!(vars["hit"], json!(true));
}

#[tokio::test]
async fn setting_a_global_announces_it() {
    let (engine, _ha) = setup().await;
    let mut events = engine.subscribe();
    let set = b("variables_set", "s", json!({"VAR": {"id": "g"}}), json!({"VALUE": blk(num("n", 7.0))}));
    let ws = workspace(vec![on_run(set)], json!([{"name": "shared", "id": "g", "type": "Global"}]));
    engine.save(program("glob", ws, "single")).await.unwrap();
    assert!(engine.run_now("glob", None, None));
    let changed = loop {
        let ev = timeout(Duration::from_secs(5), events.recv()).await.expect("timed out").unwrap();
        if let EngineEvent::GlobalChanged { name, value } = ev {
            break (name, value);
        }
    };
    assert_eq!(changed, ("shared".to_owned(), json!(7)));
    assert_eq!(engine.globals()["shared"], json!(7));
}
