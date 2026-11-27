"""Per-program entities, the run_program action and Repairs for broken programs."""

import json
from pathlib import Path
from unittest.mock import AsyncMock, patch

import pytest
from pytest_homeassistant_custom_component.common import MockConfigEntry

from custom_components.puzzlebox.bridge import BridgeClient
from custom_components.puzzlebox.compat import FEATURES, PROTOCOL
from custom_components.puzzlebox.const import CONF_TOKEN, CONF_URL, DOMAIN
from homeassistant.core import HomeAssistant
from homeassistant.exceptions import HomeAssistantError
from homeassistant.helpers import device_registry as dr, entity_registry as er, issue_registry as ir

# The app is as new as the integration, so no "update" Repair gets in the way.
MANIFEST = Path(__file__).parents[2] / "custom_components" / "puzzlebox" / "manifest.json"
HELLO = {
    "type": "hello",
    "protocol": PROTOCOL,
    "min_protocol": 1,
    "app_version": json.loads(MANIFEST.read_text())["version"],
    "features": FEATURES,
}


def _program(pid: str = "p1", name: str = "Wake up", **status) -> dict:
    extra = {k: status.pop(k) for k in ("activity", "failures", "enabled") if k in status}
    return {
        "id": pid,
        "name": name,
        "enabled": extra.pop("enabled", True),
        "mode": "single",
        "status": {"state": "idle", "active_runs": 0, "errors": [], **status},
        **extra,
    }


async def _setup(hass: HomeAssistant, *programs: dict, hello: dict = HELLO) -> BridgeClient:
    entry = MockConfigEntry(domain=DOMAIN, data={CONF_URL: "http://app:8099", CONF_TOKEN: "t"})
    entry.add_to_hass(hass)
    with patch.object(BridgeClient, "async_start", AsyncMock()):
        assert await hass.config_entries.async_setup(entry.entry_id)
    client: BridgeClient = hass.data[DOMAIN]
    client._send = AsyncMock(return_value=True)
    client._handle(hello)
    client._handle({"type": "programs", "programs": list(programs)})
    await hass.async_block_till_done()
    return client


def _entity(hass: HomeAssistant, domain: str, unique_id: str) -> str:
    entity_id = er.async_get(hass).async_get_entity_id(domain, DOMAIN, unique_id)
    assert entity_id
    return entity_id


def _issues(hass: HomeAssistant) -> dict[str, dict]:
    reg = ir.async_get(hass)
    return {i.issue_id: i.translation_placeholders for (d, _), i in reg.issues.items() if d == DOMAIN}


async def test_status_sensor(hass: HomeAssistant) -> None:
    client = await _setup(hass, _program(last_run=1_700_000_000_000, last_status="ok"))
    sensor = _entity(hass, "sensor", "p1_status")
    state = hass.states.get(sensor)
    assert state.state == "idle"
    assert state.attributes["last_run"].startswith("2023-11-14T22:13:20")
    assert state.attributes["last_result"] == "ok"
    assert state.attributes["options"] == ["idle", "running", "waiting", "paused", "error", "disabled"]

    for status, activity, expected in [
        ({"state": "running", "active_runs": 1}, None, "running"),
        ({"state": "running", "active_runs": 1}, "waiting", "waiting"),
        ({"state": "running", "active_runs": 1}, "paused", "paused"),
        ({"state": "invalid", "errors": [{"message": "choose an entity"}]}, None, "error"),
        ({"state": "disabled"}, None, "disabled"),
    ]:
        client._handle({"type": "programs", "programs": [_program(activity=activity, **status)]})
        await hass.async_block_till_done()
        assert hass.states.get(sensor).state == expected, status


async def test_status_sensor_with_an_older_app(hass: HomeAssistant) -> None:
    client = await _setup(hass, _program(), hello={"type": "hello", "version": 1, "app_version": "0.1.1"})
    assert client.app.feature_version("programs") == 1
    client._handle({"type": "programs", "programs": [_program(state="running", active_runs=1)]})
    await hass.async_block_till_done()
    assert hass.states.get(_entity(hass, "sensor", "p1_status")).state == "running"


async def test_run_program_by_entity_device_or_name(hass: HomeAssistant) -> None:
    client = await _setup(hass, _program(), _program("p2", "Night"))
    switch = _entity(hass, "switch", "p1_enabled")
    button = _entity(hass, "button", "p2_run")

    async def runs(**data) -> list[str]:
        client._send.reset_mock()
        await hass.services.async_call(DOMAIN, "run_program", data, blocking=True)
        return [c.args[0]["program_id"] for c in client._send.call_args_list]

    assert await runs(entity_id=switch) == ["p1"]
    assert await runs(entity_id=[switch, button]) == ["p1", "p2"]
    assert await runs(program="night") == ["p2"]
    assert await runs(program="Wake up", entity_id=switch) == ["p1"]
    device = dr.async_get(hass).async_get(er.async_get(hass).async_get(button).device_id)
    assert await runs(device_id=device.id) == ["p2"]

    with pytest.raises(HomeAssistantError, match="isn't a Puzzlebox program"):
        await runs(entity_id="light.kitchen")
    with pytest.raises(HomeAssistantError, match="No Puzzlebox program named"):
        await runs(program="Gone")
    with pytest.raises(Exception, match="at least one of"):
        await runs()


async def test_repairs_for_invalid_and_failing_programs(hass: HomeAssistant) -> None:
    client = await _setup(hass, _program())
    assert _issues(hass) == {}

    def update(*programs: dict) -> None:
        client._handle({"type": "programs", "programs": list(programs)})

    update(_program(state="invalid", errors=[{"block_id": "b", "message": "light.porch doesn't exist"}]))
    assert _issues(hass) == {
        "program_invalid_p1": {"name": "Wake up", "problems": "light.porch doesn't exist"}
    }

    update(_program(failures=2, last_error="timed out"))
    assert _issues(hass) == {}
    update(_program(failures=3, last_error="timed out"))
    assert _issues(hass) == {"program_failing_p1": {"name": "Wake up", "count": "3", "error": "timed out"}}

    # Turned off, fixed or deleted: the issue goes away.
    update(_program(failures=3, enabled=False, state="disabled"))
    assert _issues(hass) == {}
    update(_program(failures=3, last_error="timed out"))
    update()
    assert _issues(hass) == {}
