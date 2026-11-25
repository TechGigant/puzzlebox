"""Config flow: Supervisor discovery or manual URL + token."""

from __future__ import annotations

from typing import Any

import aiohttp
import voluptuous as vol

from homeassistant.config_entries import ConfigFlow, ConfigFlowResult
from homeassistant.helpers.aiohttp_client import async_get_clientsession
from homeassistant.helpers.service_info.hassio import HassioServiceInfo

from .const import CONF_TOKEN, CONF_URL, DOMAIN


async def _validate(hass, url: str, token: str) -> str | None:
    """Returns an error key, or None if the app accepted the token."""
    session = async_get_clientsession(hass)
    try:
        async with session.ws_connect(
            f"{url.rstrip('/')}/api/bridge", headers={"Authorization": f"Bearer {token}"}
        ) as ws:
            await ws.close()
    except aiohttp.WSServerHandshakeError as err:
        return "invalid_auth" if err.status == 401 else "cannot_connect"
    except (aiohttp.ClientError, TimeoutError):
        return "cannot_connect"
    return None


class PuzzleboxConfigFlow(ConfigFlow, domain=DOMAIN):
    VERSION = 1

    def __init__(self) -> None:
        self._discovered: dict[str, str] = {}

    async def async_step_user(self, user_input: dict[str, Any] | None = None) -> ConfigFlowResult:
        errors: dict[str, str] = {}
        if user_input is not None:
            error = await _validate(self.hass, user_input[CONF_URL], user_input[CONF_TOKEN])
            if error is None:
                await self.async_set_unique_id(DOMAIN)
                self._abort_if_unique_id_configured(updates=user_input)
                return self.async_create_entry(title="Puzzlebox", data=user_input)
            errors["base"] = error
        schema = vol.Schema(
            {
                vol.Required(CONF_URL, default=(user_input or {}).get(CONF_URL, "http://")): str,
                vol.Required(CONF_TOKEN): str,
            }
        )
        return self.async_show_form(step_id="user", data_schema=schema, errors=errors)

    async def async_step_hassio(self, discovery_info: HassioServiceInfo) -> ConfigFlowResult:
        cfg = discovery_info.config
        data = {CONF_URL: f"http://{cfg['host']}:{cfg['port']}", CONF_TOKEN: cfg["token"]}
        await self.async_set_unique_id(DOMAIN)
        # Reinstalling the app rotates nothing, but a new host name must reach the entry.
        self._abort_if_unique_id_configured(updates=data)
        self._discovered = data
        return await self.async_step_hassio_confirm()

    async def async_step_hassio_confirm(self, user_input: dict[str, Any] | None = None) -> ConfigFlowResult:
        if user_input is not None:
            return self.async_create_entry(title="Puzzlebox", data=self._discovered)
        return self.async_show_form(step_id="hassio_confirm")
