//! Starting, saving, stopping and restarting programs: what keeps running and what is told why.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use puzzlebox_engine::{Engine, EngineEvent, HaApi, IntervalSource, TriggerStream};
use puzzlebox_ir::NoExternals;
use puzzlebox_storage::{ProgramRecord, Store, now_ms};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio::time::timeout;

#[derive(Default)]
struct MockHa {
    calls: Mutex<Vec<String>>,
}

impl MockHa {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl HaApi for MockHa {
    async fn call_service(
        &self,
        service: &str,
        _d: Option<Value>,
        _t: Option<Value>,
    ) -> Result<Value, String> {
        self.calls.lock().unwrap().push(service.into());
        Ok(Value::Null)
    }
    fn state(&self, _entity_id: &str) -> Option<Value> {
        None
    }
    async fn test_condition(&self, _c: Value, _v: Value) -> Result<bool, String> {
        Ok(true)
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
fn call(id: &str, service: &str) -> Value {
    b(
        "ha_call_service",
        id,
        json!({"SERVICE": service}),
        json!({"TARGET": blk(b("text", &format!("{id}t"), json!({"TEXT": "light.a"}), json!({})))}),
    )
}
fn wait(id: &str, seconds: f64) -> Value {
    b("ha_wait", id, json!({}), json!({"SECONDS": blk(num(&format!("{id}n"), seconds))}))
}
fn timer(id: &str, name: &str, seconds: f64, body: Value) -> Value {
    b(
        "timer_start",
        id,
        json!({"NAME": name, "MODE": "restart"}),
        json!({"SECONDS": blk(num(&format!("{id}n"), seconds)), "DO": blk(body)}),
    )
}
fn top(ty: &str, id: &str, body: Value) -> Value {
    b(ty, id, json!({}), json!({"DO": blk(body)}))
}
fn workspace(tops: Vec<Value>) -> Value {
    json!({"blocks": {"languageVersion": 0, "blocks": tops}, "variables": []})
}
fn program(id: &str, ws: Value) -> ProgramRecord {
    ProgramRecord {
        folder: String::new(),
        id: id.into(),
        name: id.into(),
        description: String::new(),
        enabled: true,
        mode: "parallel".into(),
        max_runs: 10,
        workspace: ws,
        created_at: 0,
        updated_at: 0,
    }
}

async fn engine_on(store: Store) -> (Engine, Arc<MockHa>) {
    let ha = Arc::new(MockHa::default());
    let engine = Engine::new(ha.clone(), store).await.unwrap();
    (engine, ha)
}

async fn finished(rx: &mut broadcast::Receiver<EngineEvent>) -> (String, Option<String>) {
    loop {
        let ev = timeout(Duration::from_secs(5), rx.recv()).await.expect("timed out").unwrap();
        if let EngineEvent::RunFinished { status, error, .. } = ev {
            return (status, error);
        }
    }
}

async fn until(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("never happened: {what}");
}

#[tokio::test]
async fn each_program_starts_once_at_startup() {
    let store = Store::open_in_memory().unwrap();
    let ws = workspace(vec![top("ha_on_start", "s", call("c", "light.turn_on"))]);
    store.save_program(program("p", ws)).await.unwrap();
    let (engine, ha) = engine_on(store).await;
    // What startup does: the block registry is built (and announced) before programs load.
    engine.registry_changed(Arc::new(NoExternals)).await.unwrap();
    engine.load_all().await.unwrap();
    until("the start handler ran", || ha.calls().len() == 1).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    // A later registry change that doesn't touch the program leaves it alone.
    engine.registry_changed(Arc::new(NoExternals)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(ha.calls(), ["light.turn_on"]);
}

#[tokio::test]
async fn saving_without_changes_to_the_blocks_keeps_runs_and_timers() {
    let (engine, ha) = engine_on(Store::open_in_memory().unwrap()).await;
    let mut events = engine.subscribe();
    let body = chain(vec![timer("tm", "off", 30.0, call("off", "light.turn_off")), wait("w", 30.0)]);
    let ws = workspace(vec![top("ha_on_run", "run", body)]);
    let (saved, _) = engine.save(program("p", ws.clone())).await.unwrap();
    assert!(engine.run_now("p", None, None));
    until("the run waits", || engine.active_runs("p").first().is_some_and(|r| r.waiting.is_some())).await;
    assert_eq!(engine.timers("p").len(), 1);

    // A new name (and moved blocks) is not a new program.
    let mut renamed = saved.clone();
    renamed.name = "Renamed".into();
    renamed.workspace["blocks"]["blocks"][0]["x"] = json!(200);
    let impact = engine.save_impact(&renamed);
    assert!(!impact.restarts);
    engine.save(renamed.clone()).await.unwrap();
    assert_eq!(engine.active_runs("p").len(), 1, "the waiting run goes on");
    assert_eq!(engine.timers("p").len(), 1, "the timer goes on");

    // Changed blocks restart it: saving says so first, and the stopped run says why.
    let mut changed = renamed.clone();
    changed.workspace = workspace(vec![top("ha_on_run", "run", call("c", "light.toggle"))]);
    let impact = engine.save_impact(&changed);
    assert!(impact.restarts);
    assert_eq!(impact.runs.len(), 1);
    assert_eq!(impact.timers.len(), 1);
    assert_eq!(impact.timers[0].name, "off");
    engine.save(changed).await.unwrap();
    let (status, why) = finished(&mut events).await;
    assert_eq!(status, "cancelled");
    assert_eq!(why.as_deref(), Some("stopped because the program was saved"));
    assert!(engine.timers("p").is_empty());
    let runs = engine.store().list_runs("p", 5).await.unwrap();
    assert_eq!(runs[0].error.as_deref(), Some("stopped because the program was saved"));
    assert!(ha.calls().is_empty());
}

#[tokio::test]
async fn stop_button_and_turning_off_say_why() {
    let (engine, _ha) = engine_on(Store::open_in_memory().unwrap()).await;
    let mut events = engine.subscribe();
    let ws = workspace(vec![top("ha_on_run", "run", wait("w", 30.0))]);
    engine.save(program("p", ws)).await.unwrap();
    engine.run_now("p", None, None);
    until("running", || !engine.active_runs("p").is_empty()).await;
    assert_eq!(engine.cancel_runs("p"), 1);
    assert_eq!(finished(&mut events).await.1.as_deref(), Some("stopped with the Stop button"));

    engine.run_now("p", None, None);
    until("running", || !engine.active_runs("p").is_empty()).await;
    engine.set_enabled("p", false).await.unwrap();
    assert_eq!(finished(&mut events).await.1.as_deref(), Some("stopped because the program was turned off"));
}

#[tokio::test]
async fn timers_go_off_after_a_restart() {
    let store = Store::open_in_memory().unwrap();
    let body = timer("tm", "later", 0.6, call("fired", "light.turn_off"));
    let ws = workspace(vec![top("ha_on_run", "run", body)]);
    let (engine, ha) = engine_on(store.clone()).await;
    let mut events = engine.subscribe();
    engine.save(program("p", ws)).await.unwrap();
    engine.run_now("p", None, None);
    finished(&mut events).await;
    let due = engine.timers("p")[0].due;
    engine.shutdown().await;
    assert!(ha.calls().is_empty());

    // Puzzlebox starts again: the timer still goes off at the same time.
    let (engine, ha) = engine_on(store.clone()).await;
    engine.load_all().await.unwrap();
    let timers = engine.timers("p");
    assert_eq!(timers.len(), 1);
    assert_eq!(timers[0].due, due);
    until("the timer went off", || ha.calls() == ["light.turn_off"]).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(engine.timers("p").is_empty());
    assert!(store.kv_all("timers:p").await.unwrap().is_empty());
    engine.shutdown().await;

    // One that was due while Puzzlebox was stopped goes off at once; one long overdue is dropped.
    let timer =
        |due: i64| json!({"due": due, "handler": "tm", "trigger": {"platform": "timer", "timer": "later"}});
    store.kv_set("timers:p", "late", &timer(now_ms() - 60_000)).await.unwrap();
    store.kv_set("timers:p", "ancient", &timer(now_ms() - 3 * 3600_000)).await.unwrap();
    let (engine, ha) = engine_on(store.clone()).await;
    engine.load_all().await.unwrap();
    until("the late timer went off", || ha.calls() == ["light.turn_off"]).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(ha.calls().len(), 1);
    assert!(engine.log("p").iter().any(|l| l.message.contains("too long ago")));
}

#[tokio::test]
async fn saving_forgets_stored_timers() {
    let store = Store::open_in_memory().unwrap();
    let body = timer("tm", "later", 30.0, call("fired", "light.turn_off"));
    let (engine, _ha) = engine_on(store.clone()).await;
    let mut events = engine.subscribe();
    let (saved, _) = engine.save(program("p", workspace(vec![top("ha_on_run", "run", body)]))).await.unwrap();
    engine.run_now("p", None, None);
    finished(&mut events).await;
    let mut changed = saved;
    changed.workspace = workspace(vec![top("ha_on_run", "run", call("c", "light.toggle"))]);
    engine.save(changed).await.unwrap();
    engine.shutdown().await;
    assert!(store.kv_all("timers:p").await.unwrap().is_empty());
}

#[tokio::test]
async fn shutdown_records_why_runs_stopped() {
    let store = Store::open_in_memory().unwrap();
    let (engine, _ha) = engine_on(store.clone()).await;
    engine.save(program("p", workspace(vec![top("ha_on_run", "run", wait("w", 30.0))]))).await.unwrap();
    engine.run_now("p", None, None);
    until("running", || !engine.active_runs("p").is_empty()).await;
    engine.shutdown().await;
    let runs = store.list_runs("p", 5).await.unwrap();
    assert_eq!(runs[0].status, "cancelled");
    assert_eq!(runs[0].error.as_deref(), Some("stopped because Puzzlebox stopped or restarted"));
}

#[tokio::test]
async fn paused_runs_carry_on_after_the_limit_and_closing_one_editor_frees_only_its_program() {
    let (engine, ha) = engine_on(Store::open_in_memory().unwrap()).await;
    let mut events = engine.subscribe();
    for id in ["a", "b"] {
        let ws = workspace(vec![top("ha_on_run", "run", call(&format!("{id}c"), &format!("{id}.go")))]);
        engine.save(program(id, ws)).await.unwrap();
    }
    let dbg = engine.debugger();
    let watch_a = dbg.watch("a");
    let _watch_b = dbg.watch("b");
    dbg.set_pause_on_start("a", true);
    dbg.set_pause_on_start("b", true);
    engine.run_now("a", None, None);
    engine.run_now("b", None, None);
    let mut paused = vec![];
    while paused.len() < 2 {
        if let EngineEvent::Paused { summary, program_id, run_id, .. } = events.recv().await.unwrap() {
            assert_eq!(summary, "run manually");
            paused.push((program_id, run_id));
        }
    }
    assert_eq!(engine.status("a").unwrap().paused, 1);
    assert_eq!(dbg.paused_runs().len(), 2);

    // Closing a's editor releases a's run, not b's.
    drop(watch_a);
    until("a carried on", || ha.calls() == ["a.go"]).await;
    assert_eq!(dbg.paused_runs().len(), 1);
    until("a is not paused", || engine.status("a").unwrap().paused == 0).await;
    assert_eq!(engine.status("b").unwrap().paused, 1);

    let (_, b_run) = paused.iter().find(|(p, _)| p == "b").unwrap();
    assert!(dbg.resume(b_run));
    until("b carried on", || ha.calls() == ["a.go", "b.go"]).await;
    assert!(!engine.log("b").iter().any(|l| l.message.contains("carried on by itself")));

    // A run nobody continues gives up waiting after the pause limit and says so in the log.
    dbg.set_pause_limit(Duration::from_millis(100));
    engine.run_now("b", None, None);
    until("b carried on by itself", || ha.calls().len() == 3).await;
    assert!(engine.log("b").iter().any(|l| l.message.contains("carried on by itself")));
}

#[tokio::test]
async fn every_is_an_exact_interval_from_the_start() {
    let (engine, ha) = engine_on(Store::open_in_memory().unwrap()).await;
    engine.add_trigger_source("interval", Arc::new(IntervalSource::default()));
    let every = b("ha_trigger_every", "e", json!({"N": 1, "UNIT": "seconds"}), json!({}));
    let when =
        b("ha_when", "w", json!({}), json!({"TRIGGERS": blk(every), "DO": blk(call("c", "light.toggle"))}));
    let (_, errors) = engine.save(program("p", workspace(vec![when]))).await.unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(ha.calls().is_empty(), "not straight away");
    until("fired once", || ha.calls().len() == 1).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(ha.calls().len(), 2);
    let run = &engine.store().list_runs("p", 1).await.unwrap()[0];
    assert_eq!(run.summary.as_deref(), Some("every second came round"));
}

#[tokio::test]
async fn local_time_uses_home_assistants_zone() {
    use chrono::Timelike;
    // Kiritimati is UTC+14 all year, unlike any likely container zone.
    assert!(puzzlebox_engine::clock::set_zone("Pacific/Kiritimati"));
    assert!(!puzzlebox_engine::clock::set_zone("Not/AZone"));
    let hour = puzzlebox_engine::value::builtin("time_hour", &[]).unwrap();
    assert_eq!(hour, json!((chrono::Utc::now().hour() + 14) % 24));
    let at = puzzlebox_engine::value::today_at("07:00").unwrap();
    assert_eq!(at.offset().local_minus_utc(), 14 * 3600);
}
