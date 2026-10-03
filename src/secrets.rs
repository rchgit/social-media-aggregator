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

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver(pairs: &[(&str, &str)]) -> SecretResolver {
        SecretResolver::new(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    #[test]
    fn plain_ref_reads_environment_variable() {
        env::set_var("SMA_TEST_SECRET_PLAIN", "topsecret");
        let r = resolver(&[]);
        assert_eq!(r.resolve("SMA_TEST_SECRET_PLAIN").unwrap(), "topsecret");
    }

    #[test]
    fn env_prefix_reads_environment_variable() {
        env::set_var("SMA_TEST_SECRET_ENV", "v2");
        let r = resolver(&[]);
        assert_eq!(r.resolve("env:SMA_TEST_SECRET_ENV").unwrap(), "v2");
    }

    #[test]
    fn missing_environment_variable_is_an_error() {
        let r = resolver(&[]);
        let err = r.resolve("SMA_DEFINITELY_NOT_SET_XYZ").unwrap_err();
        assert!(err.to_string().contains("is not set"), "{err}");
        assert!(r.try_resolve("SMA_DEFINITELY_NOT_SET_XYZ").is_none());
    }

    #[test]
    fn unregistered_provider_is_an_error() {
        let r = resolver(&[]);
        let err = r.resolve("cmd:nothing").unwrap_err();
        assert!(err.to_string().contains("no command registered"), "{err}");
        assert!(r.try_resolve("cmd:nothing").is_none());
    }

    #[test]
    fn command_stdout_is_trimmed() {
        let r = resolver(&[("echo", "printf '  spaced  '")]);
        assert_eq!(r.resolve("cmd:echo").unwrap(), "spaced");
    }

    #[test]
    fn failing_command_is_an_error() {
        let r = resolver(&[("bad", "exit 3")]);
        let err = r.resolve("cmd:bad").unwrap_err();
        assert!(err.to_string().contains("exited with"), "{err}");
    }

    #[test]
    fn empty_output_is_an_error() {
        let r = resolver(&[("silent", "true")]);
        let err = r.resolve("cmd:silent").unwrap_err();
        assert!(err.to_string().contains("produced no output"), "{err}");
    }

    #[test]
    fn unrunnable_command_is_an_error() {
        // `sh -c` runs, but the exec of the binary itself fails.
        let r = resolver(&[("ghost", "definitely-not-a-real-binary-xyz")]);
        assert!(r.resolve("cmd:ghost").is_err());
    }

    #[test]
    fn try_resolve_returns_some_for_command_secret() {
        let r = resolver(&[("ok", "echo hi")]);
        assert_eq!(r.try_resolve("cmd:ok").as_deref(), Some("hi"));
    }
}
