"""Example: an integration contributing its own Puzzlebox blocks.

Enable with `puzzlebox_example:` in configuration.yaml.
"""

from __future__ import annotations

from datetime import datetime
import random
from typing import Any

from homeassistant.core import HomeAssistant
from homeassistant.helpers.typing import ConfigType

DOMAIN = "puzzlebox_example"

PACK = {
    "pack": DOMAIN,
    "name": "Example blocks",
    "version": "0.1.0",
    "blocks": [
        {
            "id": "pick_greeting",
            "kind": "value",
            "title": "random greeting for",
            "params": [
                {"name": "name", "label": "name", "required": True, "input": "text", "default": "you"}
            ],
            "lowering": {"type": "plugin", "plugin": DOMAIN, "function": "pick_greeting"},
        },
        {
            "id": "notify",
            "kind": "action",
            "title": "Show notification",
            "tooltip": "Creates a persistent notification in Home Assistant.",
            "params": [
                {"name": "message", "label": "message", "required": True, "input": "text"},
                {"name": "title", "label": "title", "input": "text"},
            ],
            "lowering": {"type": "plugin", "plugin": DOMAIN, "function": "notify"},
        },
        {
            "id": "is_weekend",
            "kind": "condition",
            "title": "it is the weekend",
            "lowering": {"type": "plugin", "plugin": DOMAIN, "function": "is_weekend"},
        },
    ],
}


def pick_greeting(hass: HomeAssistant, args: dict[str, Any], context: dict[str, Any]) -> str:
    return f"{random.choice(['Hello', 'Hi', 'Hey'])}, {args.get('name', 'you')}!"


async def notify(hass: HomeAssistant, args: dict[str, Any], context: dict[str, Any]) -> None:
    data = {"message": str(args["message"])}
    if args.get("title"):
        data["title"] = str(args["title"])
    await hass.services.async_call("persistent_notification", "create", data, blocking=True)


def is_weekend(hass: HomeAssistant, args: dict[str, Any], context: dict[str, Any]) -> bool:
    return datetime.now().weekday() >= 5


async def async_setup(hass: HomeAssistant, config: ConfigType) -> bool:
    try:
        from custom_components.puzzlebox.api import async_register_blocks
    except ImportError:
        return True
    async_register_blocks(
        hass, PACK, {"pick_greeting": pick_greeting, "notify": notify, "is_weekend": is_weekend}
    )
    return True
