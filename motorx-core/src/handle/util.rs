use std::net::SocketAddr;

use bytes::Bytes;
use http::{header::HOST, HeaderValue, Request, Response, StatusCode};
use http_body_util::{combinators::BoxBody, BodyExt, Empty, Full};
use hyper::{body::Incoming, upgrade::Upgraded, client};

use crate::{cfg_logging, config::{Upstream, authentication::AuthenticationSource}, conn_pool::CONN_POOLS, tcp_connect, Config};

pub fn add_proxy_headers<B>(req: &mut Request<B>, upstream: &Upstream, peer_addr: SocketAddr) {
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

pub fn remove_hop_headers<B>(req: &mut Request<B>) {
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
) -> Result<(Response<BoxBody<Bytes, crate::Error>>, Response<Bytes>), T::Error> {
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
pub async fn read_body<B: BodyExt, E>(body: B) -> Result<Bytes, B::Error> {
    Ok(body.collect().await?.to_bytes())
}

pub async fn proxy_request(
    mut req: Request<Incoming>,
    upstream: &Upstream,
    peer_addr: SocketAddr,
) -> Result<Response<BoxBody<Bytes, crate::Error>>, crate::Error> {
    let mut conn_pool = CONN_POOLS
        .get()
        .unwrap()
        .get(&upstream.addr)
        .unwrap()
        .lock()
        .await;

    let (queue, mut sender) = match conn_pool.get_sender(upstream).await {
        Ok(senders) => senders,
        Err(_) => {
            cfg_logging! {error!("Failed to connect to {}", upstream.addr);}
            return Ok(Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .body(empty())
                .unwrap());
        }
    };
    drop(conn_pool);

    // wait for conn to be ready, if it closes return a error
    if let Err(_) = sender.ready().await {
        cfg_logging! {error!("Connection to {} was unexpectedly closed.", upstream.addr);}
        return Ok(Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .body(empty())
            .unwrap());
    }

    add_proxy_headers(&mut req, upstream, peer_addr);
    remove_hop_headers(&mut req);

    cfg_logging! {
        debug!("Proxying request: {:?}", req);
    }

    let resp = sender.send_request(req).await?;

    // channel will never be closed, so this is safe
    queue.send(sender).await.unwrap();

    Ok(resp.map(|b| b.map_err(|e| e.into()).boxed()))
}

/// Returning Ok(None) means no auth needed or auth succeeded
/// Ok(Some) means respond with this because with failed with the upstream
pub(crate) async fn authenticate<B>(
    config: &Config,
    upstream: &Upstream,
    peer_addr: SocketAddr,
    req: &Request<B>,
) -> Result<Option<Response<BoxBody<Bytes, crate::Error>>>, crate::Error> {
    let Some(authentication) = &upstream.authentication else {
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
        AuthenticationSource::Upstream { name: _, path } => path,
    };
    let mut auth_req_builder = Request::builder()
        .version(req.version())
        .method(req.method())
        .uri(auth_uri);

    for (k, v) in req.headers() {
        auth_req_builder = auth_req_builder.header(k, v);
    }

    let mut auth_req = auth_req_builder.body(Empty::<Bytes>::new()).unwrap();
    add_proxy_headers(&mut auth_req, upstream, peer_addr);
    remove_hop_headers(&mut auth_req);

    let auth_upstream = match &authentication.source {
        AuthenticationSource::Path(_) => upstream,
        AuthenticationSource::Upstream { name, path: _ } => {
            config.upstreams.get(name).unwrap()
        }
    };

    // TODO in the future, somehow (idk how) use existing conn pool for this
    cfg_logging! {info!("Opened new connection to: {}", upstream.addr);}
    let stream = tcp_connect(auth_upstream.addr.authority().unwrap()).await?;
    let (mut sender, conn) = client::conn::http1::Builder::new()
        .http1_preserve_header_case(true)
        .http1_title_case_headers(true)
        .handshake::<_, Empty<Bytes>>(stream)
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
