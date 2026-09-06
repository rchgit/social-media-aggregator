use serde_json::{json, Value};

use super::{
    check_status, require_secret, FetchPage, NetworkAdapter, NetworkError, RawImage, RawPost,
};
use crate::models::{Account, Network};
use crate::secrets::SecretResolver;

const BASE: &str = "https://open.tiktokapis.com/v2/video/list/";

/// TikTok Display API. Videos carry a cover image which is surfaced as the
/// post image; the clip itself is out of scope for this text/image release.
pub struct TikTokAdapter;

impl NetworkAdapter for TikTokAdapter {
    fn network(&self) -> Network {
        Network::TikTok
    }

    fn fetch(
        &self,
        client: &reqwest::blocking::Client,
        account: &Account,
        secrets: &SecretResolver,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<FetchPage, NetworkError> {
        let token = require_secret(secrets, account, Network::TikTok)?;
        let mut body = json!({
            "max_count": limit.min(20),
            "fields": ["id", "title", "video_description", "create_time", "share_url", "cover_image_url"],
        });
        if let Some(c) = cursor {
            body["cursor"] = json!(c);
        }

        let resp = client
            .post(BASE)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .map_err(|e| NetworkError::Request(Network::TikTok, e.to_string()))?;
        let resp = check_status(resp, Network::TikTok)?;
        let json: Value = resp
            .json()
            .map_err(|e| NetworkError::Parse(Network::TikTok, e.to_string()))?;

        if let Some(code) = json.pointer("/error/code").and_then(|v| v.as_str()) {
            if code != "ok" {
                let msg = json
                    .pointer("/error/message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                return Err(NetworkError::Parse(Network::TikTok, msg.to_string()));
            }
        }

        let data = json.get("data").cloned().unwrap_or_default();
        let after = data
            .get("cursor")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
        let has_more = data
            .get("has_more")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let mut posts = Vec::new();
        for d in data
            .get("videos")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
        {
            let Some(external_id) = d.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            let content = d
                .get("title")
                .or_else(|| d.get("video_description"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let url = d
                .get("share_url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let created_at = d
                .get("create_time")
                .and_then(|v| v.as_i64())
                .and_then(|s| chrono::DateTime::<chrono::Utc>::from_timestamp(s, 0))
                .unwrap_or_else(chrono::Utc::now);

            let mut images = Vec::new();
            if let Some(cover) = d.get("cover_image_url").and_then(|v| v.as_str()) {
                images.push(RawImage {
                    url: cover.to_string(),
                    alt_text: None,
                });
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
            next_cursor: if has_more { after } else { None },
        })
    }
}
