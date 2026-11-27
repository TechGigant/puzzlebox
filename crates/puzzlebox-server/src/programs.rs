use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use puzzlebox_storage::ProgramRecord;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::routes::{ApiError, AppState, DeepJson, not_found};

type ApiResult = Result<Json<Value>, ApiError>;

fn new_id() -> String {
    format!("{:016x}", rand::random::<u64>())
}

fn with_status(st: &AppState, p: &ProgramRecord, include_workspace: bool) -> Value {
    let mut v = serde_json::to_value(p).unwrap_or_default();
    if !include_workspace && let Some(o) = v.as_object_mut() {
        o.remove("workspace");
    }
    v["status"] = serde_json::to_value(st.engine.status(&p.id)).unwrap_or_default();
    v["does_nothing"] = json!(puzzlebox_ir::does_nothing(&p.workspace));
    v
}

pub async fn list(State(st): State<AppState>) -> ApiResult {
    let programs = st.engine.store().list_programs().await?;
    Ok(Json(Value::Array(programs.iter().map(|p| with_status(&st, p, false)).collect())))
}

pub async fn get(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult {
    let p = st.engine.store().get_program(&id).await?.ok_or_else(not_found)?;
    Ok(Json(with_status(&st, &p, true)))
}

#[derive(Deserialize)]
pub struct ProgramInput {
    name: Option<String>,
    description: Option<String>,
    enabled: Option<bool>,
    mode: Option<String>,
    max_runs: Option<u32>,
    workspace: Option<Value>,
    folder: Option<String>,
}

const MODES: [&str; 4] = ["single", "restart", "queued", "parallel"];

fn apply(rec: &mut ProgramRecord, input: ProgramInput) -> Result<(), ApiError> {
    if let Some(n) = input.name {
        let n = n.trim().to_owned();
        if n.is_empty() {
            return Err(ApiError(StatusCode::UNPROCESSABLE_ENTITY, "name must not be empty".into()));
        }
        rec.name = n;
    }
    if let Some(d) = input.description {
        rec.description = d;
    }
    if let Some(e) = input.enabled {
        rec.enabled = e;
    }
    if let Some(m) = input.mode {
        if !MODES.contains(&m.as_str()) {
            return Err(ApiError(StatusCode::UNPROCESSABLE_ENTITY, format!("invalid mode '{m}'")));
        }
        rec.mode = m;
    }
    if let Some(m) = input.max_runs {
        rec.max_runs = m.clamp(1, 1000);
    }
    if let Some(w) = input.workspace {
        rec.workspace = w;
    }
    if let Some(f) = input.folder {
        // Normalise "a / b/" to "a/b"; empty means no folder.
        rec.folder = f.split('/').map(str::trim).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("/");
    }
    Ok(())
}

async fn save(st: &AppState, rec: ProgramRecord) -> ApiResult {
    let (saved, errors) = st.engine.save(rec).await?;
    let mut out = with_status(st, &saved, true);
    out["errors"] = serde_json::to_value(errors).unwrap_or_default();
    Ok(Json(out))
}

pub async fn create(State(st): State<AppState>, DeepJson(input): DeepJson<ProgramInput>) -> ApiResult {
    let mut rec = ProgramRecord {
        id: new_id(),
        name: "New program".into(),
        description: String::new(),
        enabled: true,
        mode: "single".into(),
        max_runs: 10,
        workspace: json!({}),
        folder: String::new(),
        created_at: 0,
        updated_at: 0,
    };
    apply(&mut rec, input)?;
    save(&st, rec).await
}

pub async fn update(
    State(st): State<AppState>,
    Path(id): Path<String>,
    DeepJson(input): DeepJson<ProgramInput>,
) -> ApiResult {
    let mut rec = st.engine.store().get_program(&id).await?.ok_or_else(not_found)?;
    apply(&mut rec, input)?;
    save(&st, rec).await
}

pub async fn delete(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult {
    if !st.engine.delete(&id).await? {
        return Err(not_found());
    }
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize, Default)]
pub struct RunInput {
    handler: Option<usize>,
    /// Top block of the stack to run (an entry block or a loose stack), instead of `handler`.
    block: Option<String>,
    trigger: Option<Value>,
}

pub async fn run(
    State(st): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<RunInput>>,
) -> ApiResult {
    let input = body.map(|b| b.0).unwrap_or_default();
    let handler = match &input.block {
        Some(block) => Some(st.engine.handler_index(&id, block).ok_or_else(|| {
            ApiError(
                StatusCode::CONFLICT,
                "this stack can't run: save the program, and fix any problems in it".into(),
            )
        })?),
        None => input.handler,
    };
    if !st.engine.run_now(&id, handler, input.trigger) {
        return Err(ApiError(StatusCode::CONFLICT, "program is not running (disabled or invalid)".into()));
    }
    Ok(Json(json!({"ok": true})))
}

pub async fn cancel(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult {
    st.engine.cancel_runs(&id);
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
pub struct EnabledInput {
    enabled: bool,
}

pub async fn set_enabled(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<EnabledInput>,
) -> ApiResult {
    let rec = st.engine.set_enabled(&id, input.enabled).await?.ok_or_else(not_found)?;
    Ok(Json(with_status(&st, &rec, false)))
}

#[derive(Deserialize)]
pub struct Limit {
    limit: Option<usize>,
}

pub async fn runs(State(st): State<AppState>, Path(id): Path<String>, Query(q): Query<Limit>) -> ApiResult {
    let runs = st.engine.store().list_runs(&id, q.limit.unwrap_or(25).min(100)).await?;
    Ok(Json(serde_json::to_value(runs).unwrap_or_default()))
}

pub async fn run_detail(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult {
    let run = st.engine.store().get_run(&id).await?.ok_or_else(not_found)?;
    Ok(Json(serde_json::to_value(run).unwrap_or_default()))
}

#[derive(Deserialize)]
pub struct CompileInput {
    workspace: Value,
}

/// Validates a workspace without saving, for live error markers in the editor.
pub async fn compile(State(st): State<AppState>, DeepJson(input): DeepJson<CompileInput>) -> ApiResult {
    let errors = st.engine.compile(&input.workspace).err().unwrap_or_default();
    Ok(Json(json!({"errors": errors})))
}

pub async fn versions(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult {
    let v = st.engine.store().list_versions(&id).await?;
    Ok(Json(serde_json::to_value(v).unwrap_or_default()))
}

pub async fn version(State(st): State<AppState>, Path((id, version)): Path<(String, i64)>) -> ApiResult {
    let v = st.engine.store().get_version(&id, version).await?.ok_or_else(not_found)?;
    Ok(Json(serde_json::to_value(v).unwrap_or_default()))
}
