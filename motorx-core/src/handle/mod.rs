pub mod util;

use std::net::SocketAddr;
use std::sync::{Arc, Weak};
use std::time::Instant;

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use hyper::{body::Incoming, Method, StatusCode};
use hyper::{Request, Response};

use crate::cache::{Cache, CacheEntry, CloneableRes};
use crate::{cfg_logging, UpstreamAndConnPool, Upstreams};
use crate::config::rule::Rule;
use crate::config::Config;

#[cfg_attr(
    feature = "logging",
    tracing::instrument(level = "trace", skip(req, config, cache))
)]
pub(crate) async fn handle_req(
    req: Request<hyper::body::Incoming>,
    peer_addr: SocketAddr,
    config: Arc<Config>,
    cache: Arc<Cache>,
    upstreams: Arc<Upstreams>,
) -> Result<Response<BoxBody<Bytes, crate::Error>>, crate::Error> {
    for rule in &config.rules {
        if rule.matches(&req) {
            let upstream = upstreams.get(rule.upstream_key).expect("`upstream` in a rule should match a key in the `upstreams` property at the root of the config.");

            // handle authentication if necessary
            let auth_res = util::authenticate(&upstreams, upstream, peer_addr, &req).await?;

            if let Some(res) = auth_res {
                return Ok(res);
            };

            return handle_match(
                req,
                peer_addr,
                rule,
                upstream,
                cache,
                &upstreams,
                config.max_connections,
            )
            .await;
        }
    }

    Ok(Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(util::empty())
        .unwrap())
}

#[cfg_attr(
    feature = "logging",
    tracing::instrument(level = "trace", skip(req, cache, peer_addr))
)]
async fn handle_match(
    req: Request<Incoming>,
    peer_addr: SocketAddr,
    rule: &Rule,
    upstream: &UpstreamAndConnPool,
    cache: Arc<Cache>,
    upstreams: &Upstreams,
    max_connections: usize,
) -> Result<Response<BoxBody<Bytes, crate::Error>>, crate::Error> {
    if Method::CONNECT == req.method() {
        // Don't feel comfortable supporting Connect method right now
        return Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(util::empty())
            .unwrap());
    }

    // use cache if enabled
    let refresh_cache = if let Some(cache_settings) = rule.cache.as_ref() {
        if cache_settings.methods.contains(req.method()) {
            let entry = cache.get_entry(rule, req.uri()).await;

            if let Some(entry) = entry {
                // cache found
                let entry = entry.lock().await;
                let CacheEntry {
                    cached_at: _,
                    value: _,
                    inflight,
                } = &*entry;

                if let Some(cached_res) = entry.extract_fresh_data(cache_settings.max_age) {
                    cfg_logging! {trace!("Cache hit for {}", req.uri());}
                    return Ok(cached_res);
                }

                let inflight = inflight.as_ref().cloned();
                drop(entry);

                if let Some(inflight) = inflight.as_ref().and_then(Weak::upgrade) {
                    // request is inflight to update cache, wait for it
                    cfg_logging! {trace!("No cache found for {}, waiting on inflight request...", req.uri());}

                    // dont hold lock while waiting for inflight
                    if let Ok(Some(res)) = inflight.subscribe().recv().await {
                        // Clone the inner response and use it
                        return Ok((*res).clone().0.map(|b| util::full(b)));
                    } else {
                        // inflight request failed, proceed as if caching was disabled
                        None
                    }
                } else {
                    // cache needs to be updated
                    cfg_logging! {debug!("Stale cache for {}, updating...", req.uri());}
                    Some(
                        cache
                            .insert_empty_entry(rule, req.uri(), max_connections)
                            .await,
                    )
                }
            } else {
                // no cache, refresh
                cfg_logging! {debug!("No cache found for {}, creating...", req.uri());}
                Some(
                    cache
                        .insert_empty_entry(rule, req.uri(), max_connections)
                        .await,
                )
            }
        } else {
            // method not cached
            None
        }
    } else {
        // no caching
        None
    };

    let req_uri = req.uri().clone();
    let resp = util::proxy_request(req, upstream, peer_addr).await;
    cfg_logging! {
        trace!("Got res from upstream {}", peer_addr);
    }

    if let Some(refresh_cache) = refresh_cache {
        // read response & clone to send one and save one for cache
        let status = resp.status();

        let resp = if let Some(entry) = cache.get_entry(rule, &req_uri).await {
            // cache already exists
            let mut entry = entry.lock().await;

            let resp = if status.is_success() {
                // broadcast new value to waiters if not an error status
                let (send_res, cloned_res) = util::clone_response(resp).await?;
                let cloneable = CloneableRes(cloned_res);
                refresh_cache.send(Some(Arc::new(cloneable.clone()))).ok();

                // update cache with the new response
                entry.cached_at = Some(Instant::now());
                entry.value = Some(cloneable.0);
                send_res
            } else {
                // res was an error, dont send to waiters or cache
                refresh_cache.send(None).ok();
                resp
            };

            entry.inflight = None;
            resp
        } else {
            // cache needs to be created
            let resp = if status.is_success() {
                let (send_res, cloned_res) = util::clone_response(resp).await?;
                let cloneable = CloneableRes(cloned_res);
                // broadcast new value to waiters if successful
                refresh_cache.send(Some(Arc::new(cloneable.clone()))).ok();
                // create new cache entry
                cache
                    .insert_populated_entry(rule, req_uri, cloneable.0)
                    .await;
                send_res
            } else {
                // res was an error, don't send to waiters or cache
                refresh_cache.send(None).ok();
                resp
            };

            resp
        };

        Ok(resp)
    } else {
        // Just send response
        cfg_logging! {
            trace!("Returning res form upstream {}", peer_addr);
        }
        Ok(resp)
    }
}
