//! Zigbee2MQTT: generates blocks from `<base>/bridge/devices` and `<base>/bridge/groups`
//! and caches each device's last published state for value blocks.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tracing::info;

use crate::mqtt::Mqtt;
use crate::registry::RegistryService;

/// How long after connecting a missing `bridge/devices` (it is retained) means no
/// Zigbee2MQTT uses that base topic.
const NOT_FOUND_AFTER: Duration = Duration::from_secs(10);

pub struct Z2m {
    mqtt: Arc<Mqtt>,
    /// Device topic (`<base>/<friendly name>`) → last JSON payload.
    states: Mutex<HashMap<String, Value>>,
    bridges: Mutex<Vec<(String, Arc<Mutex<Bridge>>)>>,
}

#[derive(Default)]
struct Bridge {
    devices: Value,
    groups: Value,
}

impl Z2m {
    pub fn start(mqtt: &Arc<Mqtt>, registry: Arc<RegistryService>, bases: Vec<String>) -> Arc<Self> {
        let this = Arc::new(Self {
            mqtt: mqtt.clone(),
            states: Mutex::new(HashMap::new()),
            bridges: Mutex::new(Vec::new()),
        });
        if !mqtt.enabled() {
            return this;
        }
        let weak = Arc::downgrade(&this);
        mqtt.set_seed(Arc::new(move |topic: &str, key: &str| {
            weak.upgrade().map(|z| z.value(topic, key)).filter(|v| !v.is_null())
        }));
        for base in bases {
            let base = base.trim_end_matches('/').to_owned();
            let bridge = Arc::new(Mutex::new(Bridge::default()));
            this.bridges.lock().unwrap_or_else(|p| p.into_inner()).push((base.clone(), bridge.clone()));
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();

            let (b, t) = (bridge.clone(), tx.clone());
            let prefix = format!("{base}/bridge/");
            mqtt.listen(
                &format!("{base}/bridge/+"),
                Arc::new(move |topic: &str, payload: &str| {
                    let Some(kind) = topic.strip_prefix(&prefix) else { return };
                    if kind != "devices" && kind != "groups" {
                        return;
                    }
                    let Ok(v) = serde_json::from_str::<Value>(payload) else { return };
                    let mut g = b.lock().unwrap_or_else(|p| p.into_inner());
                    if kind == "devices" {
                        g.devices = v
                    } else {
                        g.groups = v
                    }
                    let _ = t.send(());
                }),
            );

            let states = this.clone();
            let skip = format!("{base}/bridge/");
            mqtt.listen(
                &format!("{base}/#"),
                Arc::new(move |topic: &str, payload: &str| {
                    if topic.starts_with(&skip)
                        || topic.ends_with("/set")
                        || topic.ends_with("/get")
                        || topic.ends_with("/availability")
                    {
                        return;
                    }
                    let Ok(Value::Object(m)) = serde_json::from_str::<Value>(payload) else { return };
                    let mut s = states.states.lock().unwrap_or_else(|p| p.into_inner());
                    // Z2M can publish partial updates, so merge into the previous state.
                    match s.get_mut(topic) {
                        Some(Value::Object(old)) => old.extend(m),
                        _ => {
                            s.insert(topic.to_owned(), Value::Object(m));
                        }
                    }
                }),
            );

            let registry = registry.clone();
            let z2m = this.clone();
            tokio::spawn(async move {
                let mut asked = HashSet::new();
                while rx.recv().await.is_some() {
                    // Both retained messages usually arrive together; regenerate once.
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    while rx.try_recv().is_ok() {}
                    let (blocks, gets) = {
                        let g = bridge.lock().unwrap_or_else(|p| p.into_inner());
                        (
                            puzzlebox_registry::z2m::generate(&base, &g.devices, &g.groups),
                            puzzlebox_registry::z2m::get_requests(&base, &g.devices),
                        )
                    };
                    info!(base = %base, blocks = blocks.len(), "Zigbee2MQTT blocks updated");
                    registry.set_generated_source(&format!("z2m:{base}"), blocks).await;
                    z2m.ask(gets, &mut asked).await;
                }
            });
        }
        this
    }

    /// Device states aren't retained, so after a restart value blocks would stay empty until
    /// each device reports; ask the ones that can answer, once per device.
    async fn ask(&self, gets: Vec<(String, Value)>, asked: &mut HashSet<String>) {
        for (topic, payload) in gets {
            let known = self.states.lock().unwrap_or_else(|p| p.into_inner()).contains_key(&topic);
            if known || !asked.insert(topic.clone()) {
                continue;
            }
            let _ = self.mqtt.publish(&format!("{topic}/get"), &payload, false).await;
            // Paced, so a large network isn't flooded with reads at once.
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub fn value(&self, topic: &str, key: &str) -> Value {
        self.states
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(topic)
            .and_then(|s| s.get(key))
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// One entry per base topic, for Settings.
    pub fn status(&self) -> Value {
        let connected = self.mqtt.connected_at();
        let bridges = self.bridges.lock().unwrap_or_else(|p| p.into_inner());
        let list: Vec<Value> = bridges
            .iter()
            .map(|(base, b)| {
                let b = b.lock().unwrap_or_else(|p| p.into_inner());
                let count = |v: &Value, devices: bool| {
                    v.as_array().map(|a| {
                        a.iter()
                            .filter(|d| {
                                !devices || d.get("type").and_then(Value::as_str) != Some("Coordinator")
                            })
                            .count()
                    })
                };
                let devices = count(&b.devices, true);
                let state = match (devices, connected) {
                    (Some(_), _) => "ok",
                    (None, None) => "disconnected",
                    (None, Some(at)) if at.elapsed() < NOT_FOUND_AFTER => "waiting",
                    (None, Some(_)) => "not_found",
                };
                json!({"base": base, "state": state, "devices": devices, "groups": count(&b.groups, false)})
            })
            .collect();
        json!(list)
    }
}
