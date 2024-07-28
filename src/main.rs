use motorx_core::Server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    motorx::setup_tracing();

    Server::new(motorx::config_from_args()?)
        .run()
        .await
        .map_err(|e| e.into())
}
