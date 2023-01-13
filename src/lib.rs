use std::{fs::read_to_string, str::FromStr};

use motorx_core::Config;
use tracing::debug;
use tracing_subscriber::EnvFilter;

pub fn setup_tracing() {
    let filter = EnvFilter::try_from_default_env().ok();
    let filter = if let Some(filter) = filter {
        filter
    } else {
        println!("No RUST_LOG environment variable found, proceeding with INFO log level.");
        "info".into()
    };

    tracing_subscriber::fmt().with_env_filter(filter).init();
}

pub fn config_from_args() -> Result<Config, Box<dyn std::error::Error>> {
    let args = std::env::args().collect::<Vec<String>>();
    debug!("Called with args {:?}", args);

    Ok(Config::from_str(&read_to_string(
        // config file from first arg or motorx.json
        args.get(1).unwrap_or(&String::from("motorx.json")),
    )?)?)
}
