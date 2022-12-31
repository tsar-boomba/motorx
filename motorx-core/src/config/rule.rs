use std::{collections::HashMap, hash::Hash, time::Duration};

use http::Method;
use hyper::{body::Incoming, Request};

use super::match_type::MatchType;

#[cfg_attr(feature = "serde-config", derive(serde::Deserialize))]
#[derive(Debug, PartialEq, Clone)]
pub struct Rule {
    /// Rule the path must match
    pub path: MatchType,
    /// Rule that headers must match
    pub match_headers: Option<HashMap<String, MatchType>>,
    /// Where the request, should match a key in the `upstreams` object
    pub upstream: String,
    /// Settings for caching, by providing this you opt into caching for this rule based on the methods provided in `cache_methods` (defaults to ['GET'])
    pub cache: Option<CacheSettings>,
}

impl Rule {
    pub fn matches(&self, req: &Request<Incoming>) -> bool {
        let path_result = self.path.matches(req.uri().path());

        if !path_result.is_match() {
            return false;
        }

        if let Some(headers) = self.match_headers.as_ref() {
            for (header, pattern) in headers {
                if !pattern
                    .matches(
                        req.headers()
                            .get(header)
                            .map(|h| h.to_str().unwrap_or_default())
                            .unwrap_or_default(),
                    )
                    .is_match()
                {
                    return false;
                }
            }
        }

        true
    }
}

impl Eq for Rule {}

impl Hash for Rule {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        if let Some(cache) = self.cache.as_ref() {
            cache.hash(state);
        }
        self.path.hash(state);
        self.upstream.hash(state);

        if let Some(match_headers) = self.match_headers.as_ref() {
            for (k, v) in match_headers {
                k.hash(state);
                v.hash(state);
            }
        }
    }
}

#[cfg_attr(feature = "serde-config", derive(serde::Deserialize))]
#[derive(Debug, Hash, PartialEq, Clone)]
pub struct CacheSettings {
    /// What methods should have their requests cached
    #[cfg_attr(
        feature = "serde-config",
        serde(
            with = "de_method_vec",
            default = "default_cache_methods"
        )
    )]
    pub methods: Vec<Method>,
    #[cfg_attr(feature = "serde-config", serde(default = "default_cache_max_age"))]
    pub max_age: Duration,
}

impl Eq for CacheSettings {}

mod de_method_vec {
    use std::str::FromStr;

    use http::Method;
    use serde::{de::Visitor, Deserializer};
    struct MethodArrayVisitor;

    impl<'de> Visitor<'de> for MethodArrayVisitor {
        type Value = Vec<Method>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(formatter, "An array of valid http methods.")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            let mut methods = Vec::<Method>::new();
            while let Ok(Some(item)) = seq.next_element::<&str>() {
                if let Ok(method) = Method::from_str(item) {
                    methods.push(method);
                } else {
                    return Err(serde::de::Error::missing_field("Invalid method: {item:?}"));
                };
            }

            Ok(methods)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Vec<Method>, D::Error> {
        de.deserialize_seq(MethodArrayVisitor)
    }
}

fn default_cache_methods() -> Vec<Method> {
    vec![Method::GET]
}

fn default_cache_max_age() -> Duration {
    Duration::from_secs(10)
}
