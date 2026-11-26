//! Action responses, `repeat` and `wait` as Home Assistant scripts have them.

#![recursion_limit = "256"]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use puzzlebox_engine::{Engine, EngineEvent, HaApi, TemplateStream, TriggerStream};
use puzzlebox_storage::{ProgramRecord, Store};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use tokio::time::timeout;

type Sender<T> = mpsc::UnboundedSender<T>;
type TemplateSub = (String, Value, Sender<Result<Value, String>>);

#[derive(Default)]
struct MockHa {
    /// (action, asked for a response)
    calls: Mutex<Vec<(String, bool)>>,
    /// Variables of every template rendered.
    renders: Mutex<Vec<(String, Value)>>,
    /// (config, variables) of each trigger subscription.
    triggers: Mutex<Vec<(Value, Value, Sender<Value>)>>,
    templates: Mutex<Vec<TemplateSub>>,
}

fn stream<T: Send + 'static>(rx: mpsc::UnboundedReceiver<T>) -> futures_util::stream::BoxStream<'static, T> {
    Box::pin(futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|v| (v, rx)) }))
}

#[async_trait]
impl HaApi for MockHa {
    async fn call_service(&self, service: &str, _: Option<Value>, _: Option<Value>) -> Result<Value, String> {
        self.calls.lock().unwrap().push((service.into(), false));
        Ok(json!({"context": {}}))
    }
    async fn call_service_response(
        &self,
        service: &str,
        data: Option<Value>,
        _: Option<Value>,
    ) -> Result<Value, String> {
        self.calls.lock().unwrap().push((service.into(), true));
        Ok(json!({"weather.home": {"forecast": [data.unwrap_or_default()["type"].clone()]}}))
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
    async fn render_template(&self, t: &str, v: Value) -> Result<Value, String> {
        self.renders.lock().unwrap().push((t.into(), v));
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
        Ok(stream(rx))
    }
    async fn subscribe_template(&self, t: &str, variables: Value) -> Option<Result<TemplateStream, String>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.templates.lock().unwrap().push((t.into(), variables, tx));
        Some(Ok(stream(rx)))
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
fn template(id: &str, t: &str) -> Value {
    b("ha_template", id, json!({"TEMPLATE": t}), json!({}))
}
/// A statement that renders a template (so the mock sees its variables).
fn render(id: &str, t: &str) -> Value {
    b("ha_log", id, json!({"LEVEL": "info"}), json!({"MESSAGE": blk(template(&format!("{id}t"), t))}))
}
fn set(id: &str, var: &str, value: Value) -> Value {
    b("variables_set", id, json!({"VAR": {"id": var}}), json!({"VALUE": blk(value)}))
}
fn on_run(body: Value) -> Value {
    b("ha_on_run", "run", json!({}), json!({"DO": blk(body)}))
}
fn workspace(body: Value, vars: Value) -> Value {
    json!({"blocks": {"languageVersion": 0, "blocks": [on_run(body)]}, "variables": vars})
}
fn program(id: &str, ws: Value) -> ProgramRecord {
    ProgramRecord {
        folder: String::new(),
        id: id.into(),
        name: id.into(),
        description: String::new(),
        enabled: true,
        mode: "single".into(),
        max_runs: 10,
        workspace: ws,
        created_at: 0,
        updated_at: 0,
    }
}

async fn setup() -> (Engine, Arc<MockHa>, broadcast::Receiver<EngineEvent>) {
    let ha = Arc::new(MockHa::default());
    let engine = Engine::new(ha.clone(), Store::open_in_memory().unwrap()).await.unwrap();
    let events = engine.subscribe();
    (engine, ha, events)
}

/// Saves and runs the program, then returns the run's final variables.
async fn run(engine: &Engine, events: &mut broadcast::Receiver<EngineEvent>, id: &str, ws: Value) -> Value {
    let (_, errors) = engine.save(program(id, ws)).await.unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(engine.run_now(id, None, None));
    finished(engine, events, id).await
}

async fn finished(engine: &Engine, events: &mut broadcast::Receiver<EngineEvent>, id: &str) -> Value {
    loop {
        let ev = timeout(Duration::from_secs(5), events.recv()).await.expect("timed out").unwrap();
        if let EngineEvent::RunFinished { status, error, .. } = ev {
            assert_eq!(status, "ok", "{error:?}");
            break;
        }
    }
    let r = engine.store().list_runs(id, 1).await.unwrap().remove(0);
    engine.store().get_run(&r.id).await.unwrap().unwrap().trace.unwrap()["variables"].clone()
}

/// Waits until the run has subscribed `n` times.
async fn subscribed<T>(list: &Mutex<Vec<T>>, n: usize) {
    timeout(Duration::from_secs(5), async {
        while list.lock().unwrap().len() < n {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("never subscribed");
}

#[tokio::test]
async fn action_responses_are_saved_in_variables() {
    let (engine, ha, mut events) = setup().await;
    let data = b(
        "dict_create",
        "d",
        json!({}),
        json!({"ITEMS": blk(b("dict_pair", "dp", json!({"KEY": "type"}), json!({"VALUE": blk(text("dv", "daily"))})))}),
    );
    let ask = |id: &str, var: &str| {
        b(
            "ha_call_service_response",
            id,
            json!({"SERVICE": "weather.get_forecasts", "RESPONSE": {"id": var}}),
            json!({"TARGET": blk(text(&format!("{id}t"), "weather.home")), "DATA": blk(data.clone())}),
        )
    };
    let body = chain(vec![
        ask("a1", "r"),
        ask("a2", "kept"),
        b("ha_call_service", "c", json!({"SERVICE": "light.turn_on"}), json!({})),
    ]);
    let vars = json!([{"name": "r", "id": "r"}, {"name": "kept", "id": "kept", "type": "Program"}]);
    let v = run(&engine, &mut events, "resp", workspace(body, vars)).await;
    let want = json!({"weather.home": {"forecast": ["daily"]}});
    assert_eq!(v["r"], want);
    assert_eq!(v["kept"], want, "a program variable gets it too");
    assert_eq!(engine.store().kv_all("program:resp").await.unwrap()["kept"], want);
    let calls = ha.calls.lock().unwrap().clone();
    assert_eq!(
        calls,
        [
            ("weather.get_forecasts".into(), true),
            ("weather.get_forecasts".into(), true),
            ("light.turn_on".into(), false)
        ]
    );
}

#[tokio::test]
async fn loops_give_templates_their_repeat() {
    let (engine, ha, mut events) = setup().await;
    let mut list = b(
        "lists_create_with",
        "l",
        json!({}),
        json!({"ADD0": blk(text("x", "a")), "ADD1": blk(b("logic_null", "n", json!({}), json!({})))}),
    );
    list["extraState"] = json!({"itemCount": 2});
    let inner = b(
        "controls_forEach",
        "fe",
        json!({"VAR": {"id": "it"}}),
        json!({"LIST": blk(list), "DO": blk(render("in", "inner"))}),
    );
    let outer = b(
        "controls_repeat_ext",
        "rep",
        json!({}),
        json!({"TIMES": blk(num("t", 2.0)), "DO": blk(chain(vec![inner, render("out", "outer")]))}),
    );
    let index = b("loop_info", "li", json!({"WHAT": "index"}), json!({}));
    let below3 =
        b("logic_compare", "cmp", json!({"OP": "LT"}), json!({"A": blk(index), "B": blk(num("three", 3.0))}));
    let rounds = b("loop_info", "li2", json!({"WHAT": "index"}), json!({}));
    let while_loop = b(
        "controls_whileUntil",
        "wh",
        json!({"MODE": "WHILE"}),
        json!({"BOOL": blk(below3), "DO": blk(chain(vec![set("s", "n", rounds), render("w", "while")]))}),
    );
    let body = chain(vec![outer, while_loop, render("after", "after")]);
    let vars = json!([{"name": "it", "id": "it"}, {"name": "n", "id": "n"}]);
    let v = run(&engine, &mut events, "rep", workspace(body, vars)).await;
    assert_eq!(v["n"], json!(2), "the condition sees the round about to run");
    assert!(v.get("repeat").is_none(), "`repeat` isn't a run variable");

    let seen: Vec<(String, Value)> =
        ha.renders.lock().unwrap().iter().map(|(t, v)| (t.clone(), v["repeat"].clone())).collect();
    let inner = |index: u64, item: Option<&str>| {
        let mut r = json!({"first": index == 1, "index": index, "last": index == 2});
        if let Some(i) = item {
            r["item"] = json!(i);
        }
        ("inner".to_owned(), r)
    };
    let outer =
        |index: u64| ("outer".to_owned(), json!({"first": index == 1, "index": index, "last": index == 2}));
    let while_round = |index: u64| ("while".to_owned(), json!({"first": index == 1, "index": index}));
    assert_eq!(
        seen,
        [
            inner(1, Some("a")),
            inner(2, None),
            outer(1),
            inner(1, Some("a")),
            inner(2, None),
            outer(2),
            while_round(1),
            while_round(2),
            ("after".to_owned(), Value::Null),
        ]
    );
}

#[tokio::test]
async fn loop_info_only_inside_loops() {
    let (engine, _ha, _events) = setup().await;
    let body = set("s", "n", b("loop_info", "li", json!({"WHAT": "index"}), json!({})));
    let (_, errors) =
        engine.save(program("li", workspace(body, json!([{"name": "n", "id": "n"}])))).await.unwrap();
    assert_eq!(errors[0].block_id.as_deref(), Some("li"), "{errors:?}");
}

fn wait_for(id: &str, timeout_s: Option<f64>) -> Value {
    let state = |id: &str, e: &str| b("ha_trigger_state", id, json!({"ENTITY": e, "TO": "on"}), json!({}));
    let mut inputs = json!({"TRIGGERS": blk(chain(vec![state(&format!("{id}a"), "x.a"), state(&format!("{id}b"), "x.b")]))});
    if let Some(t) = timeout_s {
        inputs["TIMEOUT"] = blk(num(&format!("{id}n"), t));
    }
    b("ha_wait_for_trigger", id, json!({}), inputs)
}

#[tokio::test]
async fn wait_for_trigger_sets_wait_like_home_assistant() {
    let (engine, ha, mut events) = setup().await;
    let body = chain(vec![set("s", "x", text("xv", "hello")), wait_for("w", Some(600.0))]);
    let vars = json!([{"name": "x", "id": "x"}]);
    engine.save(program("wft", workspace(body, vars.clone()))).await.unwrap();
    assert!(engine.run_now("wft", None, None));
    subscribed(&ha.triggers, 1).await;
    {
        let subs = ha.triggers.lock().unwrap();
        let (config, variables, tx) = &subs[0];
        assert_eq!(config.as_array().map(Vec::len), Some(2), "both triggers in one subscription: {config}");
        assert_eq!(variables["x"], "hello", "HA renders the triggers with the run's variables");
        tx.send(json!({"platform": "state", "entity_id": "x.b", "id": "1", "idx": "1"})).unwrap();
    }
    let v = finished(&engine, &mut events, "wft").await;
    assert_eq!(v["wait"]["completed"], true);
    assert_eq!(v["wait"]["trigger"]["idx"], "1");
    let left = v["wait"]["remaining"].as_f64().unwrap();
    assert!(left > 590.0 && left <= 600.0, "{left}");

    // Timed out, and without a timeout.
    let body = wait_for("w", Some(0.05));
    let v = run(&engine, &mut events, "wft2", workspace(body, json!([]))).await;
    assert_eq!(v["wait"], json!({"remaining": 0.0, "completed": false, "trigger": null}));

    engine.save(program("wft3", workspace(wait_for("w", None), json!([])))).await.unwrap();
    assert!(engine.run_now("wft3", None, None));
    subscribed(&ha.triggers, 3).await;
    ha.triggers.lock().unwrap()[2].2.send(json!({"platform": "state"})).unwrap();
    let v = finished(&engine, &mut events, "wft3").await;
    assert_eq!(v["wait"], json!({"remaining": null, "completed": true, "trigger": {"platform": "state"}}));
}

fn wait_until(cond: Value, timeout_s: f64) -> Value {
    b("wait_until", "wu", json!({}), json!({"COND": blk(cond), "TIMEOUT": blk(num("to", timeout_s))}))
}

#[tokio::test]
async fn wait_until_a_template_follows_its_changes() {
    let (engine, ha, mut events) = setup().await;
    let tpl = "{{ is_state('x.y', 'on') }}";
    let body = chain(vec![set("s", "x", text("xv", "hi")), wait_until(template("t", tpl), 600.0)]);
    engine.save(program("wu", workspace(body, json!([{"name": "x", "id": "x"}])))).await.unwrap();

    // Already true: `remaining` is the whole timeout.
    assert!(engine.run_now("wu", None, None));
    subscribed(&ha.templates, 1).await;
    {
        let subs = ha.templates.lock().unwrap();
        assert_eq!(subs[0].0, tpl);
        assert_eq!(subs[0].1["x"], "hi");
        subs[0].2.send(Ok(json!(true))).unwrap();
    }
    let v = finished(&engine, &mut events, "wu").await;
    assert_eq!(v["wait"], json!({"remaining": 600.0, "completed": true}));

    // False, an error later on (ignored, as in HA), text HA reads as false, then true.
    assert!(engine.run_now("wu", None, None));
    subscribed(&ha.templates, 2).await;
    {
        let subs = ha.templates.lock().unwrap();
        for r in [Ok(json!(false)), Err("oops".to_owned()), Ok(json!("no")), Ok(json!("on"))] {
            subs[1].2.send(r).unwrap();
        }
    }
    let v = finished(&engine, &mut events, "wu").await;
    assert_eq!(v["wait"]["completed"], true);
    let left = v["wait"]["remaining"].as_f64().unwrap();
    assert!(left > 590.0 && left < 600.0, "{left}");
    assert!(ha.renders.lock().unwrap().is_empty(), "no polling");

    // Times out.
    let body = wait_until(template("t", tpl), 0.05);
    engine.save(program("wu2", workspace(body, json!([])))).await.unwrap();
    assert!(engine.run_now("wu2", None, None));
    subscribed(&ha.templates, 3).await;
    ha.templates.lock().unwrap()[2].2.send(Ok(json!(false))).unwrap();
    let v = finished(&engine, &mut events, "wu2").await;
    assert_eq!(v["wait"], json!({"remaining": 0.0, "completed": false}));
}

#[tokio::test]
async fn a_failing_template_wait_fails_the_run() {
    let (engine, ha, mut events) = setup().await;
    let body = wait_until(template("t", "{{ nope( }}"), 600.0);
    engine.save(program("bad", workspace(body, json!([])))).await.unwrap();
    assert!(engine.run_now("bad", None, None));
    subscribed(&ha.templates, 1).await;
    ha.templates.lock().unwrap()[0].2.send(Err("syntax".into())).unwrap();
    loop {
        let ev = timeout(Duration::from_secs(5), events.recv()).await.unwrap().unwrap();
        if let EngineEvent::RunFinished { status, error, .. } = ev {
            assert_eq!((status.as_str(), error.as_deref()), ("error", Some("syntax")));
            break;
        }
    }
}

#[tokio::test]
async fn other_conditions_still_poll() {
    let (engine, _ha, mut events) = setup().await;
    let started = std::time::Instant::now();
    let cond = b("logic_boolean", "f", json!({"BOOL": "FALSE"}), json!({}));
    let v = run(&engine, &mut events, "poll", workspace(wait_until(cond, 0.2), json!([]))).await;
    assert_eq!(v["wait"], json!({"remaining": 0.0, "completed": false}));
    assert!(started.elapsed() < Duration::from_millis(900), "stops when the time is up");
    let cond = b("logic_boolean", "t", json!({"BOOL": "TRUE"}), json!({}));
    let v = run(&engine, &mut events, "poll2", workspace(wait_until(cond, 30.0), json!([]))).await;
    assert_eq!(v["wait"], json!({"remaining": 30.0, "completed": true}));
}
