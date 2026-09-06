use std::collections::HashMap;
use std::env;
use std::process::Command;

use crate::error::{Error, Result};

/// Resolves account credentials without storing secrets on disk.
///
/// A secret reference is one of:
/// - `NAME` or `env:NAME` — read the environment variable `NAME`.
/// - `cmd:provider` — run the command registered under `provider` in
///   `config.secrets.commands` and use its trimmed stdout.
#[derive(Debug, Default)]
pub struct SecretResolver {
    commands: HashMap<String, String>,
}

impl SecretResolver {
    pub fn new(commands: HashMap<String, String>) -> Self {
        Self { commands }
    }

    pub fn resolve(&self, secret_ref: &str) -> Result<String> {
        if let Some(name) = secret_ref.strip_prefix("cmd:") {
            let command = self.commands.get(name).ok_or_else(|| {
                Error::Secret(format!(
                    "no command registered for secret provider `{name}`"
                ))
            })?;
            let out = Command::new("sh")
                .args(["-c", command])
                .output()
                .map_err(|e| {
                    Error::Secret(format!("failed to run secret command `{name}`: {e}"))
                })?;
            if !out.status.success() {
                return Err(Error::Secret(format!(
                    "secret command `{name}` exited with {}",
                    out.status
                )));
            }
            let secret = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if secret.is_empty() {
                return Err(Error::Secret(format!(
                    "secret command `{name}` produced no output"
                )));
            }
            return Ok(secret);
        }

        let name = secret_ref.strip_prefix("env:").unwrap_or(secret_ref);
        env::var(name)
            .map_err(|_| Error::Secret(format!("environment variable `{name}` is not set")))
    }

    /// Returns `None` when the credential is missing instead of an error, for
    /// callers that want a soft check before syncing.
    pub fn try_resolve(&self, secret_ref: &str) -> Option<String> {
        self.resolve(secret_ref).ok()
    }
}
