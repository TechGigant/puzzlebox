//! Sandboxed WASM plugins (Extism). A plugin is a directory with `plugin.yaml`
//! (a block pack plus `wasm` and `capabilities`) and the module it names.
//!
//! Calls receive `{"args": {...}, "context": {...}}` as JSON input and return the
//! block's value as JSON output. Host functions are gated by declared capabilities.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use extism::{Manifest, PTR, Plugin, PluginBuilder, UserData, Wasm, host_fn};
use puzzlebox_engine::HaApi;
use puzzlebox_registry::Pack;
use puzzlebox_registry::pack::{PackFile, from_file};
use puzzlebox_storage::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::runtime::Handle;
use tracing::{info, warn};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Capabilities {
    /// Hosts the plugin may reach with HTTP (Extism allow-list; `*.example.com` allowed).
    #[serde(default)]
    pub http: Vec<String>,
    /// Actions the plugin may call, e.g. `light.turn_on` or `notify.*`.
    #[serde(default)]
    pub services: Vec<String>,
    /// May read entity states.
    #[serde(default)]
    pub states: bool,
    /// May use its private key/value store.
    #[serde(default)]
    pub kv: bool,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub memory_mb: Option<u32>,
}

impl Capabilities {
    fn allows_service(&self, service: &str) -> bool {
        self.services.iter().any(|p| match p.strip_suffix(".*") {
            Some(domain) => service.split_once('.').is_some_and(|(d, _)| d == domain),
            None => p == service || p == "*",
        })
    }
}

#[derive(Deserialize)]
struct PluginManifest {
    wasm: String,
    #[serde(default)]
    capabilities: Capabilities,
    #[serde(flatten)]
    pack: PackFile,
}

struct HostCtx {
    plugin_id: String,
    caps: Capabilities,
    ha: Arc<dyn HaApi>,
    store: Store,
    rt: Handle,
}

type Ctx = UserData<HostCtx>;

fn ctx_get<T>(ud: &Ctx, f: impl FnOnce(&HostCtx) -> Result<T, extism::Error>) -> Result<T, extism::Error> {
    let arc = ud.get()?;
    let guard = arc.lock().map_err(|_| extism::Error::msg("host context poisoned"))?;
    f(&guard)
}

host_fn!(puzzlebox_log(user_data: HostCtx; msg: String) {
    ctx_get(&user_data, |c| {
        info!(plugin = %c.plugin_id, "{msg}");
        Ok(())
    })
});

host_fn!(puzzlebox_get_state(user_data: HostCtx; entity_id: String) -> String {
    ctx_get(&user_data, |c| {
        if !c.caps.states {
            return Err(extism::Error::msg("capability 'states' not granted"));
        }
        Ok(c.ha.state(&entity_id).unwrap_or(Value::Null).to_string())
    })
});

host_fn!(puzzlebox_call_service(user_data: HostCtx; request: String) -> String {
    ctx_get(&user_data, |c| {
        let req: Value = serde_json::from_str(&request)?;
        let service = req["service"].as_str().unwrap_or_default().to_owned();
        if !c.caps.allows_service(&service) {
            return Err(extism::Error::msg(format!("capability to call '{service}' not granted")));
        }
        let ha = c.ha.clone();
        let data = req.get("data").cloned().filter(|v| !v.is_null());
        let target = req.get("target").cloned().filter(|v| !v.is_null());
        let res = c.rt.block_on(async move { ha.call_service(&service, data, target).await });
        res.map(|v| v.to_string()).map_err(extism::Error::msg)
    })
});

host_fn!(puzzlebox_kv_get(user_data: HostCtx; key: String) -> String {
    ctx_get(&user_data, |c| {
        if !c.caps.kv {
            return Err(extism::Error::msg("capability 'kv' not granted"));
        }
        let store = c.store.clone();
        let scope = format!("plugin:{}", c.plugin_id);
        let all = c.rt.block_on(async move { store.kv_all(&scope).await }).map_err(extism::Error::msg)?;
        Ok(all.get(&key).cloned().unwrap_or(Value::Null).to_string())
    })
});

host_fn!(puzzlebox_kv_set(user_data: HostCtx; request: String) {
    ctx_get(&user_data, |c| {
        if !c.caps.kv {
            return Err(extism::Error::msg("capability 'kv' not granted"));
        }
        let req: Value = serde_json::from_str(&request)?;
        let key = req["key"].as_str().unwrap_or_default().to_owned();
        let value = req.get("value").cloned().unwrap_or(Value::Null);
        let store = c.store.clone();
        let scope = format!("plugin:{}", c.plugin_id);
        c.rt.block_on(async move { store.kv_set(&scope, &key, &value).await }).map_err(extism::Error::msg)
    })
});

pub struct LoadedPlugin {
    pub pack: Pack,
    pub capabilities: Capabilities,
    pub path: PathBuf,
    instance: Arc<Mutex<Plugin>>,
}

