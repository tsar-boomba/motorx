use std::{io, net::SocketAddr, pin::Pin, sync::Arc};

use futures_util::StreamExt;
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use crate::{config::Tls, Config};

pub(crate) enum Listener {
    Plain(tokio::net::TcpListener),
    #[cfg(feature = "tls")]
    FileTls(tokio::net::TcpListener, Arc<rustls::ServerConfig>),
    #[cfg(feature = "tls")]
    AcmeTls {
        listener: TcpListener,
        challenge_config: Arc<rustls::ServerConfig>,
        server_config: Arc<rustls::ServerConfig>,
        local_addr: SocketAddr,
    },
}

pub(crate) enum Stream {
    Plain(tokio::net::TcpStream),
    #[cfg(feature = "tls")]
    FileTls(crate::tls::stream::TlsStream),
    #[cfg(feature = "tls")]
    AcmeTls(tokio_rustls::server::TlsStream<TcpStream>, Option<Arc<str>>),
}

impl Listener {
    pub(crate) fn from_config(config: &Config) -> Result<Self, crate::Error> {
        if let Some(tls) = &config.tls {
            #[cfg(feature = "tls")]
            {
                use crate::tls;
                use rustls_acme::{caches::DirCache, AcmeConfig};

                match tls {
                    Tls::File { certs, private_key } => {
                        let tls_config = {
                            // Load public certificate.
                            let certs = tls::load_certs(certs).unwrap();

                            // Load private key.
                            let key = tls::load_private_key(private_key).unwrap();

                            rustls::crypto::aws_lc_rs::default_provider()
                                .install_default()
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

                        Ok(Self::FileTls(crate::tcp_listener(config.addr)?, tls_config))
                    }
                    Tls::Acme { domains, cache_dir } => {
                        let listener = crate::tcp_listener(config.addr)?;
                        let local_addr = listener.local_addr()?;
                        let prod = !domains.contains(&"localhost".to_string());
                        let mut state = AcmeConfig::new(&**domains)
                            .cache(DirCache::new(cache_dir.clone()))
                            .directory_lets_encrypt(prod)
                            .state();
                        let challenge_config = state.challenge_rustls_config();
                        let mut server_config = (&*state.default_rustls_config()).clone();
                        server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

                        tokio::spawn(async move {
                            loop {
                                match state.next().await.unwrap() {
                                    Ok(ok) => tracing::debug!("event: {:?}", ok),
                                    Err(err) => tracing::error!("error: {:?}", err),
                                }
                            }
                        });

                        Ok(Self::AcmeTls {
                            listener,
                            challenge_config,
                            server_config: Arc::new(server_config),
                            local_addr,
                        })
                    }
                }
            }

            #[cfg(not(feature = "tls"))]
            Ok(Self::Plain(crate::tcp_listener(config.addr)?))
        } else {
            Ok(Self::Plain(crate::tcp_listener(config.addr)?))
        }
    }

    pub(crate) fn local_addr(&self) -> io::Result<SocketAddr> {
        match self {
            Listener::Plain(tcp_listener) => tcp_listener.local_addr(),
            #[cfg(feature = "tls")]
            Listener::FileTls(tcp_listener, _) => tcp_listener.local_addr(),
            #[cfg(feature = "tls")]
            Listener::AcmeTls { local_addr, .. } => Ok(*local_addr),
        }
    }

    #[cfg(feature = "tls")]
    pub(crate) fn server_config(&self) -> Option<Arc<rustls::ServerConfig>> {
        match self {
            Listener::Plain(_) => None,
            Listener::FileTls(_, server_config) => Some(server_config.clone()),
            Listener::AcmeTls {
                listener: _,
                challenge_config: _,
                server_config,
                local_addr: _,
            } => Some(server_config.clone()),
        }
    }

    pub(crate) async fn accept(&mut self) -> io::Result<(Stream, SocketAddr)> {
        match self {
            Listener::Plain(tcp_listener) => tcp_listener
                .accept()
                .await
                .map(|(s, peer)| (Stream::Plain(s), peer)),
            #[cfg(feature = "tls")]
            Listener::FileTls(tcp_listener, server_config) => {
                let (tcp_stream, peer) = tcp_listener.accept().await?;
                let tls_stream =
                    crate::tls::stream::TlsStream::new(tcp_stream, server_config.clone());
                Ok((Stream::FileTls(tls_stream), peer))
            }
            #[cfg(feature = "tls")]
            Listener::AcmeTls {
                listener,
                challenge_config,
                server_config,
                local_addr: _local_addr,
            } => loop {
                tracing::trace!("Accepting conenction with ACME...");
                let (stream, peer) = listener.accept().await?;

                let start_handshake =
                    tokio_rustls::LazyConfigAcceptor::new(Default::default(), stream).await?;

                if rustls_acme::is_tls_alpn_challenge(&start_handshake.client_hello()) {
                    tracing::info!("received TLS-ALPN-01 validation request");
                    let mut tls = start_handshake
                        .into_stream(challenge_config.clone())
                        .await?;
                    tokio::spawn(async move {
                        if let Err(err) = tls.shutdown().await {
                            tracing::error!("Error in ACME challenge conn: {err:?}")
                        };
                    });
                } else {
                    tracing::trace!("Accepting TLS connection...");
                    let domain = start_handshake.client_hello().server_name().map(Arc::from);
                    let tls = start_handshake.into_stream(server_config.clone()).await?;

                    return Ok((Stream::AcmeTls(tls, domain), peer));
                }
            },
        }
    }
}

impl Stream {
    pub fn domain(&self) -> Option<Arc<str>> {
        match self {
            Stream::Plain(_) => None,
            Stream::FileTls(_) => None,
            Stream::AcmeTls(_, domain) => domain.clone(),
        }
    }
}

impl AsyncRead for Stream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(tcp_stream) => {
                <tokio::net::TcpStream as AsyncRead>::poll_read(Pin::new(tcp_stream), cx, buf)
            }
            #[cfg(feature = "tls")]
            Stream::FileTls(tls_stream) => {
                <crate::TlsStream as tokio::io::AsyncRead>::poll_read(Pin::new(tls_stream), cx, buf)
            }
            #[cfg(feature = "tls")]
            Stream::AcmeTls(tls_stream, _) => Pin::new(tls_stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, io::Error>> {
        match self.get_mut() {
            Stream::Plain(tcp_stream) => {
                <tokio::net::TcpStream as AsyncWrite>::poll_write(Pin::new(tcp_stream), cx, buf)
            }
            #[cfg(feature = "tls")]
            Stream::FileTls(tls_stream) => <crate::TlsStream as tokio::io::AsyncWrite>::poll_write(
                Pin::new(tls_stream),
                cx,
                buf,
            ),
            #[cfg(feature = "tls")]
            Stream::AcmeTls(tls_stream, _) => Pin::new(tls_stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), io::Error>> {
        match self.get_mut() {
            Stream::Plain(tcp_stream) => {
                <tokio::net::TcpStream as AsyncWrite>::poll_flush(Pin::new(tcp_stream), cx)
            }
            #[cfg(feature = "tls")]
            Stream::FileTls(tls_stream) => {
                <crate::TlsStream as tokio::io::AsyncWrite>::poll_flush(Pin::new(tls_stream), cx)
            }
            #[cfg(feature = "tls")]
            Stream::AcmeTls(tls_stream, _) => Pin::new(tls_stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), io::Error>> {
        match self.get_mut() {
            Stream::Plain(tcp_stream) => {
                <tokio::net::TcpStream as AsyncWrite>::poll_shutdown(Pin::new(tcp_stream), cx)
            }
            #[cfg(feature = "tls")]
            Stream::FileTls(tls_stream) => {
                <crate::TlsStream as tokio::io::AsyncWrite>::poll_shutdown(Pin::new(tls_stream), cx)
            }
            #[cfg(feature = "tls")]
            Stream::AcmeTls(tls_stream, _) => Pin::new(tls_stream).poll_shutdown(cx),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Stream::Plain(tcp_stream) => tcp_stream.is_write_vectored(),
            Stream::FileTls(tls_stream) => tls_stream.is_write_vectored(),
            Stream::AcmeTls(tls_stream, _) => tls_stream.is_write_vectored(),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> std::task::Poll<Result<usize, io::Error>> {
        match self.get_mut() {
            Stream::Plain(tcp_stream) => Pin::new(tcp_stream).poll_write_vectored(cx, bufs),
            Stream::FileTls(tls_stream) => Pin::new(tls_stream).poll_write_vectored(cx, bufs),
            Stream::AcmeTls(tls_stream, _) => Pin::new(tls_stream).poll_write_vectored(cx, bufs),
        }
    }
}
