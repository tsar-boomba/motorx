use std::net::SocketAddr;

use bytes::Bytes;
use http::{Request, Response};
use http_body_util::{combinators::BoxBody, Empty};
use hyper_util::rt::TokioIo;

use crate::{config::Proto, handle::util::bad_gateway, UpstreamAndConnPool};

use super::util;

pub(crate) async fn handle_upgrade(
    req: Request<BoxBody<Bytes, crate::Error>>,
    upstream: &UpstreamAndConnPool,
    peer_addr: SocketAddr,
) -> Result<Response<BoxBody<Bytes, crate::Error>>, crate::Error> {
    // First, proxy upgrade request to upstream to see if it is successful
    tracing::debug!("Upgrading req: {req:?}");

    // We need to make a copy of the original request's head so that we can send one to the upstream (with og body),
    // and use the other for upgrading with hyper because sending to upstream needs ownership of `req`
    let (client_req, upgrade_req) = {
        let (og_head, body) = req.into_parts();
        (
            Request::from_parts(og_head.clone(), body),
            Request::from_parts(og_head, Empty::<Bytes>::new()),
        )
    };

    // Must use an http1 send_req for upgrades
    let Ok(mut send_req) = upstream.1.new_connection(None, Proto::Http1).await else {
        return Ok(bad_gateway());
    };
    let mut res =
        util::proxy_request(client_req, &upstream.0, &mut send_req, peer_addr, true).await;

    let buf_size = upstream.0.buffer_size;
    match hyper::upgrade::on(&mut res).await {
        Ok(upgraded_upstream) => {
            tokio::task::spawn(async move {
                match hyper::upgrade::on(upgrade_req).await {
                    Ok(upgraded_client) => {
                        if let Err(err) = tokio::io::copy_bidirectional_with_sizes(
                            &mut TokioIo::new(upgraded_client),
                            &mut TokioIo::new(upgraded_upstream),
                            buf_size,
                            buf_size,
                        )
                        .await
                        {
                            tracing::error!("Error in upgraded conn: {err:?}");
                        };
                    }
                    Err(e) => eprintln!("upgrade error: {}", e),
                }
            });

            Ok(res)
        }
        Err(err) if err.is_user() => Ok(res),
        Err(err) => {
            tracing::error!("Failed to upgrade: {err:?}");

            Ok(util::bad_gateway())
        }
    }
}
