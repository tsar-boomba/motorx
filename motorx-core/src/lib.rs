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
#[cfg(feature = "h3")]
mod h3;
mod listener;
#[cfg(feature = "tls")]
pub mod tls;

#[cfg_attr(feature = "logging", macro_use(info, error, debug, trace))]
#[cfg(feature = "logging")]
extern crate tracing;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use cache::Cache;
use config::Upstream;
use conn_pool::Pool;
use futures_util::future::join;
use futures_util::TryStreamExt;
use handle::handle_req;
use http::header::ALT_SVC;
use http::{HeaderValue, Response};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::Request;
use hyper_util::rt::{TokioExecutor, TokioIo};
use listener::Listener;
#[cfg(feature = "tls")]
use tls::stream::TlsStream;
use tokio::io::{AsyncRead, AsyncWrite, BufStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub use config::{CacheSettings, Config, Rule};
pub use error::Error;

// TODO: Consider Boxing this (Or just ConnPool) to improve spacial locality
type UpstreamAndConnPool = (Arc<Upstream>, Pool);
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
    listener: Mutex<Option<Listener>>,
    #[cfg(feature = "h3")]
    h3_listener: Mutex<Option<h3::Listener>>,
    /// Used to enforce max num of connections to this server
    semaphore: Arc<Semaphore>,
}

impl Server {
    pub fn new(mut config: Config) -> Result<Self, Error> {
        let upstreams = Arc::new(init_upstreams(&mut config));
        let cache = Arc::new(Cache::from_config(&mut config));

        config.rules.sort_by(|a, b| a.path.cmp(&b.path));
        let config = Arc::new(config);
        let listener = Listener::from_config(&config)?;

        #[cfg(feature = "h3")]
        let h3_listener = Mutex::new({
            if let Some(_) = config.h3_addr {
                // Only start h3 if the address is set and TLS is enabled

                if let Some(server_config) = listener.server_config() {
                    Some(h3::Listener::new(config.clone(), &server_config))
                } else {
                    tracing::warn!("Not starting h3 server since TLS isn't enabled.");
                    None
                }
            } else {
                tracing::warn!("No address for h3 server.");
                None
            }
        });

        cfg_logging! {debug!("Starting with config: {:#?}", *config);}

        cfg_logging! {
            info!("Motorx proxy listening on http://{}", {
                listener.local_addr().unwrap()
            });
        }

        Ok(Self {
            semaphore: Arc::new(Semaphore::new(config.max_connections)),
            cache,
            upstreams,
            config,
            listener: Mutex::new(Some(listener)),
            #[cfg(feature = "h3")]
            h3_listener,
        })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener
            .lock()
            .unwrap()
            .as_ref()
            .expect("cannot call after run")
            .local_addr()
    }

    pub fn h3_local_addr(&self) -> Option<std::io::Result<SocketAddr>> {
        self.h3_listener
            .lock()
            .unwrap()
            .as_ref()
            .map(|listener| listener.local_addr())
    }

    pub async fn run(self) -> Result<(), crate::Error> {
        let tcp_task = self.run_tcp();
        let h3_task = self.run_h3();

        let (tcp_task_res, h3_task_res) = join(tcp_task, h3_task).await;

        match (tcp_task_res, h3_task_res) {
            (Ok(_), Ok(_)) => Ok(()),
            (Ok(_), Err(err)) => {
                tracing::error!("TCP serve failed with: {err:?}");
                Err(err)
            }
            (Err(err), Ok(_)) => {
                tracing::error!("h3 serve failed with: {err:?}");
                Err(err)
            }
            (Err(tcp_err), Err(h3_err)) => {
                tracing::error!("TCP serve failed with: {tcp_err:?}");
                tracing::error!("h3 serve failed with: {h3_err:?}");
                Err(tcp_err)
            }
        }
    }