pub struct WasmHost {
    plugins: RwLock<HashMap<String, Arc<LoadedPlugin>>>,
    ha: Arc<dyn HaApi>,
    store: Store,
}

#[derive(Serialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub blocks: usize,
    pub capabilities: Capabilities,
    pub path: String,
}

impl WasmHost {
    pub fn new(ha: Arc<dyn HaApi>, store: Store) -> Arc<Self> {
        Arc::new(Self { plugins: RwLock::new(HashMap::new()), ha, store })
    }

    pub fn has_plugin(&self, id: &str) -> bool {
        self.plugins.read().unwrap_or_else(|p| p.into_inner()).contains_key(id)
    }

    pub fn infos(&self) -> Vec<PluginInfo> {
        let mut out: Vec<PluginInfo> = self
            .plugins
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .map(|p| PluginInfo {
                id: p.pack.id.clone(),
                name: p.pack.name.clone(),
                version: p.pack.version.clone(),
                description: p.pack.description.clone(),
                blocks: p.pack.blocks.len(),
                capabilities: p.capabilities.clone(),
                path: p.path.display().to_string(),
            })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    pub fn packs(&self) -> Vec<Pack> {
        self.plugins.read().unwrap_or_else(|p| p.into_inner()).values().map(|p| p.pack.clone()).collect()
    }

    /// Loads one plugin directory.
    pub fn load(&self, dir: &Path) -> Result<Pack, String> {
        let text =
            std::fs::read_to_string(dir.join("plugin.yaml")).map_err(|e| format!("plugin.yaml: {e}"))?;
        let manifest: PluginManifest =
            serde_yaml::from_str(&text).map_err(|e| format!("plugin.yaml: {e}"))?;
        let pack = from_file(manifest.pack, "wasm")?;
        let wasm_path = dir.join(&manifest.wasm);
        if wasm_path.parent() != Some(dir) {
            return Err("'wasm' must name a file inside the plugin directory".into());
        }
        let bytes = std::fs::read(&wasm_path).map_err(|e| format!("{}: {e}", manifest.wasm))?;
        let instance = self.instantiate(&pack.id, &manifest.capabilities, bytes)?;
        let loaded = Arc::new(LoadedPlugin {
            pack: pack.clone(),
            capabilities: manifest.capabilities,
            path: dir.to_owned(),
            instance: Arc::new(Mutex::new(instance)),
        });
        self.plugins.write().unwrap_or_else(|p| p.into_inner()).insert(pack.id.clone(), loaded);
        Ok(pack)
    }

    fn instantiate(&self, id: &str, caps: &Capabilities, bytes: Vec<u8>) -> Result<Plugin, String> {
        let manifest = Manifest::new([Wasm::data(bytes)])
            .with_allowed_hosts(caps.http.clone().into_iter())
            .with_timeout(Duration::from_millis(caps.timeout_ms.unwrap_or(10_000)))
            .with_memory_max(caps.memory_mb.unwrap_or(64) * 16);
        let ctx = || {
            UserData::new(HostCtx {
                plugin_id: id.to_owned(),
                caps: caps.clone(),
                ha: self.ha.clone(),
                store: self.store.clone(),
                rt: Handle::current(),
            })
        };
        PluginBuilder::new(manifest)
            .with_wasi(true)
            .with_function("puzzlebox_log", [PTR], [], ctx(), puzzlebox_log)
            .with_function("puzzlebox_get_state", [PTR], [PTR], ctx(), puzzlebox_get_state)
            .with_function("puzzlebox_call_service", [PTR], [PTR], ctx(), puzzlebox_call_service)
            .with_function("puzzlebox_kv_get", [PTR], [PTR], ctx(), puzzlebox_kv_get)
            .with_function("puzzlebox_kv_set", [PTR], [], ctx(), puzzlebox_kv_set)
            .build()
            .map_err(|e| format!("failed to load WASM module: {e}"))
    }

    /// Loads every plugin directory under `root`; returns (loaded packs, errors).
    pub fn load_dir(&self, root: &Path) -> (Vec<Pack>, Vec<(String, String)>) {
        let mut packs = Vec::new();
        let mut errors = Vec::new();
        let Ok(entries) = std::fs::read_dir(root) else { return (packs, errors) };
        for e in entries.flatten() {
            let path = e.path();
            if !path.join("plugin.yaml").exists() {
                continue;
            }
            match self.load(&path) {
                Ok(p) => {
                    info!(plugin = %p.id, blocks = p.blocks.len(), "WASM plugin loaded");
                    packs.push(p);
                }
                Err(err) => {
                    warn!(path = %path.display(), "WASM plugin failed to load: {err}");
                    errors.push((path.display().to_string(), err));
                }
            }
        }
        (packs, errors)
    }

    pub async fn call(
        &self,
        plugin: &str,
        function: &str,
        args: Value,
        context: Value,
    ) -> Result<Value, String> {
        let p = self
            .plugins
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(plugin)
            .cloned()
            .ok_or_else(|| format!("WASM plugin '{plugin}' is not loaded"))?;
        let input = json!({"args": args, "context": context}).to_string();
        let function = function.to_owned();
        tokio::task::spawn_blocking(move || {
            let mut inst = p.instance.lock().map_err(|_| "plugin instance poisoned".to_owned())?;
            let out: String = inst.call(&function, input.as_str()).map_err(|e| format!("{e:#}"))?;
            if out.trim().is_empty() {
                return Ok(Value::Null);
            }
            Ok(serde_json::from_str(&out).unwrap_or(Value::String(out)))
        })
        .await
        .map_err(|e| e.to_string())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct NoHa;
    #[async_trait]
    impl HaApi for NoHa {
        async fn call_service(&self, s: &str, _: Option<Value>, _: Option<Value>) -> Result<Value, String> {
            Ok(json!({"called": s}))
        }
        fn state(&self, e: &str) -> Option<Value> {
            Some(json!({"entity_id": e, "state": "on"}))
        }
        async fn test_condition(&self, _: Value, _: Value) -> Result<bool, String> {
            Ok(true)
        }
        async fn execute_script(&self, _: Value, _: Value) -> Result<Value, String> {
            Ok(Value::Null)
        }
        async fn render_template(&self, _: &str, _: Value) -> Result<Value, String> {
            Ok(Value::Null)
        }
        async fn fire_event(&self, _: &str, _: Option<Value>) -> Result<(), String> {
            Ok(())
        }
        async fn subscribe_trigger(&self, _: Value) -> Result<puzzlebox_engine::TriggerStream, String> {
            Err("n/a".into())
        }
    }

    /// Echoes its input (exercises the Extism kernel ABI), and `state` forwards the input
    /// to `puzzlebox_get_state` to exercise host functions and capability checks.
    const PLUGIN_WAT: &str = r#"
(module
  (import "extism:host/env" "input_length" (func $input_length (result i64)))
  (import "extism:host/env" "input_load_u8" (func $input_load_u8 (param i64) (result i32)))
  (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
  (import "extism:host/env" "store_u8" (func $store_u8 (param i64 i32)))
  (import "extism:host/env" "length" (func $length (param i64) (result i64)))
  (import "extism:host/env" "output_set" (func $output_set (param i64 i64)))
  (import "extism:host/user" "puzzlebox_get_state" (func $puzzlebox_get_state (param i64) (result i64)))
  (memory (export "memory") 1)
  (func $copy_input (result i64)
    (local $len i64) (local $mem i64) (local $i i64)
    (local.set $len (call $input_length))
    (local.set $mem (call $alloc (local.get $len)))
    (block $done (loop $loop
      (br_if $done (i64.ge_u (local.get $i) (local.get $len)))
      (call $store_u8 (i64.add (local.get $mem) (local.get $i)) (call $input_load_u8 (local.get $i)))
      (local.set $i (i64.add (local.get $i) (i64.const 1)))
      (br $loop)))
    (local.get $mem))
  (func (export "echo") (result i32)
    (call $output_set (call $copy_input) (call $input_length))
    (i32.const 0))
  (func (export "state") (result i32)
    (local $out i64)
    (local.set $out (call $puzzlebox_get_state (call $copy_input)))
    (call $output_set (local.get $out) (call $length (local.get $out)))
    (i32.const 0)))
"#;

    fn write_plugin(dir: &Path, states: bool) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("echo.wasm"), wat::parse_str(PLUGIN_WAT).unwrap()).unwrap();
        let yaml = format!(
            "pack: echo\nname: Echo\nwasm: echo.wasm\ncapabilities: {{states: {states}}}\nblocks:\n  - id: echo\n    kind: value\n    title: echo\n    lowering: {{type: plugin, plugin: echo, function: echo}}\n"
        );
        std::fs::write(dir.join("plugin.yaml"), yaml).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn loads_and_calls_plugin_with_capabilities() {
        let tmp = std::env::temp_dir().join(format!("puzzlebox-wasm-{}", rand::random::<u64>()));
        let host = WasmHost::new(Arc::new(NoHa), Store::open_in_memory().unwrap());

        write_plugin(&tmp.join("echo"), false);
        let (packs, errors) = host.load_dir(&tmp);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(packs[0].blocks[0].block_type, "wasm__echo__echo");

        let out = host.call("echo", "echo", json!({"x": 1}), json!({"run_id": "r"})).await.unwrap();
        assert_eq!(out, json!({"args": {"x": 1}, "context": {"run_id": "r"}}));

        let denied = host.call("echo", "state", json!({}), json!({})).await.unwrap_err();
        assert!(denied.contains("states"), "{denied}");

        write_plugin(&tmp.join("echo"), true);
        host.load(&tmp.join("echo")).unwrap();
        // The host receives the whole input JSON as the "entity id"; it just echoes it back.
        let out = host.call("echo", "state", json!({}), json!({})).await.unwrap();
        assert_eq!(out["state"], "on");
        std::fs::remove_dir_all(&tmp).ok();
    }
}
