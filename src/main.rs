use motorx_core::Server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    motorx::setup_tracing();
    let config = motorx::config_from_args()?;

    if config.private_key.is_some() && config.certs.is_some() {
        Server::new_tls(config)
    } else {
        Server::new(config)
    }
    .run()
    .await
    .map_err(Into::into)
}
