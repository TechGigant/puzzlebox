use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use puzzlebox_ha_client::{ClientConfig, ConnectionState, Error, HaClient};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

const TOKEN: &str = "good-token";

/// Minimal fake HA: answers a few commands and lets the test push events / kill connections.
struct MockHa {
    url: String,
    connections: Arc<AtomicUsize>,
    subscribes: mpsc::UnboundedReceiver<(u64, Value)>,
    push: tokio::sync::broadcast::Sender<Value>,
    kill: tokio::sync::broadcast::Sender<()>,
}

async fn start_mock() -> MockHa {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/api/websocket", listener.local_addr().unwrap());
    let connections = Arc::new(AtomicUsize::new(0));
    let (sub_tx, subscribes) = mpsc::unbounded_channel();
    let (push, _) = tokio::sync::broadcast::channel::<Value>(16);
    let (kill, _) = tokio::sync::broadcast::channel::<()>(4);

    let conns = connections.clone();
    let push_tx = push.clone();
    let kill_tx = kill.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            conns.fetch_add(1, Ordering::SeqCst);
            let sub_tx = sub_tx.clone();
            let mut push_rx = push_tx.subscribe();
            let mut kill_rx = kill_tx.subscribe();
            tokio::spawn(async move {
                let mut ws = accept_async(stream).await.unwrap();
                ws.send(Message::Text(json!({"type": "auth_required"}).to_string())).await.unwrap();
                let Some(Ok(Message::Text(auth))) = ws.next().await else { return };
                let auth: Value = serde_json::from_str(&auth).unwrap();
                if auth["access_token"] != TOKEN {
                    let _ = ws
                        .send(Message::Text(json!({"type": "auth_invalid", "message": "bad"}).to_string()))
                        .await;
                    return;
                }
                ws.send(Message::Text(json!({"type": "auth_ok", "ha_version": "2026.11.0"}).to_string()))
                    .await
                    .unwrap();
                let mut sub_id: Option<u64> = None;
                loop {
                    tokio::select! {
                        _ = kill_rx.recv() => return,
                        ev = push_rx.recv() => {
                            if let (Ok(ev), Some(id)) = (ev, sub_id) {
                                ws.send(Message::Text(json!({"id": id, "type": "event", "event": ev}).to_string())).await.unwrap();
                            }
                        }
                        msg = ws.next() => {
                            let Some(Ok(Message::Text(t))) = msg else { return };
                            let m: Value = serde_json::from_str(&t).unwrap();
                            let id = m["id"].as_u64().unwrap();
                            let reply = match m["type"].as_str().unwrap() {
                                "supported_features" => json!({"id": id, "type": "result", "success": true, "result": null}),
                                "ping" => json!({"id": id, "type": "pong"}),
                                "get_states" => json!({"id": id, "type": "result", "success": true, "result": [
                                    {"entity_id": "light.kitchen", "state": "on", "attributes": {"brightness": 200}}
                                ]}),
                                "call_service" if m["domain"] == "nope" => json!({"id": id, "type": "result", "success": false,
                                    "error": {"code": "not_found", "message": "Service nope.x not found"}}),
                                "subscribe_trigger" => {
                                    sub_id = Some(id);
                                    sub_tx.send((id, m.clone())).unwrap();
                                    json!({"id": id, "type": "result", "success": true, "result": null})
                                }
                                _ => json!({"id": id, "type": "result", "success": true, "result": {"echo": m}}),
                            };
                            // Exercise coalesced array framing on every other reply.
                            let frame = if id.is_multiple_of(2) { json!([reply]) } else { reply };
                            ws.send(Message::Text(frame.to_string())).await.unwrap();
                        }
                    }
                }
            });
        }
    });
    MockHa { url, connections, subscribes, push, kill }
}

fn client(url: &str, token: &str) -> HaClient {
    let mut cfg = ClientConfig::new(url, token);
    cfg.max_backoff = Duration::from_millis(200);
    HaClient::spawn(cfg)
}

#[tokio::test]
async fn authenticates_and_fetches_states() {
    let mock = start_mock().await;
    let c = client(&mock.url, TOKEN);
    timeout(Duration::from_secs(5), c.wait_connected()).await.unwrap().unwrap();
    assert_eq!(c.ha_version().as_deref(), Some("2026.11.0"));
    let states = c.get_states().await.unwrap();
    assert_eq!(states[0].entity_id, "light.kitchen");
    assert_eq!(states[0].attributes["brightness"], 200);
}

#[tokio::test]
async fn maps_ha_errors() {
    let mock = start_mock().await;
    let c = client(&mock.url, TOKEN);
    c.wait_connected().await.unwrap();
    let err = c.call_service("nope", "x", None, None, false).await.unwrap_err();
    assert!(matches!(err, Error::Ha { ref code, .. } if code == "not_found"), "{err}");
}

#[tokio::test]
async fn invalid_token_never_connects() {
    let mock = start_mock().await;
    let c = client(&mock.url, "bad");
    let mut st = c.state_watch();
    let res = timeout(Duration::from_millis(500), st.wait_for(|s| *s == ConnectionState::Connected)).await;
    assert!(res.is_err());
    assert!(matches!(c.get_states().await, Err(Error::NotConnected)));
}

#[tokio::test]
async fn resubscribes_after_reconnect() {
    let mut mock = start_mock().await;
    let c = client(&mock.url, TOKEN);
    c.wait_connected().await.unwrap();

    let trigger = json!({"platform": "state", "entity_id": "binary_sensor.motion", "to": "on"});
    let mut sub = c.subscribe_trigger(trigger.clone(), None).await.unwrap();
    let (_, first) = mock.subscribes.recv().await.unwrap();
    assert_eq!(first["trigger"], trigger);

    mock.push.send(json!({"variables": {"trigger": {"n": 1}}})).unwrap();
    let ev = timeout(Duration::from_secs(2), sub.next()).await.unwrap().unwrap();
    assert_eq!(ev["variables"]["trigger"]["n"], 1);

    mock.kill.send(()).unwrap();
    let (_, again) = timeout(Duration::from_secs(5), mock.subscribes.recv()).await.unwrap().unwrap();
    assert_eq!(again["trigger"], trigger);
    assert_eq!(mock.connections.load(Ordering::SeqCst), 2);

    mock.push.send(json!({"variables": {"trigger": {"n": 2}}})).unwrap();
    let ev = timeout(Duration::from_secs(2), sub.next()).await.unwrap().unwrap();
    assert_eq!(ev["variables"]["trigger"]["n"], 2);
}
