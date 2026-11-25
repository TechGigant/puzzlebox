use std::collections::HashMap;
use std::time::Duration;

use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, MissedTickBehavior, interval, sleep};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tracing::{debug, info, warn};

use crate::error::{Error, Result};
use crate::types::ConnectionState;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub(crate) type SubId = u64;

pub(crate) enum Command {
    Request { payload: Value, reply: oneshot::Sender<Result<Value>> },
    Subscribe { payload: Value, events: mpsc::UnboundedSender<Value>, reply: oneshot::Sender<Result<SubId>> },
    Unsubscribe { sub: SubId },
}

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub url: String,
    pub token: String,
    pub ping_interval: Duration,
    pub max_backoff: Duration,
}

impl ClientConfig {
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            token: token.into(),
            ping_interval: Duration::from_secs(30),
            max_backoff: Duration::from_secs(30),
        }
    }
}

struct Subscription {
    payload: Value,
    events: mpsc::UnboundedSender<Value>,
}

enum Pending {
    Request(oneshot::Sender<Result<Value>>),
    Subscribe(SubId, Option<oneshot::Sender<Result<SubId>>>),
    Ignore,
}

/// Owns the socket. Subscriptions outlive individual connections and are
/// re-sent after every reconnect so callers never see a gap other than missed events.
pub(crate) struct Actor {
    cfg: ClientConfig,
    rx: mpsc::UnboundedReceiver<Command>,
    state: watch::Sender<ConnectionState>,
    ha_version: watch::Sender<Option<String>>,
    subs: HashMap<SubId, Subscription>,
    next_sub: SubId,
}

enum SessionEnd {
    Lost,
    Shutdown,
}

impl Actor {
    pub(crate) fn new(
        cfg: ClientConfig,
        rx: mpsc::UnboundedReceiver<Command>,
        state: watch::Sender<ConnectionState>,
        ha_version: watch::Sender<Option<String>>,
    ) -> Self {
        Self { cfg, rx, state, ha_version, subs: HashMap::new(), next_sub: 1 }
    }

    pub(crate) async fn run(mut self) {
        let mut backoff = Duration::from_millis(500);
        loop {
            self.state.send_replace(ConnectionState::Connecting);
            match connect_and_auth(&self.cfg).await {
                Ok((ws, version)) => {
                    info!(%version, "connected to Home Assistant");
                    backoff = Duration::from_millis(500);
                    self.ha_version.send_replace(Some(version));
                    self.state.send_replace(ConnectionState::Connected);
                    let end = self.session(ws).await;
                    self.state.send_replace(ConnectionState::Disconnected);
                    if matches!(end, SessionEnd::Shutdown) {
                        return;
                    }
                    warn!("connection to Home Assistant lost");
                }
                Err(Error::Auth(msg)) => {
                    warn!(%msg, "Home Assistant rejected the token");
                    self.state.send_replace(ConnectionState::Disconnected);
                    backoff = self.cfg.max_backoff;
                }
                Err(e) => {
                    warn!(error = %e, "failed to connect to Home Assistant");
                    self.state.send_replace(ConnectionState::Disconnected);
                }
            }
            if !self.wait_offline(backoff).await {
                return;
            }
            backoff = (backoff * 2).min(self.cfg.max_backoff);
        }
    }

