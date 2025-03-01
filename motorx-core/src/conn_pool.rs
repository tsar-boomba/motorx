use tokio::{io::BufReader, net::TcpStream, sync::OwnedSemaphorePermit};

use crate::{config::Proto, error::Error};

use std::{
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

use http::{uri::Authority, Request, Response};
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use tokio::{
    select,
    sync::{
        mpsc::{self, Receiver, Sender},
        Mutex, Semaphore,
    },
};
use tracing::{info_span, Instrument};

/// Pools connections for a certain host. Only pools tls connections, passthrough connection don't use pooling (right now)
///
/// Handler asks for sender (ConnPool::get_sender)
///     - if mpsc::recv is first -> use existing connection
///     - else (whichever is first):
///         - mpsc::recv -> use connection that was added back to the pool
///         - semaphore::acquire_owned -> open new connection, and pass semaphore to connection polling task
#[derive(Debug)]
pub struct Pool {
    /// Limit number of connections allowed to be opened at once
    semaphore: Arc<Semaphore>,
    receiver: Mutex<Receiver<SendRequest>>,
    /// Keep channel alive forever, send clones to handler so they can add sender back into queue
    sender: Sender<SendRequest>,
    authority: Authority,
    max_buffer_size: usize,
    h2_max_streams: usize,
    proto: Proto,
}

#[derive(Debug, Clone)]
pub struct Http2SendRequest {
    send_request: hyper::client::conn::http2::SendRequest<Incoming>,
    streams: Arc<AtomicUsize>,
    conn_in_pool: Arc<AtomicBool>,
}

#[derive(Debug)]
pub enum SendRequest {
    Http1(hyper::client::conn::http1::SendRequest<Incoming>),
    Http2(Http2SendRequest),
}

#[derive(Debug)]
pub struct PooledConn {
    /// For sending connection back to the pool
    sender: Option<Sender<SendRequest>>,
    conn: Option<SendRequest>,
}

impl Pool {
    pub fn new(
        authority: Authority,
        max_connections: usize,
        max_buffer_size: usize,
        proto: Proto,
        h2_max_streams: usize,
    ) -> Self {
        let (sender, receiver) = mpsc::channel::<SendRequest>(max_connections);
        Pool {
            semaphore: Arc::new(Semaphore::new(max_connections)),
            h2_max_streams,
            sender,
            receiver: Mutex::new(receiver),
            authority,
            max_buffer_size,
            proto
        }
    }

    pub async fn get_sender(&self) -> Result<PooledConn, crate::Error> {
        // only return if the SendRequest's underlying connection exists still
        // loop until we get a send_request that meets this criteria
        let mut receiver = self.receiver.lock().await;
        loop {
            let semaphore = self.semaphore.clone();
            let conn = select! {
                biased;
                // If there is a conn in the queue already, use that first
                send_req = receiver.recv() => {
                    match self.validate_sender(send_req.unwrap()).await? {
                        Some(send_req) => {
                            tracing::trace!(
                                "Reusing {} connection to: {}",
                                send_req.proto(),
                                self.authority
                            );

                            send_req
                        },
                        // Handle case where h2 conn cannot support any more streams
                        None => continue
                    }
                }
                // Otherwise, check if new connections are allowed to be opened
                permit = semaphore.acquire_owned() => {
                    // TODO isaiah: consider some kinda "inflight" mechanism here so waiters know a new connection is in progress of being opened
                    //              to stop concurrent requests from opening a lot of new connections while still letting them wait for new pool conns
                    let send_req = self.new_connection(Some(permit.unwrap()), self.proto).await?;

                    // Call validate_sender on new conns so that if its h2, it will be returned to the pool immediately for other waiters
                    match self.validate_sender(send_req).await? {
                        Some(send_req) => send_req,
                        // Handle case where h2 conn cannot support any more streams
                        None => continue
                    }
                },
            };

            // check that underlying conn exists before returning
            if !conn.is_closed() {
                return Ok(PooledConn {
                    sender: Some(self.sender.clone()),
                    conn: Some(conn),
                });
            }
        }
    }

    async fn validate_sender(
        &self,
        sender: SendRequest,
    ) -> Result<Option<SendRequest>, crate::Error> {
        let sender = match sender {
            SendRequest::Http2(http2_sender) => {
                let curr_num_streams = http2_sender.streams.fetch_add(1, Ordering::Relaxed);
                if curr_num_streams >= self.h2_max_streams {
                    // We can't take any more streams from this conn, undo the addition
                    http2_sender.streams.fetch_sub(1, Ordering::Relaxed);
                    tracing::debug!("Can't get more streams from h2 conn");
                    return Ok(None);
                } else if curr_num_streams == self.h2_max_streams - 1 {
                    // This is the last stream we can take from this conn
                    http2_sender.conn_in_pool.store(false, Ordering::Relaxed);
                    tracing::debug!("Exhausted streams for h2 conn");
                    SendRequest::Http2(http2_sender)
                } else {
                    // We can clone out of this conn and return the original to the pool
                    let return_sender = http2_sender.clone();
                    self.sender
                        .send(SendRequest::Http2(http2_sender))
                        .await
                        .unwrap();
                    SendRequest::Http2(return_sender)
                }
            }
            SendRequest::Http1(http1_sender) => SendRequest::Http1(http1_sender),
        };

        Ok(Some(sender))
    }

    /// Bypass the pool and open a new connection. To be used for upgrading requests which can hold onto connections for a while
    pub async fn new_connection(
        &self,
        permit: Option<OwnedSemaphorePermit>,
        proto: Proto,
    ) -> Result<SendRequest, crate::Error> {
        tracing::trace!("Opening new connection to: {}", self.authority);

        let stream = BufReader::with_capacity(
            self.max_buffer_size,
            connect_to_upstream(
                self.authority.host(),
                self.authority.port_u16().unwrap_or(80),
            )
            .await?,
        );

        let send_req = match proto {
            Proto::Http1 => {
                let (send_req, conn) = hyper::client::conn::http1::Builder::new()
                    .preserve_header_case(true)
                    .handshake::<_, Incoming>(TokioIo::new(stream))
                    .await?;

                let conn_span = info_span!("http1_conn_driver");
                tokio::task::spawn(
                    async move {
                        if let Err(err) = conn.with_upgrades().await {
                            tracing::error!("HTTP/1.1 Connection failed: {:?}", err);
                        }

                        // move semaphore into this task so it is returned when connection is closed
                        drop(permit);
                    }
                    .instrument(conn_span),
                );

                SendRequest::Http1(send_req)
            }
            Proto::Http2 => {
                let (send_request, conn) =
                    hyper::client::conn::http2::Builder::new(TokioExecutor::new())
                        .initial_max_send_streams(self.h2_max_streams)
                        .timer(TokioTimer::new())
                        .max_send_buf_size(self.max_buffer_size)
                        .handshake::<_, Incoming>(TokioIo::new(stream))
                        .await?;

                let conn_span = info_span!("h2_conn_driver");
                tokio::task::spawn(
                    async move {
                        if let Err(err) = conn.await {
                            tracing::error!("HTTP2 Connection failed: {:?}", err);
                        }

                        // move semaphore into this task so it is returned when connection is closed
                        drop(permit);
                    }
                    .instrument(conn_span),
                );

                SendRequest::Http2(Http2SendRequest {
                    send_request,
                    streams: Arc::new(AtomicUsize::new(1)),
                    conn_in_pool: Arc::new(AtomicBool::new(true)),
                })
            }
        };

        tracing::debug!(
            "Opened new {} connection to: {}",
            send_req.proto(),
            self.authority
        );

        Ok(send_req)
    }
}

impl SendRequest {
    pub async fn ready(&mut self) -> Result<(), hyper::Error> {
        match self {
            SendRequest::Http1(send_request) => send_request.ready().await,
            SendRequest::Http2(send_request) => send_request.ready().await,
        }
    }

    pub fn is_closed(&self) -> bool {
        match self {
            SendRequest::Http1(send_request) => send_request.is_closed(),
            SendRequest::Http2(send_request) => send_request.is_closed(),
        }
    }

    pub async fn send_request(
        &mut self,
        req: Request<Incoming>,
    ) -> Result<Response<Incoming>, hyper::Error> {
        match self {
            SendRequest::Http1(send_request) => send_request.send_request(req).await,
            SendRequest::Http2(send_request) => send_request.send_request(req).await,
        }
    }

    pub fn proto(&self) -> &'static str {
        match self {
            SendRequest::Http1(_) => "http/1.1",
            SendRequest::Http2(_) => "h2",
        }
    }
}

