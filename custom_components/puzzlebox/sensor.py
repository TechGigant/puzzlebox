"""One sensor per global variable, so dashboards and automations can use them."""

from __future__ import annotations

import json
from typing import Any

from homeassistant.components.sensor import SensorEntity
from homeassistant.config_entries import ConfigEntry
from homeassistant.core import HomeAssistant, callback
from homeassistant.helpers.device_registry import DeviceEntryType, DeviceInfo
from homeassistant.helpers.dispatcher import async_dispatcher_connect
from homeassistant.helpers.entity_platform import AddEntitiesCallback

from .bridge import BridgeClient
from .const import DOMAIN, SIGNAL_CONNECTION, SIGNAL_VARIABLES

MAX_STATE_LENGTH = 255


async def async_setup_entry(
    hass: HomeAssistant, entry: ConfigEntry, async_add_entities: AddEntitiesCallback
) -> None:
    client: BridgeClient = hass.data[DOMAIN]
    known: set[str] = set()

    @callback
    def sync() -> None:
        new = set(client.variables) - known
        if new:
            async_add_entities(GlobalVariableSensor(client, name) for name in sorted(new))
            known.update(new)

    sync()
    entry.async_on_unload(async_dispatcher_connect(hass, SIGNAL_VARIABLES, sync))


class GlobalVariableSensor(SensorEntity):
    _attr_has_entity_name = True
    _attr_should_poll = False
    _attr_icon = "mdi:variable"

    def __init__(self, client: BridgeClient, name: str) -> None:
        self.client = client
        self.variable = name
        self._attr_name = name
        self._attr_unique_id = f"global_{name}"
        self._attr_device_info = DeviceInfo(
            identifiers={(DOMAIN, "global_variables")},
            name="Puzzlebox variables",
            manufacturer="Puzzlebox",
            model="Global variables",
            entry_type=DeviceEntryType.SERVICE,
        )

    @property
    def available(self) -> bool:
        return self.client.connected and self.variable in self.client.variables

    @property
    def native_value(self) -> Any:
        value = self.client.variables.get(self.variable)
        if value is None or isinstance(value, bool):
            return None if value is None else str(value).lower()
        if isinstance(value, (int, float)):
            return value
        text = value if isinstance(value, str) else json.dumps(value)
        return text[:MAX_STATE_LENGTH]

    @property
    def extra_state_attributes(self) -> dict[str, Any] | None:
        # Lists and dictionaries keep their full structure here, for templates.
        value = self.client.variables.get(self.variable)
        return {"value": value} if isinstance(value, (list, dict)) else None

    async def async_added_to_hass(self) -> None:
        self.async_on_remove(async_dispatcher_connect(self.hass, SIGNAL_VARIABLES, self.async_write_ha_state))
        self.async_on_remove(async_dispatcher_connect(self.hass, SIGNAL_CONNECTION, self._connection_changed))

    @callback
    def _connection_changed(self, _connected: bool) -> None:
        self.async_write_ha_state()
