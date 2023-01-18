# Motorx

## A reverse-proxy in pure rust

## Features

- Robust configuration & request filtering
- Caching
- Wasm/wasi through wasmedge

## Usage

### Binary

Binaries for popular platforms are built for every release. You can install them with `cargo binstall` ([repo](https://github.com/cargo-bins/cargo-binstall)), `cargo install`, or on [the releases page](https://github.com/tsar-boomba/motorx/releases).

### Docker Image

Docker images are pushed to the [Docker Hub repository](https://hub.docker.com/repository/docker/igamble/motorx/general) on every release. If you would like to support more images, please open a pull request.

## motorx-core

Build your own binary

### Crate Features

- `tracing`: Emit log information through `tracing` crate
- `serde-config`: Implements `serde::Deserialize` for config structs
- `tls`: Adds tls support through `rustls` (not yet tested)
- `wasm`: no-default-features only, allows compilation for wasm32-wasi and running in wasmedge

## Contributing

From v0.1.0 motorx uses [conventional commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/).

## License

Both `motorx` and `motorx-core` are licensed under the [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE) license
