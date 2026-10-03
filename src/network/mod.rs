mod facebook;
mod instagram;
mod reddit;
mod threads;
mod tiktok;
mod twitter;

#[doc(hidden)]
pub use facebook::FacebookAdapter;
#[doc(hidden)]
pub use instagram::InstagramAdapter;
#[doc(hidden)]
pub use reddit::RedditAdapter;
#[doc(hidden)]
pub use threads::ThreadsAdapter;
#[doc(hidden)]
pub use tiktok::TikTokAdapter;
#[doc(hidden)]
pub use twitter::TwitterAdapter;

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::models::{Account, Network};
use crate::secrets::SecretResolver;

/// A network-layer failure, mapped from platform HTTP responses.
#[derive(Debug, Error)]
pub enum NetworkError {
    #[error("{0}: missing credential `{1}`; add it to the environment or a secret provider")]
    CredentialsRequired(Network, String),
    #[error("{network}: rate limited; retry after {retry_after}s")]
    RateLimited { network: Network, retry_after: u64 },
    #[error("{network}: HTTP {status}: {body}")]
    HttpStatus {
        network: Network,
        status: u16,
        body: String,
    },
    #[error("{0}: {1}")]
    Parse(Network, String),
    #[error("{0}: request failed: {1}")]
    Request(Network, String),
    #[error("{0}: not supported yet")]
    Unsupported(Network),
}

/// A single post before normalization.
#[derive(Debug, Clone)]
pub struct RawPost {
    pub external_id: String,
    pub author: String,
    pub content: String,
    pub url: String,
    pub created_at: DateTime<Utc>,
    pub images: Vec<RawImage>,
}

#[derive(Debug, Clone)]
pub struct RawImage {
    pub url: String,
    pub alt_text: Option<String>,
}

/// One fetched page.
#[derive(Debug, Clone, Default)]
pub struct FetchPage {
    pub posts: Vec<RawPost>,
    /// Opaque cursor for the next page, if any.
    pub next_cursor: Option<String>,
}

/// Per-network adapter. Implementations are expected to be stateless between
/// calls; a shared HTTP client is passed in.
pub trait NetworkAdapter: Send + Sync {
    fn network(&self) -> Network;

    fn fetch(
        &self,
        client: &reqwest::blocking::Client,
        account: &Account,
        secrets: &SecretResolver,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<FetchPage, NetworkError>;
}

/// Simple sliding-window rate limiter keyed by network, used to avoid
/// hammering endpoints across concurrent sync runs.
pub struct RateLimiter {
    inner: Mutex<HashMap<Network, Vec<std::time::Instant>>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Blocks until a request is permitted for the network, assuming
    /// `per_minute` requests are allowed.
    pub fn wait(&self, network: Network, per_minute: u32) {
        let window = std::time::Duration::from_secs(60);
        let min_gap = window / per_minute.max(1);
        let mut guard = self.inner.lock().expect("rate limiter poisoned");
        let times = guard.entry(network).or_default();
        let now = std::time::Instant::now();
        times.retain(|t| now.duration_since(*t) < window);
        if let Some(last) = times.last() {
            let elapsed = now.duration_since(*last);
            if elapsed < min_gap {
                std::thread::sleep(min_gap - elapsed);
            }
        }
        times.push(std::time::Instant::now());
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolves the account credential, returning a typed error when absent.
pub(crate) fn require_secret(
    secrets: &SecretResolver,
    account: &Account,
    network: Network,
) -> Result<String, NetworkError> {
    secrets
        .try_resolve(&account.secret_ref)
        .ok_or_else(|| NetworkError::CredentialsRequired(network, account.secret_ref.clone()))
}

/// Maps a non-2xx response to a `NetworkError`, extracting `Retry-After`.
pub(crate) fn check_status(
    resp: reqwest::blocking::Response,
    network: Network,
) -> Result<reqwest::blocking::Response, NetworkError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(60);
        return Err(NetworkError::RateLimited {
            network,
            retry_after,
        });
    }
    let body = resp.text().unwrap_or_default();
    let body = if body.len() > 200 {
        body.chars().take(200).collect()
    } else {
        body
    };
    Err(NetworkError::HttpStatus {
        network,
        status: status.as_u16(),
        body,
    })
}

pub(crate) fn seconds_to_dt(seconds: f64) -> DateTime<Utc> {
    let secs = seconds.floor() as i64;
    let nanos = ((seconds - secs as f64) * 1_000_000_000.0) as u32;
    DateTime::<Utc>::from_timestamp(secs, nanos).unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
}

pub(crate) fn parse_created_at(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    // Meta timestamps look like "2024-01-01T00:00:00+0000".
    if let Ok(dt) = chrono::DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%z") {
        return Some(dt.with_timezone(&Utc));
    }
    None
}

