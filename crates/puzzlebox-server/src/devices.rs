//! Home Assistant registries and device automations (device triggers, conditions and
//! actions) for the editor's Devices section.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::response::Json;
use futures_util::future::join_all;
use puzzlebox_ha_client::HaClient;
use serde_json::{Map, Value, json};
use tokio::sync::Mutex;

use crate::routes::{ApiError, AppState};

const AUTOMATIONS_TTL: Duration = Duration::from_secs(600);
const TRANSLATIONS_TTL: Duration = Duration::from_secs(1800);

#[derive(Default)]
pub struct DeviceCache {
    automations: Mutex<HashMap<String, (Instant, Value)>>,
    translations: Mutex<Option<(Instant, Map<String, Value>)>>,
    /// Entity registry id → entity id (device automations reference registry ids).
    entity_ids: Mutex<Option<(Instant, HashMap<String, String>)>>,
}

async fn list(ha: &HaClient, ty: &str) -> Value {
    ha.request(json!({"type": ty})).await.unwrap_or(Value::Null)
}

/// Areas, floors, labels and devices for pickers and the Devices section.
pub async fn registry(State(st): State<AppState>) -> Result<Json<Value>, ApiError> {
    let ha = &st.ha;
    let (areas, floors, labels, devices, entities) = tokio::join!(
        list(ha, "config/area_registry/list"),
        list(ha, "config/floor_registry/list"),
        list(ha, "config/label_registry/list"),
        list(ha, "config/device_registry/list"),
        list(ha, "config/entity_registry/list"),
    );
    let pick = |v: &Value, keys: &[&str]| -> Vec<Value> {
        v.as_array()
            .into_iter()
            .flatten()
            .map(|item| {
                let mut o = Map::new();
                for k in keys {
                    if let Some(x) = item.get(*k) {
                        o.insert((*k).to_owned(), x.clone());
                    }
                }
                Value::Object(o)
            })
            .collect()
    };
    let devices: Vec<Value> = devices
        .as_array()
        .into_iter()
        .flatten()
        // Services like "Sun" or integrations' hubs have no automations worth listing.
        .filter(|d| d.get("entry_type").and_then(Value::as_str) != Some("service"))
        .map(|d| {
            let name = d.get("name_by_user").filter(|v| !v.is_null()).or_else(|| d.get("name")).cloned();
            json!({"id": d["id"], "name": name, "area_id": d["area_id"], "manufacturer": d["manufacturer"],
                "model": d["model"], "labels": d.get("labels").cloned().unwrap_or(json!([]))})
        })
        .collect();
    Ok(Json(json!({
        "areas": pick(&areas, &["area_id", "name", "floor_id", "icon"]),
        "floors": pick(&floors, &["floor_id", "name", "level"]),
        "labels": pick(&labels, &["label_id", "name", "color"]),
        "devices": devices,
        "entities": pick(&entities, &["entity_id", "device_id", "area_id"]),
    })))
}

impl DeviceCache {
    async fn translations(&self, ha: &HaClient) -> Map<String, Value> {
        let mut guard = self.translations.lock().await;
        if let Some((at, t)) = guard.as_ref()
            && at.elapsed() < TRANSLATIONS_TTL
        {
            return t.clone();
        }
        let language = ha
            .get_config()
            .await
            .ok()
            .and_then(|c| c.get("language").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| "en".into());
        let res = ha
            .request(json!({"type": "frontend/get_translations", "language": language, "category": "device_automation"}))
            .await
            .ok()
            .and_then(|mut r| r.get_mut("resources").map(Value::take))
            .and_then(|r| r.as_object().cloned())
            .unwrap_or_default();
        *guard = Some((Instant::now(), res.clone()));
        res
    }

    async fn entity_ids(&self, ha: &HaClient) -> HashMap<String, String> {
        let mut guard = self.entity_ids.lock().await;
        if let Some((at, m)) = guard.as_ref()
            && at.elapsed() < AUTOMATIONS_TTL
        {
            return m.clone();
        }
        let map: HashMap<String, String> = list(ha, "config/entity_registry/list")
            .await
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| {
                Some((e.get("id")?.as_str()?.to_owned(), e.get("entity_id")?.as_str()?.to_owned()))
            })
            .collect();
        *guard = Some((Instant::now(), map.clone()));
        map
    }
}

