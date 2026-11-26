mod bridge;
mod compat;
mod config;
mod devices;
mod http;
mod mqtt;
mod plugins;
mod programs;
mod registry;
mod routes;
mod wasm;
mod webhooks;
mod ws;
mod z2m;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use puzzlebox_engine::{CronSource, Engine, LiveHa};
use puzzlebox_ha_client::{ClientConfig, HaClient};
use puzzlebox_storage::Store;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    // Blocks nest (each block of a stack sits inside the previous one), and saving, compiling
    // and storing them recurse once per block: the 2 MiB default runs out in long stacks.
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(8 << 20)
        .build()?
        .block_on(run())
}

async fn run() -> Result<()> {
    let cfg = config::Config::from_env()?;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_new(&cfg.log_level).unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    std::fs::create_dir_all(&cfg.data_dir)?;
    let ha = HaClient::spawn(ClientConfig::new(&cfg.ha_ws_url, &cfg.ha_token));
    let live = LiveHa::new(ha.clone());
    let store = Store::open(&cfg.data_dir.join("puzzlebox.db"))?;
    let engine = Engine::new(live.clone(), store).await?;
    engine.add_trigger_source("cron", Arc::new(CronSource::default()));
    engine.set_http_client(Arc::new(http::ReqwestClient::new()));

    let webhooks = Arc::new(webhooks::Webhooks::default());
    engine.add_trigger_source("webhook", webhooks.clone());

    let mqtt_cfg = match (&cfg.mqtt_url, &cfg.supervisor_token) {
        (Some(url), _) => mqtt::MqttConfig::parse(url),
        (None, Some(token)) => mqtt::supervisor_mqtt(token).await,
        _ => None,
    };
    let mqtt = match mqtt_cfg {
        Some(c) => mqtt::Mqtt::connect(c),
        None => mqtt::Mqtt::disabled(),
    };
    engine.add_trigger_source("mqtt", Arc::new(mqtt::MqttSource(mqtt.clone())));

    let mut packs_dirs = vec![cfg.data_dir.join("packs")];
    packs_dirs.extend(cfg.extra_packs_dirs.iter().cloned());
    let registry = registry::RegistryService::new(engine.clone(), ha.clone(), packs_dirs);

    let plugins = plugins::PluginRouter::new();
    let _ = plugins.mqtt.set(mqtt.clone());
    let _ = plugins.z2m.set(z2m::Z2m::start(&mqtt, registry.clone(), cfg.z2m_bases.clone()));
    let wasm = wasm::WasmHost::new(live.clone(), engine.store().clone());
    let _ = plugins.wasm.set(wasm.clone());
    let bridge =
        bridge::Bridge::new(engine.clone(), registry.clone(), webhooks.clone(), &cfg.data_dir).await?;
    let _ = plugins.bridge.set(bridge.clone());
    engine.set_plugin_host(plugins.clone());
    let wasm_dirs = routes::wasm_dirs(&cfg);
    routes::load_wasm_plugins(&wasm, &registry, &wasm_dirs).await;
    if let Some(token) = &cfg.supervisor_token {
        bridge.spawn_discovery(token.clone(), cfg.bind.port());
    }

    // Programs subscribe through the client queue either way, but on-start handlers
    // need HA, so give it a moment before starting them.

    if tokio::time::timeout(Duration::from_secs(30), ha.wait_connected()).await.is_err() {
        warn!("Home Assistant not reachable yet; starting programs anyway");
    } else if let Err(e) = registry.refresh_now().await {
        warn!(error = %e, "initial block registry build failed");
    }
    engine.set_lowerer(registry.registry.lowerer());
    engine.load_all().await?;

    let state = routes::AppState {
        ha,
        live,
        engine,
        registry,
        bridge,
        webhooks,
        wasm,
        mqtt,
        devices: Arc::new(devices::DeviceCache::default()),
        cfg: cfg.clone(),
    };

    let listener = tokio::net::TcpListener::bind(cfg.bind).await?;
    info!(addr = %cfg.bind, ha = %cfg.ha_ws_url, "puzzlebox listening");
    axum::serve(listener, routes::router(state).into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

async fn shutdown() {
    let ctrl_c = tokio::signal::ctrl_c();
    let mut term =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
    tokio::select! {
        _ = ctrl_c => {},
        _ = term.recv() => {},
    }
}
