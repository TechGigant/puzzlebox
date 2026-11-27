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
        if t.contains("area_entities('kitchen')") {
            return Ok(json!(["light.a", "light.b"]));
        }
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
        folder: String::new(),
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
    assert_eq!(vars["wait"], json!({"remaining": 0.0, "completed": false}));
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

#[tokio::test]
async fn loose_stacks_run_on_request_only() {
    let (engine, ha) = setup().await;
    let mut events = engine.subscribe();
    let mut loose = call("loose", "light.turn_on", "light.a");
    loose["x"] = json!(300);
    let half_built = b("math_number", "stray", json!({"NUM": 1}), json!({}));
    let ws = workspace(vec![on_run(call("main", "light.turn_off", "light.a")), loose, half_built], json!([]));
    let (_, errors) = engine.save(program("lo", ws, "parallel")).await.unwrap();
    assert!(errors.is_empty(), "loose blocks don't make the program invalid: {errors:?}");
    let idx = engine.handler_index("lo", "loose").expect("loose stack has a handler");
    assert_eq!(engine.handler_index("lo", "stray"), None);
    assert!(engine.run_now("lo", Some(idx), None));
    next_finished(&mut events).await;
    assert_eq!(ha.calls(), ["light.turn_on"]);
    // The default run is still the entry block, not the loose stack.
    assert!(engine.run_now("lo", None, None));
    next_finished(&mut events).await;
    assert_eq!(ha.calls(), ["light.turn_on", "light.turn_off"]);
}

fn set(id: &str, var: &str, value: Value) -> Value {
    b("variables_set", id, json!({"VAR": {"id": var}}), json!({"VALUE": blk(value)}))
}

#[tokio::test]
async fn switch_filter_map_and_text_helpers() {
    let (engine, ha) = setup().await;
    let mut events = engine.subscribe();
    let mut list = b(
        "lists_create_with",
        "l",
        json!({}),
        json!({"ADD0": blk(num("a", 3.0)), "ADD1": blk(num("b", 12.0)), "ADD2": blk(num("c", 7.0))}),
    );
    list["extraState"] = json!({"itemCount": 3});
    let item = |id: &str| b("lists_item", id, json!({}), json!({}));
    let over5 = b(
        "logic_compare",
        "cmp",
        json!({"OP": "GT"}),
        json!({"A": blk(item("i1")), "B": blk(num("five", 5.0))}),
    );
    let filtered = b("lists_filter", "f", json!({}), json!({"LIST": blk(list), "COND": blk(over5)}));
    let doubled = b(
        "lists_map",
        "m",
        json!({}),
        json!({"LIST": blk(filtered), "EXPR": blk(b("math_arithmetic", "mul", json!({"OP": "MULTIPLY"}),
            json!({"A": blk(item("i2")), "B": blk(num("two", 2.0))})))}),
    );
    let case = |id: &str, value: &str, service: &str| {
        b(
            "control_case",
            id,
            json!({}),
            json!({"VALUE": blk(text(&format!("{id}v"), value)),
            "DO": blk(call(&format!("{id}c"), service, "light.a"))}),
        )
    };
    let otherwise = b(
        "control_case_default",
        "dflt",
        json!({}),
        json!({"DO": blk(call("dc", "light.toggle", "light.a"))}),
    );
    let switch = b(
        "control_switch",
        "sw",
        json!({}),
        json!({"VALUE": blk(b("text_split_at", "sa", json!({"WHERE": "after"}),
                json!({"TEXT": blk(text("tt", "mode: night")), "SEP": blk(text("sep", ": "))}))),
            "CASES": blk(chain(vec![case("c1", "day", "light.turn_on"), case("c2", "night", "light.turn_off"), otherwise]))}),
    );
    let regex = b(
        "text_regex",
        "re",
        json!({"OP": "match", "PATTERN": "(\\d+)%"}),
        json!({"TEXT": blk(text("rt", "battery 42%"))}),
    );
    let fallback = b(
        "logic_or_default",
        "od",
        json!({}),
        json!({"A": blk(text("u", "unavailable")), "B": blk(num("z", 0.0))}),
    );
    let body = chain(vec![
        b("control_section", "sec", json!({"TEXT": "lights"}), json!({})),
        switch,
        set("s1", "out", doubled),
        set("s2", "pct", regex),
        set("s3", "fb", fallback),
    ]);
    let vars =
        json!([{"name": "out", "id": "out"}, {"name": "pct", "id": "pct"}, {"name": "fb", "id": "fb"}]);
    let (_, errors) =
        engine.save(program("sw", workspace(vec![on_run(body)], vars), "single")).await.unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(engine.run_now("sw", None, None));
    let (status, error) = next_finished(&mut events).await;
    assert_eq!(status, "ok", "{error:?}");
    assert_eq!(ha.calls(), ["light.turn_off"], "only the matching case runs");
    let run = engine.store().list_runs("sw", 1).await.unwrap().remove(0);
    let vars = engine.store().get_run(&run.id).await.unwrap().unwrap().trace.unwrap()["variables"].clone();
    assert_eq!(vars["out"], json!([24, 14]));
    assert_eq!(vars["pct"], json!("42"));
    assert_eq!(vars["fb"], json!(0));
}

