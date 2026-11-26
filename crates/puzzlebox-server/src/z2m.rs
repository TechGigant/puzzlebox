//! Zigbee2MQTT: generates blocks from `<base>/bridge/devices` and `<base>/bridge/groups`
//! and caches each device's last published state for value blocks.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tracing::info;

use crate::mqtt::Mqtt;
use crate::registry::RegistryService;

#[derive(Default)]
pub struct Z2m {
    /// Device topic (`<base>/<friendly name>`) → last JSON payload.
    states: Mutex<HashMap<String, Value>>,
}

#[derive(Default)]
struct Bridge {
    devices: Value,
    groups: Value,
}

impl Z2m {
    pub fn start(mqtt: &Arc<Mqtt>, registry: Arc<RegistryService>, bases: Vec<String>) -> Arc<Self> {
        let this = Arc::new(Self::default());
        if !mqtt.enabled() {
            return this;
        }
        for base in bases {
            let base = base.trim_end_matches('/').to_owned();
            let bridge = Arc::new(Mutex::new(Bridge::default()));
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
            tokio::spawn(async move {
                while rx.recv().await.is_some() {
                    // Both retained messages usually arrive together; regenerate once.
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    while rx.try_recv().is_ok() {}
                    let blocks = {
                        let g = bridge.lock().unwrap_or_else(|p| p.into_inner());
                        puzzlebox_registry::z2m::generate(&base, &g.devices, &g.groups)
                    };
                    info!(base = %base, blocks = blocks.len(), "Zigbee2MQTT blocks updated");
                    registry.set_generated_source(&format!("z2m:{base}"), blocks).await;
                }
            });
        }
        this
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
}
