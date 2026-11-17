//! (P5-MODELS-1, D-303, SEC-122) Model ids are inert data: one validator for every layer
//! (configuration, settings, agent PATCH, selection, the supervisor). Dependency-free.
//!
//! `MODEL_ID_PATTERN` = `^[A-Za-z0-9._:/@-]{1,200}$` (`08 §Model roles`). Bedrock profile ids and
//! ARNs and Vertex `model@version` ids fit. A value that has the shape of a credential is refused
//! as well, because no model field ever carries one.

use crate::limits::MODEL_ID_MAX;

/// The documented pattern (the code below is its implementation).
pub const MODEL_ID_PATTERN: &str = "^[A-Za-z0-9._:/@-]{1,200}$";

/// The closed role vocabulary (`large`, `medium`, `small`), in the order the docs list them.
pub const MODEL_ROLES: [&str; 3] = ["large", "medium", "small"];

/// The closed vocabulary of `model_info[..].capabilities`.
pub const MODEL_CAPABILITIES: [&str; 4] = [
    "effort",
    "thinking",
    "adaptive_thinking",
    "interleaved_thinking",
];

/// Bounds of `model_info[..].context_window`.
pub const CONTEXT_WINDOW_MIN: u32 = 1_000;
pub const CONTEXT_WINDOW_MAX: u32 = 10_000_000;

/// Whether `id` matches [`MODEL_ID_PATTERN`] and is not shaped like a credential.
pub fn validate_model_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MODEL_ID_MAX
        && id.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'/' | b'@' | b'-')
        })
        && !credential_shaped(id)
}

/// Prefixes of well-known credential formats (API keys, cloud keys, tokens, Aurora's own tokens).
fn credential_shaped(id: &str) -> bool {
    const PREFIXES: [&str; 10] = [
        "sk-", "sk_", "ghp_", "gho_", "ghs_", "xox", "aur_", "eyJ", "AIza", "glpat-",
    ];
    if PREFIXES.iter().any(|p| id.starts_with(p)) {
        return true;
    }
    // AWS access key ids: `AKIA`/`ASIA` + 16 upper-case letters or digits.
    let b = id.as_bytes();
    (id.starts_with("AKIA") || id.starts_with("ASIA"))
        && b.len() == 20
        && b.iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_real_ids() {
        for id in [
            "claude-sonnet-4-5",
            "claude-sonnet-4-5@20250929",
            "us.anthropic.claude-sonnet-4-5-20250929-v1:0",
            "arn:aws:bedrock:eu-central-1:123456789012:inference-profile/eu.anthropic.claude-haiku-4-5-v1:0",
            "glm-4.6",
            "openai/gpt-5",
        ] {
            assert!(validate_model_id(id), "{id}");
        }
        assert!(validate_model_id(&"a".repeat(200)));
    }

    #[test]
    fn refuses_inert_data_violations() {
        for id in [
            "", "a'b", "a\"b", "$(id)", "a`b`", "a\nb", "a b", "a=b", "a;b", "a\\b", "é",
        ] {
            assert!(!validate_model_id(id), "{id:?}");
        }
        assert!(!validate_model_id(&"a".repeat(201)));
    }

    #[test]
    fn refuses_credential_shaped_values() {
        for id in [
            "sk-ant-api03-abcdef",
            "sk-proj-123",
            "AKIAIOSFODNN7EXAMPLE",
            "ghp_0123456789abcdef",
            "eyJhbGciOiJIUzI1NiJ9",
            "aur_agenttoken",
            "AIzaSyA-123",
        ] {
            assert!(!validate_model_id(id), "{id}");
        }
    }
}
