pub mod authentication;
pub mod match_type;
pub mod rule;

pub use rule::{CacheSettings, Rule};

use std::{collections::HashMap, net::SocketAddr, path::PathBuf, str::FromStr, sync::Arc};

use http::Uri;

use self::authentication::Authentication;

#[cfg_attr(feature = "serde-config", derive(serde::Deserialize))]
#[derive(Debug)]
pub struct Config {
    pub addr: SocketAddr,
    pub tls: Option<Tls>,
    pub rules: Vec<Rule>,
    pub upstreams: HashMap<String, Arc<Upstream>>,
    #[cfg_attr(
        feature = "serde-config",
        serde(default = "default_server_max_connections")
    )]
    pub max_connections: usize,
}

#[cfg_attr(feature = "serde-config", derive(serde::Deserialize))]
#[derive(Debug)]
pub struct Upstream {
    #[cfg_attr(feature = "serde-config", serde(with = "http_serde::uri"))]
    pub addr: Uri,
    #[cfg_attr(
        feature = "serde-config",
        serde(default = "default_upstream_max_connections")
    )]
    pub max_connections: usize,
    pub authentication: Option<Authentication>,
    /// Upstreams key in a slab, it is overridden on startup
    #[cfg_attr(feature = "serde-config", serde(default))]
    pub key: usize,
}

#[cfg_attr(feature = "serde-config", derive(serde::Deserialize))]
#[derive(Debug)]
pub enum Tls {
    #[cfg(feature = "tls")]
    File {
        certs: PathBuf,
        private_key: PathBuf
    },
    #[cfg(feature = "tls")]
    Acme {
        domains: Arc<[String]>,
        cache_dir: PathBuf,
    }
}

const fn default_upstream_max_connections() -> usize {
    10
}

const fn default_server_max_connections() -> usize {
    100
}

impl Default for Config {
    fn default() -> Self {
        Self {
            addr: SocketAddr::V4(std::net::SocketAddrV4::new(
                std::net::Ipv4Addr::new(0, 0, 0, 0),
                80,
            )),
            tls: Default::default(),
            max_connections: default_server_max_connections(),
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