#[tokio::test]
async fn light_helpers_build_light_actions() {
    let (engine, ha) = setup().await;
    let mut events = engine.subscribe();
    ha.states.lock().unwrap().insert(
        "light.a".into(),
        json!({"entity_id": "light.a", "state": "on", "attributes": {"rgb_color": [255, 0, 0], "brightness": 255}}),
    );
    let target = || blk(text("tg", "light.a"));
    let colour = b(
        "colour_mix",
        "mix",
        json!({}),
        json!({"A": blk(b("light_colour", "lc", json!({"ENTITY": "light.a"}), json!({}))),
            "B": blk(b("colour_picker", "cp", json!({"COLOUR": "#0000FF"}), json!({}))),
            "RATIO": blk(num("r", 50.0))}),
    );
    let set = b(
        "light_set_colour",
        "sc",
        json!({}),
        json!({"TARGET": target(), "COLOUR": blk(colour), "BRIGHTNESS": blk(num("br", 30.0)), "TRANSITION": blk(num("tr", 0.0))}),
    );
    let white =
        b("light_set_white", "sw", json!({}), json!({"TARGET": target(), "KELVIN": blk(num("k", 2700.0))}));
    let dim = b(
        "light_step",
        "st",
        json!({"DIRECTION": "dim"}),
        json!({"TARGET": target(), "STEP": blk(num("s", 10.0))}),
    );
    let off = b(
        "light_power",
        "pw",
        json!({"MODE": "off"}),
        json!({"TARGET": target(), "TRANSITION": blk(num("t2", 3.0))}),
    );
    let (_, errors) = engine
        .save(program("li", workspace(vec![on_run(chain(vec![set, white, dim, off]))], json!([])), "single"))
        .await
        .unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(engine.run_now("li", None, None));
    let (status, error) = next_finished(&mut events).await;
    assert_eq!(status, "ok", "{error:?}");
    let calls: Vec<(String, Option<Value>)> =
        ha.calls.lock().unwrap().iter().map(|c| (c.0.clone(), c.1.clone())).collect();
    assert_eq!(
        calls,
        [
            ("light.turn_on".into(), Some(json!({"rgb_color": [128, 0, 128], "brightness_pct": 30.0}))),
            ("light.turn_on".into(), Some(json!({"color_temp_kelvin": 2700}))),
            ("light.turn_on".into(), Some(json!({"brightness_step_pct": -10.0}))),
            ("light.turn_off".into(), Some(json!({"transition": 3.0}))),
        ]
    );
}

