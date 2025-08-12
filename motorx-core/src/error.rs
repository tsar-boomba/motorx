use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Io error: {0:?}")]
    Io(#[from] std::io::Error),

    #[error("Hyper error: {0:?}")]
    Hyper(#[from] hyper::Error),

    #[error("Invalid host")]
    InvalidHost,

    #[error("Failed connection")]
    FailedConnection,

    #[cfg(feature = "tls")]
    #[error("Rustls error: {0:?}")]
    Rustls(#[from] rustls::Error),

    #[cfg(feature = "h3")]
    #[error("h3 connection error: {0:?}")]
    H3ConnectionError(#[from] s2n_quic_h3::h3::error::ConnectionError),

    #[cfg(feature = "h3")]
    #[error("h3 stream error: {0:?}")]
    H3StreamError(#[from] s2n_quic_h3::h3::error::StreamError),
}
