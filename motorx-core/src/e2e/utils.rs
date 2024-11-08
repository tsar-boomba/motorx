use std::{
    convert::Infallible,
    future::Future,
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use bytes::Bytes;
use http::{request::Parts, Request, Response, Uri};
use http_body_util::{combinators::BoxBody, BodyExt};
use hyper::{body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use tokio::{net::TcpListener, select, sync::mpsc};

use crate::config::Upstream;

static ID_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub struct TestUpstream {
    id: usize,
    cancel_server_task: mpsc::Sender<()>,
    socket_addr: SocketAddr,
    connections_accepted: Arc<AtomicUsize>,
    connections_failed_to_accept: Arc<AtomicUsize>,
    requests_receiver: mpsc::Receiver<Request<Bytes>>,
}

impl TestUpstream {
    pub async fn new_http1<
        Fut: Future<Output = Response<BoxBody<Bytes, Infallible>>> + Send + 'static,
        H: for<'a> Fn(&'a Parts) -> Fut + Clone + Send + Sync + 'static,
    >(
        req_handler: H,
    ) -> Self {
        let socket = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let socket_addr = socket.local_addr().unwrap();
        let (cancel_server_task, mut recv_cancel) = mpsc::channel(1);
        let (requests_sender, requests_receiver) = mpsc::channel(128);
        let connections_accepted = Arc::new(AtomicUsize::new(0));
        let connections_failed_to_accept = Arc::new(AtomicUsize::new(0));

        tokio::spawn({
            let connections_accepted = connections_accepted.clone();
            let connections_failed_to_accept = connections_failed_to_accept.clone();

            async move {
                loop {
                    println!("[upstream] accept loop!");
                    select! {
                        res = socket.accept() => {
                            match res {
                                Ok((stream, _)) => {
                                    println!("[upstream] accepted conn!");
                                    connections_accepted.fetch_add(1, Ordering::Relaxed);

                                    let service = service_fn({
                                        let requests_sender = requests_sender.clone();
                                        let req_handler = req_handler.clone();

                                        move |req: Request<Incoming>| {
                                            println!("[upstream] Serving req");
                                            let requests_sender = requests_sender.clone();
                                            let req_handler = req_handler.clone();
                                            async move {
                                                let (head, body) = req.into_parts();
                                                let body_bytes = body.collect().await.unwrap().to_bytes();
                                                let res = req_handler(&head).await;
                                                requests_sender.send(Request::from_parts(head, body_bytes)).await.unwrap();
                                                println!("[upstream] Responded");
                                                Ok::<_, Infallible>(res)
                                            }
                                        }
                                    });

                                    tokio::spawn(async move {
                                        if let Err(_) = hyper::server::conn::http1::Builder::new()
                                            .keep_alive(true)
                                            .serve_connection(TokioIo::new(stream), service).await {};
                                    });
                                },
                                Err(_) => {
                                    connections_failed_to_accept.fetch_add(1, Ordering::Relaxed);
                                },
                            }
                        },
                        _ = recv_cancel.recv() => {}
                    }
                }
            }
        });

        Self {
            id: ID_COUNTER.fetch_add(1, Ordering::Relaxed),
            cancel_server_task,
            socket_addr,
            connections_accepted,
            connections_failed_to_accept,
            requests_receiver,
        }
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn socket_addr(&self) -> SocketAddr {
        self.socket_addr
    }

    pub fn uri(&self) -> Uri {
        format!("http://{}", self.socket_addr).parse().unwrap()
    }

    pub fn connections_accepted(&self) -> usize {
        self.connections_accepted.load(Ordering::Relaxed)
    }

    pub fn connections_failed_to_accept(&self) -> usize {
        self.connections_failed_to_accept.load(Ordering::Relaxed)
    }

    /// Returns a Vec of the requests this upstream has received so far
    pub async fn requests_received(&mut self) -> Vec<Request<Bytes>> {
        let mut requests = Vec::with_capacity(self.requests_receiver.len());

        while let Ok(req) = self.requests_receiver.try_recv() {
            requests.push(req);
        }

        requests
    }

    pub fn as_upstream(&self) -> Upstream {
        Upstream {
            addr: self.uri(),
            max_connections: 10,
            authentication: None,
        }
    }
}

impl Drop for TestUpstream {
    fn drop(&mut self) {
        self.cancel_server_task.try_send(()).ok();
    }
}