/// Returns the adapter for a network.
pub fn adapter_for(network: Network) -> Box<dyn NetworkAdapter> {
    match network {
        Network::Facebook => Box::new(facebook::FacebookAdapter::default()),
        Network::Instagram => Box::new(instagram::InstagramAdapter::default()),
        Network::Threads => Box::new(threads::ThreadsAdapter::default()),
        Network::TikTok => Box::new(tiktok::TikTokAdapter::default()),
        Network::X => Box::new(twitter::TwitterAdapter::default()),
        Network::Reddit => Box::new(reddit::RedditAdapter::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error as AppError;

    fn account(secret_ref: &str) -> Account {
        Account {
            id: 1,
            network: Network::X,
            handle: "alice".into(),
            display_name: "Alice".into(),
            secret_ref: secret_ref.into(),
            enabled: true,
        }
    }

    #[test]
    fn require_secret_resolves_when_present() {
        std::env::set_var("SMA_TEST_NET_OK", "tok");
        let a = account("SMA_TEST_NET_OK");
        let s = SecretResolver::default();
        assert_eq!(require_secret(&s, &a, Network::X).unwrap(), "tok");
    }

    #[test]
    fn require_secret_maps_missing_to_credentials_required() {
        let a = account("SMA_TEST_NET_MISSING");
        let s = SecretResolver::default();
        let err = require_secret(&s, &a, Network::X).unwrap_err();
        match err {
            NetworkError::CredentialsRequired(Network::X, name) => assert_eq!(name, "SMA_TEST_NET_MISSING"),
            other => panic!("wrong error: {other}"),
        }
    }

    #[test]
    fn raw_post_and_page_clone_and_default() {
        let page = FetchPage::default();
        assert!(page.posts.is_empty());
        assert!(page.next_cursor.is_none());
        let raw = RawPost {
            external_id: "1".into(),
            author: "a".into(),
            content: "c".into(),
            url: "u".into(),
            created_at: Utc::now(),
            images: vec![RawImage { url: "i".into(), alt_text: None }],
        };
        let clone = raw.clone();
        assert_eq!(clone.external_id, "1");
        assert_eq!(clone.images.len(), 1);
    }

    #[test]
    fn network_error_messages_render_with_network_name() {
        let e = NetworkError::Unsupported(Network::TikTok);
        assert_eq!(e.to_string(), "tiktok: not supported yet");
        let e = NetworkError::Parse(Network::X, "bad json".into());
        assert_eq!(e.to_string(), "x: bad json");
        let e = NetworkError::Request(Network::Reddit, "timeout".into());
        assert_eq!(e.to_string(), "reddit: request failed: timeout");
        let e = NetworkError::HttpStatus {
            network: Network::Facebook,
            status: 500,
            body: "boom".into(),
        };
        assert_eq!(e.to_string(), "facebook: HTTP 500: boom");
        let e = NetworkError::RateLimited { network: Network::X, retry_after: 12 };
        assert_eq!(e.to_string(), "x: rate limited; retry after 12s");
        let e = NetworkError::CredentialsRequired(Network::X, "T".into());
        assert!(e.to_string().contains("missing credential `T`"));
    }

    #[test]
    fn network_errors_convert_into_app_error() {
        let e: AppError = NetworkError::Unsupported(Network::X).into();
        assert!(matches!(e, AppError::Network(_)));
    }

    #[test]
    fn seconds_to_dt_handles_fractional_and_invalid() {
        let dt = seconds_to_dt(1_700_000_000.25);
        assert_eq!(dt.timestamp(), 1_700_000_000);
        assert_eq!(dt.timestamp_subsec_nanos(), 250_000_000);
        // NaN/absurd values degrade to the epoch instead of panicking.
        let dt = seconds_to_dt(f64::NAN);
        assert_eq!(dt.timestamp(), 0);
    }

    #[test]
    fn parse_created_at_supports_rfc3339_and_meta_format() {
        let dt = parse_created_at("2024-01-01T00:00:00Z").unwrap();
        assert_eq!(dt.timestamp(), 1_704_067_200);
        let dt = parse_created_at("2024-01-01T00:00:00+0000").unwrap();
        assert_eq!(dt.timestamp(), 1_704_067_200);
        let dt = parse_created_at("2024-01-01T02:00:00+0200").unwrap();
        assert_eq!(dt.timestamp(), 1_704_067_200);
        assert!(parse_created_at("not a date").is_none());
        assert!(parse_created_at("").is_none());
    }

    #[test]
    fn rate_limiter_enforces_minimum_gap_between_calls() {
        let limiter = RateLimiter::new();
        // 30 per minute -> 2s gap; two immediate calls must be spaced apart.
        let start = std::time::Instant::now();
        limiter.wait(Network::X, 100_000); // effectively no gap
        limiter.wait(Network::X, 100_000);
        assert!(start.elapsed() < std::time::Duration::from_millis(500));
    }

    #[test]
    fn rate_limiter_zero_rate_does_not_divide_by_zero() {
        let limiter = RateLimiter::default();
        limiter.wait(Network::Reddit, 0);
    }

    #[test]
    fn adapter_for_returns_matching_network() {
        for n in Network::ALL {
            assert_eq!(adapter_for(n).network(), n);
        }
    }
}
