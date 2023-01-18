pub mod authentication;
pub mod match_type;
pub mod rule;

pub use rule::{CacheSettings, Rule};

use std::{collections::HashMap, net::SocketAddr, str::FromStr};

use http::Uri;

use self::authentication::Authentication;

#[cfg_attr(feature = "serde-config", derive(serde::Deserialize))]
#[derive(Debug)]
pub struct Config {
    pub addr: SocketAddr,
    pub certs: Option<String>,
    pub private_key: Option<String>,
    pub rules: Vec<Rule>,
    pub upstreams: HashMap<String, Upstream>,
    #[cfg_attr(feature = "serde-config", serde(default = "default_server_max_connections"))]
    pub max_connections: usize,
}

#[cfg_attr(feature = "serde-config", derive(serde::Deserialize))]
#[derive(Debug)]
pub struct Upstream {
    #[cfg_attr(feature = "serde-config", serde(with = "http_serde::uri"))]
    pub addr: Uri,
    #[cfg_attr(feature = "serde-config", serde(default = "default_max_connections"))]
    pub max_connections: usize,
    pub authentication: Option<Authentication>,
}

fn default_max_connections() -> usize {
    10
}

fn default_server_max_connections() -> usize {
    100
}

impl Default for Config {
    fn default() -> Self {
        Self {
            addr: SocketAddr::V4(std::net::SocketAddrV4::new(
                std::net::Ipv4Addr::new(0, 0, 0, 0),
                80,
            )),
            certs: Default::default(),
            private_key: Default::default(),
            max_connections: 10,
            rules: Vec::new(),
            upstreams: HashMap::new(),
        }
    }
}

#[cfg(feature = "serde-config")]
impl FromStr for Config {
    type Err = serde_json::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s)
    }
}
