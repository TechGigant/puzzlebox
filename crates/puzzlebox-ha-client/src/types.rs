use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntityState {
    pub entity_id: String,
    pub state: String,
    #[serde(default)]
    pub attributes: Map<String, Value>,
    #[serde(default)]
    pub last_changed: Option<String>,
    #[serde(default)]
    pub last_updated: Option<String>,
    #[serde(default)]
    pub context: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Target {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entity_id: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub device_id: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub area_id: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub floor_id: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub label_id: Vec<String>,
}

impl Target {
    pub fn entities<I, S>(ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Target { entity_id: ids.into_iter().map(Into::into).collect(), ..Default::default() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Disconnected,
}
