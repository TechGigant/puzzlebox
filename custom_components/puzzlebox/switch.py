"""One switch per program: on = enabled."""

from __future__ import annotations

from datetime import UTC, datetime
from typing import Any

from homeassistant.components.switch import SwitchEntity
from homeassistant.config_entries import ConfigEntry
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddEntitiesCallback

from .entity import ProgramEntity, setup_program_platform


async def async_setup_entry(
    hass: HomeAssistant, entry: ConfigEntry, async_add_entities: AddEntitiesCallback
) -> None:
    setup_program_platform(hass, entry, async_add_entities, "switch", "enabled", ProgramSwitch)


class ProgramSwitch(ProgramEntity, SwitchEntity):
    _attr_name = None
    _attr_icon = "mdi:puzzle"

    def __init__(self, client, entry, program_id) -> None:
        super().__init__(client, entry, program_id, "enabled")

    @property
    def is_on(self) -> bool:
        return bool(self.program.get("enabled"))

    @property
    def extra_state_attributes(self) -> dict[str, Any]:
        status = self.program.get("status") or {}
        last_run = status.get("last_run")
        return {
            "program_id": self.program_id,
            "mode": self.program.get("mode"),
            "state": status.get("state"),
            "active_runs": status.get("active_runs", 0),
            "last_run": datetime.fromtimestamp(last_run / 1000, UTC).isoformat() if last_run else None,
            "last_error": status.get("last_error"),
        }

    async def async_turn_on(self, **kwargs: Any) -> None:
        await self.client.async_set_enabled(self.program_id, True)

    async def async_turn_off(self, **kwargs: Any) -> None:
        await self.client.async_set_enabled(self.program_id, False)
