use std::{io, net::SocketAddr, pin::Pin, sync::Arc, task::Poll};

use bytes::{Buf, Bytes};
use futures_util::ready;
use h3::server::RequestStream;
use http::Request;
use http_body::Frame;
use http_body_util::{combinators::BoxBody, BodyExt};
use rustls::ServerConfig;
use s2n_quic_h3::{h3, RecvStream, SendStream};

use crate::Config;

#[derive(Debug)]
pub struct Listener {
    #[allow(unused)]
    config: Arc<Config>,
    server: s2n_quic::Server,
}

pub struct H3Connection {
    conn: h3::server::Connection<s2n_quic_h3::Connection, Bytes>,
    server_name: Option<Arc<str>>,
    peer_addr: SocketAddr,
}

pub struct H3Body {
    stream: RequestStream<RecvStream, Bytes>,
    state: BodyState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyState {
    Data,
    Trailers,
}

impl Listener {
    pub fn new(config: Arc<Config>, base_server_config: &ServerConfig) -> Self {
        let default_crypto_provider = rustls::crypto::aws_lc_rs::default_provider();

        let mut cfg =
            rustls::ServerConfig::builder_with_provider(Arc::new(default_crypto_provider))
                .with_protocol_versions(&[&rustls::version::TLS13])
                .unwrap()
                .with_no_client_auth()
                .with_cert_resolver(base_server_config.cert_resolver.clone());

        cfg.ignore_client_order = true;
        cfg.max_fragment_size = None;
        cfg.alpn_protocols = vec![b"h3".to_vec(), b"h3-29".to_vec(), b"h3-32".to_vec()];

        let server = s2n_quic::Server::builder()
            .with_tls(s2n_quic::provider::tls::rustls::Server::from(cfg))
            .unwrap()
            .with_io(config.h3_addr.expect("missing h3_addr"))
            .unwrap()
            .start()
            .unwrap();

        Self { config, server }
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.server.local_addr()
    }

    pub async fn accept(&mut self) -> Result<H3Connection, crate::Error> {
        let conn = self.server.accept().await.unwrap();
        let peer_addr = conn
            .remote_addr()
            .map_err(|_| io::Error::other("missing remote addr"))?;
        tracing::trace!("QUIC connection accepted from {peer_addr}");

        Ok(H3Connection {
            peer_addr,
            server_name: conn
                .server_name()
                .map_err(|_| io::Error::other("missing remote addr"))?
                .as_deref()
                .map(Arc::from),
            conn: h3::server::builder()
                .build(s2n_quic_h3::Connection::new(conn))
                .await?,
        })
    }
}

impl H3Connection {
    pub fn peer_addr(&self) -> SocketAddr {
        self.peer_addr
    }

    pub fn server_name(&self) -> Option<Arc<str>> {
        self.server_name.clone()
    }

    /// Accept a new h3 request
    pub async fn accept(
        &mut self,
    ) -> Result<
        Option<(
            Request<BoxBody<Bytes, crate::Error>>,
            RequestStream<SendStream<Bytes>, Bytes>,
        )>,
        crate::Error,
    > {
        match self.conn.accept().await? {
            Some(resolver) => {
                let (req, stream) = resolver.resolve_request().await?;
                let (head, _) = req.into_parts();
                let (send, recv) = stream.split();
                let body = H3Body {
                    stream: recv,
                    state: BodyState::Data,
                };

                Ok(Some((Request::from_parts(head, body.boxed()), send)))
            }
            None => Ok(None),
        }
    }
}

impl http_body::Body for H3Body {
    type Data = Bytes;

    type Error = crate::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        if self.state == BodyState::Data {
            match ready!(self.stream.poll_recv_data(cx)) {
                Ok(recv_result) => match recv_result {
                    Some(mut data) => {
                        // Copying is fine since we know s2n_quic_h3 uses Bytes under the hood
                        // so the copy is actually just a ref-count increment
                        return Poll::Ready(Some(Ok(Frame::data(
                            data.copy_to_bytes(data.remaining()),
                        ))));
                    }
                    None => {
                        self.state = BodyState::Trailers;
                    }
                },
                Err(err) => return Poll::Ready(Some(Err(err.into()))),
            }
        }

        // Must now be ready for Trailers
        if self.state == BodyState::Trailers {
            match ready!(self.stream.poll_recv_trailers(cx)) {
                Ok(trailers) => if let Some(trailers) = trailers {
                    return Poll::Ready(Some(Ok(Frame::trailers(trailers))));
                },
                Err(err) => return Poll::Ready(Some(Err(err.into()))),
            }
        }

        Poll::Ready(None)
    }
}
