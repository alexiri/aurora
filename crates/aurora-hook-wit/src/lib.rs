//! The `aurora:plugin@1.0.0` WIT world (docs/09 §8.1).
//!
//! Host side: `wasmtime::component::bindgen!({ path: <WIT_DIR>, world: "plugin" })`
//! (bindgen's `path` must be a literal; use `"../aurora-hook-wit/wit"` relative
//! to the host crate). Guest side: see `aurora-plugin-sdk`.

/// Absolute path of the directory holding `plugin.wit` (resolved at build time).
pub const WIT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/wit");
/// World name.
pub const WORLD: &str = "plugin";
/// Full WIT text.
pub const WIT_SOURCE: &str = include_str!("../wit/plugin.wit");
/// Package name and version.
pub const PACKAGE: &str = "aurora:plugin@1.0.0";
/// Only import a component may have.
pub const HOST_IMPORT: &str = "aurora:plugin/host@1.0.0";
/// Required export.
pub const HOOKS_EXPORT: &str = "aurora:plugin/hooks@1.0.0";

#[cfg(test)]
mod tests {
    #[test]
    fn wit_present() {
        assert!(super::WIT_SOURCE.contains("world plugin"));
        assert!(
            std::path::Path::new(super::WIT_DIR)
                .join("plugin.wit")
                .is_file()
        );
    }
}
