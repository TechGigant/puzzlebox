use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use puzzlebox_engine::{Engine, LiveHa};
use puzzlebox_ha_client::{ConnectionState, HaClient};
use rust_embed::RustEmbed;
use serde_json::{Value, json};
use tower_http::trace::TraceLayer;

use crate::bridge::Bridge;
use crate::config::Config;
use crate::mqtt::Mqtt;
use crate::registry::RegistryService;
use crate::wasm::WasmHost;
use crate::webhooks::Webhooks;
use crate::{bridge, devices, programs, webhooks, ws};

#[derive(Clone)]
pub struct AppState {
    pub ha: HaClient,
    pub live: Arc<LiveHa>,
    pub engine: Engine,
    pub registry: Arc<RegistryService>,
    pub bridge: Arc<Bridge>,
    pub webhooks: Arc<Webhooks>,
    pub wasm: Arc<WasmHost>,
    pub mqtt: Arc<Mqtt>,
    pub devices: Arc<devices::DeviceCache>,
    pub cfg: Config,
}

pub fn wasm_dirs(cfg: &Config) -> Vec<std::path::PathBuf> {
    let mut dirs = vec![cfg.data_dir.join("plugins")];
    if let Some(c) = &cfg.config_dir {
        dirs.push(c.join("plugins"));
    }
    dirs
}

pub async fn load_wasm_plugins(
    wasm: &WasmHost,
    registry: &RegistryService,
    dirs: &[std::path::PathBuf],
) -> Vec<Value> {
    let mut errors = Vec::new();
    let before: Vec<String> = wasm.packs().into_iter().map(|p| p.source).collect();
    let mut loaded = Vec::new();
    for d in dirs {
        let (packs, errs) = wasm.load_dir(d);
        loaded.extend(packs);
        errors.extend(errs.into_iter().map(|(path, error)| json!({"path": path, "error": error})));
    }
    for source in before {
        if !loaded.iter().any(|p| p.source == source) {
            registry.set_plugin_source(&source, None).await;
        }
    }
    for p in loaded {
        let source = p.source.clone();
        registry.set_plugin_source(&source, Some(p)).await;
    }
    errors
}

#[derive(RustEmbed)]
#[folder = "../../frontend/dist"]
#[allow_missing = true]
struct Frontend;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/ha/states", get(ha_states))
        .route("/api/ha/config", get(ha_config))
        .route("/api/ha/registry", get(devices::registry))
        .route("/api/ha/devices/{id}/automations", get(devices::automations))
        .route("/api/ws", get(ws::upgrade))
        .route("/api/compile", post(programs::compile))
        .route("/api/programs", get(programs::list).post(programs::create))
        .route("/api/programs/{id}", get(programs::get).put(programs::update).delete(programs::delete))
        .route("/api/programs/{id}/run", post(programs::run))
        .route("/api/programs/{id}/cancel", post(programs::cancel))
        .route("/api/programs/{id}/enabled", post(programs::set_enabled))
        .route("/api/programs/{id}/runs", get(programs::runs))
        .route("/api/runs/{id}", get(programs::run_detail))
        .route("/api/registry", get(registry_blocks))
        .route("/api/import", post(import_yaml))
        .route("/api/import/automations", get(list_automations).post(import_automations))
        .route("/api/plugins", get(plugins_info))
        .route("/api/plugins/reload", post(plugins_reload))
        .route("/api/settings", get(settings))
        .route("/api/variables", get(variables).put(set_variable))
        .route("/api/bridge", get(bridge::upgrade))
        .route("/api/webhook/{id}", axum::routing::any(webhooks::receive))
        .route("/api/registry/reload", post(registry_reload))
        .fallback(static_file)
        .layer(middleware::from_fn_with_state(state.clone(), peer_guard))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn peer_guard(
    State(st): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    // The bridge authenticates with its token; webhooks are meant to be reachable.
    let path = req.uri().path();
    let open = path == "/api/bridge" || path.starts_with("/api/webhook/");
    match st.cfg.allowed_peer {
        Some(ip) if peer.ip() != ip && !open => StatusCode::FORBIDDEN.into_response(),
        _ => next.run(req).await,
    }
}

pub fn status_json(st: &AppState) -> Value {
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "ha_connected": st.ha.state() == ConnectionState::Connected,
        "ha_version": st.ha.ha_version(),
        "bridge": st.bridge.status(),
    })
}

async fn health(State(st): State<AppState>) -> Json<Value> {
    Json(status_json(&st))
}

pub struct ApiError(pub StatusCode, pub String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}

impl From<puzzlebox_ha_client::Error> for ApiError {
    fn from(e: puzzlebox_ha_client::Error) -> Self {
        let status = match e {
            puzzlebox_ha_client::Error::NotConnected | puzzlebox_ha_client::Error::Disconnected => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            _ => StatusCode::BAD_GATEWAY,
        };
        ApiError(status, e.to_string())
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
}

pub fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "not found".into())
}

async fn ha_states(State(st): State<AppState>) -> Json<Value> {
    Json(Value::Array(st.live.all_states()))
}

