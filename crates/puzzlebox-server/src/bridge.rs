//! Link to the `puzzlebox` companion integration running inside Home Assistant.
//!
//! The companion connects here (token-authenticated) and
//! - registers Python block packs and executes their handlers on request,
//! - mirrors programs as HA entities and fires HA events for runs,
//! - forwards HA webhooks and service calls (run program, set variable).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use puzzlebox_engine::{Engine, EngineEvent};
use puzzlebox_registry::pack::{PackFile, from_file};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc, oneshot};
use tracing::{info, warn};

use crate::compat::{self, Peer};
use crate::registry::RegistryService;
use crate::routes::AppState;
use crate::webhooks::Webhooks;

const INVOKE_TIMEOUT: Duration = Duration::from_secs(30);
/// Runs looked at for "failed N times in a row" (the integration raises a repair at 3).
const FAILURES_COUNTED: usize = 5;

pub struct Bridge {
    engine: Engine,
    registry: Arc<RegistryService>,
    webhooks: Arc<Webhooks>,
    pub token: String,
    cache_dir: PathBuf,
    conn: Mutex<Option<(u64, mpsc::UnboundedSender<Value>)>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    next_id: AtomicU64,
    /// The connected integration's handshake; `None` while disconnected.
    peer: std::sync::Mutex<Option<Peer>>,
    /// Bumped when the connection or the peer changes (the UI shows it).
    changed: tokio::sync::watch::Sender<()>,
}

/// How long the app waits for the integration's hello before treating it as a pre-0.3.0 one.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

fn load_or_create_token(path: &std::path::Path) -> std::io::Result<String> {
    if let Ok(t) = std::fs::read_to_string(path)
        && t.trim().len() >= 32
    {
        return Ok(t.trim().to_owned());
    }
    let token: String = (0..4).map(|_| format!("{:016x}", rand::random::<u64>())).collect();
    std::fs::write(path, &token)?;
    Ok(token)
}

impl Bridge {
    pub async fn new(
        engine: Engine,
        registry: Arc<RegistryService>,
        webhooks: Arc<Webhooks>,
        data_dir: &std::path::Path,
    ) -> anyhow::Result<Arc<Self>> {
        let token = load_or_create_token(&data_dir.join("bridge_token"))?;
        let cache_dir = data_dir.join("py_packs");
        std::fs::create_dir_all(&cache_dir)?;
        let bridge = Arc::new(Self {
            engine,
            registry,
            webhooks,
            token,
            cache_dir,
            conn: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            peer: std::sync::Mutex::new(None),
            changed: tokio::sync::watch::Sender::new(()),
        });
        bridge.load_cached_packs().await;
        Ok(bridge)
    }

    pub async fn connected(&self) -> bool {
        self.conn.lock().await.is_some()
    }