#[tokio::test]
async fn device_helpers_call_the_right_actions() {
    let (engine, ha) = setup().await;
    let mut events = engine.subscribe();
    ha.states.lock().unwrap().insert(
        "media_player.tv".into(),
        json!({"entity_id": "media_player.tv", "state": "playing", "attributes": {"volume_level": 0.25}}),
    );
    let t = |e: &str| blk(text(&format!("t_{e}"), e));
    let blocks = vec![
        b(
            "cover_set_position",
            "cp",
            json!({"WHAT": "position"}),
            json!({"TARGET": t("cover.blinds"), "POSITION": blk(num("p", 40.0))}),
        ),
        b("climate_set_mode", "cm", json!({"MODE": "cool"}), json!({"TARGET": t("climate.living")})),
        b(
            "media_set_volume",
            "mv",
            json!({}),
            json!({"TARGET": t("media_player.tv"),
            "VOLUME": blk(b("media_volume_of", "vo", json!({"ENTITY": "media_player.tv"}), json!({})))}),
        ),
        b("lock_command", "lk", json!({"MODE": "unlock"}), json!({"TARGET": t("lock.front")})),
        b(
            "helper_set",
            "hs",
            json!({"ENTITY": "input_boolean.guest"}),
            json!({"VALUE": blk(b("logic_boolean", "f", json!({"BOOL": "FALSE"}), json!({})))}),
        ),
        b(
            "helper_set",
            "hn",
            json!({"ENTITY": "input_select.mode"}),
            json!({"VALUE": blk(text("o", "night"))}),
        ),
        b(
            "notify_send",
            "ns",
            json!({"TO": "mobile_app_phone"}),
            json!({"MESSAGE": blk(text("m", "hi")), "TITLE": blk(text("ti", "Home"))}),
        ),
    ];
    let (_, errors) = engine
        .save(program("dh", workspace(vec![on_run(chain(blocks))], json!([])), "single"))
        .await
        .unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(engine.run_now("dh", None, None));
    let (status, error) = next_finished(&mut events).await;
    assert_eq!(status, "ok", "{error:?}");
    let calls: Vec<(String, Option<Value>)> =
        ha.calls.lock().unwrap().iter().map(|c| (c.0.clone(), c.1.clone())).collect();
    assert_eq!(
        calls,
        [
            ("cover.set_cover_position".into(), Some(json!({"position": 40}))),
            ("climate.set_hvac_mode".into(), Some(json!({"hvac_mode": "cool"}))),
            ("media_player.volume_set".into(), Some(json!({"volume_level": 0.25}))),
            ("lock.unlock".into(), None),
            ("homeassistant.turn_off".into(), None),
            ("input_select.select_option".into(), Some(json!({"option": "night"}))),
            ("notify.mobile_app_phone".into(), Some(json!({"message": "hi", "title": "Home"}))),
        ]
    );
}

#[tokio::test]
async fn broken_stacks_are_skipped_and_the_rest_runs() {
    let (engine, ha) = setup().await;
    let mut events = engine.subscribe();
    let good = on_run(call("ok", "light.turn_on", "light.a"));
    // An "if" without a condition: a problem, but only in its own stack.
    let broken = when("bw", state_trigger("bt", "x.y"), b("controls_if", "bad", json!({}), json!({})));
    let (_, errors) =
        engine.save(program("pc", workspace(vec![good, broken], json!([])), "single")).await.unwrap();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].message.contains("empty"), "{}", errors[0].message);
    assert!(engine.run_now("pc", None, None));
    next_finished(&mut events).await;
    assert_eq!(ha.calls(), ["light.turn_on"]);
    assert_eq!(engine.status("pc").unwrap().state, "error");
}

