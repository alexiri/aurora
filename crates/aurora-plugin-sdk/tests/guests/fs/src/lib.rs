use aurora_plugin_sdk::*;

struct Guest;
impl Hooks for Guest {
    fn token_mint(_i: MintInput) -> Result<MintOutput, HookError> {
        let s = std::fs::read_to_string("/etc/passwd").map_err(|e| HookError::Internal(e.to_string()))?;
        Ok(MintOutput { credentials: kv([("access_token", s.as_str())]), expires_at: 0 })
    }
}
export_hooks!(Guest);
