use serde_json::Value;

use super::{
    check_status, parse_created_at, require_secret, FetchPage, NetworkAdapter, NetworkError,
    RawImage, RawPost,
};
use crate::models::{Account, Network};
use crate::secrets::SecretResolver;

const BASE: &str = "https://graph.facebook.com/v20.0";

/// Instagram via the Meta Graph API. `account.handle` is the Instagram
/// Business/creator account id.
pub struct InstagramAdapter;

impl NetworkAdapter for InstagramAdapter {
    fn network(&self) -> Network {
        Network::Instagram
    }

    fn fetch(
        &self,
        client: &reqwest::blocking::Client,
        account: &Account,
        secrets: &SecretResolver,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<FetchPage, NetworkError> {
        let token = require_secret(secrets, account, Network::Instagram)?;
        let mut req = client
            .get(format!("{BASE}/{}/media", account.handle))
            .query(&[
                (
                    "fields",
                    "id,caption,permalink,timestamp,media_type,media_url,thumbnail_url",
                ),
                ("limit", &limit.min(100).to_string()),
                ("access_token", &token),
            ]);
        if let Some(after) = cursor {
            req = req.query(&[("after", after)]);
        }

        let resp = req
            .send()
            .map_err(|e| NetworkError::Request(Network::Instagram, e.to_string()))?;
        let resp = check_status(resp, Network::Instagram)?;
        let json: Value = resp
            .json()
            .map_err(|e| NetworkError::Parse(Network::Instagram, e.to_string()))?;

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
            // Video posts have no static imagery to render in a text/image
            // feed, so skip them.
            let media_type = d.get("media_type").and_then(|v| v.as_str()).unwrap_or("");
            if media_type == "VIDEO" {
                continue;
            }
            let content = d
                .get("caption")
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
