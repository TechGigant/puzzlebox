//! The YAML documents users paste or that Home Assistant stores: an automation, a script,
//! lists and maps of them.

use serde_json::Value;

use crate::is_config;

/// The automations/scripts in a document, with their key when they came from a map
/// (`scripts.yaml`: `{script_id: {...}}`).
pub(crate) fn configs(text: &str) -> Result<Vec<(Option<String>, Value)>, String> {
    let doc: Value = serde_yaml::from_str(text).map_err(|e| format!("invalid YAML: {e}"))?;
    let items: Vec<(Option<String>, Value)> = match doc {
        Value::Array(list) => list.into_iter().map(|v| (None, v)).collect(),
        Value::Object(ref m) if is_config(&doc) => vec![(None, Value::Object(m.clone()))],
        Value::Object(m) => m.into_iter().map(|(k, v)| (Some(k), v)).collect(),
        _ => return Err("expected an automation, a script, or a list of them".into()),
    };
    Ok(items.into_iter().filter(|(_, v)| is_config(v)).collect())
}
