//! `hooks.wasm` component checks (09 §6, §8.1): the binary must be a valid
//! WebAssembly *component*, import nothing except `aurora:plugin/host@1.0.0`
//! (plus its shared `aurora:plugin/types@1.0.0` type instance) and export `aurora:plugin/hooks@1.0.0` (and nothing else).
//!
//! Deviation (documented): Rust's `std` for `wasm32-wasip2` always imports a
//! baseline of inert WASI interfaces (stdio, environment, exit, io streams,
//! clocks, random). Those are tolerated ([`std_baseline_import`]); the hook host
//! links them as denied/stubbed. Anything else (`wasi:filesystem`,
//! `wasi:sockets`, `wasi:http`, ...) is rejected.
//!
//! Limitation: an implemented hook and an SDK-default (`unsupported`) hook are
//! indistinguishable at the component level, so `permissions.hooks` is not
//! cross-checked against exports; only the world shape is enforced.
use wasmparser::{Encoding, Parser, Payload, Validator};

use crate::Violation;

const HOST_IMPORT: &str = "aurora:plugin/host@1.0.0";
const TYPES_IMPORT: &str = "aurora:plugin/types@1.0.0";
const HOOKS_EXPORT: &str = "aurora:plugin/hooks@1.0.0";

/// WASI 0.2 interfaces that Rust `std` pulls in unconditionally.
fn std_baseline_import(name: &str) -> bool {
    let Some((iface, ver)) = name.split_once('@') else {
        return false;
    };
    ver.starts_with("0.2.")
        && matches!(
            iface,
            "wasi:cli/stdin"
                | "wasi:cli/stdout"
                | "wasi:cli/stderr"
                | "wasi:cli/environment"
                | "wasi:cli/exit"
                | "wasi:cli/terminal-input"
                | "wasi:cli/terminal-output"
                | "wasi:cli/terminal-stdin"
                | "wasi:cli/terminal-stdout"
                | "wasi:cli/terminal-stderr"
                | "wasi:io/error"
                | "wasi:io/poll"
                | "wasi:io/streams"
                | "wasi:clocks/monotonic-clock"
                | "wasi:clocks/wall-clock"
                | "wasi:random/random"
        )
}

/// Check `bytes` against the `aurora:plugin/plugin` world; `path` is the
/// violation path (normally `hooks.wasm`).
pub fn hooks_component(path: &str, bytes: &[u8]) -> Vec<Violation> {
    let mut out = Vec::new();
    let bad = |code: &'static str, msg: String| Violation::new(code, path, msg);

    if !bytes.starts_with(b"\0asm") {
        return vec![bad("hooks_wasm_invalid", "not a WebAssembly binary".into())];
    }
    if let Err(e) = Validator::new().validate_all(bytes) {
        return vec![bad(
            "hooks_wasm_invalid",
            format!("invalid WebAssembly: {e}"),
        )];
    }

    let mut depth = 0usize;
    let mut is_component = false;
    let mut has_hooks = false;
    for payload in Parser::new(0).parse_all(bytes) {
        let Ok(payload) = payload else {
            return vec![bad("hooks_wasm_invalid", "malformed WebAssembly".into())];
        };
        match payload {
            Payload::Version { encoding, .. } => {
                depth += 1;
                if depth == 1 {
                    is_component = encoding == Encoding::Component;
                    if !is_component {
                        return vec![bad(
                            "hooks_wasm_not_component",
                            "hooks.wasm is a core module, not a component".into(),
                        )];
                    }
                }
            }
            Payload::End(_) => depth = depth.saturating_sub(1),
            Payload::ComponentImportSection(s) if depth == 1 => {
                for imp in s {
                    match imp {
                        Ok(i) if i.name.0 == HOST_IMPORT || i.name.0 == TYPES_IMPORT => {}
                        Ok(i) if std_baseline_import(i.name.0) => {}
                        Ok(i) => out.push(bad(
                            "hooks_wasm_import_forbidden",
                            format!("import `{}` is not allowed; only `{HOST_IMPORT}`", i.name.0),
                        )),
                        Err(_) => {
                            return vec![bad("hooks_wasm_invalid", "malformed imports".into())];
                        }
                    }
                }
            }
            Payload::ComponentExportSection(s) if depth == 1 => {
                for exp in s {
                    match exp {
                        Ok(e) if e.name.0 == HOOKS_EXPORT => has_hooks = true,
                        Ok(e) => out.push(bad(
                            "hooks_wasm_export_unexpected",
                            format!("export `{}` is not part of the plugin world", e.name.0),
                        )),
                        Err(_) => {
                            return vec![bad("hooks_wasm_invalid", "malformed exports".into())];
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if is_component && !has_hooks {
        out.push(bad(
            "hooks_wasm_export_missing",
            format!("component does not export `{HOOKS_EXPORT}`"),
        ));
    }
    out
}
