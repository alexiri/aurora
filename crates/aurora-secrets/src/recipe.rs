//! Injection recipes for generic secrets (docs/08 §5).

use serde::{Deserialize, Serialize};

const MAX_REGEX_LEN: usize = 256;
const FORBIDDEN_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

fn bearer_default() -> String {
    "{value}".to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InjectionRecipe {
    Bearer,
    Header {
        name: String,
        #[serde(default = "bearer_default")]
        format: String,
    },
    Query {
        name: String,
        #[serde(default = "bearer_default")]
        format: String,
    },
    Basic {
        username: String,
    },
    PathTemplate {
        prefix: String,
    },
    PathRegex {
        pattern: String,
        replacement: String,
    },
}

fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// `{value}` must be the only brace usage.
fn check_format(f: &str) -> Result<(), String> {
    let rest = f.replace("{value}", "");
    if rest.contains('{') || rest.contains('}') {
        return Err("only the {value} substitution is allowed".into());
    }
    if !f.contains("{value}") {
        return Err("format must contain {value}".into());
    }
    Ok(())
}

impl InjectionRecipe {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Bearer => Ok(()),
            Self::Header { name, format } => {
                if !is_token(name) {
                    return Err("header name is not a valid token".into());
                }
                if FORBIDDEN_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
                    return Err(format!("header `{name}` may not be injected"));
                }
                check_format(format)
            }
            Self::Query { name, format } => {
                if name.is_empty() || name.len() > 128 {
                    return Err("query parameter name is empty or too long".into());
                }
                check_format(format)
            }
            Self::Basic { username } => {
                if username.contains(':') || username.contains(['\r', '\n', '\0']) {
                    return Err("basic username may not contain ':' or control characters".into());
                }
                Ok(())
            }
            Self::PathTemplate { prefix } => {
                if !prefix.starts_with('/')
                    || prefix.len() < 2
                    || prefix[1..].contains(['/', '\r', '\n', '\0'])
                {
                    return Err("path prefix must start with '/' and have no further '/'".into());
                }
                Ok(())
            }
            Self::PathRegex {
                pattern,
                replacement,
            } => {
                if pattern.len() > MAX_REGEX_LEN {
                    return Err("pattern longer than 256 characters".into());
                }
                if !pattern.starts_with('^') {
                    return Err("pattern must be anchored with ^".into());
                }
                regex::Regex::new(pattern).map_err(|_| "pattern does not compile".to_string())?;
                if !replacement.contains("{value}") {
                    return Err("replacement must contain {value}".into());
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(j: &str) -> InjectionRecipe {
        serde_json::from_str(j).unwrap()
    }

    #[test]
    fn parses_and_validates() {
        assert_eq!(parse(r#"{"kind":"bearer"}"#), InjectionRecipe::Bearer);
        assert!(
            parse(r#"{"kind":"header","name":"X-Api-Key","format":"{value}"}"#)
                .validate()
                .is_ok()
        );
        assert!(
            parse(r#"{"kind":"header","name":"Host","format":"{value}"}"#)
                .validate()
                .is_err()
        );
        assert!(
            parse(r#"{"kind":"header","name":"a b","format":"{value}"}"#)
                .validate()
                .is_err()
        );
        assert!(
            parse(r#"{"kind":"header","name":"X","format":"{other}{value}"}"#)
                .validate()
                .is_err()
        );
        assert!(
            parse(r#"{"kind":"query","name":"k","format":"{value}"}"#)
                .validate()
                .is_ok()
        );
        assert!(
            parse(
                r#"{"kind":"path_regex","pattern":"^/v1/([^/]+)/","replacement":"/v1/{value}/"}"#
            )
            .validate()
            .is_ok()
        );
        assert!(
            parse(r#"{"kind":"path_regex","pattern":"/v1/","replacement":"{value}"}"#)
                .validate()
                .is_err()
        );
        assert!(
            parse(r#"{"kind":"path_regex","pattern":"^(","replacement":"{value}"}"#)
                .validate()
                .is_err()
        );
        assert!(serde_json::from_str::<InjectionRecipe>(r#"{"kind":"nope"}"#).is_err());
    }
}
