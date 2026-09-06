mod facebook;
mod instagram;
mod reddit;
mod threads;
mod tiktok;
mod twitter;

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
        Network::Facebook => Box::new(facebook::FacebookAdapter),
        Network::Instagram => Box::new(instagram::InstagramAdapter),
        Network::Threads => Box::new(threads::ThreadsAdapter),
        Network::TikTok => Box::new(tiktok::TikTokAdapter),
        Network::X => Box::new(twitter::TwitterAdapter),
        Network::Reddit => Box::new(reddit::RedditAdapter),
    }
}
