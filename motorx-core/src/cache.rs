use std::{
    collections::HashMap,
    ops::Deref,
    sync::{Arc, Weak},
    time::{Duration, Instant},
};

use bytes::Bytes;
use http::Uri;
use http_body_util::combinators::BoxBody;
use hyper::Response;
use tokio::sync::{broadcast, Mutex, RwLock};

use crate::{
    config::{rule::Rule, Config},
    handle::util,
};

#[derive(Debug)]
pub(crate) struct CloneableRes<T>(pub Response<T>);

impl<T: Clone> Clone for CloneableRes<T> {
    fn clone(&self) -> Self {
        let mut builder = Response::builder()
            .status(self.status())
            .version(self.version());

        for (k, v) in self.headers() {
            builder = builder.header(k, v);
        }

        Self(builder.body(self.body().clone()).unwrap())
    }
}

impl<T> Deref for CloneableRes<T> {
    type Target = Response<T>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub(crate) struct Cache {
    // TODO: look into other synchronization than RwLock
    cache: HashMap<Rule, RwLock<HashMap<Uri, Arc<Mutex<CacheEntry>>>>>,
}

// Thank you to fasterthanlime's great post about caching!
// https://fasterthanli.me/articles/request-coalescing-in-async-rust
#[derive(Debug)]
pub(crate) struct CacheEntry {
    /// Time it was cached at, and the value
    pub(crate) cached_at: Option<Instant>,
    // TODO: allow storing the data on disk as well as in memory
    pub(crate) value: Option<Response<Bytes>>,
    pub(crate) inflight: Option<Weak<broadcast::Sender<Option<CloneableRes<Bytes>>>>>,
}

impl Cache {
    pub(crate) fn from_config(config: &Config) -> Self {
        Self {
            cache: HashMap::from_iter(
                config
                    .rules
                    .iter()
                    .map(|rule| (rule.clone(), RwLock::new(HashMap::new()))),
            ),
        }
    }

    pub(crate) async fn get_entry(&self, rule: &Rule, uri: &Uri) -> Option<Arc<Mutex<CacheEntry>>> {
        let rule_cache = self.cache.get(rule).unwrap();
        rule_cache.read().await.get(uri).cloned()
    }

    /// Adds an empty entry to the specified cache, returning the sender for the inflight request
    pub(crate) async fn insert_empty_entry(
        &self,
        rule: &Rule,
        uri: &Uri,
        max_connections: usize,
    ) -> Arc<broadcast::Sender<Option<CloneableRes<Bytes>>>> {
        // TODO: Consider sending an Option<Arc<CloneableRes>> over the channel to make sending faster (cheaper clone)
        let sender = Arc::new(broadcast::channel::<Option<CloneableRes<Bytes>>>(max_connections).0);

        self.cache.get(rule).unwrap().write().await.insert(
            uri.clone(),
            Arc::new(Mutex::new(CacheEntry {
                cached_at: None,
                value: None,
                inflight: Some(Arc::downgrade(&sender)),
            })),
        );

        sender
    }

    pub(crate) async fn insert_populated_entry(&self, rule: &Rule, uri: Uri, res: Response<Bytes>) {
        let rule_cache = self.cache.get(rule).unwrap();
        rule_cache.write().await.insert(
            uri,
            Arc::new(Mutex::new(CacheEntry {
                cached_at: Some(Instant::now()),
                value: Some(res),
                inflight: None,
            })),
        );
    }
}

impl CacheEntry {
    pub(crate) fn extract_fresh_data(
        &self,
        max_age: Duration,
    ) -> Option<Response<BoxBody<Bytes, crate::Error>>> {
        self.cached_at.and_then(|c_at| {
            let value = self.value.as_ref().unwrap();
            (c_at.elapsed() <= max_age).then(|| util::from_response(value, value.body().clone()))
        })
    }
}
