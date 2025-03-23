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
    #[cfg(feature = "h3")]
    #[cfg_attr(all(feature = "h3", feature = "serde-config"), serde(default))]
    pub h3_addr: Option<SocketAddr>,
    #[cfg(feature = "prometheus")]
    #[cfg_attr(all(feature = "prometheus", feature = "serde-config"), serde(default))]
    pub prometheus_addr: Option<SocketAddr>,
    pub tls: Option<Tls>,
    pub rules: Vec<Rule>,
    pub upstreams: HashMap<String, Arc<Upstream>>,
    #[cfg_attr(
        feature = "serde-config",
        serde(default = "default_server_max_connections")
    )]
    pub max_connections: usize,
    #[cfg_attr(
        feature = "serde-config",
        serde(default = "default_client_buffer_size")
    )]
    pub client_buffer_size: usize,
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
    #[cfg_attr(
        feature = "serde-config",
        serde(default = "default_upstream_buffer_size")
    )]
    pub buffer_size: usize,
    #[cfg_attr(feature = "serde-config", serde(default))]
    pub proto: Proto,
    /// Upstreams key in a slab, it is overridden on startup
    #[cfg_attr(feature = "serde-config", serde(default))]
    pub key: usize,
}

#[cfg_attr(feature = "serde-config", derive(serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Proto {
    #[default]
    Http1,
    Http2,
}

#[cfg_attr(feature = "serde-config", derive(serde::Deserialize))]
#[derive(Debug)]
pub enum Tls {
    #[cfg(feature = "tls")]
    File {
        certs: PathBuf,
        private_key: PathBuf,
    },
    #[cfg(feature = "tls")]
    Acme {
        domains: Arc<[String]>,
        cache_dir: PathBuf,
    },
}

const fn default_upstream_max_connections() -> usize {
    10
}

const fn default_server_max_connections() -> usize {
    100
}

const fn default_upstream_buffer_size() -> usize {
    8 * 1024
}

const fn default_client_buffer_size() -> usize {
    8 * 1024
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
            client_buffer_size: default_client_buffer_size(),
            rules: Vec::new(),
            upstreams: HashMap::new(),
            #[cfg(feature = "h3")]
            h3_addr: None,
            #[cfg(feature = "prometheus")]
            prometheus_addr: None,
        }
    }
}

impl Config {
    pub(crate) fn will_start_h3(&self) -> bool {
        self.h3_addr.is_some() && matches!(self.tls, Some(Tls::Acme { .. } | Tls::File { .. }))
    }
}

#[cfg(feature = "serde-config")]
impl FromStr for Config {
    type Err = serde_json::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s)
    }
}
