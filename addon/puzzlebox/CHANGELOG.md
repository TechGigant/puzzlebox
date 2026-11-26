# Changelog

## 0.2.0

- **Setup is automatic.** The integration finds the app through the
  Supervisor and sets itself up without asking anything, also from "Add
  integration". After reinstalling the app it picks up the new address and
  token on its own.
- **Devices section**: browse areas and devices and drag in their device
  triggers, conditions and actions ("Button pressed twice", "Turn on with
  brightness") instead of typing IDs.
- **Zigbee2MQTT**: blocks for every device and group: action triggers with
  the device's own action list, "changes" triggers, typed set commands, last
  reported values, and group scenes. Works with the Mosquitto app or any
  broker (`mqtt_url`), and with several instances
  (`zigbee2mqtt_base_topics`).
- **Timers**: "start timer … then do", restart or keep it running, cancel,
  "is running". "Turn off 5 minutes after the last motion" is one block.
- **Waiting and limits**: "wait until … stays true for … give up after",
  "at most once every N seconds".
- **More conditions and values**: has been in a state for, has a value,
  number state with fallback, seconds since changed, any/all/none/how many
  in a state, average/min/max/median/sum, time between on weekdays,
  history from the recorder (average, min, max, state N ago, changes, time
  in state), entities in an area/floor/label/device, friendly name.
- **Scenes**: last activated scene of a list, activate the next/previous one.
- **Triggers**: "triggered by" (which trigger started the run), old/new
  state and attribute, "every N minutes".
- **Targets**: area, floor, label and device blocks, mixable with entities.
- **Time**: today at, add seconds, seconds between, durations, readable
  durations.
- Global variables appear in Home Assistant as sensors.
- Generated blocks: area/floor/label/device pickers, config entry dropdowns,
  multi-value fields, entity pickers filtered by device class, a YAML field
  for free-form values, and threshold inputs only when needed.

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
