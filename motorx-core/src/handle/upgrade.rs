use std::net::SocketAddr;

use bytes::Bytes;
use http::{Request, Response};
use http_body_util::{combinators::BoxBody, Empty};
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;

use crate::{cfg_logging, UpstreamAndConnPool};

use super::util;

pub(crate) async fn handle_upgrade(
    req: Request<Incoming>,
    upstream: &UpstreamAndConnPool,
    peer_addr: SocketAddr,
) -> Result<Response<BoxBody<Bytes, crate::Error>>, crate::Error> {
    // First, proxy upgrade request to upstream to see if it is successful

    // We need to make a copy of the original request's head so that we can send one to the upstream (with og body),
    // and use the other for upgrading with hyper because sending to upstream needs ownership of `req`
    let (client_req, upgrade_req) = {
        let (og_head, body) = req.into_parts();
        (
            Request::from_parts(og_head.clone(), body),
            Request::from_parts(og_head, Empty::<Bytes>::new()),
        )
    };
    let mut res = util::proxy_request(client_req, upstream, peer_addr, true).await;

    match hyper::upgrade::on(&mut res).await {
        Ok(upgraded_upstream) => {
            tokio::task::spawn(async move {
                match hyper::upgrade::on(upgrade_req).await {
                    Ok(upgraded_client) => {
                        if let Err(err) = tokio::io::copy_bidirectional(
                            &mut TokioIo::new(upgraded_client),
                            &mut TokioIo::new(upgraded_upstream),
                        )
                        .await
                        {
                            cfg_logging! {
                                tracing::error!("Error in upgraded conn: {err:?}");
                            }
                        };
                    }
                    Err(e) => eprintln!("upgrade error: {}", e),
                }
            });

            Ok(res)
        }
        Err(err) if err.is_user() => Ok(res),
        Err(err) => {
            cfg_logging! {
                tracing::error!("Failed to upgrade: {err:?}");
            }

            Ok(util::bad_gateway())
        }
    }
}
