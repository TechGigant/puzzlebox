# Puzzlebox

Build Home Assistant automations by snapping blocks together. Open **Puzzlebox**
from the sidebar after starting the app.

## What you get

- Every Home Assistant action, trigger and condition as a block, generated
  from your own installation (including integrations you add later).
- Variables, lists, dictionaries, loops, functions, try/catch, parallel
  branches, and stored variables that survive restarts.
- Your devices' own triggers, conditions and actions, grouped by area.
- Zigbee2MQTT devices and groups as blocks (with the Mosquitto app or any
  MQTT broker).
- Timers, "wait until", rate limits, recorder history and scene cycling.
- Schedules (cron), webhooks, HTTP requests and MQTT.
- Run history with step traces, a live log and a debugger with breakpoints.
- Import of existing automations and scripts.

## Companion integration

Install the **Puzzlebox** integration (HACS → custom repository, category
*Integration*) and restart Home Assistant. It finds this app on its own and
sets itself up; nothing to enter.

Each program then gets a switch to enable it and a button to run it, and
global variables show up as sensors. You can
start programs and set stored variables from automations with the
`puzzlebox.run_program` and `puzzlebox.set_variable` actions, and
react to runs with the `puzzlebox_program_started` and
`puzzlebox_program_finished` events. Webhook triggers are reachable on your
Home Assistant URL, and other integrations can add their own blocks.

Keep the app and the integration on the same version. If one is older, the
app shows a banner and Home Assistant lists it under **Settings → Repairs**;
everything both versions support keeps working until you update.

## Extending

Put block packs (`*.yaml`) in `/addon_configs/<this app>/packs` and WASM
plugins in `/addon_configs/<this app>/plugins`, then press *Reload* on the
Settings page. See the repository's `sdk/` folder for the formats.

## Options

| Option | Description |
| --- | --- |
| `log_level` | `trace`, `debug`, `info` (default), `warn` or `error`. |
| `mqtt_url` | Leave empty to use the Mosquitto broker app. For another broker: `mqtt://user:password@host:1883`. |
| `zigbee2mqtt_base_topics` | Base topic of each Zigbee2MQTT instance (default `zigbee2mqtt`). |
