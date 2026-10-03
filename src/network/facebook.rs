use serde_json::Value;

use super::{
    check_status, parse_created_at, require_secret, FetchPage, NetworkAdapter, NetworkError,
    RawImage, RawPost,
};
use crate::models::{Account, Network};
use crate::secrets::SecretResolver;

const DEFAULT_BASE: &str = "https://graph.facebook.com/v20.0";

/// Facebook Graph API. `account.handle` is the page or user id to read (or
/// `me` for the token owner).
pub struct FacebookAdapter {
    base: String,
}

impl Default for FacebookAdapter {
    fn default() -> Self {
        Self { base: DEFAULT_BASE.to_string() }
    }
}

impl FacebookAdapter {
    /// Test-only constructor overriding the API base URL.
    #[doc(hidden)]
    #[allow(dead_code)]
    pub fn test_with_base(base: String) -> Self {
        Self { base }
    }
}


impl NetworkAdapter for FacebookAdapter {
    fn network(&self) -> Network {
        Network::Facebook
    }

    fn fetch(
        &self,
        client: &reqwest::blocking::Client,
        account: &Account,
        secrets: &SecretResolver,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<FetchPage, NetworkError> {
        let token = require_secret(secrets, account, Network::Facebook)?;
        let mut req = client
            .get(format!("{}/{}/posts", self.base, account.handle))
            .query(&[
                (
                    "fields",
                    "id,message,story,created_time,permalink_url,full_picture,attachments",
                ),
                ("limit", &limit.min(100).to_string()),
                ("access_token", &token),
            ]);
        if let Some(after) = cursor {
            req = req.query(&[("after", after)]);
        }

        let resp = req
            .send()
            .map_err(|e| NetworkError::Request(Network::Facebook, e.to_string()))?;
        let resp = check_status(resp, Network::Facebook)?;
        let json: Value = resp
            .json()
            .map_err(|e| NetworkError::Parse(Network::Facebook, e.to_string()))?;

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
            let content = d
                .get("message")
                .or_else(|| d.get("story"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let url = d
                .get("permalink_url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let created_at = d
                .get("created_time")
                .and_then(|v| v.as_str())
                .and_then(parse_created_at)
                .unwrap_or_else(chrono::Utc::now);

            let mut images = Vec::new();
            if let Some(fp) = d.get("full_picture").and_then(|v| v.as_str()) {
                images.push(RawImage {
                    url: fp.to_string(),
                    alt_text: None,
                });
            }
            if let Some(media) = d
                .get("attachments")
                .and_then(|v| v.get("data"))
                .and_then(|v| v.as_array())
            {
                for item in media {
                    if let Some(src) = item
                        .get("media")
                        .and_then(|v| v.get("image"))
                        .and_then(|v| v.get("src"))
                        .and_then(|v| v.as_str())
                    {
                        images.push(RawImage {
                            url: src.to_string(),
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
