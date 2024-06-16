use std::{fs, io};

use itertools::Itertools;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

pub mod stream;

pub(crate) fn error(err: String) -> io::Error {
    io::Error::new(io::ErrorKind::Other, err)
}

// Load public certificate from file.
pub(crate) fn load_certs(filename: &str) -> io::Result<Vec<CertificateDer<'static>>> {
    // Open certificate file.
    let certfile = fs::File::open(filename)
        .map_err(|e| error(format!("failed to open {}: {}", filename, e)))?;
    let mut reader = io::BufReader::new(certfile);

    // Load and return certificate.
    let certs = rustls_pemfile::certs(&mut reader)
        .try_collect::<_, Vec<_>, _>()
        .map_err(|e| error(e.to_string()))?;

    Ok(certs)
}

// Load private key from file.
pub(crate) fn load_private_key(filename: &str) -> io::Result<PrivateKeyDer<'static>> {
    // Open keyfile.
    let keyfile = fs::File::open(filename)
        .map_err(|e| error(format!("failed to open {}: {}", filename, e)))?;
    let mut reader = io::BufReader::new(keyfile);

    // Load and return a single private key.
    let mut keys = rustls_pemfile::rsa_private_keys(&mut reader)
        .try_collect::<_, Vec<_>, _>()
        .map_err(|e| error(e.to_string()))?;

    if keys.len() != 1 {
        return Err(error("expected a single private key".into()));
    }

    Ok(PrivateKeyDer::Pkcs1(keys.swap_remove(0)))
}
