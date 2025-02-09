use std::{io, net::SocketAddr, pin::Pin, sync::Arc};

use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
};

use crate::{config::Tls, Config};

pub(crate) enum Listener {
    Plain(tokio::net::TcpListener),
    #[cfg(feature = "tls")]
    FileTls(tokio::net::TcpListener, Arc<rustls::ServerConfig>),
    #[cfg(feature = "tls")]
    AcmeTls(
        rustls_acme::tokio::TokioIncoming<
            tokio_util::compat::Compat<TcpStream>,
            io::Error,
            rustls_acme::tokio::TokioIncomingTcpWrapper<
                TcpStream,
                io::Error,
                tokio_stream::wrappers::TcpListenerStream,
            >,
            io::Error,
            io::Error,
        >,
        SocketAddr,
    ),
}

pub(crate) enum Stream {
    Plain(tokio::net::TcpStream),
    #[cfg(feature = "tls")]
    FileTls(crate::tls::stream::TlsStream),
    #[cfg(feature = "tls")]
    AcmeTls(
        tokio_util::compat::Compat<
            rustls_acme::futures_rustls::server::TlsStream<
                tokio_util::compat::Compat<tokio::net::TcpStream>,
            >,
        >,
    ),
}

impl Listener {
    pub(crate) fn from_config(config: &Config) -> Result<Self, crate::Error> {
        if let Some(tls) = &config.tls {
            #[cfg(feature = "tls")]
            {
                use crate::tls;
                use rustls_acme::{caches::DirCache, AcmeConfig};
                use tokio_stream::wrappers::TcpListenerStream;

                match tls {
                    Tls::File { certs, private_key } => {
                        let tls_config = {
                            // Load public certificate.
                            let certs = tls::load_certs(certs).unwrap();

                            // Load private key.
                            let key = tls::load_private_key(private_key).unwrap();

                            rustls::crypto::ring::default_provider()
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
                        let tls_incoming = AcmeConfig::new(&**domains)
                            .cache(DirCache::new(cache_dir.clone()))
                            .directory_lets_encrypt(prod)
                            .tokio_incoming(
                                TcpListenerStream::new(listener),
                                vec![b"h2".to_vec(), b"http/1.1".to_vec()],
                            );

                        Ok(Self::AcmeTls(tls_incoming, local_addr))
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
            Listener::AcmeTls(_, local_addr) => Ok(*local_addr),
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
            Listener::AcmeTls(tokio_incoming, _) => {
                use futures_util::StreamExt;
                use tokio_util::compat::FuturesAsyncReadCompatExt;

                let stream = tokio_incoming
                    .next()
                    .await
                    .expect("Listener closed unexpectedly")?
                    .into_inner();
                let peer = stream.get_ref().0.get_ref().peer_addr()?;
                Ok((Stream::AcmeTls(stream.compat()), peer))
            }
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
            Stream::AcmeTls(tls_stream) => {
                <tokio_util::compat::Compat<
                    rustls_acme::futures_rustls::server::TlsStream<
                        tokio_util::compat::Compat<tokio::net::TcpStream>,
                    >,
                > as AsyncRead>::poll_read(Pin::new(tls_stream), cx, buf)
            }
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
            Stream::AcmeTls(tls_stream) => {
                <tokio_util::compat::Compat<
                    rustls_acme::futures_rustls::server::TlsStream<
                        tokio_util::compat::Compat<tokio::net::TcpStream>,
                    >,
                > as AsyncWrite>::poll_write(Pin::new(tls_stream), cx, buf)
            }
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
            Stream::AcmeTls(tls_stream) => {
                <tokio_util::compat::Compat<
                    rustls_acme::futures_rustls::server::TlsStream<
                        tokio_util::compat::Compat<tokio::net::TcpStream>,
                    >,
                > as AsyncWrite>::poll_flush(Pin::new(tls_stream), cx)
            }
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
            Stream::AcmeTls(tls_stream) => {
                <tokio_util::compat::Compat<
                    rustls_acme::futures_rustls::server::TlsStream<
                        tokio_util::compat::Compat<tokio::net::TcpStream>,
                    >,
                > as AsyncWrite>::poll_shutdown(Pin::new(tls_stream), cx)
            }
        }
    }
}
