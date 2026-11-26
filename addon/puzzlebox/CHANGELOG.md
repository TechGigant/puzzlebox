# Changelog

## 0.4.0

- **Add a block by typing**: press Ctrl+Space (or middle-click the canvas),
  type what you want ("turn off light", "wait until") and press Enter. The
  block lands after the selected block, or where you clicked.
- **Switch a block** from its right-click menu to a related one: turn on ↔
  turn off ↔ toggle, state ↔ number state, entity ↔ area, a device's other
  triggers or actions, and so on. Inner blocks and the stack stay in place.
- **Find uses** of a variable, timer, entity, function or area (repeat to
  step through them), and **rename** a variable, timer or entity everywhere
  in the program.
- **Run this stack** from the right-click menu. Loose stacks of blocks
  outside any "when" block run only this way, so you can keep test snippets
  next to a program.
- **Backpack** (top right): drop blocks in to reuse them in other programs.
- Nested blocks of the same colour alternate between two shades, and there's
  a compact block size for big programs (Settings → Editor).
- **Program folders**: group programs into folders (also nested) by dragging
  them or with "Move"; search covers names, folders and descriptions.
- **Favourites** at the top of the toolbox: pinned blocks (right-click → Add
  to favourites) and your recently used ones.
- **History**: every save that changes the blocks is kept (the last 30) and
  can be restored from the editor.
- **New blocks**: switch / case / otherwise, section labels to organise long
  stacks, "items of a list where …" and "each item as …" with an "item"
  block, regular expressions, text before/after, "is between", and "or if
  empty" for unavailable values.
- Alt+drag leaves a copy behind, Ctrl+drag pulls out a single block, and
  Ctrl+V pastes at the pointer.
- Collapsed blocks show a summary ("when motion turns on → 6 blocks").
- Long dropdowns get a filter box; stacks outside any "when" block are
  drawn faded; the editor shows the program's block count.
- **Lights & colours**: one-block light actions (set to a colour or a white
  temperature with brightness and fade, set brightness, brighten/dim by a
  step, on/off/toggle with fade), the current colour and brightness of a
  light, and colour blocks (picker, RGB, hue, mix, random, parts of a
  colour). Colours can also be "#rrggbb" text or colour names.
- Credits for TurboWarp, Scratch Addons and PenguinMod, whose editors many of
  these features are modelled on (Settings → About and the README).
- **Home** category: one-block actions for switches, covers, thermostats,
  media players (incl. "say … on speakers"), fans, locks, vacuums, scenes,
  scripts, counters, helpers and phone notifications, plus "turn on … for 5
  minutes then off", and readings like a cover's position or a thermostat's
  temperature.
- **Easier blocks**, after a usability review:
  - Scratch-like toolbox: Events, Control, Sensing first; technical blocks
    (YAML, templates, cron, HTTP, dictionaries…) moved to "Advanced".
  - "When" blocks have a hat and triggers a ⚡, so it's clear what goes where.
  - Blocks that read an entity take any block in their entity socket (a
    variable, the trigger entity, a list item…), not only a typed-in id.
  - Durations are "5 minutes" blocks everywhere, also in triggers; units
    come after the number; times are tidied up ("7" → "07:00").
  - Entity pickers show names and start empty ("choose…"); state boxes
    suggest the entity's states; attributes are suggested too. Blocks warn
    about entities that don't exist.
  - New: "… becomes …" and "arrives at/leaves zone" triggers, "the sun is
    up", "someone is home", "today is a weekday", "wait until it is 07:00",
    "the wait timed out", "trigger entity", "how many of … are on", "state
    of … 1 hour ago", "text matches", "hex code of colour".
  - Areas, floors, labels and devices work in "any/all of", averages, scene
    blocks and loops.
  - A stack with a problem no longer stops the whole program: the rest runs.
    Problems name the block and its slot in plain words.
  - Colour settings of Home Assistant and Zigbee2MQTT blocks take colour
    blocks; Zigbee2MQTT group brightness and white are in % and K (re-enter
    those two values in existing group blocks).
- Fixes: "−" in the math block, trim left/right, list insert/remove and sort
  types, sensor states compared as text, "seconds since reported", trigger
  details inside timers, state triggers no longer fire on attribute changes,
  "every N minutes" values that don't divide an hour are reported.

## 0.3.0

- **Version check between the app and the integration.** On connecting, both
  exchange their version and the features they support, and only use what
  both have, so a mixed setup keeps working. The older side is named in a
  banner in the app and in Home Assistant's Repairs, with how to update it.
  If they can't work together at all, programs don't appear in Home
  Assistant until you update.
- Devices section: each device has its own category under its area, with
  its triggers, conditions and actions listed separately. Devices without
  entities are left out.
- Fix: the integration updated entities from a worker thread when the
  connection to the app changed.

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