    fn peer(&self) -> Option<Peer> {
        self.peer.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn set_peer(&self, peer: Option<Peer>) {
        *self.peer.lock().unwrap_or_else(|p| p.into_inner()) = peer;
        self.changed.send_replace(());
    }

    pub fn watch(&self) -> tokio::sync::watch::Receiver<()> {
        self.changed.subscribe()
    }

    /// Connection and compatibility of the integration, for the UI.
    pub fn status(&self) -> Value {
        match self.peer() {
            Some(p) => {
                let mut v = p.status();
                v["connected"] = json!(true);
                v
            }
            None => json!({"connected": false}),
        }
    }

    /// Python packs from the last session, so programs compile before HA reconnects.
    async fn load_cached_packs(&self) {
        let Ok(entries) = std::fs::read_dir(&self.cache_dir) else { return };
        for e in entries.flatten() {
            let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
            match serde_json::from_str::<PackFile>(&text)
                .map_err(|e| e.to_string())
                .and_then(|f| from_file(f, "py"))
            {
                Ok(pack) => self.registry.set_plugin_source(&pack.source.clone(), Some(pack)).await,
                Err(err) => warn!(path = %e.path().display(), "ignoring cached plugin pack: {err}"),
            }
        }
    }

    async fn send(&self, msg: Value) -> bool {
        match &*self.conn.lock().await {
            Some((_, tx)) => tx.send(msg).is_ok(),
            None => false,
        }
    }

    /// Runs a Python block handler in Home Assistant.
    pub async fn invoke(
        &self,
        plugin: &str,
        function: &str,
        args: Value,
        context: Value,
    ) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        let msg = json!({"type": "invoke", "id": id, "plugin": plugin, "function": function, "args": args, "context": context});
        if !self.send(msg).await {
            self.pending.lock().await.remove(&id);
            return Err("the Puzzlebox bridge integration is not connected".into());
        }
        match tokio::time::timeout(INVOKE_TIMEOUT, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err("bridge disconnected".into()),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err("plugin call timed out".into())
            }
        }
    }

    async fn programs_snapshot(&self) -> Value {
        let programs = self.engine.store().list_programs().await.unwrap_or_default();
        let paused: Vec<String> = self.engine.debugger().paused_runs().into_iter().map(|(run, _)| run).collect();
        let mut list = Vec::with_capacity(programs.len());
        for p in &programs {
            let active = self.engine.active_runs(&p.id);
            let activity = if active.iter().any(|r| paused.contains(&r.run_id)) {
                Some("paused")
            } else if active.iter().any(|r| r.waiting.is_some()) {
                Some("waiting")
            } else {
                None
            };
            let failures = self
                .engine
                .store()
                .list_runs(&p.id, FAILURES_COUNTED)
                .await
                .map(|runs| runs.iter().take_while(|r| r.status == "error").count())
                .unwrap_or(0);
            list.push(json!({
                "id": p.id, "name": p.name, "enabled": p.enabled, "mode": p.mode,
                "status": self.engine.status(&p.id),
                "activity": activity,
                "failures": failures,
            }));
        }
        json!({"type": "programs", "programs": list})
    }

    async fn handle(&self, msg: Value) {
        let ty = msg["type"].as_str().unwrap_or("");
        match ty {
            "result" | "error" => {
                let Some(id) = msg["id"].as_u64() else { return };
                if let Some(tx) = self.pending.lock().await.remove(&id) {
                    let r = if ty == "result" {
                        Ok(msg.get("result").cloned().unwrap_or(Value::Null))
                    } else {
                        Err(msg["message"].as_str().unwrap_or("plugin error").to_owned())
                    };
                    let _ = tx.send(r);
                }
            }
            "register_pack" => {
                let file = serde_json::from_value::<PackFile>(msg["pack"].clone()).map_err(|e| e.to_string());
                match file.and_then(|f| {
                    let raw = serde_json::to_string(&f).unwrap_or_default();
                    from_file(f, "py").map(|p| (p, raw))
                }) {
                    Ok((pack, raw)) => {
                        info!(pack = %pack.id, blocks = pack.blocks.len(), "plugin pack registered from Home Assistant");
                        let _ = std::fs::write(self.cache_dir.join(format!("{}.json", pack.id)), raw);
                        let source = pack.source.clone();
                        self.registry.set_plugin_source(&source, Some(pack)).await;
                    }
                    Err(e) => {
                        warn!("rejected plugin pack: {e}");
                        self.send(json!({"type": "pack_rejected", "pack": msg["pack"]["pack"], "error": e}))
                            .await;
                    }
                }
            }
            "unregister_pack" => {
                let Some(id) = msg["pack"].as_str() else { return };
                let _ = std::fs::remove_file(self.cache_dir.join(format!("{id}.json")));
                self.registry.set_plugin_source(&format!("py:{id}"), None).await;
            }
            "run" => {
                let Some(id) = msg["program_id"].as_str() else { return };
                let trigger = json!({"platform": "service", "variables": msg.get("variables").cloned().unwrap_or(json!({}))});
                if !self.engine.run_now(id, None, Some(trigger)) {
                    warn!(program = id, "run requested for a program that is not running");
                }
            }
            "set_enabled" => {
                let (Some(id), Some(on)) = (msg["program_id"].as_str(), msg["enabled"].as_bool()) else {
                    return;
                };
                if let Err(e) = self.engine.set_enabled(id, on).await {
                    warn!(error = %e, "set_enabled failed");
                }
            }
            "set_variable" => {
                let Some(name) = msg["name"].as_str() else { return };
                let value = msg.get("value").cloned().unwrap_or(Value::Null);
                if let Err(e) = self.engine.set_variable(msg["program_id"].as_str(), name, value).await {
                    warn!(error = %e, "set_variable failed");
                }
            }
            "webhook" => {
                let id = msg["webhook_id"].as_str().unwrap_or("");
                self.webhooks.dispatch(
                    id,
                    msg["method"].as_str().unwrap_or("POST"),
                    msg.get("request").cloned().unwrap_or_default(),
                );
            }
            _ => {}
        }
    }

    /// Announces this app to Home Assistant so the companion configures itself.
    /// Announces this app to Home Assistant so the companion configures itself. Repeats while
    /// no integration is connected (it may be installed after the app started), replacing the
    /// previous announcement instead of piling up new ones.
    pub fn spawn_discovery(self: &Arc<Self>, supervisor_token: String, port: u16) {
        let this = self.clone();
        tokio::spawn(async move {
            let client = reqwest::Client::new();
            let mut previous: Option<String> = None;
            let mut failures = 0u32;
            loop {
                if !this.connected().await {
                    match this.discover_once(&client, &supervisor_token, port, previous.as_deref()).await {
                        Ok(uuid) => {
                            if previous.is_none() {
                                info!("announced to Home Assistant via Supervisor discovery");
                            }
                            previous = uuid;
                            failures = 0;
                        }
                        Err(e) => {
                            failures += 1;
                            warn!(failures, "Supervisor discovery failed: {e}");
                        }
                    }
                }
                let wait = if failures > 0 { 30 * u64::from(failures.min(10)) } else { 300 };
                tokio::time::sleep(Duration::from_secs(wait)).await;
            }
        });
    }

    async fn discover_once(
        &self,
        client: &reqwest::Client,
        token: &str,
        port: u16,
        previous: Option<&str>,
    ) -> anyhow::Result<Option<String>> {
        let info: Value = client
            .get("http://supervisor/addons/self/info")
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let host = info.pointer("/data/hostname").and_then(Value::as_str).unwrap_or("localhost").to_owned();
        if let Some(uuid) = previous {
            // Best effort: an expired entry is fine to ignore.
            let _ =
                client.delete(format!("http://supervisor/discovery/{uuid}")).bearer_auth(token).send().await;
        }
        let res: Value = client
            .post("http://supervisor/discovery")
            .bearer_auth(token)
            .json(
                &json!({"service": "puzzlebox", "config": {"host": host, "port": port, "token": self.token}}),
            )
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .unwrap_or_default();
        Ok(res.pointer("/data/uuid").and_then(Value::as_str).map(str::to_owned))
    }
}

