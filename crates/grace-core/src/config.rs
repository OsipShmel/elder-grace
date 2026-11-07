//! `config/local.toml`: локальные пути, которые не попадают в git.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("не найден config/local.toml (искали от {0} вверх; можно задать GRACE_ROOT)")]
    NotFound(PathBuf),
    #[error("не читается {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("ошибка разбора {path}: {source}")]
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
    /// Чистая установка, только чтение.
    pub pristine_dir: PathBuf,
    /// Рабочая копия для эталона и shim.
    pub sandbox_dir: PathBuf,
    pub cache_dir: PathBuf,
    /// Исполняемый файл игры внутри sandbox_dir (для эталона).
    #[serde(default)]
    pub exe: Option<String>,
    /// Steam App ID: пишется в steam_appid.txt песочницы.
    #[serde(default)]
    pub steam_appid: Option<u32>,
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

    /// Ищет `config/local.toml` в `GRACE_ROOT`, иначе от `start` вверх по дереву.
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
        let cfg = LocalConfig::load(&example).expect("пример конфига должен разбираться");
        assert!(cfg.games.contains_key("eldenring"));
    }
}
