//! UI WebSocket: connection status, engine events, and debugger commands.

use std::collections::HashMap;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use puzzlebox_engine::WatchGuard;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::routes::{AppState, status_json};

pub async fn upgrade(State(st): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| session(socket, st))
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMsg {
    Watch { program_id: String },
    Unwatch { program_id: String },
    Breakpoints { program_id: String, blocks: Vec<String> },
    PauseOnStart { program_id: String, enabled: bool },
    Resume { run_id: String },
    Step { run_id: String },
}

async fn send(socket: &mut WebSocket, v: serde_json::Value) -> bool {
    socket.send(Message::Text(v.to_string().into())).await.is_ok()
}

async fn session(mut socket: WebSocket, st: AppState) {
    let engine = st.engine.clone();
    let dbg = engine.debugger();
    let mut conn = st.ha.state_watch();
    let mut bridge_changed = st.bridge.watch();
    let mut events = engine.subscribe();
    let mut watches: HashMap<String, WatchGuard<'_>> = HashMap::new();

    if !send(&mut socket, json!({"type": "status", "status": status_json(&st)})).await {
        return;
    }
    let statuses = serde_json::to_value(engine.statuses()).unwrap_or_default();
    if !send(&mut socket, json!({"type": "statuses", "statuses": statuses})).await {
        return;
    }

    loop {
        tokio::select! {
            changed = conn.changed() => {
                if changed.is_err() { return }
                if !send(&mut socket, json!({"type": "status", "status": status_json(&st)})).await { return }
            }
            changed = bridge_changed.changed() => {
                if changed.is_err() { return }
                if !send(&mut socket, json!({"type": "status", "status": status_json(&st)})).await { return }
            }
            ev = events.recv() => match ev {
                Ok(ev) => {
                    let v = serde_json::to_value(&ev).unwrap_or_default();
                    if !send(&mut socket, v).await { return }
                }
                Err(RecvError::Lagged(n)) => {
                    if !send(&mut socket, json!({"type": "lagged", "missed": n})).await { return }
                }
                Err(RecvError::Closed) => return,
            },
            incoming = socket.recv() => {
                let text = match incoming {
                    Some(Ok(Message::Text(t))) => t,
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                    Some(Ok(_)) => continue,
                };
                let Ok(msg) = serde_json::from_str::<ClientMsg>(&text) else { continue };
                match msg {
                    ClientMsg::Watch { program_id } => {
                        watches.entry(program_id.clone()).or_insert_with(|| dbg.watch(&program_id));
                    }
                    ClientMsg::Unwatch { program_id } => { watches.remove(&program_id); }
                    ClientMsg::Breakpoints { program_id, blocks } => dbg.set_breakpoints(&program_id, blocks),
                    ClientMsg::PauseOnStart { program_id, enabled } => dbg.set_pause_on_start(&program_id, enabled),
                    ClientMsg::Resume { run_id } => { dbg.resume(&run_id); }
                    ClientMsg::Step { run_id } => { dbg.step(&run_id); }
                }
            }
        }
    }
}
