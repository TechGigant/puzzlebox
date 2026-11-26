//! Home Assistant WebSocket API client.
//!
//! A single background task owns the socket, reconnects with backoff and
//! transparently re-establishes every live subscription.

mod connection;
mod error;
mod types;

use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot, watch};

pub use connection::ClientConfig;
use connection::{Actor, Command, SubId};
pub use error::{Error, Result};
pub use types::{ConnectionState, EntityState, Target};

pub const SUPERVISOR_WS_URL: &str = "ws://supervisor/core/websocket";

#[derive(Clone)]
pub struct HaClient {
    tx: mpsc::UnboundedSender<Command>,
    state: watch::Receiver<ConnectionState>,
    ha_version: watch::Receiver<Option<String>>,
}

impl HaClient {
    /// Spawns the connection task. Returns immediately; use [`HaClient::wait_connected`]
    /// to block until the first successful authentication.
    pub fn spawn(cfg: ClientConfig) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let (state_tx, state) = watch::channel(ConnectionState::Connecting);
        let (ver_tx, ha_version) = watch::channel(None);
        tokio::spawn(Actor::new(cfg, rx, state_tx, ver_tx).run());
        Self { tx, state, ha_version }
    }

    pub fn state(&self) -> ConnectionState {
        *self.state.borrow()
    }

    pub fn state_watch(&self) -> watch::Receiver<ConnectionState> {
        self.state.clone()
    }

    pub fn ha_version(&self) -> Option<String> {
        self.ha_version.borrow().clone()
    }

    pub async fn wait_connected(&self) -> Result<()> {
        let mut rx = self.state.clone();
        rx.wait_for(|s| *s == ConnectionState::Connected).await.map_err(|_| Error::Closed)?;
        Ok(())
    }

    /// Sends a raw command (without `id`) and returns its `result` payload.
    pub async fn request(&self, payload: Value) -> Result<Value> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Command::Request { payload, reply }).map_err(|_| Error::Closed)?;
        rx.await.map_err(|_| Error::Closed)?
    }

    /// Starts a raw subscription command (without `id`).
    pub async fn subscribe(&self, payload: Value) -> Result<Subscription> {
        let (events, rx) = mpsc::unbounded_channel();
        let (reply, reply_rx) = oneshot::channel();
        self.tx.send(Command::Subscribe { payload, events, reply }).map_err(|_| Error::Closed)?;
        let id = reply_rx.await.map_err(|_| Error::Closed)??;
        Ok(Subscription { id, rx, tx: self.tx.clone() })
    }

    pub async fn get_states(&self) -> Result<Vec<EntityState>> {
        Ok(serde_json::from_value(self.request(json!({"type": "get_states"})).await?)?)
    }

    pub async fn get_config(&self) -> Result<Value> {
        self.request(json!({"type": "get_config"})).await
    }

    pub async fn get_services(&self) -> Result<Value> {
        self.request(json!({"type": "get_services"})).await
    }

    pub async fn call_service(
        &self,
        domain: &str,
        service: &str,
        data: Option<Value>,
        target: Option<Target>,
        return_response: bool,
    ) -> Result<Value> {
        let mut payload = json!({"type": "call_service", "domain": domain, "service": service});
        if let Some(d) = data {
            payload["service_data"] = d;
        }
        if let Some(t) = target {
            payload["target"] = serde_json::to_value(t)?;
        }
        if return_response {
            payload["return_response"] = json!(true);
        }
        self.request(payload).await
    }

    pub async fn fire_event(&self, event_type: &str, data: Option<Value>) -> Result<Value> {
        let mut payload = json!({"type": "fire_event", "event_type": event_type});
        if let Some(d) = data {
            payload["event_data"] = d;
        }
        self.request(payload).await
    }

    /// Evaluates any native HA condition config.
    pub async fn test_condition(&self, condition: Value, variables: Option<Value>) -> Result<bool> {
        let mut payload = json!({"type": "test_condition", "condition": condition});
        if let Some(v) = variables {
            payload["variables"] = v;
        }
        let res = self.request(payload).await?;
        res.get("result")
            .and_then(Value::as_bool)
            .ok_or_else(|| Error::Protocol(format!("test_condition returned {res}")))
    }

    /// Runs a native HA action sequence and returns its response (if any).
    pub async fn execute_script(&self, sequence: Value, variables: Option<Value>) -> Result<Value> {
        let mut payload = json!({"type": "execute_script", "sequence": sequence});
        if let Some(v) = variables {
            payload["variables"] = v;
        }
        self.request(payload).await
    }

    pub async fn subscribe_events(&self, event_type: Option<&str>) -> Result<Subscription> {
        let mut payload = json!({"type": "subscribe_events"});
        if let Some(t) = event_type {
            payload["event_type"] = json!(t);
        }
        self.subscribe(payload).await
    }

    /// Subscribes to any native HA trigger config; each event carries `variables.trigger`.
    pub async fn subscribe_trigger(&self, trigger: Value, variables: Option<Value>) -> Result<Subscription> {
        let mut payload = json!({"type": "subscribe_trigger", "trigger": trigger});
        if let Some(v) = variables {
            payload["variables"] = v;
        }
        self.subscribe(payload).await
    }

    /// Renders a Jinja template; HA pushes a new event whenever the result changes.
    pub async fn subscribe_template(&self, template: &str, variables: Option<Value>) -> Result<Subscription> {
        let mut payload = json!({"type": "render_template", "template": template, "report_errors": true});
        if let Some(v) = variables {
            payload["variables"] = v;
        }
        self.subscribe(payload).await
    }

    /// One-shot template render built on [`HaClient::subscribe_template`].
    pub async fn render_template(&self, template: &str, variables: Option<Value>) -> Result<Value> {
        let mut sub = self.subscribe_template(template, variables).await?;
        let ev = sub.next().await.ok_or(Error::Disconnected)?;
        if let Some(err) = ev.get("error") {
            return Err(Error::Ha {
                code: "template_error".into(),
                message: err.as_str().unwrap_or_default().to_owned(),
                translation_key: None,
            });
        }
        Ok(ev.get("result").cloned().unwrap_or(Value::Null))
    }
}

/// Live subscription. Dropping it unsubscribes.
pub struct Subscription {
    id: SubId,
    rx: mpsc::UnboundedReceiver<Value>,
    tx: mpsc::UnboundedSender<Command>,
}

impl Subscription {
    pub async fn next(&mut self) -> Option<Value> {
        self.rx.recv().await
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Unsubscribe { sub: self.id });
    }
}
