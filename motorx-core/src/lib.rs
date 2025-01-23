//! A reverse-proxy written in pure rust, built on hyper, tokio, and rustls
//! # Motorx
//! ## Basic usage
//!
//! ```ignore
//! #[tokio::main]
//! async fn main() {
//!     // Register a tracing subscriber for logging
//!
//!     let server = motorx_core::Server::new(motorx_core::Config { /* Your config here */ });
//!
//!     // Start the server
//!     server.run().await.unwrap()
//! }
//! ```

pub mod config;
mod conn_pool;
pub mod error;
mod handle;
#[macro_use]
pub mod log;
mod cache;
#[cfg(test)]
mod e2e;
#[cfg(feature = "tls")]
pub mod tls;

#[cfg_attr(feature = "logging", macro_use(info, error, debug, trace))]
#[cfg(feature = "logging")]
extern crate tracing;

use std::net::SocketAddr;
use std::sync::Arc;

use cache::Cache;
use config::Upstream;
use conn_pool::ConnPool;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::Request;
use hyper_util::rt::{TokioExecutor, TokioIo};
#[cfg(feature = "tls")]
use rustls::ServerConfig;
#[cfg(feature = "tls")]
use tls::stream::TlsStream;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub use config::{CacheSettings, Config, Rule};
pub use error::Error;

// TODO: Consider Boxing this (Or just ConnPool) to improve spacial locality
type UpstreamAndConnPool = (Arc<Upstream>, ConnPool);
type Upstreams = Vec<UpstreamAndConnPool>;

/// Motorx proxy server
///
/// Usage:
/// ```ignore
/// #[tokio::main]
/// async fn main() {
///     // Register a tracing subscriber for logging
///
///     let server = motorx_core::Server::new(motorx_core::Config { /* Your config here */ });
///
///     // start polling and proxying requests
///     server.run().await.unwrap()
/// }
/// ```
pub struct Server {
    config: Arc<Config>,
    cache: Arc<Cache>,
    upstreams: Arc<Upstreams>,
    listener: TcpListener,
    /// Used to enforce max num of connections to this server
    semaphore: Arc<Semaphore>,
    #[cfg(feature = "tls")]
    tls_config: Option<Arc<ServerConfig>>,
}

impl Server {
    /// Do configuration shared between raw and tls servers
    fn common_config(mut config: Config) -> (Arc<Config>, Arc<Cache>, Arc<Upstreams>, TcpListener) {
        let upstreams = Arc::new(init_upstreams(&mut config));
        let cache = Arc::new(Cache::from_config(&mut config));

        config.rules.sort_by(|a, b| a.path.cmp(&b.path));
        let config = Arc::new(config);

        cfg_logging! {debug!("Starting with config: {:#?}", *config);}

        let listener = tcp_listener(config.addr).unwrap();

        (config, cache, upstreams, listener)
    }

    pub fn new(config: Config) -> Self {
        let (config, cache, conn_pools, listener) = Self::common_config(config);

        cfg_logging! {
            info!("Motorx proxy listening on http://{}", {
                listener.local_addr().unwrap()
            });
        }

        Self {
            semaphore: Arc::new(Semaphore::new(config.max_connections)),
            cache,
            upstreams: conn_pools,
            config,
            listener,
            #[cfg(feature = "tls")]
            tls_config: None,
        }
    }

