use std::{
    convert::Infallible,
    future::Future,
    io::Write,
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use bytes::Bytes;
use http::{header::UPGRADE, request::Parts, Request, Response, Uri};
use http_body_util::{combinators::BoxBody, BodyExt};
use hyper::{body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use rcgen::{CertificateParams, KeyPair};
use reqwest::Certificate;
use tempfile::NamedTempFile;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    select,
    sync::mpsc,
};
use tracing_subscriber::EnvFilter;

use crate::{
    config::{match_type::MatchType, Upstream},
    Rule,
};

static ID_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub struct TestUpstream {
    id: usize,
    cancel_server_task: mpsc::Sender<()>,
    socket_addr: SocketAddr,
    connections_accepted: Arc<AtomicUsize>,
    connections_failed_to_accept: Arc<AtomicUsize>,
    requests_receiver: mpsc::UnboundedReceiver<Request<Bytes>>,
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
        let (requests_sender, requests_receiver) = mpsc::unbounded_channel();
        let connections_accepted = Arc::new(AtomicUsize::new(0));
        let connections_failed_to_accept = Arc::new(AtomicUsize::new(0));

        tokio::spawn({
            let connections_accepted = connections_accepted.clone();
            let connections_failed_to_accept = connections_failed_to_accept.clone();

            async move {
                loop {
                    select! {
                        res = socket.accept() => {
                            match res {
                                Ok((stream, _)) => {
                                    connections_accepted.fetch_add(1, Ordering::Relaxed);

                                    let service = service_fn({
                                        let requests_sender = requests_sender.clone();
                                        let req_handler = req_handler.clone();

                                        move |req: Request<Incoming>| {
                                            let requests_sender = requests_sender.clone();
                                            let req_handler = req_handler.clone();
                                            async move {
                                                let (head, req, body_bytes) = {
                                                    let (head, body) = req.into_parts();
                                                    let body_bytes = body.collect().await.unwrap().to_bytes();
                                                    (head.clone(), Request::from_parts(head, body_bytes.clone()), body_bytes)
                                                };

                                                if head.headers.contains_key(UPGRADE) {
                                                    tokio::spawn(async move {
                                                        match hyper::upgrade::on(req).await {
                                                            Ok(upgraded) => {
                                                                let mut conn = TokioIo::new(upgraded);
                                                                conn.write_all(b"hello").await.unwrap();
                                                                let mut buf = vec![0; 128];
                                                                loop {
                                                                    let num_read = conn.read(&mut buf).await.unwrap();
                                                                }
                                                            },
                                                            Err(err) => {
                                                                eprintln!("Failed to upgrade: {err:?}")
                                                            },
                                                        }
                                                    });
                                                }

                                                let res = req_handler(&head).await;

                                                requests_sender.send(Request::from_parts(head, body_bytes)).unwrap();

                                                Ok::<_, Infallible>(res)
                                            }
                                        }
                                    });

                                    tokio::spawn(async move {
                                        if let Err(_) = hyper::server::conn::http1::Builder::new()
                                            .serve_connection(TokioIo::new(stream), service)
                                            .with_upgrades()
                                            .await {};
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

    pub fn as_upstream(&self) -> Arc<Upstream> {
        Arc::new(Upstream {
            addr: self.uri(),
            max_connections: 10,
            authentication: None,
            key: 0,
        })
    }
}

impl Drop for TestUpstream {
    fn drop(&mut self) {
        self.cancel_server_task.try_send(()).ok();
    }
}

pub fn tracing() {
    static INITIALIZED: AtomicBool = AtomicBool::new(false);

    if !INITIALIZED.swap(true, Ordering::Relaxed) {
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .init();
    }
}

pub fn start_rule(starts_with: &str, upstream: &TestUpstream, remove_match: bool) -> Rule {
    Rule {
        path: MatchType::Start(starts_with.into()),
        remove_match,
        match_headers: None,
        upstream: upstream.id().to_string(),
        cache: None,
        cache_key: 0,
        upstream_key: 0,
    }
}

pub fn base_client() -> reqwest::ClientBuilder {
    reqwest::ClientBuilder::new().timeout(Duration::from_secs(1))
}

pub fn client() -> reqwest::Client {
    base_client().build().unwrap()
}

pub fn http2_client() -> reqwest::Client {
    base_client().http2_prior_knowledge().build().unwrap()
}

pub fn base_tls_client(cert_pem: String) -> reqwest::ClientBuilder {
    base_client().add_root_certificate(Certificate::from_pem(cert_pem.as_bytes()).unwrap())
}

pub fn tls_client(cert_pem: String) -> reqwest::Client {
    base_tls_client(cert_pem).build().unwrap()
}

pub fn http2_tls_client(cert_pem: String) -> reqwest::Client {
    base_tls_client(cert_pem)
        .http2_prior_knowledge()
        .build()
        .unwrap()
}

pub struct CertKeyFiles {
    pub cert_file: NamedTempFile,
    pub key_file: NamedTempFile,
}

pub fn gen_self_signed() -> CertKeyFiles {
    let key_pair = KeyPair::generate().unwrap();
    let cert = CertificateParams::new(["localhost".into()])
        .unwrap()
        .self_signed(&key_pair)
        .unwrap();

    let mut cert_file = NamedTempFile::new().unwrap();
    cert_file.write_all(cert.pem().as_bytes()).unwrap();

    let mut key_file = NamedTempFile::new().unwrap();
    println!("{}", key_pair.serialize_pem());
    key_file
        .write_all(key_pair.serialize_pem().trim().as_bytes())
        .unwrap();

    CertKeyFiles {
        cert_file,
        key_file,
    }
}
