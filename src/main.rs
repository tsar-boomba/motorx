use motorx_core::Server;

#[cfg_attr(feature = "wasm", tokio::main(flavor = "current_thread"))]
#[cfg_attr(feature = "default", tokio::main)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    motorx::setup_tracing();

    Server::new(motorx::config_from_args()?)
        .await
        .map_err(|e| e.into())
}