    #[cfg(feature = "tls")]
    pub fn new_tls(config: Config) -> Self {
        let (config, cache, conn_pools, listener) = Self::common_config(config);
        let tls_config = {
            // Load public certificate.
            let certs = tls::load_certs(
                config
                    .certs
                    .as_ref()
                    .expect("Must provide `certs` in config to use tls."),
            )
            .unwrap();

            // Load private key.
            let key = tls::load_private_key(
                config
                    .private_key
                    .as_ref()
                    .expect("Must provide `private_key` in config to use tls."),
            )
            .unwrap();

            // Do not use client certificate authentication.
            let mut cfg = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(certs, key)
                .unwrap();

            // Configure ALPN to accept HTTP/2, HTTP/1.1 in that order.
            cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

            Arc::new(cfg)
        };

        cfg_logging! {
            info!("Motorx proxy listening on https://{}", listener.local_addr().unwrap());
        }

        Self {
            semaphore: Arc::new(Semaphore::new(config.max_connections)),
            cache,
            upstreams: conn_pools,
            config,
            listener,
            tls_config: Some(tls_config),
        }
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    pub async fn run(self) -> Result<(), hyper::Error> {
        loop {
            println!("Getting semaphore");
            if let Ok(permit) = self.semaphore.clone().acquire_owned().await {
                println!("Polling listener");
                match self.listener.accept().await {
                    Ok((stream, peer_addr)) => {
                        cfg_logging! {
                            trace!("Accepted connection from {}", peer_addr);
                        }

                        #[cfg(feature = "tls")]
                        if let Some(tls_config) = self.tls_config.as_ref() {
                            let tls_stream = TlsStream::new(stream, Arc::clone(tls_config));
                            handle_connection(
                                tls_stream,
                                peer_addr,
                                Arc::clone(&self.config),
                                Arc::clone(&self.cache),
                                Arc::clone(&self.upstreams),
                                permit,
                            )
                        } else {
                            handle_connection(
                                stream,
                                peer_addr,
                                Arc::clone(&self.config),
                                Arc::clone(&self.cache),
                                Arc::clone(&self.upstreams),
                                permit,
                            )
                        };
                        #[cfg(not(feature = "tls"))]
                        handle_connection(
                            stream,
                            peer_addr,
                            Arc::clone(&self.config),
                            Arc::clone(&self.cache),
                            Arc::clone(&self.conn_pools),
                            permit,
                        );
                    }
                    Err(e) => {
                        cfg_logging! {
                            error!("Error connecting, {:?}", e);
                        }
                    }
                }
            }
        }
    }
}

#[cfg_attr(
    feature = "logging",
    tracing::instrument(skip(stream, config, cache, permit))
)]
fn handle_connection<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    stream: S,
    peer_addr: SocketAddr,
    config: Arc<Config>,
    cache: Arc<Cache>,
    conn_pools: Arc<Upstreams>,
    permit: OwnedSemaphorePermit,
) {
    let service = service_fn(move |req: Request<Incoming>| {
        let config = config.clone();
        let cache = cache.clone();
        let conn_pools = conn_pools.clone();

        async move {
            let res = handle::handle_req(
                req,
                peer_addr,
                Arc::clone(&config),
                Arc::clone(&cache),
                Arc::clone(&conn_pools),
            )
            .await;

            cfg_logging! {
                trace!("Responded to req from {}", peer_addr);
            }

            res
        }
    });

    tokio::spawn(async move {
        cfg_logging! {
            trace!("Handling connection from {}", peer_addr);
        }
        let conn_build = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
        if let Err(err) = conn_build
            .serve_connection_with_upgrades(TokioIo::new(stream), service)
            .await
        {
            cfg_logging! {trace!("Error handling connection: {err:?}");}
        };

        cfg_logging! {
            trace!("Closing connection to {}", peer_addr);
        }

        drop(permit);
    });
}

#[inline]
fn tcp_listener(addr: SocketAddr) -> std::io::Result<tokio::net::TcpListener> {
    let std_listener = std::net::TcpListener::bind(addr)?;
    std_listener.set_nonblocking(true)?;
    tokio::net::TcpListener::from_std(std_listener)
}

#[inline]
async fn tcp_connect(
    addr: impl tokio::net::ToSocketAddrs,
) -> std::io::Result<tokio::net::TcpStream> {
    tokio::net::TcpStream::connect(addr).await
}

fn init_upstreams(config: &mut Config) -> Upstreams {
    let mut upstreams = Vec::with_capacity(config.upstreams.len());

    let mut upstream_order = Vec::new();

    for upstream_name in config.upstreams.keys() {
        upstream_order.push(upstream_name.clone());
    }

    for (key, upstream_name) in upstream_order.iter().enumerate() {
        // Find any authentication referencing this upstream and populate their key
        for (_, upstream) in &mut config.upstreams {
            if let Some(auth) = Arc::get_mut(upstream).unwrap().authentication.as_mut() {
                match &mut auth.source {
                    config::authentication::AuthenticationSource::Upstream {
                        name: _,
                        path: _,
                        key: upstream_key,
                    } => *upstream_key = key,
                    config::authentication::AuthenticationSource::Path(_) => {}
                }
            }
        }

        // Find any rules referencing this upstream and populate them with the key
        for rule in &mut config.rules {
            if rule.upstream == *upstream_name {
                rule.upstream_key = key;
            }
        }
    }

    // Now, add upstreams into Vec
    for (key, upstream_name) in upstream_order.iter().enumerate() {
        let upstream = config.upstreams.get_mut(upstream_name).unwrap();
        Arc::get_mut(upstream).unwrap().key = key;
        upstreams.push((
            Arc::clone(upstream),
            ConnPool::new(upstream.addr.clone(), upstream.max_connections),
        ));
    }

    upstreams.shrink_to_fit();

    upstreams
}
