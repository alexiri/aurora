//! argon2id hashing (m=19 MiB, t=2, p=1) and password policy (docs/04 §Email and password).

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use aurora_core::{ApiError, FieldError};
use std::collections::HashSet;
use std::sync::OnceLock;

pub const MIN_LEN: usize = 12;
pub const MAX_LEN: usize = 256;

static COMMON: &str = include_str!("../../../data/common-passwords.txt");

fn common() -> &'static HashSet<String> {
    static SET: OnceLock<HashSet<String>> = OnceLock::new();
    SET.get_or_init(|| {
        COMMON
            .lines()
            .map(|l| l.trim().to_lowercase())
            .filter(|l| !l.is_empty())
            .collect()
    })
}

fn argon() -> Argon2<'static> {
    // Constant, valid parameters.
    let params = Params::new(19_456, 2, 1, None).unwrap_or_default();
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// Policy check; the error message never echoes the password.
pub fn check(password: &str) -> Result<(), String> {
    let n = password.chars().count();
    if n < MIN_LEN {
        return Err(format!("must be at least {MIN_LEN} characters"));
    }
    if n > MAX_LEN {
        return Err(format!("must be at most {MAX_LEN} characters"));
    }
    if common().contains(&password.to_lowercase()) {
        return Err("is too common".to_owned());
    }
    Ok(())
}

/// 400 `invalid_request` with `details.fields[0].path == path`.
pub fn validate(path: &str, password: &str) -> Result<(), ApiError> {
    check(password)
        .map_err(|m| ApiError::invalid_request("Invalid password.", vec![FieldError::new(path, m)]))
}

fn hash_sync(password: &str) -> Result<String, ApiError> {
    let mut salt = [0u8; 16];
    aws_lc_rs::rand::fill(&mut salt).map_err(|_| internal())?;
    let salt = SaltString::encode_b64(&salt).map_err(|_| internal())?;
    argon()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|_| internal())
}

fn internal() -> ApiError {
    ApiError::internal("Internal error.")
}

fn verify_sync(password: &str, phc: &str) -> bool {
    PasswordHash::new(phc)
        .map(|h| argon().verify_password(password.as_bytes(), &h).is_ok())
        .unwrap_or(false)
}

fn dummy_hash() -> &'static str {
    static H: OnceLock<String> = OnceLock::new();
    H.get_or_init(|| hash_sync("dummy-password-for-timing").unwrap_or_default())
}

pub async fn hash(password: String) -> Result<String, ApiError> {
    tokio::task::spawn_blocking(move || hash_sync(&password))
        .await
        .map_err(|_| internal())?
}

/// Verify against `phc`; with `None` a dummy hash is verified (constant-ish time)
/// and the result is always false.
pub async fn verify(password: String, phc: Option<String>) -> bool {
    tokio::task::spawn_blocking(move || match phc {
        Some(p) => verify_sync(&password, &p),
        None => {
            let _ = verify_sync(&password, dummy_hash());
            false
        }
    })
    .await
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy() {
        assert!(check("short").is_err());
        assert!(check("UnBelievable").is_err());
        assert!(check("correct horse battery staple").is_ok());
        assert!(check(&"a".repeat(257)).is_err());
    }

    #[test]
    fn roundtrip() {
        let h = hash_sync("correct horse battery").unwrap();
        assert!(h.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        assert!(verify_sync("correct horse battery", &h));
        assert!(!verify_sync("wrong", &h));
    }
}
