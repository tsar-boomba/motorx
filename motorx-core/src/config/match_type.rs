use std::fmt::Display;
use std::hash::Hash;
use std::sync::Arc;
use std::{cmp::Ordering, str::FromStr};

use once_cell::sync::Lazy;

use regex::{Captures, Regex};

#[derive(Debug, Clone)]
pub enum MatchType {
    /// Matches from start of subject string, default behavior
    Start(String),
    /// Matches if this enum's value is contained in the subject string
    Contains(String),
    /// Uses regex pattern to match on subject string, ex. `regex(this_is_my_regex_pattern)`
    Regex(Regex),
}

pub enum MatchResult<'t> {
    Start(bool),
    Contains(bool),
    Regex(Option<Captures<'t>>),
}

impl<'t> MatchResult<'t> {
    #[inline]
    pub(crate) fn is_match(&self) -> bool {
        match self {
            MatchResult::Start(matched) => *matched,
            MatchResult::Contains(matched) => *matched,
            MatchResult::Regex(captures) => captures.is_some(),
        }
    }
}

impl MatchType {
    #[inline]
    pub(crate) fn matches<'a>(&self, string: &'a str) -> MatchResult<'a> {
        match self {
            MatchType::Start(pattern) => MatchResult::Start(string.starts_with(pattern)),
            MatchType::Contains(pattern) => MatchResult::Contains(string.contains(pattern)),
            MatchType::Regex(regex) => MatchResult::Regex(regex.captures(string)),
        }
    }

    #[inline]
    pub(crate) fn priority(&self) -> usize {
        match self {
            // highest priority
            MatchType::Start(_) => usize::MIN,
            MatchType::Contains(_) => 1,
            // lowest priority
            MatchType::Regex(_) => usize::MAX,
        }
    }

    #[inline]
    fn length(&self) -> usize {
        match self {
            MatchType::Start(pat) => pat.len(),
            MatchType::Contains(pat) => pat.len(),
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
            MatchType::Contains(pat) => match other {
                MatchType::Contains(other_pat) => pat == other_pat,
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
            MatchType::Contains(pat) => format!("contains({})", pat),
            MatchType::Regex(re) => format!("regex({})", re.as_str()),
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
static MATCH_CONTAINS: Lazy<Arc<Regex>> =
    Lazy::new(|| Arc::new(Regex::new(r"^contains\((.*)\)$").unwrap()));

#[derive(Debug)]
pub struct MatchTypeFromStrError(String);

impl Display for MatchTypeFromStrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for MatchTypeFromStrError {}

impl FromStr for MatchType {
    type Err = MatchTypeFromStrError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(captures) = MATCH_RE.captures(s) {
            // regex matcher
            captures
                .get(1)
                .map(|re| MatchType::Regex(Regex::new(re.into()).unwrap()))
                .ok_or(MatchTypeFromStrError("".into()))
        } else if let Some(captures) = MATCH_CONTAINS.captures(s) {
            // contains matcher
            captures
                .get(1)
                .map(|pat| MatchType::Contains(pat.as_str().into()))
                .ok_or(MatchTypeFromStrError("".into()))
        } else {
            // path matcher
            Ok(MatchType::Start(s.into()))
        }
    }
}

#[cfg(feature = "json-config")]
mod deserialize_match_type {
    use std::str::FromStr;

    use serde::{de::Visitor, Deserialize};

    use super::MatchType;

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
            MatchType::from_str(v).map_err(|e| E::custom(e.0))
        }

        fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            MatchType::from_str(&v).map_err(|e| E::custom(e.0))
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
