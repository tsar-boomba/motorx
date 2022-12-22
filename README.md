# Motorx

## A reverse-proxy in pure rust

## Features

- Robust configuration & request filtering
- Caching
- Wasm/wasi (coming as soon as hyper_wasi goes 1.0)

## motorx-core

Build your own binary

### Crate Features

- `tracing`: Emit log information through `tracing` crate
- `json-config`: Implements `serde::Deserialize` for config structs
- `tls`: Adds tls support through `rustls` (not yet tested)