#[derive(Deserialize)]
pub struct TokenQuery {
    token: Option<String>,
}

pub async fn upgrade(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let bearer = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(str::to_owned);
    let given = bearer.or(q.token).unwrap_or_default();
    // Constant-time compare: the bridge port may be reachable from the LAN in standalone setups.
    let ok = given.len() == st.bridge.token.len()
        && given.bytes().zip(st.bridge.token.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0;
    if !ok {
        return (StatusCode::UNAUTHORIZED, "invalid bridge token").into_response();
    }
    let bridge = st.bridge.clone();
    ws.on_upgrade(move |socket| session(socket, bridge))
}

async fn session(mut socket: WebSocket, bridge: Arc<Bridge>) {
    let conn_id = bridge.next_id.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    *bridge.conn.lock().await = Some((conn_id, tx.clone()));
    info!("Puzzlebox bridge integration connected");

    // Both sides start with their hello; nothing else is sent before the versions are compared.
    if socket.send(Message::Text(compat::hello().to_string().into())).await.is_err() {
        return;
    }
    let (peer, first) = match tokio::time::timeout(HELLO_TIMEOUT, socket.recv()).await {
        Ok(Some(Ok(Message::Text(t)))) => match serde_json::from_str::<Value>(&t) {
            Ok(v) if v["type"] == "hello" => (Peer::from_hello(&v), None),
            Ok(v) => (Peer::legacy(), Some(v)),
            Err(_) => (Peer::legacy(), None),
        },
        Ok(Some(Ok(_))) | Err(_) => (Peer::legacy(), None),
        Ok(None) | Ok(Some(Err(_))) => {
            *bridge.conn.lock().await = None;
            return;
        }
    };
    let compatible = peer.compatible();
    match (compatible, peer.outdated()) {
        (false, who) => warn!(
            integration = ?peer.version, protocol = peer.protocol,
            "the Puzzlebox integration and app can't work together; update the {}", who.unwrap_or("older one")
        ),
        (true, Some(who)) => {
            info!(integration = ?peer.version, "Puzzlebox {who} is out of date; update it to use every feature")
        }
        (true, None) => info!(integration = ?peer.version, "Puzzlebox integration is up to date"),
    }
    let has = |f: &str| compatible && peer.shared(f).is_some();
    let (programs, run_events, webhooks, variables) =
        (has("programs"), has("run_events"), has("webhooks"), has("variables"));
    bridge.set_peer(Some(peer));

    if programs {
        let _ = tx.send(bridge.programs_snapshot().await);
    }
    if webhooks {
        let _ = tx.send(json!({"type": "webhooks", "webhooks": bridge.webhooks.ids()}));
    }
    if variables {
        let _ = tx.send(json!({"type": "variables", "variables": bridge.engine.globals()}));
    }
    if compatible && let Some(v) = first {
        bridge.handle(v).await;
    }

    let mut events = bridge.engine.subscribe();
    let mut hooks_changed = bridge.webhooks.watch();
    loop {
        tokio::select! {
            changed = hooks_changed.changed() => {
                if changed.is_err() { break }
                if webhooks {
                    let _ = tx.send(json!({"type": "webhooks", "webhooks": bridge.webhooks.ids()}));
                }
            }
            out = rx.recv() => {
                let Some(out) = out else { break };
                if socket.send(Message::Text(out.to_string().into())).await.is_err() { break }
            }
            ev = events.recv() => {
                let Ok(ev) = ev else { continue };
                match &ev {
                    EngineEvent::Status { .. }
                    | EngineEvent::Removed { .. }
                    | EngineEvent::Waiting { .. }
                    | EngineEvent::Paused { .. }
                    | EngineEvent::Resumed { .. }
                        if programs =>
                    {
                        let _ = tx.send(bridge.programs_snapshot().await);
                    }
                    EngineEvent::RunStarted { .. } | EngineEvent::RunFinished { .. } if run_events => {
                        let _ = tx.send(serde_json::to_value(&ev).unwrap_or_default());
                    }
                    EngineEvent::GlobalChanged { name, value } if variables => {
                        let _ = tx.send(json!({"type": "variable", "name": name, "value": value}));
                    }
                    _ => {}
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(t))) => {
                        if let Ok(v) = serde_json::from_str::<Value>(&t)
                            && compatible
                        {
                            bridge.handle(v).await;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(_)) => {}
                }
            }
        }
    }

    let mut conn = bridge.conn.lock().await;
    if conn.as_ref().is_some_and(|(id, _)| *id == conn_id) {
        *conn = None;
    }
    drop(conn);
    bridge.set_peer(None);
    for (_, tx) in bridge.pending.lock().await.drain() {
        let _ = tx.send(Err("bridge disconnected".into()));
    }
    info!("Puzzlebox bridge integration disconnected");
}
