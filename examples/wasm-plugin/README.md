# Example: WASM plugin

A plugin is a folder with `plugin.yaml` (a [block pack](../../sdk/block-pack-schema/README.md)
plus `wasm` and `capabilities`) and the compiled module.

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
mkdir -p <plugins dir>/example_wasm
cp plugin.yaml <plugins dir>/example_wasm/
cp target/wasm32-unknown-unknown/release/puzzlebox_example_plugin.wasm <plugins dir>/example_wasm/plugin.wasm
```

The plugins dir is `/addon_configs/<app>/plugins` under Home Assistant OS (or
`$PUZZLEBOX_DATA_DIR/plugins`). Press *Reload* on the app's Settings page.

Plugins run sandboxed: they can only read states, call the listed actions,
use storage or reach the listed hosts if `capabilities` grants it, and each
call is limited in time and memory. Any language with an
[Extism PDK](https://extism.org/docs/concepts/pdk) works; use the host
functions `puzzlebox_log`, `puzzlebox_get_state`, `puzzlebox_call_service`, `puzzlebox_kv_get`,
`puzzlebox_kv_set` (see `sdk/rust-pdk`).
