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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_roundtrips_through_from_str() {
        for n in Network::ALL {
            assert_eq!(n.as_str().parse::<Network>().unwrap(), n);
            assert_eq!(n.to_string(), n.as_str());
        }
    }

    #[test]
    fn from_str_is_case_insensitive_and_accepts_twitter_alias() {
        assert_eq!("FACEBOOK".parse::<Network>().unwrap(), Network::Facebook);
        assert_eq!("Reddit".parse::<Network>().unwrap(), Network::Reddit);
        assert_eq!("twitter".parse::<Network>().unwrap(), Network::X);
        assert_eq!("x".parse::<Network>().unwrap(), Network::X);
        assert_eq!(" TikTok ".trim().parse::<Network>().unwrap(), Network::TikTok);
    }

    #[test]
    fn from_str_rejects_unknown() {
        let err = "myspace".parse::<Network>().unwrap_err();
        assert!(err.contains("unknown network `myspace`"), "{err}");
    }

    #[test]
    fn icons_are_distinct_per_network() {
        let icons: Vec<_> = Network::ALL.iter().map(|n| n.icon()).collect();
        for (i, a) in icons.iter().enumerate() {
            for b in &icons[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn default_rate_limits_are_positive() {
        for n in Network::ALL {
            assert!(n.default_rate_limit() > 0, "{n}");
        }
    }

    #[test]
    fn topic_matches_is_case_insensitive_over_content_and_author() {
        let t = Topic {
            id: 1,
            name: "rust".into(),
            keywords: vec!["Rust".into(), "TUI".into()],
            enabled: true,
        };
        assert!(t.matches("I love RUST lang"));
        assert!(t.matches("a nice tui app"));
        assert!(t.matches("mixed CaSe TuI"));
        assert!(!t.matches("golang news"));
    }

    #[test]
    fn topic_ignores_empty_keywords() {
        let t = Topic {
            id: 1,
            name: "t".into(),
            keywords: vec![String::new()],
            enabled: true,
        };
        // An empty keyword would otherwise match every haystack.
        assert!(!t.matches("anything"));
    }

    #[test]
    fn account_and_post_serialize_with_snake_case_fields() {
        let post = Post {
            id: 1,
            network: Network::X,
            account_handle: "alice".into(),
            external_id: "42".into(),
            author: "alice".into(),
            content: "hi".into(),
            url: "https://x.com/a/1".into(),
            created_at: chrono::Utc::now(),
            fetched_at: chrono::Utc::now(),
            images: Vec::new(),
        };
        let v: serde_json::Value = serde_json::to_value(&post).unwrap();
        assert_eq!(v["external_id"], "42");
        assert_eq!(v["account_handle"], "alice");
        let back: Post = serde_json::from_value(v).unwrap();
        assert_eq!(back.external_id, "42");
    }

    #[test]
    fn sync_cursor_is_cloneable_and_debuggable() {
        let c = SyncCursor {
            account_id: 3,
            cursor: Some("abc".into()),
            last_synced_at: Some(chrono::Utc::now()),
        };
        let clone = c.clone();
        assert_eq!(clone.account_id, 3);
        assert_eq!(clone.cursor.as_deref(), Some("abc"));
        assert!(format!("{c:?}").contains("SyncCursor"));
    }
}
