use std::{
    io,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use futures_util::StreamExt;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};

use crate::Config;

pub(crate) struct Listener {
    listener: TcpListener,
    proxy_protocol: bool,
    tls: ListenerTls,
}

pub(crate) enum Stream {
    Plain(tokio::net::TcpStream),
    #[cfg(feature = "tls")]
    FileTls(crate::tls::stream::TlsStream),
    #[cfg(feature = "tls")]
    AcmeTls(tokio_rustls::server::TlsStream<TcpStream>, Option<Arc<str>>),
}

/// An accepted connection that hasn't completed its PROXY protocol/TLS
/// handshakes yet. Completed by [`Incoming::handshake`].
pub(crate) struct Incoming {
    stream: TcpStream,
    peer: SocketAddr,
    proxy_protocol: bool,
    tls: ListenerTls,
}

/// TLS setup shared by every listener (and the h3 listener).
///
/// Must be built exactly once per server via [`ListenerTls::from_config`]:
/// for ACME, each instance runs its own order with its own challenge certs, so
/// multiple instances would serve each other's validation certs and fail.
#[derive(Clone)]
pub(crate) enum ListenerTls {
    None,
    #[cfg(feature = "tls")]
    File(Arc<rustls::ServerConfig>),
    #[cfg(feature = "tls")]
    Acme {
        challenge_config: Arc<rustls::ServerConfig>,
        server_config: Arc<rustls::ServerConfig>,
    },
}

impl ListenerTls {
    pub(crate) fn from_config(config: &Config) -> Self {
        let Some(tls) = &config.tls else {
            return Self::None;
        };

        #[cfg(feature = "tls")]
        {
            use crate::{config::Tls, tls};
            use rustls_acme::{caches::DirCache, AcmeConfig};

            match tls {
                Tls::File { certs, private_key } => {
                    // Load public certificate.
                    let certs = tls::load_certs(certs).unwrap();

                    // Load private key.
                    let key = tls::load_private_key(private_key).unwrap();

                    rustls::crypto::aws_lc_rs::default_provider()
                        .install_default()
                        .ok();

                    // Do not use client certificate authentication.
                    let mut cfg = rustls::ServerConfig::builder()
                        .with_no_client_auth()
                        .with_single_cert(certs, key)
                        .unwrap();

                    // Configure ALPN to accept HTTP/2, HTTP/1.1 in that order.
                    cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

                    Self::File(Arc::new(cfg))
                }
                Tls::Acme { domains, cache_dir } => {
                    let prod = !domains.contains(&"localhost".to_string());
                    let mut state = AcmeConfig::new(&**domains)
                        .cache(DirCache::new(cache_dir.clone()))
                        .directory_lets_encrypt(prod)
                        .state();
                    let challenge_config = state.challenge_rustls_config();
                    let mut server_config = (*state.default_rustls_config()).clone();
                    server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

                    tokio::spawn(async move {
                        loop {
                            match state.next().await.unwrap() {
                                Ok(ok) => tracing::debug!("event: {:?}", ok),
                                Err(err) => tracing::error!("error: {:?}", err),
                            }
                        }
                    });

                    Self::Acme {
                        challenge_config,
                        server_config: Arc::new(server_config),
                    }
                }
            }
        }

        #[cfg(not(feature = "tls"))]
        {
            let _ = tls;
            Self::None
        }
    }

    #[cfg(feature = "tls")]
    pub(crate) fn server_config(&self) -> Option<Arc<rustls::ServerConfig>> {
        match self {
            Self::None => None,
            Self::File(server_config) | Self::Acme { server_config, .. } => {
                Some(server_config.clone())
            }
        }
    }
}

impl Listener {
    pub(crate) fn new(
        addr: SocketAddr,
        proxy_protocol: bool,
        tls: ListenerTls,
    ) -> Result<Self, crate::Error> {
        Ok(Self {
            listener: crate::tcp_listener(addr)?,
            proxy_protocol,
            tls,
        })
    }

    pub(crate) fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Accepts a new tcp connection without performing any handshakes, so it
    /// returns as fast as possible. Call [`Incoming::handshake`] in the
    /// connection's task to finish setting it up.
    pub(crate) async fn accept(&self) -> io::Result<Incoming> {
        let (stream, peer) = self.listener.accept().await?;

        Ok(Incoming {
            stream,
            peer,
            proxy_protocol: self.proxy_protocol,
            tls: self.tls.clone(),
        })
    }
}

