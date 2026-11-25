# Puzzlebox

Build Home Assistant automations by snapping blocks together. Open **Puzzlebox**
from the sidebar after starting the app.

## What you get

- Every Home Assistant action, trigger and condition as a block, generated
  from your own installation (including integrations you add later).
- Variables, lists, dictionaries, loops, functions, try/catch, parallel
  branches, and stored variables that survive restarts.
- Schedules (cron), webhooks, HTTP requests and MQTT.
- Run history with step traces, a live log and a debugger with breakpoints.
- Import of existing automations and scripts.

## Companion integration

Install the **Puzzlebox** integration (HACS → custom repository, category
*Integration*). Home Assistant finds this app on its own and offers to set
it up.

Each program then gets a switch to enable it and a button to run it. You can
start programs and set stored variables from automations with the
`puzzlebox.run_program` and `puzzlebox.set_variable` actions, and
react to runs with the `puzzlebox_program_started` and
`puzzlebox_program_finished` events. Webhook triggers are reachable on your
Home Assistant URL, and other integrations can add their own blocks.

## Extending

Put block packs (`*.yaml`) in `/addon_configs/<this app>/packs` and WASM
plugins in `/addon_configs/<this app>/plugins`, then press *Reload* on the
Settings page. See the repository's `sdk/` folder for the formats.

## Options

| Option | Description |
| --- | --- |
| `log_level` | `trace`, `debug`, `info` (default), `warn` or `error`. |