    /// Serves commands while disconnected. Returns false when every client handle is gone.
    async fn wait_offline(&mut self, dur: Duration) -> bool {
        let deadline = sleep(dur);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => return true,
                cmd = self.rx.recv() => match cmd {
                    None => return false,
                    Some(Command::Request { reply, .. }) => { let _ = reply.send(Err(Error::NotConnected)); }
                    Some(Command::Subscribe { payload, events, reply }) => {
                        let id = self.add_sub(payload, events);
                        let _ = reply.send(Ok(id));
                    }
                    Some(Command::Unsubscribe { sub }) => { self.subs.remove(&sub); }
                },
            }
        }
    }

    fn add_sub(&mut self, payload: Value, events: mpsc::UnboundedSender<Value>) -> SubId {
        let id = self.next_sub;
        self.next_sub += 1;
        self.subs.insert(id, Subscription { payload, events });
        id
    }

    async fn session(&mut self, ws: Ws) -> SessionEnd {
        let (mut sink, mut stream) = ws.split();
        // Id 1 was used for `supported_features` during the handshake.
        let mut next_id: u64 = 2;
        let mut pending: HashMap<u64, Pending> = HashMap::new();
        let mut ws_to_sub: HashMap<u64, SubId> = HashMap::new();

        let resub: Vec<(SubId, Value)> = self.subs.iter().map(|(id, s)| (*id, s.payload.clone())).collect();
        for (sub, payload) in resub {
            let id = next_id;
            next_id += 1;
            pending.insert(id, Pending::Subscribe(sub, None));
            ws_to_sub.insert(id, sub);
            if send_msg(&mut sink, id, &payload).await.is_err() {
                return SessionEnd::Lost;
            }
        }

        let mut ping = interval(self.cfg.ping_interval);
        ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
        ping.tick().await;
        let mut last_pong = Instant::now();
        let mut ping_ids: Vec<u64> = Vec::new();

        let end = loop {
            tokio::select! {
                cmd = self.rx.recv() => {
                    let Some(cmd) = cmd else { break SessionEnd::Shutdown };
                    let id = next_id;
                    next_id += 1;
                    let payload = match cmd {
                        Command::Request { payload, reply } => {
                            pending.insert(id, Pending::Request(reply));
                            payload
                        }
                        Command::Subscribe { payload, events, reply } => {
                            let sub = self.add_sub(payload.clone(), events);
                            pending.insert(id, Pending::Subscribe(sub, Some(reply)));
                            ws_to_sub.insert(id, sub);
                            payload
                        }
                        Command::Unsubscribe { sub } => {
                            self.subs.remove(&sub);
                            let Some(ws_id) = ws_to_sub.iter().find(|(_, s)| **s == sub).map(|(k, _)| *k) else { continue };
                            ws_to_sub.remove(&ws_id);
                            pending.insert(id, Pending::Ignore);
                            json!({"type": "unsubscribe_events", "subscription": ws_id})
                        }
                    };
                    if send_msg(&mut sink, id, &payload).await.is_err() {
                        break SessionEnd::Lost;
                    }
                }
                _ = ping.tick() => {
                    if last_pong.elapsed() > self.cfg.ping_interval * 2 {
                        warn!("no pong from Home Assistant, reconnecting");
                        break SessionEnd::Lost;
                    }
                    let id = next_id;
                    next_id += 1;
                    ping_ids.push(id);
                    if send_msg(&mut sink, id, &json!({"type": "ping"})).await.is_err() {
                        break SessionEnd::Lost;
                    }
                }
                msg = stream.next() => {
                    let text = match msg {
                        Some(Ok(Message::Text(t))) => t,
                        Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break SessionEnd::Lost,
                        Some(Ok(_)) => continue,
                    };
                    let parsed: Value = match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(e) => { warn!(error = %e, "invalid JSON from Home Assistant"); continue }
                    };
                    // HA may coalesce messages into a JSON array.
                    let msgs = match parsed { Value::Array(a) => a, v => vec![v] };
                    for m in msgs {
                        self.handle_message(m, &mut pending, &mut ws_to_sub, &mut ping_ids, &mut last_pong);
                    }
                }
            }
        };

        for (_, p) in pending.drain() {
            match p {
                Pending::Request(r) => {
                    let _ = r.send(Err(Error::Disconnected));
                }
                Pending::Subscribe(sub, Some(r)) => {
                    // Kept registered: it will be re-sent on reconnect.
                    let _ = r.send(Ok(sub));
                }
                _ => {}
            }
        }
        end
    }

    fn handle_message(
        &mut self,
        m: Value,
        pending: &mut HashMap<u64, Pending>,
        ws_to_sub: &mut HashMap<u64, SubId>,
        ping_ids: &mut Vec<u64>,
        last_pong: &mut Instant,
    ) {
        let Some(id) = m.get("id").and_then(Value::as_u64) else { return };
        match m.get("type").and_then(Value::as_str) {
            Some("pong") => {
                ping_ids.retain(|p| *p != id);
                *last_pong = Instant::now();
            }
            Some("event") => {
                let Some(sub) = ws_to_sub.get(&id) else { return };
                let event = m.get("event").cloned().unwrap_or(Value::Null);
                if let Some(s) = self.subs.get(sub)
                    && s.events.send(event).is_err()
                {
                    debug!(sub, "subscriber dropped");
                }
            }
            Some("result") => {
                let success = m.get("success").and_then(Value::as_bool).unwrap_or(false);
                match pending.remove(&id) {
                    Some(Pending::Request(reply)) => {
                        let res = if success {
                            Ok(m.get("result").cloned().unwrap_or(Value::Null))
                        } else {
                            Err(Error::from_ha_error(m.get("error")))
                        };
                        let _ = reply.send(res);
                    }
                    Some(Pending::Subscribe(sub, reply)) => {
                        if success {
                            if let Some(r) = reply {
                                let _ = r.send(Ok(sub));
                            }
                        } else {
                            let err = Error::from_ha_error(m.get("error"));
                            warn!(sub, error = %err, "subscription rejected");
                            self.subs.remove(&sub);
                            ws_to_sub.remove(&id);
                            if let Some(r) = reply {
                                let _ = r.send(Err(err));
                            }
                        }
                    }
                    Some(Pending::Ignore) | None => {}
                }
            }
            _ => {}
        }
    }
}

async fn send_msg(sink: &mut SplitSink<Ws, Message>, id: u64, payload: &Value) -> Result<()> {
    let mut msg = payload.clone();
    msg["id"] = json!(id);
    sink.send(Message::Text(msg.to_string())).await?;
    Ok(())
}

async fn connect_and_auth(cfg: &ClientConfig) -> Result<(Ws, String)> {
    let (mut ws, _) = connect_async(cfg.url.as_str()).await?;
    let first = next_json(&mut ws).await?;
    if first.get("type").and_then(Value::as_str) != Some("auth_required") {
        return Err(Error::Protocol(format!("expected auth_required, got {first}")));
    }
    ws.send(Message::Text(json!({"type": "auth", "access_token": cfg.token}).to_string())).await?;
    let reply = next_json(&mut ws).await?;
    match reply.get("type").and_then(Value::as_str) {
        Some("auth_ok") => {
            let version = reply.get("ha_version").and_then(Value::as_str).unwrap_or("unknown").to_owned();
            ws.send(Message::Text(
                json!({"id": 1, "type": "supported_features", "features": {"coalesce_messages": 1}})
                    .to_string(),
            ))
            .await?;
            Ok((ws, version))
        }
        Some("auth_invalid") => Err(Error::Auth(
            reply.get("message").and_then(Value::as_str).unwrap_or("invalid token").to_owned(),
        )),
        _ => Err(Error::Protocol(format!("unexpected auth reply {reply}"))),
    }
}

async fn next_json(ws: &mut Ws) -> Result<Value> {
    loop {
        match ws.next().await {
            Some(Ok(Message::Text(t))) => return Ok(serde_json::from_str(&t)?),
            Some(Ok(Message::Close(_))) | None => return Err(Error::Disconnected),
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(e.into()),
        }
    }
}
