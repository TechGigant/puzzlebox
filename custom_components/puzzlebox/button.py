"""One button per program: press = run now."""

from __future__ import annotations

from homeassistant.components.button import ButtonEntity
from homeassistant.config_entries import ConfigEntry
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddEntitiesCallback

from .entity import ProgramEntity, setup_program_platform


async def async_setup_entry(
    hass: HomeAssistant, entry: ConfigEntry, async_add_entities: AddEntitiesCallback
) -> None:
    setup_program_platform(hass, entry, async_add_entities, "button", "run", ProgramRunButton)


class ProgramRunButton(ProgramEntity, ButtonEntity):
    _attr_translation_key = "run"
    _attr_name = "Run"
    _attr_icon = "mdi:play"

    def __init__(self, client, entry, program_id) -> None:
        super().__init__(client, entry, program_id, "run")

    async def async_press(self) -> None:
        await self.client.async_run(self.program_id)
