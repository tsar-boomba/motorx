//! A reverse-proxy written in pure rust, built on hyper, tokio, and rustls
//! # Motorx
//! ## Basic usage
//! 
//! ```
//! #[tokio::main]
//! async fn main() {
//!     // Register a tracing subscriber for logging
//! 
//!     let server = motorx_core::Server::new(motorx_core::Config { /* Your config here */ });
//! 
//!     // start polling and proxying requests
//!     server.await.unwrap()
//! }
//! ```

pub mod config;
mod conn_pool;
pub mod error;
mod handle;
#[macro_use]
pub mod log;
mod cache;
#[cfg(feature = "tls")]
pub mod tls;

#[cfg_attr(feature = "logging", macro_use(info, error, debug, trace))]
#[cfg(feature = "logging")]
extern crate tracing;

use std::net::SocketAddr;
use std::sync::Arc;
use std::task::ready;

use cache::init_caches;
use conn_pool::init_conn_pools;
use hyper::body::Incoming;
use hyper::server;
use hyper::service::service_fn;
use hyper::Request;
#[cfg(feature = "tls")]
use rustls::ServerConfig;
#[cfg(feature = "tls")]
use tls::stream::TlsStream;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, OwnedSemaphorePermit};

pub use error::Error;
pub use config::{Config, Rule, CacheSettings};

/// Motorx proxy server
/// 
/// Usage:
/// ```
/// #[tokio::main]
/// async fn main() {
///     // Register a tracing subscriber for logging
/// 
///     let server = motorx_core::Server::new(motorx_core::Config { /* Your config here */ });
/// 
///     // start polling and proxying requests
///     server.await.unwrap()
/// }
/// ```
#[must_use = "Server does nothing unless it is `.await`ed"]
pub struct Server {
    config: Arc<Config>,
    listener: TcpListener,
    /// Used to enforce max num of connections to this server
    semaphore: Arc<Semaphore>,
    #[cfg(feature = "tls")]
    tls_config: Option<Arc<ServerConfig>>,
}

impl Server {
    /// Do configuration shared between raw and tls servers
    fn common_config(mut config: Config) -> (Arc<Config>, TcpListener) {
        init_conn_pools(&config);
        init_caches(&config);
        
        
        config.rules.sort_by(|a, b| a.path.cmp(&b.path));
        let config = Arc::new(config);

        cfg_logging! {debug!("Starting with config: {:#?}", *config);}

        let listener =
            TcpListener::from_std(std::net::TcpListener::bind(config.addr).unwrap()).unwrap();

        (config, listener)
    }

    pub fn new(config: Config) -> Self {
        let (config, listener) = Self::common_config(config);

        cfg_logging! {
            info!("Motorx proxy listening on http://{}", listener.local_addr().unwrap());
        }

        Self {
            semaphore: Arc::new(Semaphore::new(config.max_connections)),
            config,
            listener,
            #[cfg(feature = "tls")]
            tls_config: None,
        }
    }

    #[cfg(feature = "tls")]
    pub fn new_tls(config: Config) -> Self {
        let (config, listener) = Self::common_config(config);
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
                .with_safe_defaults()
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
            config,
            listener,
            tls_config: Some(tls_config),
        }
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

impl std::future::Future for Server {
    type Output = Result<(), hyper::Error>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        loop {
            if let Ok(permit) = Arc::clone(&self.semaphore).try_acquire_owned() {
                match ready!(self.listener.poll_accept(cx)) {
                    Ok((stream, peer_addr)) => {
                        cfg_logging! {
                            trace!("Accepted connection from {}", peer_addr);
                        }

                        #[cfg(feature = "tls")]
                        if let Some(tls_config) = self.tls_config.as_ref() {
                            let tls_stream = TlsStream::new(stream, Arc::clone(tls_config));
                            handle_connection(tls_stream, peer_addr, Arc::clone(&self.config), permit)
                        } else {
                            handle_connection(stream, peer_addr, Arc::clone(&self.config), permit)
                        };
                        #[cfg(not(feature = "tls"))]
                        handle_connection(stream, peer_addr, Arc::clone(&self.config), permit);
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

#[cfg_attr(feature = "logging", tracing::instrument(skip(stream, config)))]
fn handle_connection<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    stream: S,
    peer_addr: SocketAddr,
    config: Arc<Config>,
    permit: OwnedSemaphorePermit
) {
    let service = service_fn(move |req: Request<Incoming>| {
        handle::handle_req(req, peer_addr, Arc::clone(&config))
    });

    tokio::spawn(async move {
        if let Err(err) = server::conn::http1::Builder::new()
            .http1_preserve_header_case(true)
            .http1_title_case_headers(true)
            .http1_keep_alive(true)
            .serve_connection(stream, service)
            .with_upgrades()
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