async fn registry_blocks(State(st): State<AppState>) -> Json<Value> {
    let packs: Vec<Value> = st
        .registry
        .packs()
        .await
        .iter()
        .map(|p| json!({"id": p.id, "name": p.name, "version": p.version, "description": p.description, "blocks": p.blocks.len()}))
        .collect();
    let errors: Vec<Value> = st
        .registry
        .pack_errors()
        .await
        .into_iter()
        .map(|(path, error)| json!({"path": path, "error": error}))
        .collect();
    Json(json!({"blocks": st.registry.registry.all(), "packs": packs, "pack_errors": errors}))
}

async fn registry_reload(State(st): State<AppState>) -> Json<Value> {
    st.registry.reload_packs().await;
    st.registry.invalidate();
    Json(json!({"ok": true}))
}

#[derive(serde::Deserialize)]
struct ImportInput {
    yaml: String,
}

async fn import_yaml(Json(input): Json<ImportInput>) -> Result<Json<Value>, ApiError> {
    let items = puzzlebox_yaml_import::import_yaml(&input.yaml)
        .map_err(|e| ApiError(StatusCode::UNPROCESSABLE_ENTITY, e))?;
    Ok(Json(serde_json::to_value(items).unwrap_or_default()))
}

async fn list_automations(State(st): State<AppState>) -> Json<Value> {
    let mut list: Vec<Value> = st
        .live
        .all_states()
        .into_iter()
        .filter(|s| s["entity_id"].as_str().is_some_and(|e| e.starts_with("automation.")))
        .map(|s| {
            json!({
                "entity_id": s["entity_id"],
                "name": s.pointer("/attributes/friendly_name").cloned().unwrap_or(s["entity_id"].clone()),
                "state": s["state"],
            })
        })
        .collect();
    list.sort_by_key(|v| v["name"].as_str().unwrap_or("").to_lowercase());
    Json(Value::Array(list))
}

#[derive(serde::Deserialize)]
struct AutomationsInput {
    entity_ids: Vec<String>,
}

async fn import_automations(
    State(st): State<AppState>,
    Json(input): Json<AutomationsInput>,
) -> Result<Json<Value>, ApiError> {
    let mut out = Vec::new();
    for id in input.entity_ids {
        let res = st.ha.request(json!({"type": "automation/config", "entity_id": id})).await;
        match res {
            Ok(v) => {
                let cfg = v.get("config").cloned().unwrap_or(v);
                let mut item = serde_json::to_value(puzzlebox_yaml_import::import_one(&cfg, Some(&id)))
                    .unwrap_or_default();
                item["entity_id"] = json!(id);
                out.push(item);
            }
            Err(e) => out.push(json!({"entity_id": id, "error": e.to_string()})),
        }
    }
    Ok(Json(Value::Array(out)))
}

async fn plugins_info(State(st): State<AppState>) -> Json<Value> {
    let python: Vec<String> = st
        .registry
        .registry
        .sources()
        .into_iter()
        .filter_map(|s| s.strip_prefix("py:").map(str::to_owned))
        .collect();
    Json(json!({
        "wasm": st.wasm.infos(),
        "python": python,
        "wasm_dirs": wasm_dirs(&st.cfg),
        "packs_dirs": st.registry.packs_dirs,
    }))
}

async fn plugins_reload(State(st): State<AppState>) -> Json<Value> {
    let errors = load_wasm_plugins(&st.wasm, &st.registry, &wasm_dirs(&st.cfg)).await;
    st.registry.reload_packs().await;
    Json(json!({"ok": true, "errors": errors}))
}

async fn settings(State(st): State<AppState>) -> Json<Value> {
    Json(json!({
        "bridge_token": st.bridge.token,
        "bridge_connected": st.bridge.connected().await,
        "bridge": st.bridge.status(),
        "mqtt_enabled": st.mqtt.enabled(),
        "supervised": st.cfg.supervisor_token.is_some(),
        "webhooks": st.webhooks.ids(),
    }))
}

async fn variables(State(st): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(st.engine.variables().await?))
}

#[derive(serde::Deserialize)]
struct SetVariable {
    program_id: Option<String>,
    name: String,
    value: Value,
}

async fn set_variable(
    State(st): State<AppState>,
    Json(v): Json<SetVariable>,
) -> Result<Json<Value>, ApiError> {
    st.engine.set_variable(v.program_id.as_deref(), &v.name, v.value).await?;
    Ok(Json(json!({"ok": true})))
}

async fn ha_config(State(st): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(st.ha.get_config().await?))
}

async fn static_file(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let (file, path) = match Frontend::get(path) {
        Some(f) if !path.is_empty() => (f, path),
        // SPA fallback: unknown non-API paths serve the app shell.
        _ if path.starts_with("api/") => return StatusCode::NOT_FOUND.into_response(),
        _ => match Frontend::get("index.html") {
            Some(f) => (f, "index.html"),
            None => {
                return (StatusCode::OK, [(header::CONTENT_TYPE, "text/plain")], "frontend not built")
                    .into_response();
            }
        },
    };
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cache = if path.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
    Response::builder()
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(header::CACHE_CONTROL, cache)
        .body(Body::from(file.data.into_owned()))
        .unwrap()
}
