"""Per-program entities that follow the app's program list."""

from __future__ import annotations

from collections.abc import Callable
from typing import Any

from homeassistant.config_entries import ConfigEntry
from homeassistant.core import HomeAssistant, callback
from homeassistant.helpers import entity_registry as er
from homeassistant.helpers.device_registry import DeviceEntryType, DeviceInfo
from homeassistant.helpers.dispatcher import async_dispatcher_connect
from homeassistant.helpers.entity import Entity
from homeassistant.helpers.entity_platform import AddEntitiesCallback

from .bridge import BridgeClient
from .const import DOMAIN, SIGNAL_CONNECTION, SIGNAL_PROGRAMS


class ProgramEntity(Entity):
    _attr_has_entity_name = True
    _attr_should_poll = False

    def __init__(self, client: BridgeClient, entry: ConfigEntry, program_id: str, key: str) -> None:
        self.client = client
        self.program_id = program_id
        self._attr_unique_id = f"{program_id}_{key}"
        self._attr_device_info = DeviceInfo(
            identifiers={(DOMAIN, program_id)},
            name=self.program.get("name", program_id),
            manufacturer="Puzzlebox",
            model="Program",
            entry_type=DeviceEntryType.SERVICE,
        )

    @property
    def program(self) -> dict[str, Any]:
        return self.client.programs.get(self.program_id, {})

    @property
    def available(self) -> bool:
        return self.client.connected and self.program_id in self.client.programs

    async def async_added_to_hass(self) -> None:
        self.async_on_remove(async_dispatcher_connect(self.hass, SIGNAL_PROGRAMS, self.async_write_ha_state))
        self.async_on_remove(
            async_dispatcher_connect(self.hass, SIGNAL_CONNECTION, lambda _: self.async_write_ha_state())
        )


def setup_program_platform(
    hass: HomeAssistant,
    entry: ConfigEntry,
    async_add_entities: AddEntitiesCallback,
    domain: str,
    key: str,
    factory: Callable[[BridgeClient, ConfigEntry, str], ProgramEntity],
) -> None:
    """Adds entities for new programs and removes those of deleted programs."""
    client: BridgeClient = hass.data[DOMAIN]
    known: set[str] = set()

    @callback
    def sync() -> None:
        current = set(client.programs)
        new = current - known
        if new:
            async_add_entities(factory(client, entry, pid) for pid in new)
            known.update(new)
        ent_reg = er.async_get(hass)
        for pid in known - current:
            if entity_id := ent_reg.async_get_entity_id(domain, DOMAIN, f"{pid}_{key}"):
                ent_reg.async_remove(entity_id)
            known.discard(pid)

    sync()
    entry.async_on_unload(async_dispatcher_connect(hass, SIGNAL_PROGRAMS, sync))
