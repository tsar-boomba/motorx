use std::net::SocketAddr;

use bytes::Bytes;
use http::{header::HOST, HeaderValue, Request, Response};
use http_body_util::{combinators::BoxBody, BodyExt, Empty, Full};
use hyper::{body::Incoming, upgrade::Upgraded};

use crate::{config::Upstream, tcp_connect};

pub fn add_proxy_headers(req: &mut Request<Incoming>, upstream: &Upstream, peer_addr: SocketAddr) {
    let proto = req.uri().scheme_str().unwrap_or_default();
    let proto = if proto.is_empty() {
        proto.to_string()
    } else {
        format!("proto={}", proto)
    };

    let host = req
        .headers()
        .get("Host")
        .map(|h| h.to_str().unwrap_or_default())
        .unwrap_or_default();
    let host = if host.is_empty() {
        host.to_string()
    } else {
        format!("host={};", host)
    };

    let headers = req.headers_mut();
    headers.append(
        "Forwarded",
        HeaderValue::from_str(&format!(
            "for={};{}{}",
            match peer_addr {
                SocketAddr::V4(v4) => v4.to_string(),
                SocketAddr::V6(v6) => {
                    format!(r#""{}""#, v6)
                }
            },
            host,
            proto
        ))
        .unwrap(),
    );
    headers.append(
        "X-Forwarded-For",
        HeaderValue::from_str(&format!("{}", peer_addr)).unwrap(),
    );
    // change host to be correct for the upstream
    headers.insert(
        HOST,
        upstream
            .addr
            .authority()
            .unwrap()
            .as_str()
            .try_into()
            .unwrap(),
    );
}

const HOP_HEADERS: [&str; 8] = [
    "Connection",
    "Keep-Alive",
    "Proxy-Authenticate",
    "Proxy-Authorization",
    "TE",
    "Trailer",
    "Transfer-Encoding",
    "Upgrade",
];

pub fn remove_hop_headers(req: &mut Request<Incoming>) {
    let headers = req.headers_mut();
    for hop_header in HOP_HEADERS {
        headers.remove(hop_header);
    }
}

pub fn empty() -> BoxBody<Bytes, crate::Error> {
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}

pub fn full<T: Into<Bytes>>(chunk: T) -> BoxBody<Bytes, crate::Error> {
    Full::new(chunk.into())
        .map_err(|never| match never {})
        .boxed()
}

pub fn from_response<T>(res: &Response<T>, body: Bytes) -> Response<BoxBody<Bytes, crate::Error>> {
    let mut builder = Response::builder()
        .status(res.status())
        .version(res.version());

    for (k, v) in res.headers() {
        builder = builder.header(k, v);
    }

    builder.body(full(body)).unwrap()
}

pub async fn clone_response<T: BodyExt>(
    res: Response<T>,
) -> Result<
    (
        Response<BoxBody<Bytes, crate::Error>>,
        Response<Bytes>,
    ),
    T::Error,
> {
    let (og_parts, og_body) = res.into_parts();
    let mut builder = Response::builder()
        .status(og_parts.status)
        .version(og_parts.version);

    for (k, v) in &og_parts.headers {
        builder = builder.header(k, v);
    }

    let body = read_body::<_, crate::Error>(og_body).await?;

    return Ok((
        Response::from_parts(og_parts, full(body.clone())),
        builder.body(body).unwrap(),
    ));
}

#[inline]
pub async fn read_body<B: BodyExt, E>(
    body: B,
) -> Result<Bytes, B::Error> {
    Ok(body.collect().await?.to_bytes())
}

// Create a TCP connection to host:port, build a tunnel between the connection and
// the upgraded connection
pub async fn tunnel(mut upgraded: Upgraded, addr: &str) -> std::io::Result<()> {
    // Connect to remote server
    let mut server = tcp_connect(addr).await?;

    // Proxying data
    let (from_client, from_server) =
        tokio::io::copy_bidirectional(&mut upgraded, &mut server).await?;

    // Print message when done
    println!(
        "client wrote {} bytes and received {} bytes",
        from_client, from_server
    );

    Ok(())
}