impl Incoming {
    /// Performs the PROXY protocol and TLS handshakes. If `proxy_protocol` was
    /// enabled, the SocketAddr will be the address of the original peer.
    /// Returns `None` if the connection was fully handled here (ACME challenges).
    pub(crate) async fn handshake(self) -> io::Result<Option<(Stream, SocketAddr)>> {
        let Self {
            mut stream,
            mut peer,
            proxy_protocol,
            tls,
        } = self;

        if proxy_protocol {
            peer = match timeout(
                Duration::from_secs(2),
                parse_proxy_header(&mut stream, peer),
            )
            .await
            {
                Ok(Ok(addr)) => addr,
                Ok(Err(e)) => return Err(e),
                Err(_) => {
                    tracing::warn!("Timeout reading PROXY header from {peer}");
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "proxy header timeout",
                    ));
                }
            };
        }

        match tls {
            ListenerTls::None => Ok(Some((Stream::Plain(stream), peer))),
            #[cfg(feature = "tls")]
            ListenerTls::File(server_config) => {
                let tls_stream = crate::tls::stream::TlsStream::new(stream, server_config);
                Ok(Some((Stream::FileTls(tls_stream), peer)))
            }
            #[cfg(feature = "tls")]
            ListenerTls::Acme {
                challenge_config,
                server_config,
            } => {
                let Ok(start_handshake) = timeout(
                    Duration::from_secs(2),
                    tokio_rustls::LazyConfigAcceptor::new(Default::default(), stream),
                )
                .await
                else {
                    tracing::warn!("Timeout receiving client hello from {peer}");
                    return Ok(None);
                };
                let start_handshake = start_handshake?;

                if rustls_acme::is_tls_alpn_challenge(&start_handshake.client_hello()) {
                    let domain = start_handshake.client_hello().server_name().map(str::to_owned);
                    tracing::info!("received TLS-ALPN-01 validation request for {domain:?}");
                    match timeout(
                        Duration::from_secs(2),
                        start_handshake.into_stream(challenge_config),
                    )
                    .await
                    {
                        Ok(Ok(mut tls)) => {
                            if let Err(err) = tls.shutdown().await {
                                tracing::debug!("Error closing ACME challenge conn: {err:?}")
                            };
                        }
                        // The validator usually hangs up as soon as it has read
                        // the certificate, so handshake errors here are expected.
                        Ok(Err(err)) => {
                            tracing::debug!("ACME challenge handshake ended: {err:?}")
                        }
                        Err(_) => tracing::warn!("Timeout in ACME challenge handshake from {peer}"),
                    }
                    Ok(None)
                } else {
                    tracing::trace!("Accepting TLS connection...");
                    let domain = start_handshake.client_hello().server_name().map(Arc::from);
                    let tls_res = match timeout(
                        Duration::from_secs(2),
                        start_handshake.into_stream(server_config),
                    )
                    .await
                    {
                        Ok(tls_res) => tls_res,
                        Err(_) => {
                            tracing::warn!("Timeout accepting ACME TLS conn form {peer}");
                            return Ok(None);
                        }
                    };

                    Ok(Some((Stream::AcmeTls(tls_res?, domain), peer)))
                }
            }
        }
    }
}

impl Stream {
    pub fn domain(&self) -> Option<Arc<str>> {
        match self {
            Stream::Plain(_) => None,
            #[cfg(feature = "tls")]
            Stream::FileTls(_) => None,
            #[cfg(feature = "tls")]
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
            #[cfg(feature = "tls")]
            Stream::FileTls(tls_stream) => tls_stream.is_write_vectored(),
            #[cfg(feature = "tls")]
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
            #[cfg(feature = "tls")]
            Stream::FileTls(tls_stream) => Pin::new(tls_stream).poll_write_vectored(cx, bufs),
            #[cfg(feature = "tls")]
            Stream::AcmeTls(tls_stream, _) => Pin::new(tls_stream).poll_write_vectored(cx, bufs),
        }
    }
}

/// The 12-byte block that every PROXY protocol v2 header starts with.
const SIGNATURE: [u8; 12] = [
    0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A,
];

/// Reads the first bytes of a tcp stream expecting a Proxy Protocol v2 header.
/// Returns the source ip and port.
async fn parse_proxy_header(
    tcp_stream: &mut TcpStream,
    peer: SocketAddr,
) -> Result<SocketAddr, io::Error> {
    // Fixed 16-byte prefix: 12 signature + 1 ver/cmd + 1 fam/proto + 2 length.
    let mut header = [0u8; 16];
    tcp_stream.read_exact(&mut header).await?;

    if header[..12] != SIGNATURE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid PROXY protocol v2 signature",
        ));
    }

    // Byte 12: high nibble = version (must be 2), low nibble = command.
    if header[12] >> 4 != 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported PROXY protocol version",
        ));
    }
    let command = header[12] & 0x0F; // 0 = LOCAL, 1 = PROXY

    // Byte 13: high nibble = address family, low nibble = transport protocol.
    let family = header[13] >> 4; // 1 = AF_INET, 2 = AF_INET6, 3 = AF_UNIX

    // Bytes 14..16: length of the address block that follows (big-endian).
    let addr_len = u16::from_be_bytes([header[14], header[15]]) as usize;

    // Always drain the full block so the stream is positioned at the payload,
    // even when we end up ignoring the contents (LOCAL, or trailing TLVs).
    let mut addrs = vec![0u8; addr_len];
    tcp_stream.read_exact(&mut addrs).await?;

    // LOCAL: the sender has no real client to declare (health checks, etc.);
    // fall back to the underlying socket's real peer address.
    if command == 0 {
        return Ok(peer);
    }

    match family {
        // AF_INET: src_addr[4] dst_addr[4] src_port[2] dst_port[2]
        1 => {
            if addrs.len() < 12 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "truncated IPv4 address block",
                ));
            }
            let ip = Ipv4Addr::new(addrs[0], addrs[1], addrs[2], addrs[3]);
            let port = u16::from_be_bytes([addrs[8], addrs[9]]);
            Ok(SocketAddr::V4(SocketAddrV4::new(ip, port)))
        }
        // AF_INET6: src_addr[16] dst_addr[16] src_port[2] dst_port[2]
        2 => {
            if addrs.len() < 36 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "truncated IPv6 address block",
                ));
            }
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&addrs[0..16]);
            let ip = Ipv6Addr::from(octets);
            let port = u16::from_be_bytes([addrs[32], addrs[33]]);
            Ok(SocketAddr::V6(SocketAddrV6::new(ip, port, 0, 0)))
        }
        // AF_UNSPEC (0) or AF_UNIX (3) don't map to an IP SocketAddr.
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported PROXY protocol address family",
        )),
    }
}
