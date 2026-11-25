# Contributing

## Project layout

| Path | Contents |
| --- | --- |
| `crates/` | The app, in Rust: Home Assistant client, compiler, engine, block registry, storage, YAML import, server |
| `frontend/` | The editor (Svelte + Blockly) |
| `custom_components/puzzlebox` | The companion integration |
| `addon/puzzlebox` | App packaging |
| `sdk/`, `examples/` | Block pack schema, plugin kit and examples |

## Running it locally

You need Rust, Node.js 24 and a Home Assistant instance with a long-lived
access token.

```sh
(cd frontend && npm ci && npm run build)
HA_URL=http://homeassistant.local:8123 HA_TOKEN=... cargo run -p puzzlebox-server
# open http://localhost:8099
```

For hot reload of the editor, keep the server running and use `npm run dev`
in `frontend/` (it proxies `/api` to port 8099).

| Variable | Purpose |
| --- | --- |
| `HA_URL`, `HA_TOKEN` | Home Assistant to connect to when running outside the app |
| `PUZZLEBOX_BIND` | Listen address (default `0.0.0.0:8099`) |
| `PUZZLEBOX_DATA_DIR` | Database, bridge token, `packs/` and `plugins/` |
| `PUZZLEBOX_CONFIG_DIR`, `PUZZLEBOX_PACKS_DIR` | Extra folders for packs and plugins |
| `PUZZLEBOX_MQTT_URL` | `mqtt://user:pass@host:1883` (the app uses the Supervisor's broker instead) |

## Tests

```sh
cargo test
(cd frontend && npm run check && npm test)
uvx ruff check custom_components examples/python-plugin
# Tests against a real Home Assistant with the `demo` integration:
HA_WS_URL=ws://localhost:8123/api/websocket HA_TOKEN=... cargo test -p puzzlebox-ha-client --test live_ha
```

## Container image

The Dockerfile's build context is the repository root:

```sh
docker build -f addon/puzzlebox/Dockerfile -t puzzlebox .
```

Releases are built by pushing a `v*` tag; the release workflow publishes the
images to GHCR.
