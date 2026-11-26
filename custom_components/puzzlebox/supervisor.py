"""Finds the Puzzlebox app through the Supervisor's discovery messages."""

from __future__ import annotations

import logging
from typing import Any

from homeassistant.core import HomeAssistant
from homeassistant.helpers.hassio import is_hassio

from .const import CONF_TOKEN, CONF_URL, DOMAIN

_LOGGER = logging.getLogger(__name__)


def data_from_discovery(config: dict[str, Any]) -> dict[str, str]:
    return {CONF_URL: f"http://{config['host']}:{config['port']}", CONF_TOKEN: config["token"]}


async def async_find_app(hass: HomeAssistant) -> dict[str, str] | None:
    """Address and token of the installed app, or None (not on Home Assistant OS, or not installed)."""
    if not is_hassio(hass):
        return None
    try:
        from homeassistant.components.hassio import get_supervisor_client

        messages = await get_supervisor_client(hass).discovery.list()
    except Exception as err:  # noqa: BLE001 - absent or unready Supervisor means "not found"
        _LOGGER.debug("Could not read Supervisor discovery: %s", err)
        return None
    for message in messages:
        if message.service == DOMAIN and {"host", "port", "token"} <= message.config.keys():
            return data_from_discovery(message.config)
    return None
