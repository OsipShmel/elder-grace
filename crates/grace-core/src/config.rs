//! `config/local.toml`: machine-local paths that never go into git.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config/local.toml not found (searched upwards from {0}; GRACE_ROOT can point to it)")]
    NotFound(PathBuf),
    #[error("cannot read {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("cannot parse {path}: {source}")]
    Parse { path: PathBuf, source: toml::de::Error },
}

#[derive(Debug, Clone, Deserialize)]
pub struct LocalConfig {
    pub paths: Paths,
    #[serde(default)]
    pub games: BTreeMap<String, GameConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Paths {
    pub local_dir: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GameConfig {
    /// Pristine install, read-only.
    pub pristine_dir: PathBuf,
    /// Working copy for the oracle and the shims.
    pub sandbox_dir: PathBuf,
    pub cache_dir: PathBuf,
    /// Game executable inside sandbox_dir (for the oracle).
    #[serde(default)]
    pub exe: Option<String>,
    /// Steam App ID: written to steam_appid.txt in the sandbox.
    #[serde(default)]
    pub steam_appid: Option<u32>,
    /// Title (or a substring of it) of the game window, for `oracle shot`.
    #[serde(default)]
    pub window_title: Option<String>,
}

impl LocalConfig {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_owned(),
            source,
        })?;
        toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_owned(),
            source,
        })
    }

    /// Looks for `config/local.toml` in `GRACE_ROOT`, otherwise upwards from `start`.
    pub fn discover(start: &Path) -> Result<Self, ConfigError> {
        if let Some(root) = std::env::var_os("GRACE_ROOT") {
            return Self::load(&PathBuf::from(root).join("config/local.toml"));
        }
        start
            .ancestors()
            .map(|dir| dir.join("config/local.toml"))
            .find(|candidate| candidate.is_file())
            .map_or_else(|| Err(ConfigError::NotFound(start.to_owned())), |p| Self::load(&p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_example_config() {
        let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/local.example.toml");
        let cfg = LocalConfig::load(&example).expect("example config must parse");
        assert!(cfg.games.contains_key("eldenring"));
    }
}
