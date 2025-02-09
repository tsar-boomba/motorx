use std::net::SocketAddr;

use bytes::Bytes;
use http::{header::HOST, HeaderValue, Request, Response, StatusCode};
use http_body_util::{combinators::BoxBody, BodyExt, Empty, Full};
use hyper::{body::Incoming, client, upgrade::Upgraded};
use hyper_util::rt::TokioIo;

use crate::{
    cfg_logging,
    config::{authentication::AuthenticationSource, Upstream},
    tcp_connect, UpstreamAndConnPool, Upstreams,
};

pub(crate) fn add_proxy_headers<B>(
    req: &mut Request<B>,
    upstream: &Upstream,
    peer_addr: SocketAddr,
) {
    let proto = req.uri().scheme_str().unwrap_or_default();
    let proto = if proto.is_empty() {
        proto.to_string()
    } else {
        format!("proto={}", proto)
    };

    let host = req
        .headers()
        .get("host")
        .map(|h| h.to_str().unwrap_or_default())
        .unwrap_or_default();
    let host = if host.is_empty() {
        host.to_string()
    } else {
        format!("host={};", host)
    };

    let headers = req.headers_mut();
    headers.append(
        "forwarded",
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
        "x-forwarded-for",
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

const HOP_HEADERS_NO_UPGRADE: [&str; 6] = [
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "tt",
    "trailer",
    "transfer-encoding",
];

pub(crate) fn remove_hop_headers<B>(req: &mut Request<B>, upgrading: bool) {
    let headers = req.headers_mut();
    for hop_header in HOP_HEADERS_NO_UPGRADE {
        headers.remove(hop_header);
    }

    if !upgrading {
        for hop_header in ["connection", "upgrade"] {
            headers.remove(hop_header);
        }
    }
}

pub(crate) fn empty() -> BoxBody<Bytes, crate::Error> {
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}

pub(crate) fn full(chunk: impl Into<Bytes>) -> BoxBody<Bytes, crate::Error> {
    Full::new(chunk.into())
        .map_err(|never| match never {})
        .boxed()
}

pub(crate) fn from_response<T>(
    res: &Response<T>,
    body: Bytes,
) -> Response<BoxBody<Bytes, crate::Error>> {
    let mut builder = Response::builder()
        .status(res.status())
        .version(res.version());

    for (k, v) in res.headers() {
        builder = builder.header(k, v);
    }

    builder.body(full(body)).unwrap()
}

pub(crate) async fn clone_response<T: BodyExt>(
    res: Response<T>,
) -> Result<(Response<BoxBody<Bytes, crate::Error>>, Response<Bytes>), T::Error> {
    let (parts, og_body) = res.into_parts();
    let body = read_body::<_, crate::Error>(og_body).await?;

    return Ok((
        Response::from_parts(parts.clone(), full(body.clone())),
        Response::from_parts(parts, body),
    ));
}

#[inline]
pub(crate) async fn read_body<B: BodyExt, E>(body: B) -> Result<Bytes, B::Error> {
    Ok(body.collect().await?.to_bytes())
}

pub(crate) async fn proxy_request(
    mut req: Request<Incoming>,
    upstream: &UpstreamAndConnPool,
    peer_addr: SocketAddr,
    upgrading: bool,
) -> Response<BoxBody<Bytes, crate::Error>> {
    const RETRY_COUNT: usize = 1;
    let mut tries = 0;

    let mut conn = loop {
        if tries > RETRY_COUNT {
            return bad_gateway();
        }

        let mut conn = match upstream.1.get_sender().await {
            Ok(senders) => senders,
            Err(err) => {
                cfg_logging! {error!("Failed to connect to {}: {err}", upstream.0.addr);}
                tries += 1;
                continue;
            }
        };

        if let Err(err) = conn.ready().await {
            cfg_logging! {error!("Connection to {} was unexpectedly closed: {err}", upstream.0.addr);}
            tries += 1;
            continue;
        }

        break conn;
    };

    // wait for conn to be ready, if it closes return a error

    add_proxy_headers(&mut req, &upstream.0, peer_addr);
    remove_hop_headers(&mut req, upgrading);

    cfg_logging! {
        debug!("Proxying request: {:?}", req);
    }

    let resp = match conn.send_request(req).await {
        Ok(resp) => resp,
        Err(err) => {
            cfg_logging! {error!("Failed to proxy request to {}: {err}", upstream.0.addr);};
            return bad_gateway();
        }
    };

    // Dropping a pooled connection returns it to the pool
    drop(conn);

    resp.map(|b| b.map_err(|e| e.into()).boxed())
}

pub(crate) fn bad_gateway() -> Response<BoxBody<Bytes, crate::Error>> {
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .body(empty())
        .unwrap()
}

// TODO: test this, I don't think its correct right now
/// Returning Ok(None) means no auth needed or auth succeeded
/// Ok(Some) means respond with this because with failed with the upstream
pub(crate) async fn authenticate<B>(
    upstreams: &Upstreams,
    upstream: &UpstreamAndConnPool,
    peer_addr: SocketAddr,
    req: &Request<B>,
) -> Result<Option<Response<BoxBody<Bytes, crate::Error>>>, crate::Error> {
    let Some(authentication) = &upstream.0.authentication else {
        return Ok(None);
    };

    if authentication
        .exclude
        .iter()
        .any(|path| path.matches(req.uri().path()))
    {
        // req path matched one of the exclude rules
        return Ok(None);
    }

    cfg_logging! {debug!("Authorizing request.");}

    let auth_uri = match &authentication.source {
        AuthenticationSource::Path(path) => path,
        AuthenticationSource::Upstream {
            key: _,
            name: _,
            path,
        } => path,
    };
    let mut auth_req_builder = Request::builder()
        .version(req.version())
        .method(req.method())
        .uri(auth_uri);

    for (k, v) in req.headers() {
        auth_req_builder = auth_req_builder.header(k, v);
    }

    let mut auth_req = auth_req_builder.body(Empty::<Bytes>::new()).unwrap();
    add_proxy_headers(&mut auth_req, &upstream.0, peer_addr);
    remove_hop_headers(&mut auth_req, false);

    let auth_upstream = match &authentication.source {
        AuthenticationSource::Path(_) => &upstream,
        AuthenticationSource::Upstream {
            key,
            name: _,
            path: _,
        } => upstreams.get(*key).unwrap(),
    };

    // TODO: Refactor to use auth upstream's conn pool
    cfg_logging! {info!("Opened new connection to: {}", upstream.0.addr);}
    let stream = tcp_connect(auth_upstream.0.addr.authority().unwrap().as_str()).await?;
    let (mut sender, conn) = client::conn::http1::Builder::new()
        .preserve_header_case(true)
        .title_case_headers(true)
        .handshake::<_, Empty<Bytes>>(TokioIo::new(stream))
        .await?;

    tokio::task::spawn(async move {
        if let Err(err) = conn.await {
            cfg_logging! {error!("Connection failed: {:?}", err);}
        }
    });

    let res = sender.send_request(auth_req).await?;

    if res.status().is_success() {
        Ok(None)
    } else {
        Ok(Some(res.map(|b| b.map_err(|e| e.into()).boxed())))
    }
}

// Create a TCP connection to host:port, build a tunnel between the connection and
// the upgraded connection
pub async fn tunnel(upgraded: Upgraded, addr: &str) -> std::io::Result<()> {
    // Connect to remote server
    let mut server = tcp_connect(addr).await?;
    let mut upgraded = TokioIo::new(upgraded);

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