impl Deref for PooledConn {
    type Target = SendRequest;

    fn deref(&self) -> &Self::Target {
        self.conn.as_ref().unwrap()
    }
}

impl DerefMut for PooledConn {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.conn.as_mut().unwrap()
    }
}

impl Drop for PooledConn {
    fn drop(&mut self) {
        let conn = self.conn.take().unwrap();
        let send_back_to_pool = match &conn {
            SendRequest::Http1(send_request) => !send_request.is_closed(),
            SendRequest::Http2(http2_send_request) => {
                // Always decrement the stream count, whether or not we send the conn back
                http2_send_request.streams.fetch_sub(1, Ordering::Relaxed);

                !http2_send_request.is_closed()
                    && http2_send_request
                        .conn_in_pool
                        // Completely atomically, if conn_in_pool is currently false, set to true
                        .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                        // The compare_exchange was successful, meaning conn_in_pool was false and is now set to true
                        .is_ok()
            }
        };

        tracing::trace!(
            "Dropped {} conn, sending back to pool: {}",
            conn.proto(),
            send_back_to_pool
        );

        if send_back_to_pool {
            // Only try to send back to pool if the underlying connection is still open
            // and for h2, theres no SendRequest in the pool for the underlying conn

            if let Some(sender) = self.sender.as_ref() {
                if let Err(err) = sender.try_send(conn) {
                    tracing::error!("Failed to send conn back to pool! {err:?}");
                };
            };
        };
    }
}

impl Deref for Http2SendRequest {
    type Target = hyper::client::conn::http2::SendRequest<Incoming>;

    fn deref(&self) -> &Self::Target {
        &self.send_request
    }
}

impl DerefMut for Http2SendRequest {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.send_request
    }
}

pub async fn connect_to_upstream(host: &str, port: u16) -> Result<TcpStream, Error> {
    tracing::trace!("Resolving address for {host}:{port}");
    // TODO: consider replacing hot path formats with ufmt or faster formatter
    let mut addrs = tokio::net::lookup_host(format!("{host}:{port}"))
        .await?
        .peekable();

    if addrs.peek().is_none() {
        tracing::error!("Couldn't find any addresses for host: {host}");
        return Err(Error::InvalidHost);
    }

    let upstream_stream = loop {
        match addrs.next() {
            Some(addr) => {
                tracing::trace!("Attempting to connect to {host}:{port} at {addr}");
                match TcpStream::connect(addr).await {
                    Ok(stream) => {
                        tracing::trace!("Connected to {host}:{port} at {addr}");
                        break stream;
                    }
                    Err(err) => {
                        tracing::error!("Failed to connect to {host} at {addr}: {err:?}");
                    }
                }
            }
            None => {
                tracing::error!("Couldn't connect to any {host} addresses");
                return Err(Error::FailedConnection);
            }
        }
    };

    Ok(upstream_stream)
}
