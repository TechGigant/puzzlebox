"""Version handshake: mixed versions keep working and ask for an update."""

from custom_components.puzzlebox.bridge import BridgeClient
from custom_components.puzzlebox.compat import FEATURES, PROTOCOL, AppInfo, parse_version
from custom_components.puzzlebox.const import DOMAIN
from homeassistant.core import HomeAssistant
from homeassistant.helpers import issue_registry as ir

PROGRAMS = {"type": "programs", "programs": [{"id": "p1", "name": "Lights", "enabled": True}]}


def _client(hass: HomeAssistant, version: str = "0.3.0") -> BridgeClient:
    return BridgeClient(hass, "http://app:8099", "t", version)


def _issues(hass: HomeAssistant) -> dict[str, str]:
    reg = ir.async_get(hass)
    return {i.issue_id: i.translation_key for (d, _), i in reg.issues.items() if d == DOMAIN}


async def test_same_version(hass: HomeAssistant) -> None:
    client = _client(hass)
    client._handle(
        {
            "type": "hello",
            "protocol": PROTOCOL,
            "min_protocol": 1,
            "app_version": "0.3.0",
            "features": FEATURES,
        }
    )
    client._handle(PROGRAMS)
    assert client.connected
    assert "p1" in client.programs
    assert _issues(hass) == {}


async def test_old_app_still_works_and_asks_for_update(hass: HomeAssistant) -> None:
    client = _client(hass)
    client._handle({"type": "hello", "version": 1, "app_version": "0.1.1"})
    client._handle(PROGRAMS)
    assert client.connected
    assert "p1" in client.programs
    assert not client.app.has("variables")
    assert _issues(hass) == {"update_app": "update_app"}


async def test_newer_app_asks_to_update_integration(hass: HomeAssistant) -> None:
    client = _client(hass, "0.3.0")
    client._handle(
        {
            "type": "hello",
            "protocol": PROTOCOL,
            "min_protocol": 1,
            "app_version": "0.4.0",
            "features": FEATURES,
        }
    )
    assert client.connected
    assert _issues(hass) == {"update_integration": "update_integration"}


async def test_incompatible_app_is_not_used(hass: HomeAssistant) -> None:
    client = _client(hass)
    client._handle(
        {"type": "hello", "protocol": 9, "min_protocol": 9, "app_version": "9.0.0", "features": {}}
    )
    client._handle(PROGRAMS)
    assert not client.connected
    assert client.programs == {}
    assert _issues(hass) == {"update_integration": "update_integration_required"}
    await client.async_stop()
    assert _issues(hass) == {}


def test_parse_version() -> None:
    assert parse_version("v0.10.1-beta") == (0, 10, 1)
    assert parse_version(None) == (0, 0, 0)
    assert AppInfo.from_hello({"version": 1, "app_version": "0.2.0"}).compatible()
