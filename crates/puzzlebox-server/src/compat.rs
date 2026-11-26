//! Version handshake with the companion integration.
//!
//! Both sides open with a `hello` carrying their product version, protocol range and a map
//! of features with a version each. A side only uses features both support, so mismatched
//! versions keep working with what they share. A protocol outside the other side's range means
//! the two can't talk at all; then only the hello is exchanged and both ask for an update.

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// Version of the message framing and handshake. Bump when old peers can't be served at all.
pub const PROTOCOL: u32 = 2;
/// Oldest integration protocol this app still serves (1 = integrations before 0.3.0).
pub const MIN_PEER_PROTOCOL: u32 = 1;

/// Features this app offers, with their version. Bump a feature's version when its messages
/// change; add a feature when adding a message kind.
pub fn features() -> BTreeMap<String, u32> {
    [
        ("programs", 1),   // program list, enable/disable, run, set_variable
        ("run_events", 1), // run_started / run_finished
        ("webhooks", 1),   // webhook ids for the integration to register
        ("packs", 1),      // blocks and handlers registered by integrations (invoke)
        ("variables", 1),  // global variables mirrored as sensors
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect()
}

pub fn hello() -> Value {
    json!({
        "type": "hello",
        // `version` is the protocol for integrations before 0.3.0, which read only this.
        "version": PROTOCOL,
        "protocol": PROTOCOL,
        "min_protocol": MIN_PEER_PROTOCOL,
        "app_version": env!("CARGO_PKG_VERSION"),
        "features": features(),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Peer {
    /// Integration version; `None` for integrations from before the handshake.
    pub version: Option<String>,
    pub protocol: u32,
    pub min_protocol: u32,
    pub features: BTreeMap<String, u32>,
}

impl Peer {
    /// An integration that connected without a hello (0.2.0 and older).
    pub fn legacy() -> Self {
        let features = ["programs", "run_events", "webhooks", "packs", "variables"]
            .into_iter()
            .map(|f| (f.to_owned(), 1))
            .collect();
        Self { version: None, protocol: 1, min_protocol: 1, features }
    }

    pub fn from_hello(msg: &Value) -> Self {
        let num = |k: &str| msg.get(k).and_then(Value::as_u64).map(|n| n as u32);
        let protocol = num("protocol").unwrap_or(1);
        Self {
            version: msg.get("integration_version").and_then(Value::as_str).map(str::to_owned),
            protocol,
            min_protocol: num("min_protocol").unwrap_or(protocol),
            features: msg
                .get("features")
                .and_then(Value::as_object)
                .map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_u64()? as u32))).collect())
                .unwrap_or_default(),
        }
    }

    /// Version of a feature both sides support (the lower one), if any.
    pub fn shared(&self, feature: &str) -> Option<u32> {
        let ours = features().get(feature).copied()?;
        self.features.get(feature).map(|&theirs| theirs.min(ours))
    }

    pub fn compatible(&self) -> bool {
        self.protocol >= MIN_PEER_PROTOCOL && PROTOCOL >= self.min_protocol
    }

    /// Which side should be updated, if the versions differ.
    pub fn outdated(&self) -> Option<&'static str> {
        let Some(theirs) = self.version.as_deref().map(parse_version) else {
            return Some("integration");
        };
        let ours = parse_version(env!("CARGO_PKG_VERSION"));
        if theirs < ours || self.protocol < PROTOCOL {
            Some("integration")
        } else if theirs > ours || self.min_protocol > PROTOCOL {
            Some("app")
        } else {
            None
        }
    }

    /// Our features the integration lacks or supports only in an older version.
    pub fn missing(&self) -> Vec<String> {
        features()
            .into_iter()
            .filter(|(f, v)| self.features.get(f).is_none_or(|theirs| theirs < v))
            .map(|(f, _)| f)
            .collect()
    }

    pub fn status(&self) -> Value {
        json!({
            "integration_version": self.version,
            "protocol": self.protocol,
            "compatible": self.compatible(),
            "outdated": self.outdated(),
            "missing": self.missing(),
        })
    }
}

/// `1.2.3` → (1, 2, 3); anything after a `-`/`+` and unparsable parts count as 0.
pub fn parse_version(v: &str) -> (u64, u64, u64) {
    let core = v.trim().trim_start_matches('v').split(['-', '+']).next().unwrap_or("");
    let mut parts = core.split('.').map(|p| p.parse().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_integration_works_but_should_update() {
        let p = Peer::legacy();
        assert!(p.compatible());
        assert_eq!(p.outdated(), Some("integration"));
        assert_eq!(p.shared("variables"), Some(1));
    }

    #[test]
    fn same_version_is_current() {
        let p = Peer::from_hello(&json!({
            "type": "hello", "protocol": PROTOCOL, "min_protocol": 1,
            "integration_version": env!("CARGO_PKG_VERSION"), "features": features(),
        }));
        assert!(p.compatible());
        assert_eq!(p.outdated(), None);
        assert!(p.missing().is_empty());
    }

    #[test]
    fn newer_integration_and_incompatible_protocols() {
        let newer = Peer::from_hello(&json!({
            "protocol": PROTOCOL + 1, "min_protocol": PROTOCOL, "integration_version": "99.0.0",
            "features": {"programs": 2, "teleport": 1},
        }));
        assert!(newer.compatible());
        assert_eq!(newer.outdated(), Some("app"));
        assert_eq!(newer.shared("programs"), Some(1));
        assert_eq!(newer.shared("teleport"), None);
        assert!(newer.missing().contains(&"variables".to_owned()));

        let future =
            Peer::from_hello(&json!({"protocol": 9, "min_protocol": 9, "integration_version": "9.0.0"}));
        assert!(!future.compatible());
        assert_eq!(future.outdated(), Some("app"));
    }

    #[test]
    fn versions_parse() {
        assert_eq!(parse_version("v0.2.10-beta"), (0, 2, 10));
        assert!(parse_version("0.10.0") > parse_version("0.9.9"));
    }
}
