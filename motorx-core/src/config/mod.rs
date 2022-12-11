pub mod match_type;
pub mod rule;

use std::{net::SocketAddr, str::FromStr, collections::HashMap};

use http::Uri;

#[cfg_attr(feature = "json-config", derive(serde::Deserialize))]
#[derive(Debug)]
pub struct Config {
    pub addr: SocketAddr,
    pub certs: Option<String>,
    pub private_key: Option<String>,
    pub rules: Vec<rule::Rule>,
    pub upstreams: HashMap<String, Upstream>,
    #[serde(default = "default_server_max_connections")]
    pub max_connections: usize
}

#[cfg_attr(feature = "json-config", derive(serde::Deserialize))]
#[derive(Debug)]
pub struct Upstream {
    #[cfg_attr(feature = "json-config", serde(with = "http_serde::uri"))]
    pub addr: Uri,
    #[serde(default = "default_max_connections")]
    pub max_connections: usize
}

fn default_max_connections() -> usize { 10 }

fn default_server_max_connections() -> usize { 100 }

#[cfg(feature = "json-config")]
impl FromStr for Config {
    type Err = serde_json::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s)
    }
}
