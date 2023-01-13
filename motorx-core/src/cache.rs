use std::{
    collections::HashMap,
    ops::Deref,
    sync::{Arc, Weak},
    time::Instant,
};

use bytes::Bytes;
use http::Uri;
use hyper::Response;
use once_cell::sync::OnceCell;
use tokio::sync::{broadcast, Mutex, RwLock};

use crate::config::{rule::Rule, Config};

pub(crate) static CACHES: OnceCell<HashMap<Rule, RwLock<HashMap<Uri, Arc<Mutex<Cache>>>>>> =
    OnceCell::new();

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

// Thank you to fasterthanlime's great post about caching!
// https://fasterthanli.me/articles/request-coalescing-in-async-rust
#[derive(Debug)]
pub(crate) struct Cache {
    /// Time it was cached at, and the value
    pub(crate) cached_at: Option<Instant>,
    pub(crate) value: Option<Response<Bytes>>,
    pub(crate) inflight: Option<Weak<broadcast::Sender<Option<CloneableRes<Bytes>>>>>,
}

pub(crate) fn init_caches(config: &Config) {
    CACHES
        .set(HashMap::from_iter(
            config
                .rules
                .iter()
                .map(|rule| (rule.clone(), RwLock::new(HashMap::new()))),
        ))
        .unwrap();
}
