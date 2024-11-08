use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
    sync::Arc,
};

use http::Uri;
use hyper::{
    body::Incoming,
    client::{self, conn::http1::SendRequest},
};
use hyper_util::rt::TokioIo;
use tokio::{
    select,
    sync::{
        mpsc::{self, Receiver, Sender},
        Mutex, Semaphore,
    },
};

use crate::{cfg_logging, config::Upstream, tcp_connect};

// TODO: consider using better hash algorithm
// TODO: use SocketAddr as key instead of Uri (more efficient hash impl)
// TODO: consider making this non-static part of Server
pub(crate) type ConnPools = HashMap<Uri, Mutex<ConnPool>>;

/// Handler asks for sender (ConnPool::get_sender)
///     - if mpsc::recv is first -> use existing connection
///     - else (whichever is first):
///         - mpsc::recv -> use connection that was added back to the pool
///         - semaphore::acquire_owned -> open new connection, and pass semaphore to connection polling task
#[derive(Debug)]
pub(crate) struct ConnPool {
    /// Limit number of connections allowed to be opened at once
    semaphore: Arc<Semaphore>,
    receiver: Receiver<SendRequest<Incoming>>,
    /// Keep channel alive forever, send clones to handler so they can add sender back into queue
    sender: Sender<SendRequest<Incoming>>,
}

#[derive(Debug)]
pub(crate) struct PooledConn {
    sender: Sender<SendRequest<Incoming>>,
    conn: Option<SendRequest<Incoming>>,
}

impl ConnPool {
    pub(crate) fn new(max_connections: usize) -> Self {
        let (sender, receiver) = mpsc::channel::<SendRequest<Incoming>>(max_connections);
        ConnPool {
            semaphore: Arc::new(Semaphore::new(max_connections)),
            sender,
            receiver,
        }
    }

    pub(crate) async fn get_sender(
        &mut self,
        upstream: &Upstream,
    ) -> Result<PooledConn, crate::Error> {
        // only return if the SendRequest's underlying connection exists still
        // loop until we get a sender that meets this criteria
        loop {
            let mut conn = select! {
                biased;
                // If there is a conn in the queue already, use that first
                sender = self.receiver.recv() => {
                    cfg_logging! {trace!("Reusing connection to: {}", upstream.addr);}
                    Ok::<_, crate::Error>(sender.unwrap())
                },
                // Otherwise, check if new connections are allowed to be opened
                permit = Arc::clone(&self.semaphore).acquire_owned() => {
                    let permit = permit.unwrap();
                    cfg_logging! {info!("Opened new connection to: {}", upstream.addr);}
                    let stream = tcp_connect(upstream.addr.authority().unwrap().as_str()).await?;
                    let (sender, conn) = client::conn::http1::Builder::new()
                        .preserve_header_case(true)
                        .title_case_headers(true)
                        .handshake(TokioIo::new(stream))
                        .await?;

                    tokio::task::spawn(async move {
                        if let Err(err) = conn.await {
                            cfg_logging! {error!("Connection failed: {:?}", err);}
                        }

                        // move semaphore into this task so it is returned when connection is closed
                        drop(permit);
                    });

                    Ok(sender)
                }
            }?;

            // check that underlying conn exists
            if let Ok(_) = conn.ready().await {
                return Ok(PooledConn {
                    sender: self.sender.clone(),
                    conn: Some(conn),
                });
            }
        }
    }
}

impl Deref for PooledConn {
    type Target = SendRequest<Incoming>;

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
        if let Err(err) = self.sender.try_send(self.conn.take().unwrap()) {
            cfg_logging! {tracing::error!("Failed to send conn back to pool! {err:?}");}
        };
    }
}
