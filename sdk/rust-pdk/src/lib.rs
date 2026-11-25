//! Helpers for writing puzzlebox WASM plugins with Extism.
//!
//! ```ignore
//! use puzzlebox_pdk::prelude::*;
//!
//! #[plugin_fn]
//! pub fn greet(input: Json<Call>) -> FnResult<Json<Value>> {
//!     let name = input.0.arg_str("name").unwrap_or("world");
//!     Ok(Json(json!(format!("Hello, {name}!"))))
//! }
//! ```
//! Host functions are only usable if `plugin.yaml` grants the matching capability.

use extism_pdk::*;
use serde::Deserialize;
use serde_json::{Map, Value, json};

pub mod prelude {
    pub use super::{Call, call_service, get_state, kv_get, kv_set, log};
    pub use extism_pdk::{FnResult, Json, plugin_fn};
    pub use serde_json::{Value, json};
}

/// Input every block function receives.
#[derive(Debug, Deserialize)]
pub struct Call {
    #[serde(default)]
    pub args: Map<String, Value>,
    /// `program_id`, `run_id`, `trigger`.
    #[serde(default)]
    pub context: Value,
}

impl Call {
    pub fn arg(&self, name: &str) -> Option<&Value> {
        self.args.get(name)
    }
    pub fn arg_str(&self, name: &str) -> Option<&str> {
        self.arg(name).and_then(Value::as_str)
    }
    pub fn arg_f64(&self, name: &str) -> Option<f64> {
        match self.arg(name)? {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.parse().ok(),
            _ => None,
        }
    }
}

#[host_fn]
extern "ExtismHost" {
    fn puzzlebox_log(msg: String);
    fn puzzlebox_get_state(entity_id: String) -> String;
    fn puzzlebox_call_service(request: String) -> String;
    fn puzzlebox_kv_get(key: String) -> String;
    fn puzzlebox_kv_set(request: String);
}

pub fn log(msg: &str) -> FnResult<()> {
    unsafe { puzzlebox_log(msg.to_owned())? };
    Ok(())
}

/// Full state object (`{"state": ..., "attributes": {...}}`) or `null`. Needs `states: true`.
pub fn get_state(entity_id: &str) -> FnResult<Value> {
    let raw = unsafe { puzzlebox_get_state(entity_id.to_owned())? };
    Ok(serde_json::from_str(&raw)?)
}

/// Calls an action. Needs it listed under `services`.
pub fn call_service(service: &str, data: Option<Value>, target: Option<Value>) -> FnResult<Value> {
    let req = json!({"service": service, "data": data, "target": target});
    let raw = unsafe { puzzlebox_call_service(req.to_string())? };
    Ok(serde_json::from_str(&raw)?)
}

/// Private persistent storage. Needs `kv: true`.
pub fn kv_get(key: &str) -> FnResult<Value> {
    let raw = unsafe { puzzlebox_kv_get(key.to_owned())? };
    Ok(serde_json::from_str(&raw)?)
}

pub fn kv_set(key: &str, value: Value) -> FnResult<()> {
    unsafe { puzzlebox_kv_set(json!({"key": key, "value": value}).to_string())? };
    Ok(())
}
