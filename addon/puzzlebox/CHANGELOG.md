# Changelog

## Unreleased

### Fixes

- Opening a program while Home Assistant (or Zigbee2MQTT, a pack or a
  plugin) wasn't available and then saving could delete the blocks it
  provides. Such blocks now stay as "‹…› isn't available right now", the
  editor waits read-only until they're back, and the server refuses a save
  that would drop them. Home Assistant's blocks are also kept on disk, so
  they're there at start-up without Home Assistant.
- Programs started twice when Puzzlebox started, so "when program starts"
  ran twice.
- "21,5" typed into a number became 215. A decimal comma now works, and
  unclear input is refused.
- Saving a program no longer stops its waiting runs and timers when the
  blocks didn't change; when they did, the editor says what will be stopped
  first. Timers survive a restart. Stopped runs say why.
- A run paused at a breakpoint carries on after 10 minutes, closing one
  program's editor no longer releases another's paused runs, and the
  debugger says where it's paused.
- Puzzlebox shows "Waiting for Home Assistant…" instead of an error page
  while Home Assistant starts.
- All times use Home Assistant's time zone.
- "every 7 minutes" really runs every 7 minutes (from when the program
  starts); for "on the hour" use "the clock shows". "every 90 minutes" works.
- Zigbee2MQTT: "changes" triggers no longer fire on routine reports after a
  restart or save, both gangs of a switch have their own name, and values
  are asked for at start-up.

### Editing

- Type to add (Ctrl+Space) puts the block where you'd expect: a trigger into
  the selected or an empty "when", an action at the end of its "do", a value
  into the first empty socket. Everyday blocks come first in the results, and
  "motion", "notify", "lamp" and similar words find them.
- Leaving the editor by any link or the browser's Back button asks before
  throwing away unsaved changes.
- Problems also warn about a "when" with nothing to do and blocks outside any
  "when", name the block they're about, and list entities that don't exist
  ("hall" isn't an entity).
- Generated action blocks without a target are caught when saving, and Home
  Assistant's errors are shown in plain words (the original under "details").
- Device triggers, conditions and actions show which device they belong to
  ("⚡ Bedroom remote · action double click"), with the name from Home
  Assistant. Click the name to move the block to another device with the same
  trigger; its settings stay.
- The entity picker searches area and device names, sorts by name, says when
  there are more results, and offers "All lights in Bedroom" where a block
  accepts an area.
- Blocks generated from Home Assistant read like the others (lower-case
  labels, "5 minutes" durations), and their categories list the easier
  Puzzlebox blocks first.
- Clearer labels: "set target temperature of …", "set mode of …", "set helper
  …", and notifications "to 📱 Pixel 7". The time trigger only offers a shift
  for a helper or sensor time. "Say" starts with your first voice.
- Big programs open at their first stack, and the minimap is smaller and
  see-through.
- Copy and paste works between programs and tabs. Type to add follows the
  keyboard cursor, matches part of what you type ("door open", "wait 10"),
  and fills in numbers.
- Menus in plain words ("Delete this and the 2 blocks inside"), a keyboard
  shortcuts help ("?"), and dialogs, tabs and messages that work with the
  keyboard and screen readers.
- Starters for a first program (motion light, door left open, vacation
  lights, good night), and hints in empty "when" and "do" slots. New
  programs say "Not saved yet".
- History shows a version before you restore it, and restoring brings back
  the name and run mode too.

### New blocks

- "⚡ the first person arrives at / the last person leaves [zone]".
- "remember the state of … as …" and "restore …".
- Alarm panels: arm, disarm and "alarm is armed / disarmed / triggered".
- "press [button]".
- "send notification … with buttons … and wait for a reply" and "the reply".
- Each kind of variable explains itself: only during this run, kept between
  runs, or shared by all programs.
- "add … to list", "remove … from list", "list … contains …".
- "run program …" and "run program … and wait until it finishes".
- "fade … from 1 % to 100 % over 15 minutes", which stops if someone changes
  the light in between.
- "carry on if this fails", "try" and "run in parallel" are in Control.
- Zigbee2MQTT blocks read like the others: "⚡ Front door contact becomes
  open", white temperature in kelvin and brightness in % (saved programs are
  converted), devices grouped as lights, switches, sensors and remotes.

### Programs and runs

- The Runs tab shows runs in progress and what they're doing ("waiting 30 s,
  12 s left"), and the editor marks the block.
