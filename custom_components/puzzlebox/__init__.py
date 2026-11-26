"""Puzzlebox bridge: connects Home Assistant to the Puzzlebox app."""

from __future__ import annotations

from datetime import timedelta

import voluptuous as vol

from homeassistant.config_entries import ConfigEntry
from homeassistant.const import Platform
from homeassistant.core import HomeAssistant, ServiceCall
from homeassistant.exceptions import HomeAssistantError
from homeassistant.helpers import config_validation as cv
from homeassistant.helpers.event import async_track_time_interval
from homeassistant.helpers.typing import ConfigType
from homeassistant.loader import async_get_integration

from .bridge import BridgeClient
from .const import CONF_TOKEN, CONF_URL, DOMAIN, PENDING_PACKS
from .supervisor import async_find_app

PLATFORMS = [Platform.SWITCH, Platform.BUTTON, Platform.SENSOR]

CONFIG_SCHEMA = cv.config_entry_only_config_schema(DOMAIN)

RUN_SCHEMA = vol.Schema({vol.Required("program"): cv.string, vol.Optional("variables"): dict})
SET_VAR_SCHEMA = vol.Schema(
    {vol.Required("name"): cv.string, vol.Required("value"): object, vol.Optional("program"): cv.string}
)


def _client(hass: HomeAssistant) -> BridgeClient:
    client: BridgeClient | None = hass.data.get(DOMAIN)
    if client is None or not client.connected:
        raise HomeAssistantError("The Puzzlebox app is not connected")
    return client


def _program_id(client: BridgeClient, program: str) -> str:
    pid = client.resolve(program)
    if pid is None:
        raise HomeAssistantError(f"No Puzzlebox program named '{program}'")
    return pid


async def async_setup(hass: HomeAssistant, config: ConfigType) -> bool:
    async def run_program(call: ServiceCall) -> None:
        client = _client(hass)
        await client.async_run(_program_id(client, call.data["program"]), call.data.get("variables"))

    async def set_variable(call: ServiceCall) -> None:
        client = _client(hass)
        program = call.data.get("program")
        pid = _program_id(client, program) if program else None
        await client.async_set_variable(call.data["name"], call.data["value"], pid)

    hass.services.async_register(DOMAIN, "run_program", run_program, schema=RUN_SCHEMA)
    hass.services.async_register(DOMAIN, "set_variable", set_variable, schema=SET_VAR_SCHEMA)
    return True


async def async_setup_entry(hass: HomeAssistant, entry: ConfigEntry) -> bool:
    integration = await async_get_integration(hass, DOMAIN)
    client = BridgeClient(hass, entry.data[CONF_URL], entry.data[CONF_TOKEN], str(integration.version))
    hass.data[DOMAIN] = client
    # Integrations may have registered blocks before the bridge was ready.
    for pack, handlers in hass.data.get(PENDING_PACKS, {}).values():
        client.register_pack(pack, handlers)
    await client.async_start()
    await hass.config_entries.async_forward_entry_setups(entry, PLATFORMS)

    async def _refresh(_now=None) -> None:
        # Picks up a reinstalled app (new token) or a changed host name without user action.
        found = await async_find_app(hass)
        if found and found != {k: entry.data.get(k) for k in found}:
            hass.config_entries.async_update_entry(entry, data={**entry.data, **found})
            hass.config_entries.async_schedule_reload(entry.entry_id)
        elif not client.connected and found:
            client.nudge()

    entry.async_on_unload(async_track_time_interval(hass, _refresh, timedelta(minutes=5)))
    return True


async def async_unload_entry(hass: HomeAssistant, entry: ConfigEntry) -> bool:
    ok = await hass.config_entries.async_unload_platforms(entry, PLATFORMS)
    client: BridgeClient | None = hass.data.pop(DOMAIN, None)
    if client:
        await client.async_stop()
    return ok