#[tokio::test]
async fn areas_expand_to_entities_and_review_fixes() {
    let (engine, ha) = setup().await;
    let mut events = engine.subscribe();
    {
        let mut st = ha.states.lock().unwrap();
        st.insert("light.a".into(), json!({"entity_id": "light.a", "state": "on"}));
        st.insert("light.b".into(), json!({"entity_id": "light.b", "state": "off"}));
        st.insert("sensor.x".into(), json!({"entity_id": "sensor.x", "state": "9.5"}));
        st.insert("sensor.y".into(), json!({"entity_id": "sensor.y", "state": "10.2"}));
    }
    let area = || blk(b("ha_area", "ar", json!({"ID": "kitchen"}), json!({})));
    let count = b("ha_state_count", "cnt", json!({"OP": "is", "STATE": "on"}), json!({"LIST": area()}));
    let state = |id: &str, e: &str| {
        b(
            "ha_state",
            id,
            json!({}),
            json!({"ENTITY": {"shadow": {"type": "ha_entity", "id": format!("{id}e"), "fields": {"ENTITY": e}}}}),
        )
    };
    // Two sensor states compare as numbers: 9.5 < 10.2 (as text it would be the other way round).
    let less = b(
        "logic_compare",
        "lt",
        json!({"OP": "LT"}),
        json!({"A": blk(state("sx", "sensor.x")), "B": blk(state("sy", "sensor.y"))}),
    );
    let mut list = b(
        "lists_create_with",
        "l",
        json!({}),
        json!({"ADD0": blk(num("1", 1.0)), "ADD1": blk(num("3", 3.0))}),
    );
    list["extraState"] = json!({"itemCount": 2});
    let insert = b(
        "lists_setIndex",
        "ins",
        json!({"MODE": "INSERT", "WHERE": "FROM_START"}),
        json!({"LIST": blk(b("variables_get", "lg", json!({"VAR": {"id": "lst"}}), json!({}))), "AT": blk(num("at", 2.0)), "TO": blk(num("two", 2.0))}),
    );
    let mut each = b(
        "controls_forEach",
        "fe",
        json!({"VAR": {"id": "it"}}),
        json!({"LIST": area(), "DO": blk(b("ha_call_service", "c", json!({"SERVICE": "light.toggle"}),
            json!({"TARGET": blk(b("variables_get", "ig", json!({"VAR": {"id": "it"}}), json!({})))})))}),
    );
    each["id"] = json!("fe");
    let neg = b("math_single", "ng", json!({"OP": "NEG"}), json!({"NUM": blk(num("n", 4.0))}));
    let body = chain(vec![
        set("s1", "count", count),
        set("s2", "less", less),
        set("s3", "lst", list),
        insert,
        each,
        set("s4", "neg", neg),
    ]);
    let vars = json!([{"name": "count", "id": "count"}, {"name": "less", "id": "less"}, {"name": "lst", "id": "lst"},
        {"name": "it", "id": "it"}, {"name": "neg", "id": "neg"}]);
    let (_, errors) =
        engine.save(program("ar", workspace(vec![on_run(body)], vars), "single")).await.unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(engine.run_now("ar", None, None));
    let (status, error) = next_finished(&mut events).await;
    assert_eq!(status, "ok", "{error:?}");
    assert_eq!(ha.calls(), ["light.toggle", "light.toggle"], "looping over an area visits its entities");
    let run = engine.store().list_runs("ar", 1).await.unwrap().remove(0);
    let v = engine.store().get_run(&run.id).await.unwrap().unwrap().trace.unwrap()["variables"].clone();
    assert_eq!(v["count"], json!(1));
    assert_eq!(v["less"], json!(true));
    assert_eq!(v["lst"], json!([1, 2, 3]));
    assert_eq!(v["neg"], json!(-4));
}