- Run details start with what started the run ("Movement Backyard turned
  on"), show how each step went ("false → else", "waited 2 s"), and keep the
  text the blocks had at the time.
- "Run as if ⚡ … fired" (Run ▾ or right-click a trigger) tests a program with
  trigger data.
- The program list shows each program's trigger, marks failed runs and
  programs that do nothing, and uses the editor's words for run modes. The
  editor's on/off switch works straight away.
- New programs are created when first saved. "Duplicate" copies a program,
  and deleting moves it to a recycle bin for 30 days.
- The Log tab keeps the last 200 lines per program, warnings and errors
  included. History says what changed. Stop is only active while something
  runs. Settings explains what needs the integration.
- After importing from Home Assistant, "Switch over" turns off the original
  automation and turns the program on.

### Home Assistant

- Each program has a status sensor (idle, running, waiting, paused, error).
- `puzzlebox.run_program` takes the program's switch or button.
- Repairs report programs with problems or that keep failing.
- Settings: stored variables by program name with delete, folder paths as
  seen over Samba, Zigbee2MQTT status, and a note if time zones differ.

### Look

- Readable block colours in light and dark mode (all text at least 4.5:1
  contrast).
- Works on phones: the program list fits, and the side panel opens as a sheet.
- The entity list updates with only what changed, which is lighter on big
  installations, and responses are compressed.
- Dates and times follow your Home Assistant profile (12/24 h).

## 0.5.0

### Triggers

- State triggers can watch a detail (attribute) instead of the state, take
  several states ("open, opening"), match "anything but" a state, or fire on
  any change including details. A list of states used to be sent as one text.
- Numeric triggers can watch a detail or a template and wait "for" a while.
- Time triggers can be limited to weekdays and shifted from a date/time
  entity ("10 minutes before the alarm").
- New triggers: the clock shows (hour/minute/second patterns), Home Assistant
  shuts down, a template becomes true, calendar events, NFC tags, Assist
  sentences, and Home Assistant's own MQTT and webhook triggers.
- Home Assistant triggers can have a trigger id and a name ("＋ option"). Several
  triggers may share an id, and "triggered by" uses it.
- "Home Assistant trigger … on … with options …" and the matching condition
  cover the newer integration triggers and conditions (such as
  `light.turned_on`). "Switch to" offers the integration's own block when your
  Home Assistant has it.

### Conditions

- "… is …" can check a detail (attribute), several states, and "any of" several
  entities.
- New conditions: "… is above … and below …", "it is after/before sunset or
  sunrise" with an offset, "… is in zone …", and "template is true".
- A condition that fails while Home Assistant checks it (an unknown entity, a
  state that isn't a number) now counts as false with a warning in the log,
  as in Home Assistant, instead of stopping the run.

### Actions and waits

- "Perform action … and save the response in …" keeps what an action sends
  back (weather forecasts, calendar events…) in a variable.
- "loop round number / item / is on its first or last round" inside loops.
  Templates in loops see Home Assistant's `repeat` (`repeat.index`,
  `repeat.item`…), nested loops their own.
- After "wait for" and "wait until", templates see Home Assistant's full
  `wait` (`remaining`, `completed`, `trigger`), and several triggers are
  numbered as in Home Assistant (`wait.trigger.idx`). Trigger templates can
  use the program's variables.
- "Wait until" a template reacts as soon as something the template uses
  changes, instead of checking once a second.
- "Carry on if this fails" runs the blocks inside and, if Home Assistant
  reports an error it would skip with `continue_on_error`, carries on below.
  Unknown actions, invalid data and lost connections still stop the run.
- Waits have "at most …" or "with no time limit"; 0 now means give up at once.
  Saved programs keep waiting without a limit where they had 0.

### Values

- "… as a decimal / whole number, if not a number …" reads numbers the way
  Home Assistant templates do, and can stop with an error like `| float`.
- "round … to … decimals" rounds like Home Assistant (halves to even, or
  floor, ceil, half).
- "text of … like Home Assistant" writes True, None and 20.0 as templates do.
- "… = …, text stays text" compares without turning text into numbers, and
  "… is nothing" checks for an empty value.
- "format … as …" formats a time with strftime codes.

### Import

- Import understands whatever you paste: a single automation or script,
  `automations.yaml`, `scripts.yaml`, `configuration.yaml` (including
  `automation manual:` style keys and packages), several documents separated
  by `---`, or just a list of actions.
- Blueprints import with their defaults filled in. Paste a blueprint together
  with automations that use it and they are expanded too.
- `!secret`, `!include` and other Home Assistant tags are kept as visible
  placeholders, never resolved, and listed as warnings in the preview.
- Far more becomes real blocks instead of YAML: triggers with their ids;
  state, time, weekday, trigger and device conditions in any and/or/not
  form; every kind
  of delay, wait (`wait_template` too), choose, if, repeat (count, for each,
  while, until, also when they use `repeat`), parallel and stop; actions
  with a `response_variable`; and service calls that have a Home or Lights
  block, such as `light.turn_on` with a colour or
  `climate.set_temperature`. The new triggers and conditions above import as
  blocks too.
- Templates become blocks when the result is exactly the same:
  `states('sensor.x')`, `is_state(…)`, `state_attr(…)`, `trigger.to_state`,
  variables, comparisons, maths, `now().hour` and text with values in it.
- Anything that would behave even slightly differently stays a Home Assistant
  YAML block, as small as possible, so imported programs run exactly like the
  original.

### Fixes

- Programs with long stacks (about 60 blocks or more in one stack) could not
  be saved, and the backpack could not hold them.

## 0.4.0

### Blocks

- New Home category with one-block actions for switches, covers,
  thermostats, media players, fans, locks, vacuums, scenes, scripts,
  counters and helpers. "Say … on speakers", phone notifications and "turn
  on … for 5 minutes then off" are single blocks, and so are readings like a
  cover's position or a thermostat's temperature.
- New Lights & colours category. One block sets a light to a colour or a
  white temperature with brightness and fade; others set brightness, dim or
  brighten by a step, and turn lights on, off or toggle them with a fade.
  You can read a light's colour and brightness, and build colours from RGB or
  hue, mix two, or pick one at random. "#rrggbb" text and colour names work
  as colours too.
- The toolbox follows Scratch's order: Events, Control and Sensing first,
  and technical blocks (YAML, templates, cron, HTTP, dictionaries) under
  Advanced. "When" blocks have a hat and triggers start with ⚡.
- Blocks that read an entity have a socket, so a variable, the trigger
  entity or a list item fits there, not only a typed-in ID.
- Durations are "5 minutes" blocks everywhere, triggers included. Units come
  after the number, and times are tidied up as you type ("7" becomes
  "07:00").
- Entity pickers show names and start empty ("choose…"). State and
  attribute boxes suggest values, and blocks warn about entities that don't
  exist in Home Assistant.
- New triggers: "… becomes …" and "… arrives at / leaves a zone".
- New conditions and values: "the sun is up", "someone is home", "today is
  a weekday", "how many of … are on", "state of … 1 hour ago", "trigger
  entity", "the wait timed out", "what ended the wait".
- New control blocks: switch / case / otherwise, section labels for long
  stacks, and "wait until it is 07:00".
- New list and text blocks: "items of … where …" and "each item of … as …"
  (with a "current item" block), regular expressions, text before or after
  a separator, "is between", and "… or if empty …" for unavailable values.
- Areas, floors, labels and devices work in "any/all of", averages, scene
  blocks and loops.
- Colour settings in Home Assistant and Zigbee2MQTT blocks take colour
  blocks. Zigbee2MQTT group brightness and white temperature are now in %
  and K; values in saved programs are converted when you open them.

### Editor

- Press Ctrl+Space (or middle-click the canvas), type what you want ("turn
  off light", "wait until") and press Enter. The block attaches after the
  selected one, or lands where you clicked.
- Right-click a block to switch it to a related one (turn on, turn off,
  toggle; an entity, area or floor; a device's other triggers). Its contents
  and its place in the stack stay.
- Right-click to find the uses of a variable, timer, entity, function or
  area (repeat to step through them) and to rename a variable, timer or
  entity everywhere in the program.
- Right-click "Run this stack" runs one stack. Stacks outside any "when"
  block run only this way, so you can keep test snippets next to a program;
  they are drawn faded.
- Programs can go in folders, nested if you like. Drag a program onto a
  folder or use "Move". The search also looks at folders and descriptions.
- The Favourites category at the top holds blocks you pin (right-click, Add
  to favourites) and the ones you used last. The backpack (top right) keeps
  blocks for use in other programs. Both follow you across browsers.
- History keeps the last 30 saves that changed the blocks, and you can
  restore any of them.
- Alt+drag leaves a copy behind, Ctrl+drag pulls out a single block, and
  Ctrl+V pastes at the pointer.
- Collapsed blocks show a summary such as "when motion turns on → 6 blocks".
  Long dropdowns have a filter box, and the editor shows the block count.
- Nested blocks of the same colour alternate between two shades. A compact
  block size helps with big programs (Settings → Editor).
- A stack with a problem no longer stops the whole program; the other stacks
  keep running. Problems name the block and the slot in the block's own
  words.
- Settings → About and the README credit TurboWarp, Scratch Addons and
  PenguinMod, whose editors many of these features are modelled on.

### Fixes

- The "−" option of the math block, trim left/right, list insert/remove at
  any position, and list sort types work.
- Two sensor states compare as numbers ("9.5" is less than "10.2").
- "Seconds since … reported" uses the reported time.
- Trigger details are available inside timers.
- State triggers no longer fire when only an attribute changes.
- "Every 60 minutes" and "every 24 hours" become every hour and every day;
  intervals Home Assistant can't run are reported.

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