    async fn run_tcp(&self) -> Result<(), crate::Error> {
        let mut listener = self
            .listener
            .lock()
            .unwrap()
            .take()
            .expect("cannot call run twice");
        loop {
            if let Ok(permit) = self.semaphore.clone().acquire_owned().await {
                match listener.accept().await {
                    Ok((stream, peer_addr)) => {
                        cfg_logging! {
                            trace!("Accepted connection from {}", peer_addr);
                        }
                        let domain = stream.domain();

                        handle_connection(
                            BufStream::with_capacity(8 * 1024, 8 * 1024, stream),
                            peer_addr,
                            domain,
                            Arc::clone(&self.config),
                            Arc::clone(&self.cache),
                            Arc::clone(&self.upstreams),
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

    async fn run_h3(&self) -> Result<(), crate::Error> {
        let Some(mut h3_listener) = self.h3_listener.lock().unwrap().take() else {
            tracing::info!("Not starting h3 server.");
            return Ok(());
        };
        tracing::info!("h3 on: https://{}", h3_listener.local_addr()?);

        loop {
            if let Ok(permit) = self.semaphore.clone().acquire_owned().await {
                let mut conn = match h3_listener.accept().await {
                    Ok(conn) => conn,
                    Err(err) => {
                        tracing::error!("Error accepting h3 conn: {err:?}");
                        println!("h3 connection failed: {err:?}");
                        continue;
                    }
                };
                let peer_addr = conn.peer_addr();
                let server_name = conn.server_name();
                let config = self.config.clone();
                let cache = self.cache.clone();
                let upstreams = self.upstreams.clone();

                tokio::spawn(async move {
                    loop {
                        let (req, mut send_res) = match conn.accept().await {
                            Ok(Some((req, send_res))) => (req, send_res),
                            Ok(None) => break,
                            Err(err) => {
                                tracing::error!("Error handling h3 conn: {err:?}");
                                break;
                            }
                        };
                        let server_name = server_name.clone();
                        let config = config.clone();
                        let cache = cache.clone();
                        let upstreams = upstreams.clone();

                        tokio::spawn(async move {
                            let res = match handle_req(
                                req,
                                peer_addr,
                                server_name,
                                config,
                                cache,
                                upstreams,
                            )
                            .await
                            {
                                Ok(res) => res,
                                Err(err) => {
                                    tracing::error!("Error handling request: {err:?}");
                                    return Ok::<_, crate::Error>(());
                                }
                            };

                            let (head, body) = res.into_parts();
                            send_res
                                .send_response(Response::from_parts(head, ()))
                                .await?;
                            let mut body_stream = body.into_data_stream();

                            while let Some(frame) = body_stream.try_next().await? {
                                send_res.send_data(frame).await?;
                            }

                            send_res.finish().await.map_err(Into::into)
                        });
                    }

                    drop(permit);
                });
            }
        }
    }
}

#[cfg_attr(
    feature = "logging",
    tracing::instrument(skip(stream, config, cache, conn_pools, permit))
)]
fn handle_connection<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    stream: S,
    peer_addr: SocketAddr,
    domain: Option<Arc<str>>,
    config: Arc<Config>,
    cache: Arc<Cache>,
    conn_pools: Arc<Upstreams>,
    permit: OwnedSemaphorePermit,
) {
    let h3_port = config.h3_addr.map(|s| s.port());
    let service = service_fn({
        move |req: Request<Incoming>| {
            let domain = domain.clone();
            let config = config.clone();
            let cache = cache.clone();
            let conn_pools = conn_pools.clone();

            async move {
                let mut res = handle::handle_req(
                    req.map(|incoming| incoming.map_err(Error::from).boxed()),
                    peer_addr,
                    domain,
                    Arc::clone(&config),
                    Arc::clone(&cache),
                    Arc::clone(&conn_pools),
                )
                .await;

                cfg_logging! {
                    trace!("Responded to req from {}", peer_addr);
                }

                #[cfg(feature = "h3")]
                {
                    // add alt-svc header so client know we support h3
                    // TODO: make the max-age, persist configurable. I'm never turning it off so I don't care
                    if config.will_start_h3() {
                        res = res.map(|mut res| {
                            res.headers_mut().insert(
                                ALT_SVC,
                                HeaderValue::try_from(format!("h3=\":{}\"; ma=2592000; persist=1", h3_port.unwrap()))
                                    .unwrap(),
                            );
                            res
                        });
                    }
                }

                res
            }
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
            Pool::new(
                upstream.addr.authority().unwrap().clone(),
                upstream.max_connections,
                upstream.buffer_size,
                upstream.proto,
                10, // TODO: make configurable
            ),
        ));
    }

    upstreams.shrink_to_fit();

    upstreams
}
