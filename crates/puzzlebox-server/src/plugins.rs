//! Routes plugin block calls to the runtime that owns the plugin (Python bridge or WASM).

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use puzzlebox_engine::PluginHost;
use serde_json::{Map, Value};

use crate::bridge::Bridge;
use crate::mqtt::Mqtt;
use crate::wasm::WasmHost;

pub struct PluginRouter {
    pub bridge: OnceLock<Arc<Bridge>>,
    pub wasm: OnceLock<Arc<WasmHost>>,
    pub mqtt: OnceLock<Arc<Mqtt>>,
    pub z2m: OnceLock<Arc<crate::z2m::Z2m>>,
}

impl PluginRouter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            bridge: OnceLock::new(),
            wasm: OnceLock::new(),
            mqtt: OnceLock::new(),
            z2m: OnceLock::new(),
        })
    }
}

#[async_trait]
impl PluginHost for PluginRouter {
    async fn call(
        &self,
        plugin: &str,
        function: &str,
        args: Map<String, Value>,
        ctx: Value,
    ) -> Result<Value, String> {
        if plugin == puzzlebox_registry::BUILTIN_Z2M {
            let z2m = self.z2m.get().ok_or("Zigbee2MQTT is not available")?;
            let topic = args.get("topic").and_then(Value::as_str).unwrap_or("");
            let key = args.get("key").and_then(Value::as_str).ok_or("choose a value")?;
            return Ok(z2m.value(topic, key));
        }
        if plugin == puzzlebox_ir::BUILTIN_MQTT {
            let mqtt = self.mqtt.get().ok_or("MQTT is not configured")?;
            let topic = args.get("topic").and_then(Value::as_str).ok_or("topic must be text")?;
            let retain = args.get("retain").and_then(Value::as_bool).unwrap_or(false);
            mqtt.publish(topic, args.get("payload").unwrap_or(&Value::Null), retain).await?;
            return Ok(Value::Null);
        }
        if let Some(w) = self.wasm.get()
            && w.has_plugin(plugin)
        {
            return w.call(plugin, function, Value::Object(args), ctx).await;
        }
        match self.bridge.get() {
            Some(b) => b.invoke(plugin, function, Value::Object(args), ctx).await,
            None => Err(format!("plugin '{plugin}' is not available")),
        }
    }
}
