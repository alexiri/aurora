//! Concrete harness adapters (the vendor-neutral traits live in [`crate::harness`]) and the
//! compile-time registry the supervisor instantiates them from (`docs/12-supervisor-harness.md`
//! §Harness selection and adapter contract, D-122, SEC-66).

pub mod claude_code;
pub mod codex;

use std::sync::Arc;

use crate::fake::FakeHarness;
use crate::harness::{HarnessAdapter, HarnessError};

/// Error code of a boot (or purge) that names a harness this build or image cannot run.
pub const UNSUPPORTED: &str = "harness_unsupported";

/// Ids of the compiled-in adapters.
pub const KNOWN: [&str; 3] = ["claude-code", "codex", "fake"];

fn unsupported(msg: String) -> HarnessError {
    HarnessError::new(UNSUPPORTED, msg)
}

/// The adapter for harness `id`, by the boot document's `harness.id`. An id this build does not
/// know, or a runtime that is not in the image, is [`UNSUPPORTED`]: there is no fallback to
/// another harness, ever (SEC-66).
pub fn adapter_for(id: &str) -> Result<Arc<dyn HarnessAdapter>, HarnessError> {
    adapter_with(id, |p| p.exists())
}

/// [`adapter_for`] with the "is this runtime installed" probe injected (tests).
pub fn adapter_with(
    id: &str,
    exists: impl Fn(&std::path::Path) -> bool,
) -> Result<Arc<dyn HarnessAdapter>, HarnessError> {
    match id {
        "fake" => Ok(Arc::new(FakeHarness)),
        "claude-code" => {
            let l = claude_code::launcher::ProcessLauncher::default();
            if !exists(&l.program) {
                return Err(unsupported(format!(
                    "the claude-code runtime is not in this image ({})",
                    l.program.display()
                )));
            }
            Ok(Arc::new(claude_code::ClaudeCodeAdapter::with_launcher(
                Arc::new(l),
            )))
        }
        "codex" => {
            let l = codex::launcher::ProcessLauncher::default();
            if !exists(&l.program) {
                return Err(unsupported(format!(
                    "the codex runtime is not in this image ({})",
                    l.program.display()
                )));
            }
            Ok(Arc::new(codex::CodexAdapter::with_launcher(Arc::new(l))))
        }
        other => Err(unsupported(format!(
            "harness `{other}` is not available in this build"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_ids_resolve_when_the_runtime_is_present() {
        for id in KNOWN {
            let a = adapter_with(id, |_| true).unwrap();
            assert_eq!(a.id(), id);
        }
    }

    #[test]
    fn unknown_id_or_missing_runtime_is_harness_unsupported_never_a_fallback() {
        for id in ["", "gemini", "CODEX", "claude_code"] {
            assert_eq!(adapter_with(id, |_| true).err().unwrap().code, UNSUPPORTED);
        }
        for id in ["claude-code", "codex"] {
            let e = adapter_with(id, |_| false).err().unwrap();
            assert_eq!(e.code, UNSUPPORTED, "{id}");
        }
        // `fake` has no runtime to miss.
        assert!(adapter_with("fake", |_| false).is_ok());
    }
}