#[tokio::test]
async fn timers_in_loose_stacks_and_per_target_on_for() {
    let (engine, ha) = setup().await;
    let mut events = engine.subscribe();
    // A loose stack starting with a timer: the timer must run its body, not the stack again.
    let mut timer = b(
        "timer_start",
        "lt",
        json!({"NAME": "t", "MODE": "restart"}),
        json!({"SECONDS": blk(num("s", 0.2)), "DO": blk(call("off", "light.turn_off", "light.a"))}),
    );
    timer["x"] = json!(400);
    // "turn on … for … then off" in a loop: each light gets its own timer.
    let mut list = b(
        "lists_create_with",
        "l",
        json!({}),
        json!({"ADD0": blk(text("a", "light.x")), "ADD1": blk(text("b", "light.y"))}),
    );
    list["extraState"] = json!({"itemCount": 2});
    let on_for = b(
        "device_on_for",
        "of",
        json!({}),
        json!({
        "TARGET": blk(b("variables_get", "g", json!({"VAR": {"id": "it"}}), json!({}))),
        "SECONDS": blk(num("d", 0.2))}),
    );
    let each = b(
        "controls_forEach",
        "fe",
        json!({"VAR": {"id": "it"}}),
        json!({"LIST": blk(list), "DO": blk(on_for)}),
    );
    let ws = workspace(vec![on_run(each), timer], json!([{"name": "it", "id": "it"}]));
    let (_, errors) = engine.save(program("lt", ws, "parallel")).await.unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    let idx = engine.handler_index("lt", "lt").unwrap();
    assert!(engine.run_now("lt", Some(idx), None));
    assert!(engine.run_now("lt", None, None));
    for _ in 0..5 {
        next_finished(&mut events).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let calls: Vec<(String, Option<Value>)> =
        ha.calls.lock().unwrap().iter().map(|c| (c.0.clone(), c.2.clone())).collect();
    let offs: Vec<_> =
        calls.iter().filter(|(s, _)| s == "homeassistant.turn_off").map(|(_, t)| t.clone()).collect();
    assert_eq!(offs.len(), 2, "{calls:?}");
    assert!(offs.contains(&Some(json!({"entity_id": ["light.x"]}))), "{offs:?}");
    assert!(offs.contains(&Some(json!({"entity_id": ["light.y"]}))), "{offs:?}");
    assert_eq!(
        calls.iter().filter(|(s, _)| s == "light.turn_off").count(),
        1,
        "the loose timer fired its body once"
    );
}

#[tokio::test]
async fn loops_keep_records_and_nested_calls_to_broken_functions_are_caught() {
    let (engine, _ha) = setup().await;
    let mut events = engine.subscribe();
    let records =
        b("yaml_value", "y", json!({"CONFIG": "[{entity_id: light.a, brightness: 50}]"}), json!({}));
    let get = b(
        "dict_get",
        "dg",
        json!({}),
        json!({"KEY": blk(text("k", "brightness")),
        "DICT": blk(b("variables_get", "g", json!({"VAR": {"id": "it"}}), json!({})))}),
    );
    let each = b(
        "controls_forEach",
        "fe",
        json!({"VAR": {"id": "it"}}),
        json!({"LIST": blk(records), "DO": blk(set("s", "got", get))}),
    );
    let vars = json!([{"name": "it", "id": "it"}, {"name": "got", "id": "got"}]);
    engine.save(program("rec", workspace(vec![on_run(each)], vars.clone()), "single")).await.unwrap();
    assert!(engine.run_now("rec", None, None));
    let (status, error) = next_finished(&mut events).await;
    assert_eq!(status, "ok", "{error:?}");
    let run = engine.store().list_runs("rec", 1).await.unwrap().remove(0);
    let v = engine.store().get_run(&run.id).await.unwrap().unwrap().trace.unwrap()["variables"].clone();
    assert_eq!(v["got"], json!(50), "records are looped over as they are");

    // A function with a problem, called inside "set": that handler is reported and left out.
    let broken_fn = json!({"type": "procedures_defreturn", "id": "f", "x": 300, "fields": {"NAME": "f"}, "extraState": {"params": []},
        "inputs": {"STACK": {"block": {"type": "controls_if", "id": "emptyif"}}}});
    let call_f = b("procedures_callreturn", "cf", json!({}), json!({}));
    let mut call_f = call_f;
    call_f["extraState"] = json!({"name": "f", "params": []});
    let ws = workspace(vec![on_run(set("s2", "got", call_f)), broken_fn], vars);
    let (_, errors) = engine.save(program("bf", ws, "single")).await.unwrap();
    assert!(errors.iter().any(|e| e.message.contains("function \"f\"")), "{errors:?}");
}

fn motion_states(ha: &MockHa) {
    ha.states.lock().unwrap().insert(
        "binary_sensor.motion".into(),
        json!({"entity_id": "binary_sensor.motion", "state": "off", "attributes": {"friendly_name": "Motion"}}),
    );
}

#[tokio::test]
async fn active_runs_show_waits_and_steps_say_how_they_went() {
    let (engine, ha) = setup().await;
    motion_states(&ha);
    let mut events = engine.subscribe();
    let check = b(
        "controls_if",
        "if",
        json!({}),
        json!({"IF0": blk(b("logic_boolean", "yes", json!({"BOOL": "TRUE"}), json!({}))),
               "DO0": blk(b("ha_wait", "w1", json!({}), json!({"SECONDS": blk(num("n", 0.3))})))}),
    );
    let ws = workspace(vec![when("w", state_trigger("t", "binary_sensor.motion"), check)], json!([]));
    engine.save(program("p", ws, "single")).await.unwrap();
    let status = engine.status("p").unwrap();
    assert_eq!(status.summary, "when Motion turns on");

    ha.fire(json!({"platform": "state", "entity_id": "binary_sensor.motion",
        "from_state": {"state": "off", "attributes": {"friendly_name": "Motion"}},
        "to_state": {"state": "on", "attributes": {"friendly_name": "Motion"}}}));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let active = engine.active_runs("p");
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].summary, "Motion turned on");
    assert_eq!(active[0].block_id.as_deref(), Some("w1"));
    assert_eq!(active[0].waiting.as_ref().unwrap().text, "waiting 0.3 s");

    assert_eq!(next_finished(&mut events).await.0, "ok");
    assert!(engine.active_runs("p").is_empty());
    let runs = engine.store().list_runs("p", 1).await.unwrap();
    assert_eq!(runs[0].summary.as_deref(), Some("Motion turned on"));
    let trace = engine.store().get_run(&runs[0].id).await.unwrap().unwrap().trace.unwrap();
    let note = |block: &str| {
        trace["steps"].as_array().unwrap().iter().find(|s| s["block"] == block).unwrap()["note"].clone()
    };
    assert_eq!(note("if"), "true");
    assert_eq!(note("w1"), "waited 0.3 s");
}
#[tokio::test]
async fn runs_as_if_a_trigger_fired() {
    let (engine, ha) = setup().await;
    motion_states(&ha);
    let mut events = engine.subscribe();
    let ws = workspace(
        vec![when("w", state_trigger("t", "binary_sensor.motion"), call("c", "light.turn_on", "light.a"))],
        json!([]),
    );
    engine.save(program("p", ws, "single")).await.unwrap();
    assert_eq!(engine.triggers("p"), [("t".to_owned(), "Motion turns on".to_owned())]);
    assert!(engine.run_as_trigger("p", "t"));
    assert!(!engine.run_as_trigger("p", "nope"));
    next_finished(&mut events).await;
    let run = &engine.store().list_runs("p", 1).await.unwrap()[0];
    let t = run.trigger.as_ref().unwrap();
    assert_eq!((&t["from_state"]["state"], &t["to_state"]["state"]), (&json!("off"), &json!("on")));
    assert_eq!(t["id"], "t");
    assert_eq!(run.summary.as_deref(), Some("run as if Motion turned on"));
}

