"""Config flow: Supervisor discovery or manual URL + token."""

from __future__ import annotations

from typing import Any

import aiohttp
import voluptuous as vol

from homeassistant.config_entries import ConfigFlow, ConfigFlowResult
from homeassistant.core import HomeAssistant
from homeassistant.helpers.aiohttp_client import async_get_clientsession
from homeassistant.helpers.hassio import is_hassio
from homeassistant.helpers.service_info.hassio import HassioServiceInfo

from .const import CONF_TOKEN, CONF_URL, DOMAIN
from .supervisor import async_find_app, data_from_discovery

APP_PORT = 8099


def _guess_app_url(hass: HomeAssistant) -> str:
    """URL of the installed Puzzlebox app, if Home Assistant runs under the Supervisor."""
    if not is_hassio(hass):
        return "http://"
    try:
        from homeassistant.components.hassio import get_addons_list

        for addon in get_addons_list(hass):
            slug = str(addon.get("slug", ""))
            if slug == "puzzlebox" or slug.endswith("_puzzlebox"):
                # The Supervisor derives app host names from the slug.
                host = addon.get("hostname") or slug.replace("_", "-")
                return f"http://{host}:{APP_PORT}"
    except Exception:  # noqa: BLE001 - only a convenience prefill
        pass
    return "http://"


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

    async def async_step_user(self, user_input: dict[str, Any] | None = None) -> ConfigFlowResult:
        errors: dict[str, str] = {}
        if user_input is None and (found := await async_find_app(self.hass)):
            # On Home Assistant OS the app announces its address and token: nothing to ask.
            return await self._async_create(found)
        if user_input is not None:
            error = await _validate(self.hass, user_input[CONF_URL], user_input[CONF_TOKEN])
            if error is None:
                return await self._async_create(user_input)
            errors["base"] = error
        schema = vol.Schema(
            {
                vol.Required(
                    CONF_URL, default=(user_input or {}).get(CONF_URL) or _guess_app_url(self.hass)
                ): str,
                vol.Required(CONF_TOKEN): str,
            }
        )
        return self.async_show_form(step_id="user", data_schema=schema, errors=errors)

    async def async_step_hassio(self, discovery_info: HassioServiceInfo) -> ConfigFlowResult:
        return await self._async_create(data_from_discovery(discovery_info.config))

    async def _async_create(self, data: dict[str, str]) -> ConfigFlowResult:
        await self.async_set_unique_id(DOMAIN)
        # A reinstalled app has a new token or host name; keep the entry, update its data.
        self._abort_if_unique_id_configured(updates=data)
        return self.async_create_entry(title="Puzzlebox", data=data)
