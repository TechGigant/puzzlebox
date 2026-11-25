# Block packs

A block pack is a YAML or JSON file that describes blocks. Three kinds of
source use the format:

- Declarative packs: put `*.yaml` files in the app's `packs/` folder
  (`/addon_configs/<app>/packs` under Home Assistant OS) and press *Reload*
  on the Settings page.
- Python integrations call
  `custom_components.puzzlebox.api.async_register_blocks(hass, pack, handlers)`.
- WASM plugins ship a `plugin.yaml` next to the module, with `wasm:` and
  `capabilities:` added.

Schema: [`schema.json`](schema.json) (version 1). Block types are namespaced
(`pack__<pack>__<id>`, `py__…`, `wasm__…`), so saved programs keep working
when packs change.

## Block kinds

| kind | Shape | Allowed lowerings |
| --- | --- | --- |
| `action` | statement | `service`, `ha_action`, `plugin` |
| `trigger` | goes into a *when* block | `ha_trigger`, `ha_trigger_platform` |
| `condition` | boolean value | `ha_condition`, `ha_condition_platform`, `template`, `plugin` |
| `value` | value | `template`, `plugin`, `service` with `response: true` |

## Parameters

Each param becomes an input on the block. `input` picks the editor:
`number` (`min`/`max`/`step`/`unit`), `text` (`multiline`), `boolean`, `select`
(`options`), `entity` (`domains`), `duration` (seconds), `time`, `color`
(sent as `[r, g, b]`), `any`. Optional params are added with the block's
*＋ option* menu.

A `target` gives the block an entity/area target slot.

## Placeholders

`ha_trigger`, `ha_condition` and `ha_action` configs are native Home Assistant
YAML. `{"$arg": "<param>"}` (or `{"$arg": "target"}`) is replaced by the
block's value; keys whose optional param is unset are dropped. Trigger values
must be fixed (they are subscribed once); condition and action values are
evaluated each run. `template` lowerings are Jinja rendered by Home Assistant
with params, program variables and `trigger` available.

```yaml
pack: phone
name: My phone
blocks:
  - id: notify
    kind: action
    title: Notify my phone
    params:
      - {name: message, label: message, required: true, input: text}
    lowering:
      type: ha_action
      sequence:
        - action: notify.mobile_app_pixel
          data: {message: {"$arg": message}}
```

## Plugin lowerings

`{"type": "plugin", "plugin": "<pack id>", "function": "<name>"}` calls code:
a Python handler `(hass, args, context) -> value` or a WASM export taking
`{"args": {...}, "context": {...}}` as JSON input and returning JSON. Only
Python and WASM sources may use them, and only for their own pack id.