#[tokio::test]
async fn failed_runs_are_logged_and_marked() {
    let (engine, _ha) = setup().await;
    let mut events = engine.subscribe();
    let ws = workspace(vec![on_run(call("c", "fail.now", "x.y"))], json!([]));
    engine.save(program("p", ws, "single")).await.unwrap();
    assert!(engine.run_now("p", None, None));
    assert_eq!(next_finished(&mut events).await.0, "error");
    let log = engine.log("p");
    assert!(log.last().unwrap().message.contains("boom"), "{log:?}");
    assert_eq!(engine.status("p").unwrap().last_status.as_deref(), Some("error"));
    assert_eq!(engine.cancel_runs("p"), 0);
}

#[tokio::test]
async fn log_survives_a_restart_and_goes_with_the_program() {
    let ha = Arc::new(MockHa::default());
    let store = Store::open_in_memory().unwrap();
    let engine = Engine::new(ha.clone(), store.clone()).await.unwrap();
    let mut events = engine.subscribe();
    let ws = workspace(vec![on_run(call("c", "fail.now", "x.y"))], json!([]));
    engine.save(program("p", ws, "single")).await.unwrap();
    assert!(engine.run_now("p", None, None));
    next_finished(&mut events).await;
    engine.flush_log().await;

    let again = Engine::new(ha.clone(), store.clone()).await.unwrap();
    let log = again.log("p");
    assert!(log.last().unwrap().message.contains("boom"), "{log:?}");
    again.delete("p").await.unwrap();
    let in_bin = Engine::new(ha.clone(), store.clone()).await.unwrap();
    assert_eq!(in_bin.log("p").len(), log.len(), "the bin keeps the log");
    in_bin.purge("p").await.unwrap();
    assert!(Engine::new(ha, store).await.unwrap().log("p").is_empty());
}

#[tokio::test]
async fn log_is_written_behind() {
    let ha = Arc::new(MockHa::default());
    let store = Store::open_in_memory().unwrap();
    let engine = Engine::new(ha.clone(), store.clone()).await.unwrap();
    let mut events = engine.subscribe();
    let ws = workspace(vec![on_run(call("c", "fail.now", "x.y"))], json!([]));
    engine.save(program("p", ws, "single")).await.unwrap();
    assert!(engine.run_now("p", None, None));
    next_finished(&mut events).await;
    for _ in 0..20 {
        if !store.all_logs().await.unwrap().is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("the log line was never written");
}

#[tokio::test]
async fn periodic_purge_spares_fresh_strays_and_unloads_old_ones() {
    let (engine, _ha) = setup().await;
    let mut events = engine.subscribe();
    let mut stray = program("stray", json!({"blocks": {}}), "single");
    stray.name = "New program".into();
    engine.save(stray).await.unwrap();
    assert!(engine.status("stray").is_some());
    assert_eq!(engine.purge_old(3_600_000).await, 0, "the editor may still have it open");
    assert_eq!(engine.purge_old(0).await, 1);
    assert!(engine.status("stray").is_none());
    loop {
        let ev = timeout(Duration::from_secs(5), events.recv()).await.unwrap().unwrap();
        if let EngineEvent::Removed { program_id } = ev {
            assert_eq!(program_id, "stray");
            break;
        }
    }
}
