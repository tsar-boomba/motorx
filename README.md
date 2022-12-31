# Motorx

## A reverse-proxy in pure rust

## Features

- Robust configuration & request filtering
- Caching
- Wasm/wasi (only with wasmedge)

## motorx-core

Build your own binary

### Crate Features

- `tracing`: Emit log information through `tracing` crate
- `serde-config`: Implements `serde::Deserialize` for config structs
- `tls`: Adds tls support through `rustls` (not yet tested)
- `wasm`: no-default-features only, allows compilation for wasm32-wasi and running in wasmedge

## License

Both `motorx` and `motorx-core` are licensed under the MIT or Apache 2.0 license
