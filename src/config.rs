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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_documented_values() {
        let c = Config::default();
        assert_eq!(c.settings.db_path, "sma.db");
        assert_eq!(c.settings.cache_dir, "sma-cache");
        assert_eq!(c.settings.page_size, 50);
        assert!(c.secrets.commands.is_empty());
        assert_eq!(DEFAULT_DB_FILE, "sma.db");
        assert_eq!(DEFAULT_CACHE_DIR, "sma-cache");
    }

    #[test]
    fn serde_defaults_fill_missing_fields() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.settings.db_path, "sma.db");
        assert_eq!(c.settings.page_size, 50);
        let c: Config = toml::from_str("[settings]\npage_size = 7\n").unwrap();
        assert_eq!(c.settings.page_size, 7);
        assert_eq!(c.settings.db_path, "sma.db");
    }

    #[test]
    fn roundtrip_preserves_secrets_and_settings() {
        let text = r#"
[settings]
db_path = "data/other.db"
cache_dir = "data/cache"
page_size = 10

[secrets.commands]
twitter = "pass show tw"
"#;
        let c: Config = toml::from_str(text).unwrap();
        assert_eq!(c.secrets.commands.get("twitter").unwrap(), "pass show tw");
        let serialized = toml::to_string(&c).unwrap();
        let back: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(back.settings.db_path, "data/other.db");
        assert_eq!(back.secrets.commands.get("twitter").unwrap(), "pass show tw");
    }

    #[test]
    fn load_parses_file_and_migrates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sma.toml");
        std::fs::write(&path, "[settings]\npage_size = 3\n").unwrap();
        let c = Config::load(&path).unwrap();
        assert_eq!(c.settings.page_size, 3);
    }

    #[test]
    fn load_reports_read_failure() {
        let err = Config::load(Path::new("/definitely/missing/sma.toml")).unwrap_err();
        assert!(err.to_string().contains("failed to read"), "{err}");
    }

    #[test]
    fn load_reports_parse_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "[settings\nbroken").unwrap();
        let err = Config::load(&path).unwrap_err();
        assert!(err.to_string().contains("failed to parse"), "{err}");
    }

    #[test]
    fn load_or_default_uses_file_when_present_and_resolves_paths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sma.toml");
        std::fs::write(&path, "[settings]\ndb_path = \"x.db\"\n").unwrap();
        let c = Config::load_or_default(&path).unwrap();
        assert_eq!(c.db_path(), dir.path().join("x.db"));
        assert_eq!(c.cache_dir(), dir.path().join("sma-cache"));
    }

    #[test]
    fn load_or_default_falls_back_to_defaults_for_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.toml");
        let c = Config::load_or_default(&path).unwrap();
        assert_eq!(c.db_path(), dir.path().join("sma.db"));
    }

    #[test]
    fn resolve_paths_leaves_absolute_paths_alone() {
        let mut c = Config::default();
        c.settings.db_path = "/abs/db.sqlite".into();
        c.settings.cache_dir = "/abs/cache".into();
        c.resolve_paths(Path::new("/base"));
        assert_eq!(c.settings.db_path, "/abs/db.sqlite");
        assert_eq!(c.settings.cache_dir, "/abs/cache");
    }

    #[test]
    fn secret_resolver_carries_commands() {
        let mut c = Config::default();
        c.secrets.commands.insert("p".into(), "echo hi".into());
        let r = c.secret_resolver();
        assert_eq!(r.try_resolve("cmd:p").as_deref(), Some("hi"));
    }

    #[test]
    fn sample_toml_parses_into_default_config() {
        let c: Config = toml::from_str(Config::sample_toml()).unwrap();
        assert_eq!(c.settings.page_size, 50);
    }
}
