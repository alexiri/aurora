# CHAN-001: `aurora-channels` crate — neutral content, cards, action tokens, capability model

## Linked requirements
- M6/13 `§2 Neutral model`, `§4.3 Neutral content` (text/markdown-as-data, mentions, attachments, cards; sanitization rules), `§4.4 Action tokens` (core-signed HMAC `action-token` purpose, single-use, short-lived `ACTION_TOKEN_TTL`, bound to card/action/integration; verification API), `§4.5 Capabilities and fallbacks` (capability names as open vocabulary; fallback rendering when a capability is absent), `02 §Repository layout` (`aurora-channels`: neutral content, card schema, action tokens, capability model), `08 §4` HKDF purpose `action-token`; SEC-18 (untrusted text never promoted to platform voice), SEC-19 (action tokens HMAC-signed, single use — single-use enforcement is the API's DB job (`action_token_uses`); this crate signs/verifies and exposes the token id).

## Context
- Read `docs/13-channels-connectors.md` §2 and §4.3–§4.5 (normative). `aurora_crypto::DerivedKeys` (`Purpose::ActionToken`, HMAC sign/verify), `aurora_core::sanitize`, `aurora_protocol::vocab::open_vocab!`/`BoundedString`. Types are wire types for the connector contract (W8) ⇒ derive Serialize/Deserialize/JsonSchema; register in `aurora_protocol::schema::all()`? No — keep them in `aurora-channels` with their own schema drift test writing `schemas/channels_*.json`.

## Scope (`crates/aurora-channels/**`, `schemas/channels_*.json`)
## Acceptance
- [ ] Tests: content builders sanitize untrusted text (bidi/control stripped, markdown rendered as data); card schema round trips + JSON Schema drift; action token sign/verify (expired, tampered, wrong integration, wrong purpose key all rejected; token id stable for single-use bookkeeping); capability fallback table (e.g. no buttons ⇒ numbered text commands) per `§4.5`.
- [ ] `cargo test -p aurora-channels`, clippy, fmt, guards (no platform names — SEC-33).

## Dependencies: BE-001/003 (done). Unblocks: M6 channel core. Agent: rust-developer (Sonnet).
