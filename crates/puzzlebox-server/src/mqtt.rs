//! MQTT trigger source and publisher.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;

use puzzlebox_engine::{TriggerSink, TriggerSource};
use puzzlebox_ir::Trigger;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use serde_json::{Value, json};
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct MqttConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl MqttConfig {
    /// `mqtt://user:pass@host:1883`
    pub fn parse(url: &str) -> Option<Self> {
        let rest = url.strip_prefix("mqtt://").or_else(|| url.strip_prefix("tcp://"))?;
        let (auth, hostport) = match rest.rsplit_once('@') {
            Some((a, h)) => (Some(a), h),
            None => (None, rest),
        };
        let (host, port) = match hostport.trim_end_matches('/').rsplit_once(':') {
            Some((h, p)) => (h.to_owned(), p.parse().ok()?),
            None => (hostport.trim_end_matches('/').to_owned(), 1883),
        };
        let (username, password) = match auth.map(|a| a.split_once(':')) {
            Some(Some((u, p))) => (Some(u.to_owned()), Some(p.to_owned())),
            Some(None) => (auth.map(str::to_owned), None),
            None => (None, None),
        };
        Some(Self { host, port, username, password })
    }
}

/// MQTT topic filter match (`+` one level, `#` the rest).
pub fn topic_matches(filter: &str, topic: &str) -> bool {
    let mut f = filter.split('/');
    let mut t = topic.split('/');
    loop {
        match (f.next(), t.next()) {
            (Some("#"), _) => return true,
            (Some("+"), Some(_)) => {}
            (Some(a), Some(b)) if a == b => {}
            (None, None) => return true,
            _ => return false,
        }
    }
}

struct Sub {
    program_id: String,
    filter: String,
    sink: TriggerSink,
}

pub struct Mqtt {
    client: Option<AsyncClient>,
    subs: Mutex<Vec<Sub>>,
    subscribed: Mutex<HashSet<String>>,
}

