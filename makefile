release_wasm:
	RUSTFLAGS="--cfg tokio_unstable" cargo build --release --target=wasm32-wasi --no-default-features -F wasm