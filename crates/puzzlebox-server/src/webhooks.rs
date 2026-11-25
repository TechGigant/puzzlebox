//! Webhook trigger source. Requests arrive either directly (`/api/webhook/{id}`) or
//! through Home Assistant's own webhook endpoint via the bridge integration.

use std::collections::HashMap;
use std::sync::Mutex;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::IntoResponse;
use puzzlebox_engine::{TriggerSink, TriggerSource};
use puzzlebox_ir::Trigger;
use serde_json::{Map, Value, json};
use tokio::sync::watch;

use crate::routes::AppState;

struct Hook {
    program_id: String,
    methods: Vec<String>,
    sink: TriggerSink,
}

pub struct Webhooks {
    hooks: Mutex<HashMap<String, Vec<Hook>>>,
    changed: watch::Sender<u64>,
}

impl Default for Webhooks {
    fn default() -> Self {
        Self { hooks: Mutex::new(HashMap::new()), changed: watch::channel(0).0 }
    }
}

impl Webhooks {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Vec<Hook>>> {
        self.hooks.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.lock().keys().cloned().collect();
        ids.sort();
        ids
    }

    pub fn watch(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// Delivers a request; returns how many programs accepted it.
    pub fn dispatch(&self, id: &str, method: &str, request: Value) -> usize {
        let hooks = self.lock();
        let Some(list) = hooks.get(id) else { return 0 };
        let payload = json!({"platform": "webhook", "webhook_id": id, "method": method, "request": request});
        list.iter()
            .filter(|h| h.methods.is_empty() || h.methods.iter().any(|m| m.eq_ignore_ascii_case(method)))
            .filter(|h| (h.sink)(payload.clone()))
            .count()
    }
}

impl TriggerSource for Webhooks {
    fn register(&self, program_id: &str, trigger: &Trigger, sink: TriggerSink) -> Result<(), String> {
        let Trigger::Webhook { webhook_id, methods, .. } = trigger else {
            return Err("not a webhook trigger".into());
        };
        if !webhook_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err("webhook id may only contain letters, digits, '-' and '_'".into());
        }
        self.lock().entry(webhook_id.clone()).or_default().push(Hook {
            program_id: program_id.to_owned(),
            methods: methods.clone(),
            sink,
        });
        self.changed.send_modify(|v| *v += 1);
        Ok(())
    }

    fn unregister_program(&self, program_id: &str) {
        let mut hooks = self.lock();
        let before: usize = hooks.values().map(Vec::len).sum();
        for list in hooks.values_mut() {
            list.retain(|h| h.program_id != program_id);
        }
        hooks.retain(|_, l| !l.is_empty());
        let after: usize = hooks.values().map(Vec::len).sum();
        drop(hooks);
        if before != after {
            self.changed.send_modify(|v| *v += 1);
        }
    }
}

pub async fn receive(
    State(st): State<AppState>,
    Path(id): Path<String>,
    method: Method,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let text = String::from_utf8_lossy(&body).into_owned();
    let json_body = serde_json::from_str::<Value>(&text).ok();
    let hdrs: Map<String, Value> = headers
        .iter()
        .filter(|(k, _)| k.as_str() != "authorization" && k.as_str() != "cookie")
        .map(|(k, v)| (k.as_str().to_owned(), json!(v.to_str().unwrap_or(""))))
        .collect();
    let request = json!({
        "query": query,
        "headers": hdrs,
        "body": text,
        "json": json_body,
    });
    match st.webhooks.dispatch(&id, method.as_str(), request) {
        0 => (StatusCode::NOT_FOUND, "no program listens on this webhook").into_response(),
        _ => (StatusCode::OK, "ok").into_response(),
    }
}
