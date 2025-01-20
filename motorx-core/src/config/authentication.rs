use regex::Regex;

#[cfg_attr(feature = "serde-config", derive(serde::Deserialize))]
#[derive(Debug)]
pub struct Authentication {
    #[cfg_attr(feature = "serde-config", serde(default))]
    pub exclude: Vec<PathWithWildCard>,
    pub source: AuthenticationSource,
}

#[cfg_attr(feature = "serde-config", derive(serde::Deserialize))]
#[cfg_attr(feature = "serde-config", serde(rename_all = "snake_case"))]
#[derive(Debug)]
pub enum AuthenticationSource {
    /// Authenticate with another registered upstream
    Upstream {
        name: String,
        path: String,
        #[cfg_attr(feature = "serde-config", serde(default))]
        key: usize,
    },
    Path(String),
}

#[derive(Debug)]
pub enum PathWithWildCard {
    Path(String),
    WithWildCard(Regex),
}

impl PathWithWildCard {
    pub fn matches(&self, subject_path: &str) -> bool {
        match self {
            PathWithWildCard::Path(path) => path == subject_path,
            PathWithWildCard::WithWildCard(re) => re.is_match(subject_path),
        }
    }
}

#[cfg(feature = "serde-config")]
mod de_path_with_wild_card {
    use regex::Regex;
    use serde::de::{Deserialize, Visitor};

    use super::PathWithWildCard;

    struct PathWithWildCardVisitor;

    impl<'de> Visitor<'de> for PathWithWildCardVisitor {
        type Value = PathWithWildCard;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(
                formatter,
                "A valid path, optionally with wildcards (ex. /path/here or /path/with/*/wildcards"
            )
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            if !v.contains("*") {
                Ok(PathWithWildCard::Path(v.to_owned()))
            } else {
                let re_string = v.replace("*", ".+");
                let regex =
                    Regex::new(&format!(r"^{re_string}$")).map_err(|e| E::custom(e.to_string()))?;
                Ok(PathWithWildCard::WithWildCard(regex))
            }
        }
    }

    impl<'de> Deserialize<'de> for PathWithWildCard {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            deserializer.deserialize_str(PathWithWildCardVisitor)
        }
    }
}
