//! One integration-test binary for `aurora-secrets`.
//!
//! * `seam`: the inline / memory stores and `SecretVault` (no external service).
//! * `vault_mock`: the HashiCorp Vault store against a mock HTTP server (always runs).
//! * `vault_reference`, `vault_managed`: the same store against a real Vault dev server in a
//!   throwaway container; skipped unless `AURORA_TEST_VAULT=1` (see `support::vault`).

mod seam;
mod support;
mod vault_managed;
mod vault_mock;
mod vault_reference;
