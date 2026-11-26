"""Version handshake with the Puzzlebox app.

Both sides open with a ``hello`` carrying their product version, protocol range and a map of
features with a version each. Features only one side knows are not used, so mixed versions keep
working with what they share; a protocol outside the other side's range means they can't talk.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

PROTOCOL = 2
# Oldest app protocol this integration still works with (1 = apps before 0.3.0).
MIN_PEER_PROTOCOL = 1

FEATURES: dict[str, int] = {
    "programs": 1,  # program list, enable/disable, run, set_variable
    "run_events": 1,  # run_started / run_finished events
    "webhooks": 1,  # webhooks registered on the app's behalf
    "packs": 1,  # blocks and handlers from other integrations
    "variables": 1,  # global variables as sensors
}

# What apps before the handshake (0.2.0 and older) offer: 0.1.x had no "variables".
LEGACY_APP_FEATURES = {"programs": 1, "run_events": 1, "webhooks": 1, "packs": 1}


def parse_version(version: str | None) -> tuple[int, int, int]:
    core = (version or "").strip().lstrip("v").split("-")[0].split("+")[0]
    parts = []
    for p in core.split(".")[:3]:
        try:
            parts.append(int(p))
        except ValueError:
            parts.append(0)
    while len(parts) < 3:
        parts.append(0)
    return parts[0], parts[1], parts[2]


def hello(integration_version: str) -> dict[str, Any]:
    return {
        "type": "hello",
        "protocol": PROTOCOL,
        "min_protocol": MIN_PEER_PROTOCOL,
        "integration_version": integration_version,
        "features": FEATURES,
    }


@dataclass
class AppInfo:
    version: str | None
    protocol: int
    min_protocol: int
    features: dict[str, int] = field(default_factory=dict)

    @classmethod
    def from_hello(cls, msg: dict[str, Any]) -> AppInfo:
        if "protocol" not in msg:
            # Apps before 0.3.0 send only {"version": <protocol>, "app_version": ...}.
            proto = int(msg.get("version") or 1)
            return cls(msg.get("app_version"), proto, proto, dict(LEGACY_APP_FEATURES))
        features = {str(k): int(v) for k, v in (msg.get("features") or {}).items() if isinstance(v, int)}
        return cls(
            msg.get("app_version"),
            int(msg["protocol"]),
            int(msg.get("min_protocol") or msg["protocol"]),
            features,
        )

    def has(self, feature: str) -> bool:
        return feature in FEATURES and feature in self.features

    def compatible(self) -> bool:
        return self.protocol >= MIN_PEER_PROTOCOL and self.min_protocol <= PROTOCOL

    def outdated(self, integration_version: str) -> str | None:
        """Which side should be updated: "app", "integration" or None."""
        app, ours = parse_version(self.version), parse_version(integration_version)
        if app < ours or self.protocol < PROTOCOL:
            return "app"
        if app > ours or self.min_protocol > PROTOCOL:
            return "integration"
        return None
