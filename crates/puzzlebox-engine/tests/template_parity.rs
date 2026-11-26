//! Templates the importer turns into blocks must give what Home Assistant gives. Each case runs
//! twice against a real Home Assistant with the `demo` integration: once as HA renders it in a
//! script, once as the imported blocks compute it in the engine.
//! Skipped unless HA_WS_URL and HA_TOKEN are set.

use std::time::Duration;

use puzzlebox_engine::{Engine, EngineEvent, HaApi, LiveHa};
use puzzlebox_ha_client::{ClientConfig, HaClient};
use puzzlebox_storage::{ProgramRecord, Store};
use serde_json::{Value, json};
use tokio::time::timeout;

/// Templates for values (`variables:`).
const VALUES: &[&str] = &[
    "{{ states('sensor.outside_temperature') | float }}",
    "{{ states('sensor.outside_temperature') | float + 1 }}",
    "{{ states('sensor.outside_humidity') | int * 2 }}",
    "{{ states('sensor.outside_humidity') | int(0) > 50 }}",
    "{{ states('sensor.outside_temperature') | float | round(0) }}",
    "{{ 2.5 | round(0) }}",
    "{{ 3.14159 | round(2) }}",
    "{{ 2.5 | round(0, 'floor') }}",
    "{{ state_attr('light.office_rgbw_lights', 'brightness') }}",
    "{{ state_attr('light.office_rgbw_lights', 'brightness') * 2 }}",
    "{{ state_attr('light.bed_light', 'brightness') is none }}",
    "{{ state_attr('light.office_rgbw_lights', 'brightness') is not none }}",
    "{{ is_state('light.bed_light', 'off') }}",
    "{{ is_state('sensor.outside_humidity', '54') }}",
    "{{ states('sensor.outside_humidity') == '54' }}",
    "{{ states('sensor.outside_humidity') == 54 }}",
    "{{ states('climate.hvac') }}",
    "{{ states('sensor.no_such_sensor') }}",
    "{{ 'on' if is_state('binary_sensor.movement_backyard', 'on') else 'off' }}",
    "{{ max(3, states('sensor.outside_humidity') | int) }}",
    "{{ states('sensor.outside_temperature') | float(0) > states('sensor.outside_humidity') | float(0) }}",
    "{{ states('sensor.power_consumption') | float * 0.001 }}",
    "{{ 7 // 2 }}",
    "Outside it is {{ states('sensor.outside_temperature') }} °C",
    "Motion: {{ is_state('binary_sensor.movement_backyard', 'on') }}",
    "Heat pump reads {{ state_attr('climate.heatpump', 'current_temperature') }} degrees",
    "{{ now().year }}",
    "{{ now().strftime('%Y-%m-%d') }}",
];

/// Template conditions (`if:`).
const CONDITIONS: &[&str] = &[
    "{{ is_state('light.bed_light', 'off') and is_state('binary_sensor.movement_backyard', 'on') }}",
    "{{ states('sensor.outside_temperature') | float > 10 }}",
    "{{ states('sensor.outside_humidity') | int >= 54 }}",
    "{{ not is_state('climate.hvac', 'off') }}",
    "{{ state_attr('climate.heatpump', 'current_temperature') == 25 }}",
    "{{ states('sensor.outside_humidity') in ['54', '55'] }}",
];

/// What HA's `variables` gives `v`, run as a script.
async fn ha_side(ha: &HaClient, step: Value) -> Value {
    let res = ha
        .execute_script(json!([step, {"variables": {"out": {"v": "{{ v }}"}}}, {"stop": "done", "response_variable": "out"}]), None)
        .await
        .unwrap();
    res["response"]["v"].clone()
}

/// What the engine gives `v` running the imported blocks, and whether a template was left.
async fn engine_side(engine: &Engine, n: usize, step: &Value) -> (Value, bool) {
    let yaml = serde_yaml::to_string(&json!({"alias": format!("parity {n}"), "sequence": [step]})).unwrap();
    let imported = puzzlebox_yaml_import::import_yaml(&yaml).unwrap().remove(0);
    let left = imported.workspace.to_string().contains("\"ha_template");
    let id = format!("p{n}");
    let mut events = engine.subscribe();
    let record = ProgramRecord {
        id: id.clone(),
        name: id.clone(),
        description: String::new(),
        enabled: true,
        mode: "single".into(),
        max_runs: 1,
        workspace: imported.workspace,
        folder: String::new(),
        created_at: 0,
        updated_at: 0,
    };
    let (_, errors) = engine.save(record).await.unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(engine.run_now(&id, None, None));
    loop {
        let ev = timeout(Duration::from_secs(10), events.recv()).await.unwrap().unwrap();
        if let EngineEvent::RunFinished { program_id, status, error, .. } = ev
            && program_id == id
        {
            assert_eq!(status, "ok", "{error:?}");
            break;
        }
    }
    let run = engine.store().list_runs(&id, 1).await.unwrap().remove(0);
    let trace = engine.store().get_run(&run.id).await.unwrap().unwrap().trace.unwrap();
    (trace["variables"]["v"].clone(), left)
}

/// Numbers compare by value (HA keeps 16 and 16.0 apart only in how they print).
fn same(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() < 1e-9,
        _ => a == b,
    }
}

#[tokio::test]
async fn converted_templates_match_home_assistant() {
    let (Ok(url), Ok(token)) = (std::env::var("HA_WS_URL"), std::env::var("HA_TOKEN")) else {
        eprintln!("skipping: HA_WS_URL/HA_TOKEN not set");
        return;
    };
    let client = HaClient::spawn(ClientConfig::new(url, token));
    timeout(Duration::from_secs(10), client.wait_connected()).await.unwrap().unwrap();
    let live = LiveHa::new(client.clone());
    // The engine reads states from its cache, which fills right after connecting.
    while live.state("sensor.outside_temperature").is_none() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let engine = Engine::new(live, Store::open_in_memory().unwrap()).await.unwrap();

    let cases = VALUES.iter().map(|t| json!({"variables": {"v": t}})).chain(CONDITIONS.iter().map(
        |t| json!({"if": [t], "then": [{"variables": {"v": true}}], "else": [{"variables": {"v": false}}]}),
    ));
    let (mut differ, mut kept) = (vec![], vec![]);
    for (n, step) in cases.enumerate() {
        let expected = ha_side(&client, step.clone()).await;
        let (got, left) = engine_side(&engine, n, &step).await;
        if left {
            kept.push(step.to_string());
        }
        if !same(&expected, &got) {
            differ.push(format!("{step}\n    Home Assistant: {expected}\n    blocks:         {got}"));
        }
    }
    eprintln!(
        "{} cases, {} kept as templates:\n  {}",
        VALUES.len() + CONDITIONS.len(),
        kept.len(),
        kept.join("\n  ")
    );
    assert!(differ.is_empty(), "results differ:\n{}", differ.join("\n"));
}
