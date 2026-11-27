//! Keeps the block registry in sync with Home Assistant and installed packs.
//!
//! Blocks generated from Home Assistant, Zigbee2MQTT and Python plugins are also kept in the
//! store, so programs compile and the editor can show them while those aren't connected yet.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use puzzlebox_engine::Engine;
use puzzlebox_ha_client::{ConnectionState, HaClient};
use puzzlebox_registry::autogen::{HaDescriptions, generate};
use puzzlebox_registry::{BlockDef, Pack, Registry, parse_pack};
use puzzlebox_storage::now_ms;
use serde_json::{Map, Value, json};
use tokio::sync::{Mutex, Notify};
use tracing::{info, warn};

pub struct RegistryService {
    pub registry: Registry,
    engine: Engine,
    ha: HaClient,
    pub packs_dirs: Vec<PathBuf>,
    packs: Mutex<Vec<Pack>>,
    pack_errors: Mutex<Vec<(String, String)>>,
    dirty: Notify,
    /// Sources loaded from the store that haven't been confirmed since Puzzlebox started.
    cached: std::sync::Mutex<HashSet<String>>,
    /// The Home Assistant blocks were built from HA since it last (re)connected.
    ha_current: AtomicBool,
}

const CACHE_SCOPE: &str = "registry_cache";
/// Stored blocks of a source that hasn't been seen for this long are forgotten.
const CACHE_DAYS: i64 = 30;

/// Sources that come from something that may not be connected at startup.
fn cacheable(source: &str) -> bool {
    source == "ha" || source.starts_with("z2m:") || source.starts_with("py:")
}

const TRANSLATION_CATEGORIES: [&str; 5] = ["title", "services", "triggers", "conditions", "selector"];

impl RegistryService {
    pub fn new(engine: Engine, ha: HaClient, packs_dirs: Vec<PathBuf>) -> Arc<Self> {
        let svc = Arc::new(Self {
            registry: Registry::default(),
            engine,
            ha,
            packs_dirs,
            packs: Mutex::new(Vec::new()),
            pack_errors: Mutex::new(Vec::new()),
            dirty: Notify::new(),
            cached: std::sync::Mutex::new(HashSet::new()),
            ha_current: AtomicBool::new(false),
        });
        tokio::spawn(svc.clone().run());
        svc
    }

    /// Requests a (debounced) rebuild.
    pub fn invalidate(&self) {
        self.dirty.notify_one();
    }

    pub async fn packs(&self) -> Vec<Pack> {
        self.packs.lock().await.clone()
    }

    pub async fn pack_errors(&self) -> Vec<(String, String)> {
        self.pack_errors.lock().await.clone()
    }

    /// Sources shown from the store, not yet confirmed by their integration.
    pub fn cached_sources(&self) -> Vec<String> {
        let mut v: Vec<String> =
            self.cached.lock().unwrap_or_else(|p| p.into_inner()).iter().cloned().collect();
        v.sort();
        v
    }

    pub fn ha_current(&self) -> bool {
        self.ha_current.load(Ordering::Relaxed)
    }

    /// Loads the blocks stored by an earlier run (before programs start).
    pub async fn load_cache(&self) {
        let stored = match self.engine.store().kv_all(CACHE_SCOPE).await {
            Ok(s) => s,
            Err(e) => return warn!(error = %e, "failed to read the stored blocks"),
        };
        let oldest = now_ms() - CACHE_DAYS * 24 * 3600 * 1000;
        let mut n = 0;
        for (source, v) in stored {
            let fresh = v["saved_at"].as_i64().is_some_and(|t| t >= oldest);
            match serde_json::from_value::<Vec<BlockDef>>(v["blocks"].clone()) {
                Ok(blocks) if fresh && cacheable(&source) => {
                    n += blocks.len();
                    self.registry.set_source(&source, blocks);
                    self.cached.lock().unwrap_or_else(|p| p.into_inner()).insert(source);
                }
                _ => {
                    let _ = self.engine.store().kv_delete(CACHE_SCOPE, &source).await;
                }
            }
        }
        if n > 0 {
            info!(blocks = n, "loaded stored blocks");
        }
    }

    /// Stores a source's current blocks (None: forgets them).
    async fn remember(&self, source: &str, blocks: Option<&[BlockDef]>) {
        if !cacheable(source) {
            return;
        }
        self.cached.lock().unwrap_or_else(|p| p.into_inner()).remove(source);
        let store = self.engine.store();
        let res = match blocks {
            Some(b) => store.kv_set(CACHE_SCOPE, source, &json!({"saved_at": now_ms(), "blocks": b})).await,
            None => store.kv_delete(CACHE_SCOPE, source).await,
        };
        if let Err(e) = res {
            warn!(error = %e, "failed to store blocks");
        }
    }

    async fn run(self: Arc<Self>) {
        self.reload_packs().await;
        let watcher = self.clone();
        tokio::spawn(async move { watcher.watch_ha().await });
        loop {
            self.dirty.notified().await;
            // Integrations register services in bursts while HA starts.
            tokio::time::sleep(Duration::from_secs(2)).await;
            if let Err(e) = self.refresh_ha().await {
                warn!(error = %e, "failed to build Home Assistant blocks");
            }
        }
    }

