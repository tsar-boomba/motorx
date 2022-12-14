use std::{fs::read_to_string, str::FromStr};

use tracing::debug;
use tracing_subscriber::EnvFilter;

use motorx_core::{Config, Server};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    setup_tracing();

    let args = std::env::args().collect::<Vec<String>>();
    debug!("Called with args {:?}", args);

    let config = Config::from_str(&read_to_string(
        // config file from first arg or motorx.json
        args.get(1).unwrap_or(&String::from("motorx.json")),
    )?)?;

    Server::new(config).await.map_err(|e| e.into())
}

fn setup_tracing() {
    let filter = EnvFilter::try_from_default_env().ok();
    let filter = if let Some(filter) = filter {
        filter
    } else {
        println!("No RUST_LOG environment variable found, proceeding with INFO log level.");
        "info".into()
    };

    tracing_subscriber::fmt().with_env_filter(filter).init();
}
