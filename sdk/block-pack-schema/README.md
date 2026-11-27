# Block packs

A block pack is a YAML or JSON file that describes blocks. Three kinds of
source use the format:

- Declarative packs: put `*.yaml` files in the app's `packs/` folder and
  press *Reload* on the Settings page. Inside the app this is `/config/packs`.
  You find it under `addon_configs/e46cc951_puzzlebox/packs`, through the
  Samba share app (the `addon_configs` share) or the Studio Code Server app.
  The prefix is a hash of the repository URL, so if you added the repository
  under a different URL, use the folder whose name ends in `_puzzlebox`.
- Python integrations call
  `custom_components.puzzlebox.api.async_register_blocks(hass, pack, handlers)`.
- WASM plugins ship a `plugin.yaml` next to the module, with `wasm:` and
  `capabilities:` added.

Schema: [`schema.json`](schema.json) (version 1). Block types are namespaced
(`pack__<pack>__<id>`, `py__…`, `wasm__…`), so saved programs keep working
when packs change.

## Block fields

| field | Default | Meaning |
| --- | --- | --- |
| `id` | required | `[a-z0-9_]+`, unique in the pack. Don't rename it: saved programs refer to it. |
| `kind` | required | `action`, `trigger`, `condition` or `value` (see below). |
| `title` | required | Text on the block and in quick-add search. Write it like the built-in blocks, in lower case: `set room temperature`, not `Set Room Temperature`. |
| `tooltip` | none | Shown when hovering over the block. |
| `help_url` | none | Opened by *Help* in the block's right-click menu. |
| `category` | the pack's `name` | Toolbox category as a list. A second entry makes a subcategory: `[Comfort, Heating]`. |
| `style` | one colour per pack | Colour. Use a built-in name to match those blocks: `trigger`, `condition`, `action`, `logic`, `math`, `text`, `list`, `variable`, `time`, `flow`, `home`, `http`, `plugin`. Any other name gets its own colour that stays the same. |
| `target` | none | Entity/area target slot: `{domains: [light], required: true}`. |
| `params` | none | Inputs on the block (see below). |
| `lowering` | required | What the block does (see below). |

## Block kinds

| kind | Shape | Allowed lowerings |
| --- | --- | --- |
| `action` | statement | `service`, `ha_action`, `plugin` |
| `trigger` | goes into a *when* block | `ha_trigger`, `ha_trigger_platform` |
| `condition` | boolean value | `ha_condition`, `ha_condition_platform`, `template`, `plugin` |
| `value` | value | `template`, `plugin`, `service` with `response: true` |

## Parameters

Each param becomes an input on the block. `name` (required) is the key used in
the lowering and must not be `target`. `label` (required) is the text before
the input. `description` is added to the block's tooltip as
"• label: description". `required: true` shows the input right away; optional
params are added with the block's *＋ option* menu. `default` fills the input
when the block is created.

`input` picks the editor:
`number` (`min`/`max`/`step`/`unit`), `text` (`multiline`), `boolean`, `select`
(`options`), `entity` (`domains`), `duration` (seconds), `time`, `color`
(sent as `[r, g, b]`), `any`.

For an `entity` param, leave out `default`: the block then gets an empty entity
picker, filtered to `domains`, so users pick one of their own entities. A
default like `light.porch` only works if that entity exists.

## Placeholders

`ha_trigger`, `ha_condition` and `ha_action` configs are native Home Assistant
YAML. `{"$arg": "<param>"}` (or `{"$arg": "target"}`) is replaced by the
block's value; keys whose optional param is unset are dropped. Trigger values
must be fixed (they are subscribed once); condition and action values are
evaluated each run. `template` lowerings are Jinja rendered by Home Assistant
with params, program variables and `trigger` available.

In this example, replace `mobile_app_pixel` with your phone's notify action:

```yaml
pack: phone
name: My phone
blocks:
  - id: notify
    kind: action
    title: notify my phone
    tooltip: Sends a push notification to the Home Assistant app.
    help_url: https://companion.home-assistant.io/docs/notifications/notifications-basic
    category: [Phone]
    style: action
    params:
      - {name: message, label: message, required: true, input: text}
      - name: tag
        label: replace
        description: Notifications with the same tag replace each other.
        input: text
    lowering:
      type: ha_action
      sequence:
        - action: notify.mobile_app_pixel
          data:
            message: {"$arg": message}
            data: {tag: {"$arg": tag}}
```

## Plugin lowerings

`{"type": "plugin", "plugin": "<pack id>", "function": "<name>"}` calls code:
a Python handler `(hass, args, context) -> value` or a WASM export taking
`{"args": {...}, "context": {...}}` as JSON input and returning JSON. Only
Python and WASM sources may use them, and only for their own pack id.
