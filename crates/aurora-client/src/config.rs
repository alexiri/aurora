//! `~/.config/aurora/config.toml` (0600).
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::WorkspaceRef;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct AgentEntry {
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    pub url: String,
    pub api_key: String,
    pub workspace: WorkspaceRef,
    /// Stored BYO agent tokens keyed by handle.
    #[serde(default)]
    pub agents: BTreeMap<String, AgentEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("not logged in (run `aurora login`)")]
    NotLoggedIn,
    #[error("config io: {0}")]
    Io(#[from] std::io::Error),
    #[error("config parse: {0}")]
    Parse(String),
}

/// Config directory: `$AURORA_CONFIG_DIR`, else `$HOME/.config/aurora`.
pub fn config_dir() -> Result<PathBuf, ConfigError> {
    if let Some(d) = std::env::var_os("AURORA_CONFIG_DIR") {
        return Ok(PathBuf::from(d));
    }
    let home = std::env::var_os("HOME").ok_or(ConfigError::NotLoggedIn)?;
    Ok(PathBuf::from(home).join(".config").join("aurora"))
}

pub fn default_path() -> Result<PathBuf, ConfigError> {
    Ok(config_dir()?.join("config.toml"))
}

impl Config {
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ConfigError::NotLoggedIn);
            }
            Err(e) => return Err(e.into()),
        };
        toml::from_str(&text).map_err(|e| ConfigError::Parse(e.to_string()))
    }

    /// Atomic write, file mode 0600, parent directory 0700 when created.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let text = toml::to_string_pretty(self).map_err(|e| ConfigError::Parse(e.to_string()))?;
        if let Some(dir) = path.parent() {
            if !dir.exists() {
                std::fs::create_dir_all(dir)?;
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        let tmp = path.with_extension("toml.tmp");
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}
