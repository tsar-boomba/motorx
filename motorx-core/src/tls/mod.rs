use std::{fs, io, path::Path};

use itertools::Itertools;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

pub mod stream;

// Load public certificate from file.
pub(crate) fn load_certs(filename: impl AsRef<Path>) -> io::Result<Vec<CertificateDer<'static>>> {
    // Open certificate file.
    let filename = filename.as_ref();
    let certfile = fs::File::open(filename).map_err(|e| {
        error(format!(
            "failed to open {}: {}",
            filename.to_string_lossy(),
            e
        ))
    })?;
    let mut reader = io::BufReader::new(certfile);

    // Load and return certificate.
    let certs = rustls_pemfile::certs(&mut reader)
        .try_collect::<_, Vec<_>, _>()
        .map_err(|e| error(e.to_string()))?;

    if certs.len() < 1 {
        return Err(error("Cannot have empty certs.".into()));
    }

    Ok(certs)
}

// Load private key from file.
pub(crate) fn load_private_key(filename: impl AsRef<Path>) -> io::Result<PrivateKeyDer<'static>> {
    // Open keyfile.
    let filename = filename.as_ref();
    let keyfile = fs::File::open(filename).map_err(|e| {
        error(format!(
            "failed to open {}: {}",
            filename.to_string_lossy(),
            e
        ))
    })?;
    let mut reader = io::BufReader::new(keyfile);

    // TODO: migrate to rustls-pki-types
    // Load and return a single private key.
    let key = rustls_pemfile::private_key(&mut reader)?
        .ok_or_else(|| error("Missing private key".into()))?;

    Ok(key)
}

fn error(err: String) -> io::Error {
    io::Error::new(io::ErrorKind::Other, err)
}
