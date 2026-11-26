"""Automatic setup: the integration finds the app without asking for anything."""

from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock, patch

from pytest_homeassistant_custom_component.common import MockConfigEntry

from custom_components.puzzlebox.const import CONF_TOKEN, CONF_URL, DOMAIN
from custom_components.puzzlebox.supervisor import async_find_app
from homeassistant import config_entries
from homeassistant.core import HomeAssistant
from homeassistant.data_entry_flow import FlowResultType
from homeassistant.helpers.service_info.hassio import HassioServiceInfo

DISCOVERY = {"host": "abc123-puzzlebox", "port": 8099, "token": "secret"}
DATA = {CONF_URL: "http://abc123-puzzlebox:8099", CONF_TOKEN: "secret"}


def _no_setup():
    return patch("custom_components.puzzlebox.async_setup_entry", return_value=True)


async def test_hassio_discovery_creates_entry_without_confirmation(hass: HomeAssistant) -> None:
    info = HassioServiceInfo(config=DISCOVERY, name="Puzzlebox", slug="abc123_puzzlebox", uuid="u1")
    with _no_setup():
        result = await hass.config_entries.flow.async_init(
            DOMAIN, context={"source": config_entries.SOURCE_HASSIO}, data=info
        )
    assert result["type"] is FlowResultType.CREATE_ENTRY
    assert result["data"] == DATA


async def test_user_step_uses_supervisor_discovery(hass: HomeAssistant) -> None:
    with _no_setup(), patch("custom_components.puzzlebox.config_flow.async_find_app", return_value=DATA):
        result = await hass.config_entries.flow.async_init(
            DOMAIN, context={"source": config_entries.SOURCE_USER}
        )
    assert result["type"] is FlowResultType.CREATE_ENTRY
    assert result["data"] == DATA


async def test_user_step_falls_back_to_form(hass: HomeAssistant) -> None:
    with patch("custom_components.puzzlebox.config_flow.async_find_app", return_value=None):
        result = await hass.config_entries.flow.async_init(
            DOMAIN, context={"source": config_entries.SOURCE_USER}
        )
    assert result["type"] is FlowResultType.FORM
    assert result["step_id"] == "user"


async def test_reinstalled_app_updates_existing_entry(hass: HomeAssistant) -> None:
    entry = MockConfigEntry(
        domain=DOMAIN, unique_id=DOMAIN, data={CONF_URL: "http://old:8099", CONF_TOKEN: "old"}
    )
    entry.add_to_hass(hass)
    info = HassioServiceInfo(config=DISCOVERY, name="Puzzlebox", slug="abc123_puzzlebox", uuid="u2")
    with _no_setup():
        result = await hass.config_entries.flow.async_init(
            DOMAIN, context={"source": config_entries.SOURCE_HASSIO}, data=info
        )
    assert result["type"] is FlowResultType.ABORT
    assert result["reason"] == "already_configured"
    assert entry.data == DATA


async def test_find_app_reads_supervisor_discovery(hass: HomeAssistant) -> None:
    client = MagicMock()
    client.discovery.list = AsyncMock(
        return_value=[
            SimpleNamespace(service="mqtt", config={"host": "core-mosquitto"}),
            SimpleNamespace(service=DOMAIN, config=DISCOVERY),
        ]
    )
    with (
        patch("custom_components.puzzlebox.supervisor.is_hassio", return_value=True),
        patch("homeassistant.components.hassio.get_supervisor_client", return_value=client),
    ):
        assert await async_find_app(hass) == DATA


async def test_find_app_outside_supervisor(hass: HomeAssistant) -> None:
    assert await async_find_app(hass) is None


async def test_user_step_with_existing_entry_aborts(hass: HomeAssistant) -> None:
    MockConfigEntry(domain=DOMAIN, unique_id=DOMAIN, data=DATA).add_to_hass(hass)
    with patch("custom_components.puzzlebox.config_flow.async_find_app", return_value=None):
        result = await hass.config_entries.flow.async_init(
            DOMAIN, context={"source": config_entries.SOURCE_USER}
        )
    assert result["type"] is FlowResultType.ABORT
    assert result["reason"] == "already_configured"
