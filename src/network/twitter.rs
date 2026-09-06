use serde_json::Value;

use super::{
    check_status, parse_created_at, require_secret, FetchPage, NetworkAdapter, NetworkError,
    RawImage, RawPost,
};
use crate::models::{Account, Network};
use crate::secrets::SecretResolver;

const BASE: &str = "https://api.x.com/2";

/// X (Twitter) API v2. `account.handle` is the @username; it is resolved to a
/// user id before listing tweets.
pub struct TwitterAdapter;

impl NetworkAdapter for TwitterAdapter {
    fn network(&self) -> Network {
        Network::X
    }

    fn fetch(
        &self,
        client: &reqwest::blocking::Client,
        account: &Account,
        secrets: &SecretResolver,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<FetchPage, NetworkError> {
        let token = require_secret(secrets, account, Network::X)?;
        let user_id = resolve_user_id(client, &token, &account.handle)?;

        let mut req = client
            .get(format!("{BASE}/users/{user_id}/tweets"))
            .bearer_auth(&token)
            .query(&[
                ("tweet.fields", "created_at,attachments,author_id"),
                ("expansions", "attachments.media_keys,author_id"),
                ("media.fields", "url,alt_text,preview_image_url"),
                ("user.fields", "username"),
                ("max_results", &limit.min(100).to_string()),
            ]);
        if let Some(next) = cursor {
            req = req.query(&[("pagination_token", next)]);
        }

        let resp = req
            .send()
            .map_err(|e| NetworkError::Request(Network::X, e.to_string()))?;
        let resp = check_status(resp, Network::X)?;
        let json: Value = resp
            .json()
            .map_err(|e| NetworkError::Parse(Network::X, e.to_string()))?;

        let after = json
            .pointer("/meta/next_token")
            .and_then(|v| v.as_str())
            .map(String::from);

        // Build media_key -> url/alt map from includes.
        let media = json
            .pointer("/includes/media")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

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
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let author = account.handle.trim_start_matches('@').to_string();
            let url = format!("https://x.com/{author}/status/{external_id}");
            let created_at = d
                .get("created_at")
                .and_then(|v| v.as_str())
                .and_then(parse_created_at)
                .unwrap_or_else(chrono::Utc::now);

            let mut images = Vec::new();
            if let Some(keys) = d
                .get("attachments")
                .and_then(|v| v.get("media_keys"))
                .and_then(|v| v.as_array())
            {
                for key in keys.iter().filter_map(|k| k.as_str()) {
                    if let Some(m) = media
                        .iter()
                        .find(|m| m.get("media_key").and_then(|v| v.as_str()) == Some(key))
                    {
                        let img_url = m
                            .get("url")
                            .or_else(|| m.get("preview_image_url"))
                            .and_then(|v| v.as_str());
                        if let Some(u) = img_url {
                            images.push(RawImage {
                                url: u.to_string(),
                                alt_text: m
                                    .get("alt_text")
                                    .and_then(|v| v.as_str())
                                    .map(String::from),
                            });
                        }
                    }
                }
            }

            posts.push(RawPost {
                external_id: external_id.to_string(),
                author,
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

fn resolve_user_id(
    client: &reqwest::blocking::Client,
    token: &str,
    handle: &str,
) -> Result<String, NetworkError> {
    if handle.chars().all(|c| c.is_ascii_digit()) {
        return Ok(handle.to_string());
    }
    let resp = client
        .get(format!(
            "{BASE}/users/by/username/{}",
            handle.trim_start_matches('@')
        ))
        .bearer_auth(token)
        .send()
        .map_err(|e| NetworkError::Request(Network::X, e.to_string()))?;
    let resp = check_status(resp, Network::X)?;
    let json: Value = resp
        .json()
        .map_err(|e| NetworkError::Parse(Network::X, e.to_string()))?;
    json.pointer("/data/id")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| NetworkError::Parse(Network::X, "no user id in response".into()))
}
