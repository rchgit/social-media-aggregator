use serde_json::Value;

use super::{
    check_status, seconds_to_dt, FetchPage, NetworkAdapter, NetworkError, RawImage, RawPost,
};
use crate::models::{Account, Network};
use crate::secrets::SecretResolver;

const USER_AGENT: &str = "linux:sma:0.1 (social media aggregator)";
const BASE: &str = "https://www.reddit.com";

/// Reddit's public JSON API works without authentication for read-only
/// listing of a user's submissions, so this adapter is fully functional
/// out of the box. An optional OAuth bearer token is used when configured.
pub struct RedditAdapter;

impl NetworkAdapter for RedditAdapter {
    fn network(&self) -> Network {
        Network::Reddit
    }

    fn fetch(
        &self,
        client: &reqwest::blocking::Client,
        account: &Account,
        secrets: &SecretResolver,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<FetchPage, NetworkError> {
        let mut req = client
            .get(format!("{BASE}/user/{}/submitted.json", account.handle))
            .query(&[("limit", limit.min(100).to_string())])
            .header(reqwest::header::USER_AGENT, USER_AGENT);
        if let Some(token) = secrets.try_resolve(&account.secret_ref) {
            req = req.bearer_auth(token);
        }
        if let Some(after) = cursor {
            req = req.query(&[("after", after)]);
        }

        let resp = req
            .send()
            .map_err(|e| NetworkError::Request(Network::Reddit, e.to_string()))?;
        let resp = check_status(resp, Network::Reddit)?;
        let json: Value = resp
            .json()
            .map_err(|e| NetworkError::Parse(Network::Reddit, e.to_string()))?;

        let data = json
            .get("data")
            .ok_or_else(|| NetworkError::Parse(Network::Reddit, "missing `data`".into()))?;
        let after = data.get("after").and_then(|v| v.as_str()).map(String::from);

        let mut posts = Vec::new();
        let children = data
            .get("children")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for child in children {
            let Some(d) = child.get("data").cloned() else {
                continue;
            };
            let Some(external_id) = d.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            let author = d
                .get("author")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let title = d.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let selftext = d.get("selftext").and_then(|v| v.as_str()).unwrap_or("");
            let content = if title.is_empty() {
                selftext.to_string()
            } else if selftext.is_empty() {
                title.to_string()
            } else {
                format!("{title}\n\n{selftext}")
            };
            let permalink = d.get("permalink").and_then(|v| v.as_str()).unwrap_or("");
            let url = format!("{BASE}{permalink}");
            let created = d
                .get("created_utc")
                .and_then(|v| v.as_f64())
                .map(seconds_to_dt)
                .unwrap_or_else(Utc::now);
            let images = extract_images(&d);
            posts.push(RawPost {
                external_id: external_id.to_string(),
                author,
                content,
                url,
                created_at: created,
                images,
            });
        }

        Ok(FetchPage {
            posts,
            next_cursor: after,
        })
    }
}

use chrono::Utc;

fn extract_images(d: &Value) -> Vec<RawImage> {
    let mut out = Vec::new();

    // Single-image posts and link previews expose `preview.images[].source.url`.
    if let Some(images) = d
        .get("preview")
        .and_then(|v| v.get("images"))
        .and_then(|v| v.as_array())
    {
        for img in images {
            if let Some(url) = img
                .get("source")
                .and_then(|v| v.get("url"))
                .and_then(|v| v.as_str())
            {
                push_unique(&mut out, url);
            }
        }
    }

    // Gallery posts expose `media_metadata` keyed by id with an `s.u` url.
    if let Some(meta) = d.get("media_metadata").and_then(|v| v.as_object()) {
        for item in meta.values() {
            if let Some(url) = item
                .get("s")
                .and_then(|v| v.get("u"))
                .and_then(|v| v.as_str())
            {
                push_unique(&mut out, url);
            }
        }
    }

    out
}

fn push_unique(out: &mut Vec<RawImage>, url: &str) {
    let url = html_unescape(url);
    if !out.iter().any(|i| i.url == url) {
        out.push(RawImage {
            url,
            alt_text: None,
        });
    }
}

/// Reddit encodes `&amp;` inside image URLs.
fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
}