    async fn watch_ha(self: Arc<Self>) {
        let mut conn = self.ha.state_watch();
        let mut events = self.ha.subscribe_events(None).await.ok();
        loop {
            if *conn.borrow_and_update() == ConnectionState::Connected {
                self.invalidate();
            } else {
                self.ha_current.store(false, Ordering::Relaxed);
            }
            tokio::select! {
                changed = conn.changed() => if changed.is_err() { return },
                ev = async {
                    match events.as_mut() { Some(e) => e.next().await, None => std::future::pending().await }
                } => {
                    let Some(ev) = ev else { continue };
                    let ty = ev["event_type"].as_str().unwrap_or("");
                    if matches!(ty, "service_registered" | "service_removed" | "component_loaded") {
                        self.invalidate();
                    }
                }
            }
        }
    }

    async fn first_event(&self, ty: &str) -> anyhow::Result<Value> {
        let mut sub = self.ha.subscribe(json!({"type": ty})).await?;
        let ev = tokio::time::timeout(Duration::from_secs(10), sub.next()).await?;
        Ok(ev.unwrap_or(Value::Null))
    }

    /// Builds HA blocks immediately (startup), so programs compile on their first start.
    pub async fn refresh_now(&self) -> anyhow::Result<()> {
        self.refresh_ha().await
    }

    async fn refresh_ha(&self) -> anyhow::Result<()> {
        let services = self.ha.get_services().await?;
        // Older HA versions lack these; services alone still work.
        let triggers = self.first_event("trigger_platforms/subscribe").await.unwrap_or(Value::Null);
        let conditions = self.first_event("condition_platforms/subscribe").await.unwrap_or(Value::Null);
        let config = self.ha.get_config().await.ok();
        if let Some(tz) = config.as_ref().and_then(|c| c.get("time_zone")).and_then(Value::as_str)
            && !puzzlebox_engine::clock::set_zone(tz)
        {
            warn!(zone = tz, "unknown Home Assistant time zone; using the container's");
        }
        let language = config
            .as_ref()
            .and_then(|c| c.get("language").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| "en".into());
        let mut translations = Map::new();
        for cat in TRANSLATION_CATEGORIES {
            // HA falls back to English for strings missing in the configured language.
            let res = self
                .ha
                .request(json!({"type": "frontend/get_translations", "language": language, "category": cat}))
                .await;
            if let Ok(Value::Object(mut r)) = res
                && let Some(Value::Object(resources)) = r.remove("resources")
            {
                translations.extend(resources);
            }
        }
        let config_entries =
            self.ha.request(json!({"type": "config_entries/get"})).await.unwrap_or(Value::Null);
        let defs = generate(&HaDescriptions {
            config_entries: &config_entries,
            services: &services,
            triggers: &triggers,
            conditions: &conditions,
            translations: &translations,
        });
        let n = defs.len();
        let changed = self.registry.set_source("ha", defs.clone());
        self.remember("ha", Some(&defs)).await;
        let was_current = self.ha_current.swap(true, Ordering::Relaxed);
        if changed {
            info!(blocks = n, "Home Assistant blocks updated");
        }
        // Also unchanged ones after a (re)connect: the editor waits for this to know which
        // blocks Home Assistant really has.
        if changed || !was_current {
            self.publish().await;
        }
        Ok(())
    }

    pub async fn reload_packs(&self) {
        let mut packs = Vec::new();
        let mut errors = Vec::new();
        for dir in &self.packs_dirs {
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
            paths.sort();
            for path in paths {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if !matches!(ext, "yaml" | "yml" | "json") {
                    continue;
                }
                match std::fs::read_to_string(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|t| parse_pack(&t, "pack"))
                {
                    Ok(p) => packs.push(p),
                    Err(e) => {
                        warn!(path = %path.display(), "invalid block pack: {e}");
                        errors.push((path.display().to_string(), e));
                    }
                }
            }
        }
        let mut changed = false;
        let old: Vec<String> =
            self.registry.sources().into_iter().filter(|s| s.starts_with("pack:")).collect();
        for s in old {
            if !packs.iter().any(|p| p.source == s) {
                changed |= self.registry.remove_source(&s);
            }
        }
        for p in &packs {
            changed |= self.registry.set_source(&p.source, p.blocks.clone());
        }
        *self.packs.lock().await = packs;
        *self.pack_errors.lock().await = errors;
        if changed {
            self.publish().await;
        }
    }

    /// Registers blocks from a runtime plugin source (`py:<id>` / `wasm:<id>`).
    pub async fn set_plugin_source(&self, source: &str, pack: Option<Pack>) {
        self.remember(source, pack.as_ref().map(|p| p.blocks.as_slice())).await;
        let changed = match pack {
            Some(p) => self.registry.set_source(source, p.blocks),
            None => self.registry.remove_source(source),
        };
        if changed {
            self.publish().await;
        }
    }

    /// Replaces the blocks of a generated source (e.g. `z2m:<base topic>`).
    pub async fn set_generated_source(&self, source: &str, blocks: Vec<puzzlebox_registry::BlockDef>) {
        self.remember(source, Some(&blocks)).await;
        if self.registry.set_source(source, blocks) {
            self.publish().await;
        }
    }

    async fn publish(&self) {
        if let Err(e) = self.engine.registry_changed(self.registry.lowerer()).await {
            warn!(error = %e, "failed to restart programs after registry change");
        }
    }
}
