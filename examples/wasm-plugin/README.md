# Example: WASM plugin

A plugin is a folder with `plugin.yaml` (a [block pack](../../sdk/block-pack-schema/README.md)
plus `wasm` and `capabilities`) and the compiled module. This one is written
in Rust with the [puzzlebox PDK](../../sdk/rust-pdk/README.md).

Build it for the `wasm32-unknown-unknown` target (install that once with
`rustup`):

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
```

Then copy it into a folder of its own in the app's plugins folder:

```sh
mkdir -p <plugins folder>/example_wasm
cp plugin.yaml <plugins folder>/example_wasm/
cp target/wasm32-unknown-unknown/release/puzzlebox_example_plugin.wasm <plugins folder>/example_wasm/plugin.wasm
```

Inside the app the plugins folder is `/config/plugins`. You find it under
`addon_configs/e46cc951_puzzlebox/plugins`, through the Samba share app (the
`addon_configs` share) or the Studio Code Server app. The prefix is a hash of
the repository URL, so if you added the repository under a different URL, use
the folder whose name ends in `_puzzlebox`. Outside Home Assistant OS it is
`$PUZZLEBOX_DATA_DIR/plugins`. Press *Reload* on the app's Settings page.

Plugins run sandboxed: they can only read states, call the listed actions,
use storage or reach the listed hosts if `capabilities` grants it, and each
call is limited in time and memory. Any language with an
[Extism PDK](https://extism.org/docs/concepts/pdk) works; use the host
functions `puzzlebox_log`, `puzzlebox_get_state`, `puzzlebox_call_service`, `puzzlebox_kv_get`,
`puzzlebox_kv_set` (see `sdk/rust-pdk`).
