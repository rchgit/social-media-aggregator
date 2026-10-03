use reqwest::blocking::Client;

use crate::error::Result;
use crate::models::{Network, Post, PostImage, Topic};
use crate::network::{adapter_for, NetworkAdapter, NetworkError, RateLimiter, RawPost};
use crate::secrets::SecretResolver;
use crate::storage::Storage;

/// Hard cap on pages per account per run so a single noisy account cannot
/// stall the whole sync.
const MAX_PAGES_PER_ACCOUNT: usize = 3;

/// Outcome of syncing a single account.
#[derive(Debug, Clone)]
pub struct AccountSync {
    pub account_id: i64,
    pub network: Network,
    pub handle: String,
    pub fetched: usize,
    pub inserted: usize,
    pub status: SyncStatus,
}

#[derive(Debug, Clone)]
pub enum SyncStatus {
    Ok,
    Skipped,
    CredentialsRequired(String),
    RateLimited(u64),
    Error(String),
}

impl SyncStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SyncStatus::Ok => "ok",
            SyncStatus::Skipped => "skipped",
            SyncStatus::CredentialsRequired(_) => "missing credentials",
            SyncStatus::RateLimited(_) => "rate limited",
            SyncStatus::Error(_) => "error",
        }
    }
}

/// Syncs every enabled account (optionally filtered by exact handle),
/// returning a per-account report.
pub fn sync(
    storage: &Storage,
    secrets: &SecretResolver,
    account_filter: Option<&str>,
) -> Result<Vec<AccountSync>> {
    let accounts: Vec<_> = storage
        .list_accounts()?
        .into_iter()
        .filter(|a| a.enabled)
        .filter(|a| account_filter.is_none_or(|f| a.handle == f))
        .collect();

    let topics = storage.list_topics()?;
    let enabled_topics: Vec<&Topic> = topics.iter().filter(|t| t.enabled).collect();

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("sma/0.1")
        .build()?;
    let limiter = RateLimiter::new();

    let mut report = Vec::new();
    for account in accounts {
        let adapter = adapter_for(account.network);
        report.push(sync_account(
            &client,
            storage,
            secrets,
            &limiter,
            &account,
            &enabled_topics,
            adapter.as_ref(),
        ));
    }
    Ok(report)
}

fn sync_account(
    client: &Client,
    storage: &Storage,
    secrets: &SecretResolver,
    limiter: &RateLimiter,
    account: &crate::models::Account,
    topics: &[&Topic],
    adapter: &dyn NetworkAdapter,
) -> AccountSync {
    let mut result = AccountSync {
        account_id: account.id,
        network: account.network,
        handle: account.handle.clone(),
        fetched: 0,
        inserted: 0,
        status: SyncStatus::Ok,
    };
    limiter.wait(account.network, account.network.default_rate_limit());

    let mut cursor = match storage.get_cursor(account.id) {
        Ok(c) => c.and_then(|c| c.cursor),
        Err(e) => {
            result.status = SyncStatus::Error(e.to_string());
            return result;
        }
    };

    for _page in 0..MAX_PAGES_PER_ACCOUNT {
        let page = match adapter.fetch(client, account, secrets, cursor.as_deref(), 50) {
            Ok(p) => p,
            Err(NetworkError::CredentialsRequired(_, secret)) => {
                result.status = SyncStatus::CredentialsRequired(secret);
                break;
            }
            Err(NetworkError::RateLimited { retry_after, .. }) => {
                result.status = SyncStatus::RateLimited(retry_after);
                break;
            }
            Err(e) => {
                result.status = SyncStatus::Error(e.to_string());
                break;
            }
        };

        for raw in &page.posts {
            result.fetched += 1;
            let post = normalize(raw, account);
            let matched = match_topics(topics, &post);
            match storage.insert_post(&post, &matched) {
                Ok(Some(_)) => result.inserted += 1,
                Ok(None) => {}
                Err(e) => {
                    result.status = SyncStatus::Error(e.to_string());
                    return result;
                }
            }
        }

        match page.next_cursor {
            Some(next) => {
                if let Err(e) = storage.set_cursor(account.id, Some(&next)) {
                    result.status = SyncStatus::Error(e.to_string());
                    return result;
                }
                cursor = Some(next);
            }
            None => {
                break;
            }
        }
    }

    result
}

fn normalize(raw: &RawPost, account: &crate::models::Account) -> Post {
    Post {
        id: 0,
        network: account.network,
        account_handle: account.handle.clone(),
        external_id: raw.external_id.clone(),
        author: raw.author.clone(),
        content: raw.content.clone(),
        url: raw.url.clone(),
        created_at: raw.created_at,
        fetched_at: chrono::Utc::now(),
        images: raw
            .images
            .iter()
            .map(|i| PostImage {
                id: 0,
                post_id: 0,
                url: i.url.clone(),
                local_path: None,
                width: None,
                height: None,
                alt_text: i.alt_text.clone(),
            })
            .collect(),
    }
}

fn match_topics(topics: &[&Topic], post: &Post) -> Vec<i64> {
    let haystack = format!("{} {}", post.content, post.author);
    topics
        .iter()
        .filter(|t| t.matches(&haystack))
        .map(|t| t.id)
        .collect()
}

/// Like [`sync`], but driven by caller-provided adapters; used by tests to
/// exercise the loop without touching the network.
#[doc(hidden)]
pub fn sync_with(
    storage: &Storage,
    secrets: &SecretResolver,
    account_filter: Option<&str>,
    client: &Client,
    adapters: &std::collections::HashMap<Network, Box<dyn NetworkAdapter>>,
) -> Result<Vec<AccountSync>> {
    let accounts: Vec<_> = storage
        .list_accounts()?
        .into_iter()
        .filter(|a| a.enabled)
        .filter(|a| account_filter.is_none_or(|f| a.handle == f))
        .collect();
    let topics = storage.list_topics()?;
    let enabled_topics: Vec<&Topic> = topics.iter().filter(|t| t.enabled).collect();
    let limiter = RateLimiter::new();
    let mut report = Vec::new();
    for account in accounts {
        let adapter = adapters
            .get(&account.network)
            .expect("no adapter registered for account network");
        report.push(sync_account(
            client,
            storage,
            secrets,
            &limiter,
            &account,
            &enabled_topics,
            adapter.as_ref(),
        ));
    }
    Ok(report)
}
