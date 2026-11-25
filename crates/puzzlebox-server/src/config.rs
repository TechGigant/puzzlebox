use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use puzzlebox_ha_client::SUPERVISOR_WS_URL;
use serde::Deserialize;

/// Supervisor's Ingress proxy address; under the app only it may reach the server.
pub const INGRESS_PROXY_IP: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(172, 30, 32, 2));

#[derive(Debug, Clone)]
pub struct Config {
    pub ha_ws_url: String,
    pub ha_token: String,
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    pub log_level: String,
    /// `None` allows every peer (standalone/dev mode).
    pub allowed_peer: Option<IpAddr>,
    /// Extra block-pack directories (the app's public config dir, `PUZZLEBOX_PACKS_DIR`).
    pub extra_packs_dirs: Vec<PathBuf>,
    /// User-visible config dir (`/config` under the app): packs, WASM plugins.
    pub config_dir: Option<PathBuf>,
    pub supervisor_token: Option<String>,
    /// `mqtt://user:pass@host:port`; under the app the Supervisor's MQTT service is used instead.
    pub mqtt_url: Option<String>,
}

#[derive(Deserialize, Default)]
struct AddonOptions {
    log_level: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let data_dir = PathBuf::from(env("PUZZLEBOX_DATA_DIR").unwrap_or_else(|| {
            if std::path::Path::new("/data/options.json").exists() { "/data".into() } else { "./data".into() }
        }));
        let options: AddonOptions = std::fs::read_to_string(data_dir.join("options.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        let (ha_ws_url, ha_token, allowed_peer) = if let Some(token) = env("SUPERVISOR_TOKEN") {
            (SUPERVISOR_WS_URL.to_owned(), token, Some(INGRESS_PROXY_IP))
        } else {
            let url = env("HA_URL")
                .context("set HA_URL (e.g. http://localhost:8123) or run as a Home Assistant app")?;
            let token = env("HA_TOKEN").context("set HA_TOKEN to a long-lived access token")?;
            (to_ws_url(&url)?, token, None)
        };

        let config_dir = env("PUZZLEBOX_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| allowed_peer.is_some().then(|| PathBuf::from("/config")));
        let mut extra_packs_dirs: Vec<PathBuf> =
            env("PUZZLEBOX_PACKS_DIR").map(PathBuf::from).into_iter().collect();
        if let Some(c) = &config_dir {
            extra_packs_dirs.push(c.join("packs"));
        }

        Ok(Self {
            supervisor_token: env("SUPERVISOR_TOKEN"),
            mqtt_url: env("PUZZLEBOX_MQTT_URL"),
            extra_packs_dirs,
            config_dir,
            ha_ws_url,
            ha_token,
            bind: env("PUZZLEBOX_BIND")
                .unwrap_or_else(|| "0.0.0.0:8099".into())
                .parse()
                .context("PUZZLEBOX_BIND")?,
            data_dir,
            log_level: env("RUST_LOG").or(options.log_level).unwrap_or_else(|| "info".into()),
            allowed_peer: if env("PUZZLEBOX_ALLOW_ALL_PEERS").is_some() { None } else { allowed_peer },
        })
    }
}

fn to_ws_url(base: &str) -> Result<String> {
    let base = base.trim_end_matches('/');
    let rest = if let Some(r) = base.strip_prefix("https://") {
        format!("wss://{r}")
    } else if let Some(r) = base.strip_prefix("http://") {
        format!("ws://{r}")
    } else if base.starts_with("ws://") || base.starts_with("wss://") {
        return Ok(base.to_owned());
    } else {
        bail!("HA_URL must start with http(s):// or ws(s)://");
    };
    Ok(format!("{rest}/api/websocket"))
}

#[cfg(test)]
mod tests {
    use super::to_ws_url;

    #[test]
    fn ws_url_conversion() {
        assert_eq!(to_ws_url("http://ha.local:8123/").unwrap(), "ws://ha.local:8123/api/websocket");
        assert_eq!(to_ws_url("https://ha.example").unwrap(), "wss://ha.example/api/websocket");
        assert_eq!(to_ws_url("ws://x/api/websocket").unwrap(), "ws://x/api/websocket");
        assert!(to_ws_url("ha.local").is_err());
    }
}
