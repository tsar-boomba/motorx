pub mod util;

use std::net::SocketAddr;
use std::sync::{Arc, Weak};
use std::time::Instant;

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use hyper::{body::Incoming, Method, StatusCode};
use hyper::{Request, Response};
use tokio::sync::{broadcast, Mutex};

use crate::cache::{Cache, CloneableRes, CACHES};
use crate::cfg_logging;
use crate::config::rule::Rule;
use crate::config::{Config, Upstream};

#[cfg_attr(
    feature = "logging",
    tracing::instrument(level = "trace", skip(req, config))
)]
pub(crate) async fn handle_req(
    req: Request<hyper::body::Incoming>,
    peer_addr: SocketAddr,
    config: Arc<Config>,
) -> Result<Response<BoxBody<Bytes, crate::Error>>, crate::Error> {
    for rule in &config.rules {
        if rule.matches(&req) {
            let upstream = config.upstreams.get(&rule.upstream).expect("`upstream` in a rule should match a key in the `upstreams` property at the root of the config.");

            // handle authentication if necessary
            let auth_res = util::authenticate(&*config, upstream, peer_addr, &req).await?;

            if let Some(res) = auth_res {
                return Ok(res);
            };

            return handle_match(req, peer_addr, rule, upstream, config.max_connections).await;
        }
    }

    Ok(Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(util::empty())
        .unwrap())
}

#[cfg_attr(
    feature = "logging",
    tracing::instrument(level = "trace", skip(req, peer_addr))
)]
async fn handle_match(
    req: Request<Incoming>,
    peer_addr: SocketAddr,
    rule: &Rule,
    upstream: &Upstream,
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
            let rule_cache = CACHES.get().unwrap().get(rule).unwrap().read().await;
            let cache = rule_cache.get(req.uri()).cloned();

            // drop here so that cache hits can use a read lock (supa fast)
            drop(rule_cache);

            if let Some(cache) = cache {
                // cache found
                let cache = cache.lock().await;
                let Cache {
                    cached_at,
                    value,
                    inflight,
                } = &*cache;

                if let Some(cached_at) = cached_at {
                    if let Some(value) = value {
                        if cached_at.elapsed() < cache_settings.max_age {
                            // cache hit!
                            cfg_logging! {trace!("Cache hit for {}", req.uri());}
                            return Ok(util::from_response(value, value.body().clone()));
                        }
                    }
                }

                let inflight = inflight.as_ref().cloned();
                drop(cache);

                if let Some(inflight) = inflight.as_ref().and_then(Weak::upgrade) {
                    // request is inflight to update cache, wait for it
                    cfg_logging! {trace!("No cache found for {}, waiting on inflight request...", req.uri());}

                    // dont hold lock while waiting for inflight
                    if let Ok(Some(res)) = inflight.subscribe().recv().await {
                        return Ok(res.0.map(|b| util::full(b)));
                    } else {
                        // inflight request failed, proceed as if caching was disabled
                        None
                    }
                } else {
                    // cache needs to be updated
                    cfg_logging! {debug!("Stale cache for {}, updating...", req.uri());}
                    let sender = Arc::new(
                        broadcast::channel::<Option<CloneableRes<Bytes>>>(max_connections).0,
                    );
                    CACHES
                        .get()
                        .unwrap()
                        .get(rule)
                        .unwrap()
                        .write()
                        .await
                        .insert(
                            req.uri().clone(),
                            Arc::new(Mutex::new(Cache {
                                cached_at: None,
                                value: None,
                                inflight: Some(Arc::downgrade(&sender)),
                            })),
                        );

                    Some(sender)
                }
            } else {
                // no cache, refresh
                cfg_logging! {debug!("No cache found for {}, creating...", req.uri());}
                let sender =
                    Arc::new(broadcast::channel::<Option<CloneableRes<Bytes>>>(max_connections).0);
                CACHES
                    .get()
                    .unwrap()
                    .get(rule)
                    .unwrap()
                    .write()
                    .await
                    .insert(
                        req.uri().clone(),
                        Arc::new(Mutex::new(Cache {
                            cached_at: None,
                            value: None,
                            inflight: Some(Arc::downgrade(&sender)),
                        })),
                    );

                Some(sender)
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
    let result = util::proxy_request(req, upstream, peer_addr).await;
    cfg_logging! {
                            trace!("Got res from upstream {}", peer_addr);
                        }

    if let Some(refresh_cache) = refresh_cache {
        match result {
            Ok(res) => {
                // read response & clone to send one and save one for cache
                let (send_res, cloned_res) = util::clone_response(res).await?;
                let cloneable = CloneableRes(cloned_res);
                let status = cloneable.status();

                let rule_cache = CACHES.get().unwrap().get(rule).unwrap();
                if let Some(cache) = rule_cache.read().await.get(&req_uri) {
                    // cache already exists
                    let mut cache = cache.lock().await;

                    if status.is_client_error() || status.is_server_error() {
                        // broadcast new value to waiters if not an error status
                        refresh_cache.send(Some(cloneable.clone())).ok();
                    } else {
                        // res was an error, dont send to waiters or cache
                        refresh_cache.send(None).ok();
                    };

                    cache.cached_at = Some(Instant::now());
                    cache.value = Some(cloneable.0);
                    cache.inflight = None;
                } else {
                    // cache needs to be created
                    let mut rule_cache = rule_cache.write().await;

                    if status.is_client_error() || status.is_server_error() {
                        // broadcast new value to waiters if not an error status
                        refresh_cache.send(Some(cloneable.clone())).ok();
                    } else {
                        // res was an error, dont send to waiters or cache
                        refresh_cache.send(None).ok();
                    };

                    rule_cache.insert(
                        req_uri,
                        Arc::new(Mutex::new(Cache {
                            cached_at: Some(Instant::now()),
                            value: Some(cloneable.0),
                            inflight: None,
                        })),
                    );
                };

                Ok(send_res)
            }
            Err(err) => Err(err),
        }
    } else {
        // Just send response
        cfg_logging! {
                            trace!("Returning res form upstream {}", peer_addr);
                        }
        result
    }
}
