# TEST-020: Gateway follow-up tests (from GW-005 review)

- SEC-37: a generic secret and (later, M3) an app connection whose host pattern covers a governed LLM host are never injection candidates — gateway test with a generic secret on `api.openai.com` / an endpoint host (secret row inserted directly, bypassing API validation) ⇒ nothing injected, governance refusal or pass-through as appropriate.
- Origin write-back: against a real test DB, a preflight mismatch updates `secrets.origin` (`status = mismatch`) and emits `NOTIFY aurora_secret_origin {secret_id, status}` (LISTEN in the test).
- Files: `services/aurora-gateway/tests/governance_db.rs` only. Agent: haiku-tester (escalate to rust-developer if the harness setup is non-obvious).
