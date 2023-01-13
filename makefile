debug_wasm:
	cargo build --target=wasm32-wasi --no-default-features -F wasm
release_wasm:
	cargo build --release --target=wasm32-wasi --no-default-features -F wasm
compile_wasm:
	make release_wasm && wasmedgec --optimize 0 target/wasm32-wasi/release/motorx.wasm aot.wasm
