# puzzlebox-pdk

Helpers for writing Puzzlebox WASM plugins in Rust. It wraps the
[Extism PDK](https://github.com/extism/rust-pdk) with the input type every
block function gets and the Puzzlebox host functions.

## Set up

A plugin is a `cdylib` crate. The PDK isn't on crates.io, so depend on it
from git (or by `path` inside this repository). `extism-pdk` is needed
directly too, because `#[plugin_fn]` refers to it.

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
puzzlebox-pdk = { git = "https://github.com/TechGigant/puzzlebox" }
extism-pdk = "1"
```

## Write a block function

Each block with a `plugin` lowering calls one exported function. It gets the
block's params in `args` (and `program_id`, `run_id` and `trigger` in
`context`) and returns the block's value as JSON (`null` for actions).

```rust
use puzzlebox_pdk::prelude::*;

#[plugin_fn]
pub fn greet(input: Json<Call>) -> FnResult<Json<Value>> {
    let name = input.0.arg_str("name").unwrap_or("world");
    log(&format!("greeting {name}"))?;
    Ok(Json(json!(format!("Hello, {name}!"))))
}
```

`Call` has `arg`, `arg_str` and `arg_f64` (which also parses numbers sent as
text). The host functions only work if `plugin.yaml` grants the capability:

| function | Needs | Does |
| --- | --- | --- |
| `log(msg)` | nothing | Writes to the app's log. |
| `get_state(entity_id)` | `states: true` | Returns `{"state": ..., "attributes": {...}}`, or `null` if the entity doesn't exist. |
| `call_service(action, data, target)` | the action under `services` | Calls a Home Assistant action, e.g. `call_service("light.turn_on", None, Some(json!({"entity_id": "light.kitchen"})))`. |
| `kv_get(key)` / `kv_set(key, value)` | `kv: true` | Private storage that survives restarts. Missing keys are `null`. |

HTTP requests go through Extism's own `extism_pdk::http::request`, limited to
the hosts listed under `capabilities.http`.

The matching `plugin.yaml` declares the block and points at the function:

```yaml
pack: hello
name: Hello
wasm: plugin.wasm
blocks:
  - id: greet
    kind: value
    title: greeting for
    params:
      - {name: name, label: name, required: true, input: text, default: you}
    lowering: {type: plugin, plugin: hello, function: greet}
```

`plugin` must be the pack's own id. See the
[block pack docs](../block-pack-schema/README.md) for all fields.

## Build and install

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
```

Copy `plugin.yaml` and `target/wasm32-unknown-unknown/release/<crate>.wasm`
(renamed to the file named by `wasm:`) into a folder of their own in the app's
plugins folder, then press *Reload* on the Settings page. Inside the app the
plugins folder is `/config/plugins`; you find it under
`addon_configs/e46cc951_puzzlebox/plugins` through the Samba share or Studio
Code Server app (the prefix is a hash of the repository URL; if yours differs,
use the folder whose name ends in `_puzzlebox`).

A complete example is in [`examples/wasm-plugin`](../../examples/wasm-plugin).
