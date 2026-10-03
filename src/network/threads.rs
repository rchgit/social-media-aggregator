use serde_json::Value;

use super::{
    check_status, parse_created_at, require_secret, FetchPage, NetworkAdapter, NetworkError,
    RawImage, RawPost,
};
use crate::models::{Account, Network};
use crate::secrets::SecretResolver;

const DEFAULT_BASE: &str = "https://graph.threads.net/v1.0";

/// Threads via the Threads API (a Meta Graph API variant). `account.handle`
/// is the Threads user id.
pub struct ThreadsAdapter {
    base: String,
}

impl Default for ThreadsAdapter {
    fn default() -> Self {
        Self { base: DEFAULT_BASE.to_string() }
    }
}

impl ThreadsAdapter {
    /// Test-only constructor overriding the API base URL.
    #[doc(hidden)]
    #[allow(dead_code)]
    pub fn test_with_base(base: String) -> Self {
        Self { base }
    }
}

impl NetworkAdapter for ThreadsAdapter {
    fn network(&self) -> Network {
        Network::Threads
    }

    fn fetch(
        &self,
        client: &reqwest::blocking::Client,
        account: &Account,
        secrets: &SecretResolver,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<FetchPage, NetworkError> {
        let token = require_secret(secrets, account, Network::Threads)?;
        let mut req = client
            .get(format!("{}/{}/threads", self.base, account.handle))
            .query(&[
                (
                    "fields",
                    "id,text,timestamp,permalink,media_type,media_url,thumbnail_url",
                ),
                ("limit", &limit.min(100).to_string()),
                ("access_token", &token),
            ]);
        if let Some(after) = cursor {
            req = req.query(&[("after", after)]);
        }

        let resp = req
            .send()
            .map_err(|e| NetworkError::Request(Network::Threads, e.to_string()))?;
        let resp = check_status(resp, Network::Threads)?;
        let json: Value = resp
            .json()
            .map_err(|e| NetworkError::Parse(Network::Threads, e.to_string()))?;

        let after = json
            .pointer("/paging/cursors/after")
            .and_then(|v| v.as_str())
            .map(String::from);

        let mut posts = Vec::new();
        for d in json
            .get("data")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
        {
            let Some(external_id) = d.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            if d.get("media_type").and_then(|v| v.as_str()) == Some("VIDEO") {
                continue;
            }
            let content = d
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let url = d
                .get("permalink")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let created_at = d
                .get("timestamp")
                .and_then(|v| v.as_str())
                .and_then(parse_created_at)
                .unwrap_or_else(chrono::Utc::now);

            let mut images = Vec::new();
            for key in ["media_url", "thumbnail_url"] {
                if let Some(u) = d.get(key).and_then(|v| v.as_str()) {
                    if !images.iter().any(|i: &RawImage| i.url == u) {
                        images.push(RawImage {
                            url: u.to_string(),
                            alt_text: None,
                        });
                    }
                }
            }

            posts.push(RawPost {
                external_id: external_id.to_string(),
                author: account.display_name.clone(),
                content,
                url,
                created_at,
                images,
            });
        }

        Ok(FetchPage {
            posts,
            next_cursor: after,
        })
    }
}
