use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Supported social networks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Network {
    Facebook,
    Reddit,
    X,
    Instagram,
    Threads,
    TikTok,
}

impl Network {
    pub const ALL: [Network; 6] = [
        Network::Facebook,
        Network::Reddit,
        Network::X,
        Network::Instagram,
        Network::Threads,
        Network::TikTok,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Network::Facebook => "facebook",
            Network::Reddit => "reddit",
            Network::X => "x",
            Network::Instagram => "instagram",
            Network::Threads => "threads",
            Network::TikTok => "tiktok",
        }
    }

    /// A short glyph used to mark posts in the feed.
    pub fn icon(self) -> &'static str {
        match self {
            Network::Facebook => "f",
            Network::Reddit => "r",
            Network::X => "X",
            Network::Instagram => "Ig",
            Network::Threads => "Th",
            Network::TikTok => "Tk",
        }
    }

    /// Conservative per-minute request budget per account when no
    /// platform-specific limit is known.
    pub fn default_rate_limit(self) -> u32 {
        match self {
            Network::Reddit => 60,
            Network::X => 15,
            Network::Facebook => 200,
            Network::Instagram => 200,
            Network::Threads => 200,
            Network::TikTok => 50,
        }
    }
}

impl std::str::FromStr for Network {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "facebook" => Ok(Network::Facebook),
            "reddit" => Ok(Network::Reddit),
            "x" | "twitter" => Ok(Network::X),
            "instagram" => Ok(Network::Instagram),
            "threads" => Ok(Network::Threads),
            "tiktok" => Ok(Network::TikTok),
            _ => Err(format!("unknown network `{s}`")),
        }
    }
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single account on a network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: i64,
    pub network: Network,
    /// Username/handle without a leading `@`.
    pub handle: String,
    pub display_name: String,
    /// Name of the credential that authorizes access (see `secrets`).
    pub secret_ref: String,
    pub enabled: bool,
}

/// A topic used to filter and aggregate posts across accounts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Topic {
    pub id: i64,
    pub name: String,
    /// Case-insensitive keywords; a post matches when its text or author
    /// contains any of them.
    pub keywords: Vec<String>,
    pub enabled: bool,
}

impl Topic {
    pub fn matches(&self, haystack: &str) -> bool {
        let hay = haystack.to_lowercase();
        self.keywords
            .iter()
            .any(|k| !k.is_empty() && hay.contains(&k.to_lowercase()))
    }
}

/// A normalized post. Media other than text/images is intentionally omitted
/// in this first release.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Post {
    pub id: i64,
    pub network: Network,
    pub account_handle: String,
    /// Platform-assigned id, used for deduplication.
    pub external_id: String,
    pub author: String,
    pub content: String,
    pub url: String,
    pub created_at: DateTime<Utc>,
    pub fetched_at: DateTime<Utc>,
    pub images: Vec<PostImage>,
}

/// An image attached to a post.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostImage {
    pub id: i64,
    pub post_id: i64,
    pub url: String,
    /// On-disk cache location once downloaded.
    pub local_path: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub alt_text: Option<String>,
}

/// Per-account sync position.
#[derive(Debug, Clone)]
pub struct SyncCursor {
    pub account_id: i64,
    pub cursor: Option<String>,
    pub last_synced_at: Option<DateTime<Utc>>,
}
