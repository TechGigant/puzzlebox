# Changelog

## 0.1.1

- Trigger and condition blocks with thresholds (for example "Light brightness
  crossed threshold") get a type dropdown and number inputs instead of an
  empty slot.
- "Event received" and similar blocks suggest values from the chosen entity,
  such as an event entity's event types.
- The "if" block can get an "else" branch: right-click it, or use the new
  "if … else" block in Flow.
- The app keeps announcing itself to Home Assistant until the companion
  integration connects, so the integration is found even when it's installed
  later. Manual setup pre-fills the app's address.
- Schedules and time blocks use your time zone instead of UTC.
- Programs restart when an update changes the blocks they use.
- "Last run" survives an app restart; run traces show block names.
- Fix: images are published with the version tag the Supervisor expects.

## 0.1.0

- First release: visual editor, Rust engine, HA block generation, debugger,
  YAML import, block packs, Python and WASM plugins, MQTT, webhooks, HTTP.
