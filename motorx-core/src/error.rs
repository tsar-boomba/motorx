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
    #[error("h3 error: {0:?}")]
    H3(#[from] s2n_quic_h3::h3::Error),
}
