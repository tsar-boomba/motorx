use std::fmt::Display;
use std::hash::Hash;
use std::sync::Arc;
use std::{cmp::Ordering, str::FromStr};

use once_cell::sync::Lazy;

use regex::{Captures, Regex};

#[derive(Debug, Clone)]
pub enum MatchType {
    Start(String),
    Regex(Regex),
}

pub enum MatchResult<'t> {
    /// Matches from start of string, default behavior
    Start(bool),
    /// Uses regex pattern to match, ex. `regex(this_is_my_regex_pattern)`
    Regex(Option<Captures<'t>>),
}

impl<'t> MatchResult<'t> {
    #[inline]
    pub fn is_match(&self) -> bool {
        match self {
            MatchResult::Start(matched) => *matched,
            MatchResult::Regex(captures) => captures.is_some(),
        }
    }
}

impl MatchType {
    #[inline]
    pub fn matches<'a>(&self, string: &'a str) -> MatchResult<'a> {
        match self {
            MatchType::Start(ref pattern) => MatchResult::Start(string.starts_with(pattern)),
            MatchType::Regex(ref regex) => MatchResult::Regex(regex.captures(string)),
        }
    }

    #[inline]
    pub fn priority(&self) -> usize {
        match self {
            // highest priority
            MatchType::Start(_) => usize::MIN,
            // lowest priority
            MatchType::Regex(_) => usize::MAX,
        }
    }

    #[inline]
    fn length(&self) -> usize {
        match self {
            MatchType::Start(pat) => pat.len(),
            MatchType::Regex(re) => re.as_str().len(),
        }
    }
}

impl PartialEq for MatchType {
    fn eq(&self, other: &Self) -> bool {
        match self {
            MatchType::Start(pat) => match other {
                MatchType::Start(other_pat) => pat == other_pat,
                _ => false,
            },
            MatchType::Regex(re) => match other {
                MatchType::Regex(other_re) => re.as_str() == other_re.as_str(),
                _ => false,
            },
        }
    }
}

impl Eq for MatchType {}

impl PartialOrd for MatchType {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match self.priority().partial_cmp(&other.priority()) {
            Some(ord) => {
                match ord {
                    Ordering::Greater => Some(Ordering::Greater),
                    Ordering::Less => Some(Ordering::Less),
                    Ordering::Equal => {
                        // priority was same, use length to break tie
                        // longer / more specific should be less (go first)
                        other.length().partial_cmp(&self.length())
                    }
                }
            }
            None => None,
        }
    }
}

impl Ord for MatchType {
    fn cmp(&self, other: &Self) -> Ordering {
        // Will never return None, because integer cmp's will never return None
        self.partial_cmp(other).unwrap()
    }
}

impl Display for MatchType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let string = match self {
            MatchType::Start(path) => format!("start({})", path),
            MatchType::Regex(re) => format!("regex({})", re.as_str())
        };
        write!(f, "{}", string)
    }
}

impl Hash for MatchType {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.to_string().hash(state)
    }
}

static MATCH_RE: Lazy<Arc<Regex>> = Lazy::new(|| Arc::new(Regex::new(r"^regex\((.*)\)$").unwrap()));

pub struct MatchTypeFromStrError(String);

impl FromStr for MatchType {
    type Err = MatchTypeFromStrError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(captures) = MATCH_RE.captures(s) {
            // regex matcher
            captures
                .get(1)
                .map(|re| MatchType::Regex(Regex::new(re.into()).unwrap()))
                .ok_or(MatchTypeFromStrError("".into()))
        } else {
            // path matcher
            Ok(MatchType::Start(s.into()))
        }
    }
}

#[cfg(feature = "json-config")]
mod deserialize_match_type {
    use regex::Regex;
    use serde::{de::Visitor, Deserialize};

    use super::MatchType;

    fn from_str<E: serde::de::Error>(v: &str) -> Result<MatchType, E> {
        if let Some(captures) = super::MATCH_RE.captures(v) {
            // regex matcher
            captures
                .get(1)
                .map(|re| MatchType::Regex(Regex::new(re.into()).unwrap()))
                .ok_or(E::custom(""))
        } else {
            // path matcher
            Ok(MatchType::Start(v.into()))
        }
    }

    struct MatchTypeVisitor;

    impl<'de> Visitor<'de> for MatchTypeVisitor {
        type Value = MatchType;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter
                .write_str("A plain string for path matching or re({regex}) for regex matching.")
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            from_str(v)
        }

        fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            from_str(&v)
        }
    }

    impl<'de> Deserialize<'de> for MatchType {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            deserializer.deserialize_str(MatchTypeVisitor)
        }
    }
}