fn humanize(s: &str) -> String {
    let s = s.replace('_', " ");
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Maps a serialized voluptuous field (HA `extra_fields`) to a block parameter.
fn param_from_extra_field(f: &Value, domain: &str, t: &Map<String, Value>) -> Option<Value> {
    let name = f.get("name")?.as_str()?;
    let label = t
        .get(&format!("component.{domain}.device_automation.extra_fields.{name}"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| humanize(name));
    let required = f.get("required").and_then(Value::as_bool).unwrap_or(false);
    let mut p = json!({"name": name, "label": label, "required": required});
    match f.get("type").and_then(Value::as_str).unwrap_or("") {
        "float" | "integer" => {
            p["input"] = json!("number");
            if let Some(v) = f.get("valueMin") {
                p["min"] = v.clone();
            }
            if let Some(v) = f.get("valueMax") {
                p["max"] = v.clone();
            }
        }
        "boolean" => p["input"] = json!("boolean"),
        "string" => p["input"] = json!("text"),
        "select" => {
            let options: Vec<Value> = f
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|o| match o {
                    Value::Array(pair) => {
                        Some(json!({"value": pair.first()?.as_str()?, "label": pair.get(1).and_then(Value::as_str).unwrap_or("")}))
                    }
                    Value::String(s) => Some(json!({"value": s, "label": humanize(s)})),
                    _ => None,
                })
                .collect();
            p["input"] = json!("select");
            p["options"] = json!(options);
        }
        // HA rejects plain seconds here; the compiler turns seconds into {hours, minutes, seconds}.
        "positive_time_period_dict" => {
            p["input"] = json!("duration");
            p["duration_dict"] = json!(true);
        }
        _ => p["input"] = json!("any"),
    }
    Some(p)
}

async fn describe(
    ha: &HaClient,
    kind: &str,
    item: Value,
    t: &Map<String, Value>,
    entity_ids: &HashMap<String, String>,
) -> Value {
    let domain = item.get("domain").and_then(Value::as_str).unwrap_or("").to_owned();
    let ty = item.get("type").and_then(Value::as_str).unwrap_or("").to_owned();
    let caps = ha
        .request(json!({"type": format!("device_automation/{kind}/capabilities"), kind: item.clone()}))
        .await
        .unwrap_or(Value::Null);
    let params: Vec<Value> = caps
        .get("extra_fields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|f| param_from_extra_field(f, &domain, t))
        .collect();
    let entity = item
        .get("entity_id")
        .and_then(Value::as_str)
        .map(|e| entity_ids.get(e).cloned().unwrap_or_else(|| e.to_owned()));
    let entity_name = match &entity {
        Some(e) => ha_friendly(ha, e).await.unwrap_or_else(|| e.clone()),
        None => String::new(),
    };
    let subtype = item.get("subtype").and_then(Value::as_str).map(|s| {
        t.get(&format!("component.{domain}.device_automation.{kind}_subtype.{s}"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| humanize(s))
    });
    let template = t
        .get(&format!("component.{domain}.device_automation.{kind}_type.{ty}"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let mut s = humanize(&ty);
            if subtype.is_some() {
                s.push_str(" {subtype}");
            }
            s
        });
    let label = template
        .replace("{entity_name}", &entity_name)
        .replace("{subtype}", subtype.as_deref().unwrap_or(""))
        .trim()
        .to_owned();
    let mut config = item.clone();
    if let Some(o) = config.as_object_mut() {
        o.remove("metadata");
    }
    let secondary = item.pointer("/metadata/secondary").and_then(Value::as_bool).unwrap_or(false);
    json!({"kind": kind, "label": label, "config": config, "params": params, "secondary": secondary})
}

async fn ha_friendly(ha: &HaClient, entity_id: &str) -> Option<String> {
    let rendered = ha
        .render_template(&format!("{{{{ state_attr('{entity_id}', 'friendly_name') or '' }}}}"), None)
        .await
        .ok()?;
    rendered.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

/// Triggers, conditions and actions of one device, with labels and option fields.
pub async fn automations(
    State(st): State<AppState>,
    Path(device_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let cache = &st.devices;
    if let Some((at, v)) = cache.automations.lock().await.get(&device_id)
        && at.elapsed() < AUTOMATIONS_TTL
    {
        return Ok(Json(v.clone()));
    }
    let ha = &st.ha;
    let (t, entity_ids) = tokio::join!(cache.translations(ha), cache.entity_ids(ha));
    let mut out = Vec::new();
    for kind in ["trigger", "condition", "action"] {
        let items = ha
            .request(json!({"type": format!("device_automation/{kind}/list"), "device_id": device_id}))
            .await
            .unwrap_or(Value::Null);
        let described = join_all(
            items.as_array().into_iter().flatten().map(|i| describe(ha, kind, i.clone(), &t, &entity_ids)),
        )
        .await;
        out.extend(described);
    }
    let result = Value::Array(out);
    cache.automations.lock().await.insert(device_id, (Instant::now(), result.clone()));
    Ok(Json(result))
}
