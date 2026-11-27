"""WebSocket link to the Puzzlebox app."""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable
import contextlib
import inspect
import json
import logging
from typing import Any

from aiohttp import ClientWSTimeout, WSMsgType, web

from homeassistant.components import webhook
from homeassistant.core import HomeAssistant, callback
from homeassistant.helpers import issue_registry as ir
from homeassistant.helpers.aiohttp_client import async_get_clientsession
from homeassistant.helpers.dispatcher import async_dispatcher_send

from .compat import AppInfo, hello
from .const import (
    DOMAIN,
    EVENT_RUN_FINISHED,
    EVENT_RUN_STARTED,
    SIGNAL_CONNECTION,
    SIGNAL_PROGRAMS,
    SIGNAL_VARIABLES,
)

_LOGGER = logging.getLogger(__name__)

# Failed runs in a row before Repairs says a program keeps failing.
FAILING_RUNS = 3
PROGRAM_ISSUES = ("program_invalid", "program_failing")

Handler = Callable[[HomeAssistant, dict[str, Any], dict[str, Any]], Any | Awaitable[Any]]


class BridgeClient:
    """Keeps a connection to the app and mirrors its state into Home Assistant."""

    def __init__(self, hass: HomeAssistant, url: str, token: str, version: str = "0.0.0") -> None:
        self.hass = hass
        self.version = version
        self.app: AppInfo | None = None
        self.url = url.rstrip("/")
        self.token = token
        self.programs: dict[str, dict[str, Any]] = {}
        self.variables: dict[str, Any] = {}
        self.connected = False
        self._packs: dict[str, tuple[dict[str, Any], dict[str, Handler]]] = {}
        self._ws = None
        self._task: asyncio.Task | None = None
        self._webhooks: set[str] = set()
        self._wake = asyncio.Event()

    # -- lifecycle -----------------------------------------------------------

    async def async_start(self) -> None:
        self._task = self.hass.async_create_background_task(self._run(), f"{DOMAIN}_bridge")

    async def async_stop(self) -> None:
        self._update_issues(None)
        self._clear_program_issues(set())
        if self._task:
            self._task.cancel()
        for wid in list(self._webhooks):
            webhook.async_unregister(self.hass, wid)
        self._webhooks.clear()
        if self._ws is not None:
            await self._ws.close()

    async def _run(self) -> None:
        delay = 1
        while True:
            try:
                await self._session()
                delay = 1
            except asyncio.CancelledError:
                raise
            except Exception as err:  # noqa: BLE001 - reconnect on anything
                _LOGGER.debug("Puzzlebox app connection failed: %s", err)
            self._set_connected(False)
            self._wake.clear()
            with contextlib.suppress(TimeoutError):
                await asyncio.wait_for(self._wake.wait(), delay)
            delay = min(delay * 2, 30)

    async def _session(self) -> None:
        session = async_get_clientsession(self.hass)
        async with session.ws_connect(
            f"{self.url}/api/bridge",
            headers={"Authorization": f"Bearer {self.token}"},
            heartbeat=30,
            timeout=ClientWSTimeout(ws_close=10),
        ) as ws:
            self._ws = ws
            # Our hello goes first; everything else waits for the app's hello (see compat.py).
            await ws.send_json(hello(self.version))
            async for msg in ws:
                if msg.type == WSMsgType.TEXT:
                    self._handle(msg.json())
                elif msg.type in (WSMsgType.CLOSED, WSMsgType.ERROR):
                    break
        self._ws = None

    @callback
    def _on_hello(self, msg: dict[str, Any]) -> None:
        app = AppInfo.from_hello(msg)
        self.app = app
        self._update_issues(app)
        if not app.compatible():
            _LOGGER.error(
                "Puzzlebox app %s and integration %s can't work together; update the %s",
                app.version,
                self.version,
                app.outdated(self.version) or "older one",
            )
            return
        _LOGGER.info("Connected to the Puzzlebox app %s at %s", app.version, self.url)
        self._set_connected(True)
        if app.has("packs"):
            for pack, _ in self._packs.values():
                self.hass.async_create_task(self._send({"type": "register_pack", "pack": pack}))

    @callback
    def _update_issues(self, app: AppInfo | None) -> None:
        """Repairs entries asking to update whichever side is older."""
        outdated = app.outdated(self.version) if app else None
        for side in ("app", "integration"):
            issue_id = f"update_{side}"
            if app is None or outdated != side:
                ir.async_delete_issue(self.hass, DOMAIN, issue_id)
                continue
            required = not app.compatible()
            ir.async_create_issue(
                self.hass,
                DOMAIN,
                issue_id,
                is_fixable=False,
                severity=ir.IssueSeverity.ERROR if required else ir.IssueSeverity.WARNING,
                translation_key=f"{issue_id}_required" if required else issue_id,
                translation_placeholders={
                    "app_version": app.version or "unknown",
                    "integration_version": self.version,
                },
            )

    @callback
    def _update_program_issues(self) -> None:
        """Repairs entries for programs that can't run as built or keep failing."""
        wanted: set[str] = set()
        for pid, program in self.programs.items():
            if not program.get("enabled"):
                continue
            status = program.get("status") or {}
            name = program.get("name") or pid
            problems = [e.get("message", "") for e in status.get("errors") or []]
            if status.get("state") == "error" and not problems and status.get("last_error"):
                problems = [status["last_error"]]
            if problems:
                wanted.add(f"program_invalid_{pid}")
                ir.async_create_issue(
                    self.hass,
                    DOMAIN,
                    f"program_invalid_{pid}",
                    is_fixable=False,
                    severity=ir.IssueSeverity.ERROR,
                    translation_key="program_invalid",
                    translation_placeholders={"name": name, "problems": "; ".join(problems)},
                )
            failures = program.get("failures") or 0
            if failures >= FAILING_RUNS:
                wanted.add(f"program_failing_{pid}")
                ir.async_create_issue(
                    self.hass,
                    DOMAIN,
                    f"program_failing_{pid}",
                    is_fixable=False,
                    severity=ir.IssueSeverity.WARNING,
                    translation_key="program_failing",
                    translation_placeholders={
                        "name": name,
                        "count": str(failures),
                        "error": status.get("last_error") or "unknown error",
                    },
                )
        self._clear_program_issues(wanted)

    @callback
    def _clear_program_issues(self, keep: set[str]) -> None:
        reg = ir.async_get(self.hass)
        for domain, issue_id in list(reg.issues):
            if domain == DOMAIN and issue_id.startswith(PROGRAM_ISSUES) and issue_id not in keep:
                ir.async_delete_issue(self.hass, DOMAIN, issue_id)

    @callback
    def _set_connected(self, value: bool) -> None:
        if self.connected != value:
            self.connected = value
            async_dispatcher_send(self.hass, SIGNAL_CONNECTION, value)

    @callback
    def nudge(self) -> None:
        """Retry the connection now instead of after the backoff."""
        self._wake.set()

    async def _send(self, msg: dict[str, Any]) -> bool:
        if self._ws is None or self._ws.closed:
            return False
        await self._ws.send_json(msg)
        return True

    # -- incoming ------------------------------------------------------------

    @callback
    def _handle(self, msg: dict[str, Any]) -> None:
        kind = msg.get("type")
        if kind == "hello":
            self._on_hello(msg)
            return
        if not self.connected:
            # Before a compatible hello, nothing else is acted on.
            return
        if kind == "programs":
            self.programs = {p["id"]: p for p in msg.get("programs", [])}
            self._update_program_issues()
            async_dispatcher_send(self.hass, SIGNAL_PROGRAMS)
        elif kind == "variables":
            self.variables = dict(msg.get("variables") or {})
            async_dispatcher_send(self.hass, SIGNAL_VARIABLES)
        elif kind == "variable":
            self.variables[msg["name"]] = msg.get("value")
            async_dispatcher_send(self.hass, SIGNAL_VARIABLES)
        elif kind == "run_started":
            self._fire(EVENT_RUN_STARTED, msg, ("program_id", "run_id"))
        elif kind == "run_finished":
            self._fire(EVENT_RUN_FINISHED, msg, ("program_id", "run_id", "status", "error"))
        elif kind == "invoke":
            self.hass.async_create_task(self._invoke(msg))
        elif kind == "webhooks":
            self._sync_webhooks(set(msg.get("webhooks", [])))
        elif kind == "pack_rejected":
            _LOGGER.error("Puzzlebox rejected block pack %s: %s", msg.get("pack"), msg.get("error"))

    @callback
    def _fire(self, event: str, msg: dict[str, Any], keys: tuple[str, ...]) -> None:
        data = {k: msg.get(k) for k in keys}
        program = self.programs.get(msg.get("program_id", ""))
        data["name"] = program["name"] if program else None
        self.hass.bus.async_fire(event, data)

    async def _invoke(self, msg: dict[str, Any]) -> None:
        reply: dict[str, Any] = {"id": msg["id"]}
        try:
            pack = self._packs.get(msg["plugin"])
            if pack is None:
                raise LookupError(f"no plugin '{msg['plugin']}' is registered")
            handler = pack[1].get(msg["function"])
            if handler is None:
                raise LookupError(f"plugin '{msg['plugin']}' has no function '{msg['function']}'")
            result = handler(self.hass, msg.get("args") or {}, msg.get("context") or {})
            if inspect.isawaitable(result):
                result = await result
            reply.update(type="result", result=result)
        except Exception as err:
            _LOGGER.debug("Puzzlebox plugin call failed", exc_info=True)
            reply.update(type="error", message=str(err))
        await self._send(reply)

    @callback
    def _sync_webhooks(self, wanted: set[str]) -> None:
        for wid in self._webhooks - wanted:
            webhook.async_unregister(self.hass, wid)
            self._webhooks.discard(wid)
        for wid in wanted - self._webhooks:
            try:
                webhook.async_register(
                    self.hass, DOMAIN, f"Puzzlebox: {wid}", wid, self._handle_webhook, local_only=False
                )
                self._webhooks.add(wid)
            except ValueError:
                _LOGGER.warning("Webhook id %s is already used by another integration", wid)

    async def _handle_webhook(
        self, hass: HomeAssistant, webhook_id: str, request: web.Request
    ) -> web.Response:
        body = await request.text()
        payload = {
            "type": "webhook",
            "webhook_id": webhook_id,
            "method": request.method,
            "request": {
                "query": dict(request.query),
                "headers": {
                    k: v for k, v in request.headers.items() if k.lower() not in ("authorization", "cookie")
                },
                "body": body,
                "json": _try_json(body),
            },
        }
        ok = await self._send(payload)
        return web.Response(status=200 if ok else 503)

    # -- outgoing ------------------------------------------------------------

    def resolve(self, program: str) -> str | None:
        """Accepts a program id or (case-insensitive) name."""
        if program in self.programs:
            return program
        for pid, p in self.programs.items():
            if p["name"].lower() == program.lower():
                return pid
        return None

    async def async_run(self, program_id: str, variables: dict[str, Any] | None = None) -> None:
        await self._send({"type": "run", "program_id": program_id, "variables": variables or {}})

    async def async_set_enabled(self, program_id: str, enabled: bool) -> None:
        await self._send({"type": "set_enabled", "program_id": program_id, "enabled": enabled})

    async def async_set_variable(self, name: str, value: Any, program_id: str | None = None) -> None:
        await self._send({"type": "set_variable", "name": name, "value": value, "program_id": program_id})

    # -- plugin API ------------------------------------------------------------

    @callback
    def register_pack(self, pack: dict[str, Any], handlers: dict[str, Handler]) -> Callable[[], None]:
        pack_id = pack["pack"]
        self._packs[pack_id] = (pack, handlers)
        if self.connected and self.app and self.app.has("packs"):
            self.hass.async_create_task(self._send({"type": "register_pack", "pack": pack}))

        @callback
        def unregister() -> None:
            if self._packs.pop(pack_id, None) is not None and self.connected:
                self.hass.async_create_task(self._send({"type": "unregister_pack", "pack": pack_id}))

        return unregister


def _try_json(text: str) -> Any:
    try:
        return json.loads(text)
    except ValueError:
        return None
