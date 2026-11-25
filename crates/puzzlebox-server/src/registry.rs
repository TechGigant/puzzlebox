//! Keeps the block registry in sync with Home Assistant and installed packs.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use puzzlebox_engine::Engine;
use puzzlebox_ha_client::{ConnectionState, HaClient};
use puzzlebox_registry::autogen::{HaDescriptions, generate};
use puzzlebox_registry::{Pack, Registry, parse_pack};
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
        let language = self
            .ha
            .get_config()
            .await
            .ok()
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
        let defs = generate(&HaDescriptions {
            services: &services,
            triggers: &triggers,
            conditions: &conditions,
            translations: &translations,
        });
        let n = defs.len();
        if self.registry.set_source("ha", defs) {
            info!(blocks = n, "Home Assistant blocks updated");
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
        let changed = match pack {
            Some(p) => self.registry.set_source(source, p.blocks),
            None => self.registry.remove_source(source),
        };
        if changed {
            self.publish().await;
        }
    }

    async fn publish(&self) {
        if let Err(e) = self.engine.registry_changed(self.registry.lowerer()).await {
            warn!(error = %e, "failed to restart programs after registry change");
        }
    }
}
