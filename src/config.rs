use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::secrets::SecretResolver;

pub const DEFAULT_DB_FILE: &str = "sma.db";
pub const DEFAULT_CACHE_DIR: &str = "sma-cache";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub secrets: Secrets,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_db_path")]
    pub db_path: String,
    #[serde(default = "default_cache_dir")]
    pub cache_dir: String,
    #[serde(default = "default_page_size")]
    pub page_size: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Secrets {
    /// Named shell commands that emit a secret on stdout.
    #[serde(default)]
    pub commands: HashMap<String, String>,
}

fn default_db_path() -> String {
    DEFAULT_DB_FILE.to_string()
}

fn default_cache_dir() -> String {
    DEFAULT_CACHE_DIR.to_string()
}

fn default_page_size() -> usize {
    50
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            db_path: default_db_path(),
            cache_dir: default_cache_dir(),
            page_size: default_page_size(),
        }
    }
}

impl Config {
    /// Loads `path` if it exists; otherwise returns a default config with
    /// paths resolved against the file's directory (or the current dir).
    pub fn load_or_default(path: &Path) -> Result<Self> {
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let mut config = if path.exists() {
            Self::load(path)?
        } else {
            Self::default()
        };
        config.resolve_paths(base);
        Ok(config)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .map_err(|e| Error::Config(format!("failed to read {}: {e}", path.display())))?;
        let mut config: Config = toml::from_str(&text)
            .map_err(|e| Error::Config(format!("failed to parse {}: {e}", path.display())))?;
        config.migrate(path);
        Ok(config)
    }

    /// Upgrades older config shapes in place. Currently a no-op because the
    /// only schema has been stable, but kept as the migration seam so adding
    /// a `version` field later does not break existing files.
    fn migrate(&mut self, _path: &Path) {
        // Reserved for forward-compatible config migration.
    }

    pub fn resolve_paths(&mut self, base: &Path) {
        if Path::new(&self.settings.db_path).is_relative() {
            self.settings.db_path = base.join(&self.settings.db_path).display().to_string();
        }
        if Path::new(&self.settings.cache_dir).is_relative() {
            self.settings.cache_dir = base.join(&self.settings.cache_dir).display().to_string();
        }
    }

    pub fn db_path(&self) -> PathBuf {
        PathBuf::from(&self.settings.db_path)
    }

    pub fn cache_dir(&self) -> PathBuf {
        PathBuf::from(&self.settings.cache_dir)
    }

    pub fn secret_resolver(&self) -> SecretResolver {
        SecretResolver::new(self.secrets.commands.clone())
    }

    pub fn sample_toml() -> &'static str {
        r#"# Social Media Aggregator configuration.

[settings]
# Where the SQLite database lives. Relative paths resolve against the
# directory containing this config file.
db_path = "sma.db"
# Where downloaded images are cached.
cache_dir = "sma-cache"
# Feed page size used by the TUI and `sma feed`.
page_size = 50

[secrets]
# Named commands that emit a credential on stdout. Reference them from an
# account's `--secret` as `cmd:<name>`.
#   [secrets.commands]
#   twitter = "pass show social/twitter"
"#
    }
}
