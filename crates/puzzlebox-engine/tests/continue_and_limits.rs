//! "carry on if this fails" (HA's `continue_on_error`) and wait time limits.

#![recursion_limit = "256"]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use puzzlebox_engine::{ActionError, Engine, EngineEvent, HaApi, OnError, TemplateStream, TriggerStream};
use puzzlebox_storage::{ProgramRecord, Store};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use tokio::time::timeout;

type Sender<T> = mpsc::UnboundedSender<T>;

#[derive(Default)]
struct MockHa {
    calls: Mutex<Vec<String>>,
    scripts: Mutex<Vec<Value>>,
    triggers: Mutex<Vec<Sender<Value>>>,
    templates: Mutex<Vec<Sender<Result<Value, String>>>>,
}

fn stream<T: Send + 'static>(rx: mpsc::UnboundedReceiver<T>) -> futures_util::stream::BoxStream<'static, T> {
    Box::pin(futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|v| (v, rx)) }))
}

#[async_trait]
impl HaApi for MockHa {
    async fn call_service(&self, _: &str, _: Option<Value>, _: Option<Value>) -> Result<Value, String> {
        unreachable!("the engine asks call_service_checked")
    }
    async fn call_service_checked(
        &self,
        service: &str,
        _: Option<Value>,
        _: Option<Value>,
        _: bool,
    ) -> Result<Value, ActionError> {
        self.calls.lock().unwrap().push(service.into());
        let on_error = match service {
            "fail.skip" => OnError::Skip,
            "fail.stop" => OnError::Stop,
            "fail.odd" => OnError::SkipIfCall,
            _ => return Ok(json!({})),
        };
        Err(ActionError { message: format!("{service} failed"), on_error })
    }
    fn state(&self, _: &str) -> Option<Value> {
        None
    }
    async fn test_condition(&self, _: Value, _: Value) -> Result<bool, String> {
        Ok(true)
    }
    async fn execute_script(&self, sequence: Value, _: Value) -> Result<Value, String> {
        self.scripts.lock().unwrap().push(sequence);
        Ok(Value::Null)
    }
    async fn render_template(&self, t: &str, _: Value) -> Result<Value, String> {
        match t {
            "{{ bad }}" => {
                Err("Home Assistant returned an error (template_error): UndefinedError: 'bad'".into())
            }
            "{{ lost }}" => Err("connection lost before a reply arrived".into()),
            "{{ 5 }}" => Ok(json!(5)),
            _ => Ok(json!(t)),
        }
    }
    async fn fire_event(&self, _: &str, _: Option<Value>) -> Result<(), String> {
        Ok(())
    }
    async fn subscribe_trigger(&self, _: Value) -> Result<TriggerStream, String> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.triggers.lock().unwrap().push(tx);
        Ok(stream(rx))
    }
    async fn subscribe_template(&self, _: &str, _: Value) -> Option<Result<TemplateStream, String>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.templates.lock().unwrap().push(tx);
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
fn call(id: &str, service: &str) -> Value {
    b("ha_call_service", id, json!({"SERVICE": service}), json!({}))
}
fn carry_on(id: &str, body: Value) -> Value {
    b("control_carry_on", id, json!({}), json!({"DO": blk(body)}))
}
fn workspace(body: Value) -> Value {
    let run = b("ha_on_run", "run", json!({}), json!({"DO": blk(body)}));
    json!({"blocks": {"languageVersion": 0, "blocks": [run]}, "variables": [{"name": "error", "id": "e"}]})
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

async fn start(engine: &Engine, id: &str, body: Value) {
    let (_, errors) = engine.save(program(id, workspace(body))).await.unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(engine.run_now(id, None, None));
}

/// (status, failing block, the run's variables)
async fn finished(
    engine: &Engine,
    events: &mut broadcast::Receiver<EngineEvent>,
    id: &str,
) -> (String, Option<String>, Value) {
    let (status, block) = loop {
        let ev = timeout(Duration::from_secs(5), events.recv()).await.expect("timed out").unwrap();
        if let EngineEvent::RunFinished { status, block_id, .. } = ev {
            break (status, block_id);
        }
    };
    let r = engine.store().list_runs(id, 1).await.unwrap().remove(0);
    let vars = engine.store().get_run(&r.id).await.unwrap().unwrap().trace.unwrap()["variables"].clone();
    (status, block, vars)
}

async fn run(
    engine: &Engine,
    events: &mut broadcast::Receiver<EngineEvent>,
    id: &str,
    body: Value,
) -> (String, Option<String>) {
    start(engine, id, body).await;
    let (status, block, _) = finished(engine, events, id).await;
    (status, block)
}

#[tokio::test]
async fn skips_only_what_home_assistant_skips() {
    let (engine, ha, mut events) = setup().await;
    let calls = |ha: &MockHa| std::mem::take(&mut *ha.calls.lock().unwrap());

    // A Home Assistant error ends the blocks inside; the run carries on below.
    let body = chain(vec![
        carry_on("w", chain(vec![call("a", "fail.skip"), call("b", "light.b")])),
        call("c", "light.c"),
    ]);
    assert_eq!(run(&engine, &mut events, "p1", body).await.0, "ok");
    assert_eq!(calls(&ha), ["fail.skip", "light.c"]);

    // Configuration errors and the like stop the run anyway.
    let body = chain(vec![carry_on("w", call("a", "fail.stop")), call("c", "light.c")]);
    assert_eq!(run(&engine, &mut events, "p2", body).await, ("error".into(), Some("a".into())));
    assert_eq!(calls(&ha), ["fail.stop"]);

    // Other exceptions only when the flag is on that very call.
    let body = chain(vec![carry_on("w", call("a", "fail.odd")), call("c", "light.c")]);
    assert_eq!(run(&engine, &mut events, "p3", body).await.0, "ok");
    assert_eq!(calls(&ha), ["fail.odd", "light.c"]);
    let body = chain(vec![carry_on("w", chain(vec![call("a", "fail.odd"), call("b", "light.b")]))]);
    assert_eq!(run(&engine, &mut events, "p4", body).await, ("error".into(), Some("a".into())));
    calls(&ha);
    let inner = b(
        "controls_if",
        "if",
        json!({}),
        json!({"IF0": blk(b("logic_boolean", "t", json!({"BOOL": "TRUE"}), json!({}))), "DO0": blk(call("a", "fail.odd"))}),
    );
    assert_eq!(run(&engine, &mut events, "p5", carry_on("w", inner)).await.0, "error");
    calls(&ha);

    // The program's own errors are never skipped; stop still just stops.
    let throw = b(
        "control_throw",
        "x",
        json!({}),
        json!({"MESSAGE": blk(b("text", "m", json!({"TEXT": "no"}), json!({})))}),
    );
    assert_eq!(
        run(&engine, &mut events, "p6", carry_on("w", throw)).await,
        ("error".into(), Some("x".into()))
    );
    let stop = b("control_stop", "s", json!({}), json!({}));
    let body = chain(vec![carry_on("w", stop), call("c", "light.c")]);
    assert_eq!(run(&engine, &mut events, "p7", body).await.0, "ok");
    assert!(calls(&ha).is_empty());

    // Nested in a try, the error is skipped before try sees it.
    let try_block = b(
        "control_try",
        "try",
        json!({"ERROR": {"id": "e"}}),
        json!({"TRY": blk(chain(vec![carry_on("w", call("a", "fail.skip")), call("c", "light.c")])), "CATCH": blk(call("h", "light.h"))}),
    );
    assert_eq!(run(&engine, &mut events, "p8", try_block).await.0, "ok");
    assert_eq!(calls(&ha), ["fail.skip", "light.c"]);
}

#[tokio::test]
async fn templates_of_an_action_fail_like_home_assistant_errors() {
    let (engine, ha, mut events) = setup().await;
    let tpl = |id: &str, t: &str| b("ha_template", id, json!({"TEMPLATE": t}), json!({}));
    let with_data = |id: &str, data: Value| {
        b("ha_call_service", id, json!({"SERVICE": "light.turn_on"}), json!({"DATA": blk(data)}))
    };
    let dict = |t: &str| {
        let pair = b("dict_pair", "dp", json!({"KEY": "x"}), json!({"VALUE": blk(tpl("t", t))}));
        b("dict_create", "d", json!({}), json!({"ITEMS": blk(pair)}))
    };
    let after = call("c", "light.c");

    let body = chain(vec![carry_on("w", with_data("a", dict("{{ bad }}"))), after.clone()]);
    assert_eq!(run(&engine, &mut events, "p1", body).await.0, "ok");
    // A template that isn't a dictionary of data.
    let body = chain(vec![carry_on("w", with_data("a", tpl("t", "{{ 5 }}"))), after.clone()]);
    assert_eq!(run(&engine, &mut events, "p2", body).await.0, "ok");
    assert_eq!(*ha.calls.lock().unwrap(), ["light.c", "light.c"]);

    let body = chain(vec![carry_on("w", with_data("a", dict("{{ lost }}"))), after.clone()]);
    assert_eq!(run(&engine, &mut events, "p3", body).await, ("error".into(), Some("t".into())));
    // Elsewhere a template error stops the run, as HA's TemplateError does.
    let set =
        b("variables_set", "s", json!({"VAR": {"id": "e"}}), json!({"VALUE": blk(tpl("t", "{{ bad }}"))}));
    assert_eq!(run(&engine, &mut events, "p4", carry_on("w", set)).await, ("error".into(), Some("t".into())));
}

#[tokio::test]
async fn a_lone_native_action_gets_continue_on_error() {
    let (engine, ha, mut events) = setup().await;
    let raw = b("ha_action_raw", "r", json!({"CONFIG": "action: light.turn_on"}), json!({}));
    assert_eq!(run(&engine, &mut events, "p", carry_on("w", raw)).await.0, "ok");
    assert_eq!(
        *ha.scripts.lock().unwrap(),
        [json!([{"action": "light.turn_on", "continue_on_error": true}])]
    );
}

fn wait_trigger(id: &str, limit: Option<&str>, seconds: f64) -> Value {
    let trig =
        b("ha_trigger_raw", &format!("{id}t"), json!({"CONFIG": "trigger: event\nevent_type: x"}), json!({}));
    let fields = limit.map_or(json!({}), |l| json!({"LIMIT": l}));
    b(
        "ha_wait_for_trigger",
        id,
        fields,
        json!({"TRIGGERS": blk(trig), "TIMEOUT": blk(num(&format!("{id}n"), seconds))}),
    )
}

fn wait_until(id: &str, limit: Option<&str>, seconds: f64) -> Value {
    let cond =
        b("ha_template", &format!("{id}c"), json!({"TEMPLATE": "{{ is_state('a.b', 'on') }}"}), json!({}));
    let fields = limit.map_or(json!({}), |l| json!({"LIMIT": l}));
    b("wait_until", id, fields, json!({"COND": blk(cond), "TIMEOUT": blk(num(&format!("{id}n"), seconds))}))
}

async fn wait_for<T>(list: &Mutex<Vec<T>>, n: usize) {
    timeout(Duration::from_secs(5), async {
        while list.lock().unwrap().len() < n {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("never subscribed");
}

#[tokio::test]
async fn a_time_limit_of_zero_gives_up_at_once() {
    let (engine, ha, mut events) = setup().await;
    // Like HA, which doesn't set the triggers up at all.
    start(&engine, "t0", wait_trigger("w", Some("limit"), 0.0)).await;
    let (status, _, vars) = finished(&engine, &mut events, "t0").await;
    assert_eq!(status, "ok");
    assert_eq!(vars["wait"], json!({"remaining": 0.0, "completed": false, "trigger": null}));
    assert!(ha.triggers.lock().unwrap().is_empty());

    // Saved before "no time limit": 0 still waits as long as it takes.
    start(&engine, "old", wait_trigger("w", None, 0.0)).await;
    wait_for(&ha.triggers, 1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    ha.triggers.lock().unwrap()[0].send(json!({"id": "0"})).unwrap();
    let (_, _, vars) = finished(&engine, &mut events, "old").await;
    assert_eq!(vars["wait"], json!({"remaining": null, "completed": true, "trigger": {"id": "0"}}));

    // The template is still looked at once.
    for (i, (now, completed)) in [(false, false), (true, true)].into_iter().enumerate() {
        start(&engine, "u0", wait_until("w", Some("limit"), 0.0)).await;
        wait_for(&ha.templates, i + 1).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        ha.templates.lock().unwrap()[i].send(Ok(json!(now))).unwrap();
        let (_, _, vars) = finished(&engine, &mut events, "u0").await;
        assert_eq!(vars["wait"], json!({"remaining": 0.0, "completed": completed}));
    }

    start(&engine, "neg", wait_until("w", Some("limit"), -1.0)).await;
    assert_eq!(finished(&engine, &mut events, "neg").await.1.as_deref(), Some("w"));
}

#[tokio::test]
async fn no_time_limit_waits_as_long_as_it_takes() {
    let (engine, ha, mut events) = setup().await;
    start(&engine, "n", wait_until("w", Some("none"), 0.01)).await;
    wait_for(&ha.templates, 1).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let tx = ha.templates.lock().unwrap()[0].clone();
    tx.send(Ok(json!(false))).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    tx.send(Ok(json!(true))).unwrap();
    let (_, _, vars) = finished(&engine, &mut events, "n").await;
    assert_eq!(vars["wait"], json!({"remaining": null, "completed": true}));
}
