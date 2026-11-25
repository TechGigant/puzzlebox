# Example: Python plugin blocks

`custom_components/puzzlebox_example` shows how any integration can add blocks to
Puzzlebox through its companion integration:

1. Describe blocks as a [block pack](../../sdk/block-pack-schema/README.md).
   Blocks with a `plugin` lowering call back into Python.
2. Call `async_register_blocks(hass, PACK, handlers)` from
   `custom_components.puzzlebox.api`. It returns an unregister callback.

Handlers receive `(hass, args, context)`: `args` holds the block's parameters,
`context` holds `program_id`, `run_id` and `trigger`. They may be sync or async
and return any JSON value (for `value`/`condition` blocks).

Try it: copy the folder into `config/custom_components/`, add
`puzzlebox_example:` to `configuration.yaml` and restart Home Assistant. The
"Example blocks" category appears in the editor.