impl Mqtt {
    pub fn disabled() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            client: None,
            subs: Mutex::new(Vec::new()),
            subscribed: Mutex::new(HashSet::new()),
        })
    }

    pub fn connect(cfg: MqttConfig) -> std::sync::Arc<Self> {
        let id = format!("puzzlebox-{:08x}", rand::random::<u32>());
        let mut opts = MqttOptions::new(id, cfg.host.clone(), cfg.port);
        opts.set_keep_alive(Duration::from_secs(30));
        if let Some(u) = &cfg.username {
            opts.set_credentials(u.clone(), cfg.password.clone().unwrap_or_default());
        }
        let (client, mut eventloop) = AsyncClient::new(opts, 100);
        let this = std::sync::Arc::new(Self {
            client: Some(client),
            subs: Mutex::new(Vec::new()),
            subscribed: Mutex::new(HashSet::new()),
        });
        let weak = std::sync::Arc::downgrade(&this);
        tokio::spawn(async move {
            loop {
                let ev = eventloop.poll().await;
                let Some(this) = weak.upgrade() else { return };
                match ev {
                    Ok(Event::Incoming(Packet::ConnAck(_))) => {
                        info!(host = %cfg.host, "MQTT connected");
                        // Broker sessions are clean: re-subscribe everything.
                        this.subscribed.lock().unwrap_or_else(|p| p.into_inner()).clear();
                        this.sync_subscriptions().await;
                    }
                    Ok(Event::Incoming(Packet::Publish(p))) => this.deliver(&p.topic, &p.payload, p.retain),
                    Ok(_) => {}
                    Err(e) => {
                        warn!("MQTT connection error: {e}");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        });
        this
    }

    pub fn enabled(&self) -> bool {
        self.client.is_some()
    }

    fn deliver(&self, topic: &str, payload: &[u8], retain: bool) {
        let text = String::from_utf8_lossy(payload).into_owned();
        let json_payload = serde_json::from_str::<Value>(&text).ok();
        let msg = json!({"platform": "mqtt", "topic": topic, "payload": text, "payload_json": json_payload, "retain": retain});
        for s in self.subs.lock().unwrap_or_else(|p| p.into_inner()).iter() {
            if topic_matches(&s.filter, topic) {
                (s.sink)(msg.clone());
            }
        }
    }

    async fn sync_subscriptions(&self) {
        let Some(client) = &self.client else { return };
        let wanted: HashSet<String> =
            self.subs.lock().unwrap_or_else(|p| p.into_inner()).iter().map(|s| s.filter.clone()).collect();
        let current = self.subscribed.lock().unwrap_or_else(|p| p.into_inner()).clone();
        for f in wanted.difference(&current) {
            if client.subscribe(f.clone(), QoS::AtLeastOnce).await.is_ok() {
                self.subscribed.lock().unwrap_or_else(|p| p.into_inner()).insert(f.clone());
            }
        }
        for f in current.difference(&wanted) {
            if client.unsubscribe(f.clone()).await.is_ok() {
                self.subscribed.lock().unwrap_or_else(|p| p.into_inner()).remove(f);
            }
        }
    }

    pub async fn publish(&self, topic: &str, payload: &Value, retain: bool) -> Result<(), String> {
        let client = self.client.as_ref().ok_or("MQTT is not configured")?;
        let bytes = match payload {
            Value::String(s) => s.clone().into_bytes(),
            other => other.to_string().into_bytes(),
        };
        client.publish(topic, QoS::AtLeastOnce, retain, bytes).await.map_err(|e| e.to_string())
    }
}

/// Trigger-source adapter (the trait lives in another crate, so it can't be implemented on `Arc<Mqtt>`).
pub struct MqttSource(pub std::sync::Arc<Mqtt>);

impl TriggerSource for MqttSource {
    fn register(&self, program_id: &str, trigger: &Trigger, sink: TriggerSink) -> Result<(), String> {
        let Trigger::Mqtt { topic, .. } = trigger else { return Err("not an MQTT trigger".into()) };
        if !self.0.enabled() {
            return Err(
                "MQTT is not configured (install the Mosquitto broker app or set PUZZLEBOX_MQTT_URL)".into(),
            );
        }
        self.0.subs.lock().unwrap_or_else(|p| p.into_inner()).push(Sub {
            program_id: program_id.to_owned(),
            filter: topic.clone(),
            sink,
        });
        let this = self.0.clone();
        tokio::spawn(async move { this.sync_subscriptions().await });
        Ok(())
    }

    fn unregister_program(&self, program_id: &str) {
        self.0.subs.lock().unwrap_or_else(|p| p.into_inner()).retain(|s| s.program_id != program_id);
        let this = self.0.clone();
        tokio::spawn(async move { this.sync_subscriptions().await });
    }
}

/// MQTT broker from the Supervisor's `mqtt` service (Mosquitto app), if available.
pub async fn supervisor_mqtt(token: &str) -> Option<MqttConfig> {
    let res: Value = reqwest::Client::new()
        .get("http://supervisor/services/mqtt")
        .bearer_auth(token)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    let d = res.get("data")?;
    Some(MqttConfig {
        host: d.get("host")?.as_str()?.to_owned(),
        port: d.get("port").and_then(Value::as_u64).unwrap_or(1883) as u16,
        username: d.get("username").and_then(Value::as_str).map(str::to_owned),
        password: d.get("password").and_then(Value::as_str).map(str::to_owned),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_filters() {
        assert!(topic_matches("home/+/state", "home/kitchen/state"));
        assert!(!topic_matches("home/+/state", "home/kitchen/light/state"));
        assert!(topic_matches("home/#", "home/kitchen/light/state"));
        assert!(topic_matches("a/b", "a/b"));
        assert!(!topic_matches("a/b", "a/b/c"));
    }

    #[test]
    fn parses_urls() {
        let c = MqttConfig::parse("mqtt://u:p@broker:1884").unwrap();
        assert_eq!(
            (c.host.as_str(), c.port, c.username.as_deref(), c.password.as_deref()),
            ("broker", 1884, Some("u"), Some("p"))
        );
        assert_eq!(MqttConfig::parse("mqtt://broker").unwrap().port, 1883);
    }
}
