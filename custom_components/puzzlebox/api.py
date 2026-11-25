"""Public API for integrations that want to add blocks to Puzzlebox.

Example (in your integration's ``async_setup_entry``)::

    from homeassistant.loader import async_get_integration

    try:
        from custom_components.puzzlebox.api import async_register_blocks
    except ImportError:
        async_register_blocks = None

    if async_register_blocks:
        entry.async_on_unload(
            async_register_blocks(hass, PACK, {"say_hello": say_hello})
        )

``pack`` follows the block-pack schema (``sdk/block-pack-schema``). Blocks whose
lowering is ``{"type": "plugin", "plugin": <pack id>, "function": <name>}`` call
``handlers[name](hass, args, context)``; handlers may be sync or async and return
any JSON-serializable value. Registration works before or after the bridge is set up.
"""

from __future__ import annotations

from collections.abc import Callable
from typing import Any

from homeassistant.core import HomeAssistant, callback

from .bridge import Handler
from .const import DOMAIN, PENDING_PACKS

API_VERSION = 1


@callback
def async_register_blocks(
    hass: HomeAssistant, pack: dict[str, Any], handlers: dict[str, Handler] | None = None
) -> Callable[[], None]:
    """Registers a block pack; returns a callback that unregisters it."""
    if "pack" not in pack or "blocks" not in pack:
        raise ValueError("pack needs 'pack' (id) and 'blocks'")
    handlers = handlers or {}
    pending: dict[str, tuple[dict[str, Any], dict[str, Handler]]] = hass.data.setdefault(PENDING_PACKS, {})
    pending[pack["pack"]] = (pack, handlers)

    client = hass.data.get(DOMAIN)
    unregister_live = client.register_pack(pack, handlers) if client else None

    @callback
    def unregister() -> None:
        pending.pop(pack["pack"], None)
        if unregister_live:
            unregister_live()
        elif (c := hass.data.get(DOMAIN)) is not None:
            c.register_pack(pack, handlers)()

    return unregister
