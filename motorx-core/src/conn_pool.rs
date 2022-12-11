use std::{collections::HashMap, sync::Arc};

use http::Uri;
use hyper::{
    body::Incoming,
    client::{self, conn::http1::SendRequest},
};
use once_cell::sync::OnceCell;
use tokio::{
    net::TcpStream,
    select,
    sync::{
        mpsc::{self, Receiver, Sender},
        Mutex, Semaphore,
    },
};

use crate::{
    cfg_logging,
    config::{Config, Upstream},
};

pub(crate) static CONN_POOLS: OnceCell<HashMap<Uri, Arc<Mutex<ConnPool>>>> = OnceCell::new();

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

impl ConnPool {
    pub(crate) async fn get_sender(
        &mut self,
        upstream: &Upstream,
    ) -> Result<(Sender<SendRequest<Incoming>>, SendRequest<Incoming>), crate::Error> {
        // only return if the SendRequest's underlying connection exists still
        // loop until we get a sender that meets this criteria
        loop {
            let mut sender = select! {
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
                    let stream = TcpStream::connect(upstream.addr.authority().unwrap().to_string()).await?;
                    let (sender, conn) = client::conn::http1::Builder::new()
                        .http1_preserve_header_case(true)
                        .http1_title_case_headers(true)
                        .handshake(stream)
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
            if let Ok(_) = sender.ready().await {
                return Ok((self.sender.clone(), sender))
            }
        }
    }
}

pub(crate) fn init_conn_pools(config: &Config) {
    CONN_POOLS
        .set(HashMap::from_iter(config.upstreams.values().map(|v| {
            let (sender, receiver) = mpsc::channel::<SendRequest<Incoming>>(v.max_connections);
            (
                v.addr.clone(),
                Arc::new(Mutex::new(ConnPool {
                    semaphore: Arc::new(Semaphore::new(v.max_connections)),
                    sender,
                    receiver,
                })),
            )
        })))
        .unwrap();
}
