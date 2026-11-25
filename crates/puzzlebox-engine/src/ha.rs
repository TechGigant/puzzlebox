//! The engine's view of Home Assistant. A trait so the interpreter can be tested without HA.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use futures_util::Stream;
use puzzlebox_ha_client::{ConnectionState, HaClient};
use serde_json::{Value, json};
use tracing::warn;

pub type TriggerStream = Pin<Box<dyn Stream<Item = Value> + Send>>;

#[async_trait]
pub trait HaApi: Send + Sync + 'static {
    async fn call_service(
        &self,
        service: &str,
        data: Option<Value>,
        target: Option<Value>,
    ) -> Result<Value, String>;
    /// Calls an action with `return_response` and returns the response.
    async fn call_service_response(
        &self,
        service: &str,
        _data: Option<Value>,
        _target: Option<Value>,
    ) -> Result<Value, String> {
        Err(format!("{service}: responses are not supported"))
    }
    /// Cached full state object (`{entity_id, state, attributes, ...}`).
    fn state(&self, entity_id: &str) -> Option<Value>;
    async fn test_condition(&self, condition: Value, variables: Value) -> Result<bool, String>;
    async fn execute_script(&self, sequence: Value, variables: Value) -> Result<Value, String>;
    async fn render_template(&self, template: &str, variables: Value) -> Result<Value, String>;
    async fn fire_event(&self, event_type: &str, data: Option<Value>) -> Result<(), String>;
    /// Each item is the trigger payload (HA's `trigger` variable).
    async fn subscribe_trigger(&self, trigger: Value) -> Result<TriggerStream, String>;
}

/// [`HaApi`] backed by the live WebSocket client, with an entity state cache.
pub struct LiveHa {
    client: HaClient,
    states: Arc<RwLock<HashMap<String, Value>>>,
}

impl LiveHa {
    pub fn new(client: HaClient) -> Arc<Self> {
        let states = Arc::new(RwLock::new(HashMap::new()));
        tokio::spawn(maintain_cache(client.clone(), states.clone()));
        Arc::new(Self { client, states })
    }

    pub fn client(&self) -> &HaClient {
        &self.client
    }

    pub fn all_states(&self) -> Vec<Value> {
        self.states.read().unwrap_or_else(|p| p.into_inner()).values().cloned().collect()
    }
}

async fn maintain_cache(client: HaClient, states: Arc<RwLock<HashMap<String, Value>>>) {
    let mut changes = match client.subscribe_events(Some("state_changed")).await {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "state cache disabled");
            return;
        }
    };
    let mut conn = client.state_watch();
    loop {
        // (Re)load everything on each connect: events missed while offline are lost.
        if *conn.borrow_and_update() == ConnectionState::Connected {
            match client.get_states().await {
                Ok(list) => {
                    let mut map = states.write().unwrap_or_else(|p| p.into_inner());
                    map.clear();
                    for s in list {
                        map.insert(s.entity_id.clone(), serde_json::to_value(s).unwrap_or_default());
                    }
                }
                Err(e) => warn!(error = %e, "failed to load states"),
            }
        }
        loop {
            tokio::select! {
                ev = changes.next() => {
                    let Some(ev) = ev else { return };
                    let data = &ev["data"];
                    let Some(id) = data["entity_id"].as_str() else { continue };
                    let mut map = states.write().unwrap_or_else(|p| p.into_inner());
                    if data["new_state"].is_null() {
                        map.remove(id);
                    } else {
                        map.insert(id.to_owned(), data["new_state"].clone());
                    }
                }
                changed = conn.changed() => {
                    if changed.is_err() { return }
                    break;
                }
            }
        }
    }
}

fn split_service(service: &str) -> Result<(&str, &str), String> {
    service.split_once('.').ok_or_else(|| format!("invalid action '{service}'"))
}

#[async_trait]
impl HaApi for LiveHa {
    async fn call_service(
        &self,
        service: &str,
        data: Option<Value>,
        target: Option<Value>,
    ) -> Result<Value, String> {
        let (domain, svc) = split_service(service)?;
        let mut payload = json!({"type": "call_service", "domain": domain, "service": svc});
        if let Some(d) = data {
            payload["service_data"] = d;
        }
        if let Some(t) = target {
            payload["target"] = t;
        }
        self.client.request(payload).await.map_err(|e| e.to_string())
    }

    async fn call_service_response(
        &self,
        service: &str,
        data: Option<Value>,
        target: Option<Value>,
    ) -> Result<Value, String> {
        let (domain, svc) = split_service(service)?;
        let mut payload =
            json!({"type": "call_service", "domain": domain, "service": svc, "return_response": true});
        if let Some(d) = data {
            payload["service_data"] = d;
        }
        if let Some(t) = target {
            payload["target"] = t;
        }
        let res = self.client.request(payload).await.map_err(|e| e.to_string())?;
        Ok(res.get("response").cloned().unwrap_or(Value::Null))
    }

    fn state(&self, entity_id: &str) -> Option<Value> {
        self.states.read().unwrap_or_else(|p| p.into_inner()).get(entity_id).cloned()
    }

    async fn test_condition(&self, condition: Value, variables: Value) -> Result<bool, String> {
        self.client.test_condition(condition, Some(variables)).await.map_err(|e| e.to_string())
    }

    async fn execute_script(&self, sequence: Value, variables: Value) -> Result<Value, String> {
        self.client.execute_script(sequence, Some(variables)).await.map_err(|e| e.to_string())
    }

    async fn render_template(&self, template: &str, variables: Value) -> Result<Value, String> {
        self.client.render_template(template, Some(variables)).await.map_err(|e| e.to_string())
    }

    async fn fire_event(&self, event_type: &str, data: Option<Value>) -> Result<(), String> {
        self.client.fire_event(event_type, data).await.map(|_| ()).map_err(|e| e.to_string())
    }

    async fn subscribe_trigger(&self, trigger: Value) -> Result<TriggerStream, String> {
        let sub = self.client.subscribe_trigger(trigger, None).await.map_err(|e| e.to_string())?;
        Ok(Box::pin(futures_util::stream::unfold(sub, |mut sub| async move {
            let ev = sub.next().await?;
            let trig = ev.pointer("/variables/trigger").cloned().unwrap_or(ev);
            Some((trig, sub))
        })))
    }
}
