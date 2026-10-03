//! Adapter tests drive each `NetworkAdapter` against a throwaway local HTTP
//! server via the test-only `with_base` seam, so parsing, pagination, and
//! error mapping are exercised without touching real network APIs.

use social_media_aggregator::models::{Account, Network};
use social_media_aggregator::network::{
    adapter_for, NetworkAdapter, NetworkError,
};
use social_media_aggregator::secrets::SecretResolver;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::Duration;

// ---- tiny blocking HTTP server ------------------------------------------

/// Serves one queued response per accepted connection and records each raw
/// request. Runs on a background thread; dropped when the test ends.
struct TestServer {
    base: String,
    requests: mpsc::Receiver<String>,
}

impl TestServer {
    fn start(responses: Vec<(u16, String)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = match listener.accept() {
                    Ok(s) => s,
                    Err(_) => return,
                };
                let request = read_request(&mut stream);
                let _ = tx.send(request);
                let reason = match status {
                    200 => "OK",
                    401 => "Unauthorized",
                    404 => "Not Found",
                    429 => "Too Many Requests",
                    500 => "Internal Server Error",
                    _ => "OK",
                };
                let resp = format!(
                    "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\nretry-after: 42\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        Self {
            base: format!("http://127.0.0.1:{port}"),
            requests: rx,
        }
    }

    fn last_request(&self) -> String {
        self.requests
            .recv_timeout(Duration::from_secs(5))
            .expect("server recorded no request")
    }
}

/// Reads until end of headers plus any content-length body.
fn read_request(stream: &mut TcpStream) -> String {
    let mut buf = [0u8; 4096];
    let mut data = Vec::new();
    let n = stream.read(&mut buf).unwrap_or(0);
    data.extend_from_slice(&buf[..n]);
    String::from_utf8_lossy(&data).to_string()
}

fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

fn account(network: Network, handle: &str, secret_ref: &str) -> Account {
    Account {
        id: 1,
        network,
        handle: handle.into(),
        display_name: "Display".into(),
        secret_ref: secret_ref.into(),
        enabled: true,
    }
}

fn secrets() -> SecretResolver {
    SecretResolver::default()
}

fn secrets_with(var: &str) -> SecretResolver {
    std::env::set_var(var, "test-token");
    SecretResolver::default()
}

// ---- facebook ------------------------------------------------------------

#[test]
fn facebook_parses_posts_attachments_and_cursor() {
    let body = r#"{
        "data": [
            {
                "id": "fb1",
                "message": "hello world",
                "created_time": "2024-01-01T00:00:00+0000",
                "permalink_url": "https://fb.com/fb1",
                "full_picture": "https://cdn/full.jpg",
                "attachments": {"data": [{"media": {"image": {"src": "https://cdn/attach.png"}}}]}
            },
            {"id": "fb2", "story": "story text", "created_time": "2024-01-02T03:04:05+0000"}
        ],
        "paging": {"cursors": {"after": "CURSOR1"}}
    }"#;
    let server = TestServer::start(vec![(200, body.to_string())]);
    let adapter = social_media_aggregator::network::FacebookAdapter::test_with_base(server.base.clone());
    let acc = account(Network::Facebook, "pageid", "SMA_FB_TOKEN");
    let _s = secrets_with("SMA_FB_TOKEN");

    let page = adapter.fetch(&client(), &acc, &_s, None, 25).unwrap();
    assert_eq!(page.posts.len(), 2);
    assert_eq!(page.next_cursor.as_deref(), Some("CURSOR1"));

    let p0 = &page.posts[0];
    assert_eq!(p0.external_id, "fb1");
    assert_eq!(p0.content, "hello world");
    assert_eq!(p0.author, "Display");
    assert_eq!(p0.url, "https://fb.com/fb1");
    assert_eq!(p0.created_at.timestamp(), 1_704_067_200);
    assert_eq!(p0.images.len(), 2);
    assert_eq!(p0.images[0].url, "https://cdn/full.jpg");
    assert_eq!(p0.images[1].url, "https://cdn/attach.png");

    // story is used as fallback content.
    assert_eq!(page.posts[1].content, "story text");

    let req = server.last_request();
    assert!(req.contains("GET /pageid/posts"), "{req}");
    assert!(req.contains("access_token=test-token"), "{req}");
    assert!(req.contains("limit=25"), "{req}");

    // Cursor is forwarded as `after`.
    let server2 = TestServer::start(vec![(200, r#"{"data":[]}"#.to_string())]);
    let adapter2 = social_media_aggregator::network::FacebookAdapter::test_with_base(server2.base.clone());
    adapter2.fetch(&client(), &acc, &_s, Some("AFT1"), 25).unwrap();
    assert!(server2.last_request().contains("after=AFT1"));
}

#[test]
fn facebook_requires_token() {
    let server = TestServer::start(vec![]);
    let adapter = social_media_aggregator::network::FacebookAdapter::test_with_base(server.base.clone());
    let acc = account(Network::Facebook, "pageid", "SMA_FB_MISSING");
    let err = adapter.fetch(&client(), &acc, &secrets(), None, 25).unwrap_err();
    assert!(matches!(err, NetworkError::CredentialsRequired(Network::Facebook, _)));
}

#[test]
fn facebook_maps_http_status_and_parse_errors() {
    let server = TestServer::start(vec![(500, "boom".to_string())]);
    let adapter = social_media_aggregator::network::FacebookAdapter::test_with_base(server.base.clone());
    let acc = account(Network::Facebook, "p", "SMA_FB_TOKEN2");
    let s = secrets_with("SMA_FB_TOKEN2");
    match adapter.fetch(&client(), &acc, &s, None, 25).unwrap_err() {
        NetworkError::HttpStatus { status, body, .. } => {
            assert_eq!(status, 500);
            assert_eq!(body, "boom");
        }
        other => panic!("wrong error: {other}"),
    }

    let server = TestServer::start(vec![(200, "{bad json".to_string())]);
    let adapter = social_media_aggregator::network::FacebookAdapter::test_with_base(server.base.clone());
    match adapter.fetch(&client(), &acc, &s, None, 25).unwrap_err() {
        NetworkError::Parse(Network::Facebook, _) => {}
        other => panic!("wrong error: {other}"),
    }
}

// ---- instagram -----------------------------------------------------------

#[test]
fn instagram_parses_media_skips_video_and_dedupes_images() {
    let body = r#"{
        "data": [
            {
                "id": "ig1",
                "caption": "sunset pic",
                "permalink": "https://instagr.am/p/ig1",
                "timestamp": "2024-06-01T12:00:00+0000",
                "media_type": "IMAGE",
                "media_url": "https://cdn/img.jpg",
                "thumbnail_url": "https://cdn/img.jpg"
            },
            {"id": "ig2", "media_type": "VIDEO", "media_url": "https://cdn/v.mp4"}
        ],
        "paging": {"cursors": {"after": "IGC"}}
    }"#;
    let server = TestServer::start(vec![(200, body.to_string())]);
    let adapter = social_media_aggregator::network::InstagramAdapter::test_with_base(server.base.clone());
    let acc = account(Network::Instagram, "1789", "SMA_IG_TOKEN");
    let s = secrets_with("SMA_IG_TOKEN");

    let page = adapter.fetch(&client(), &acc, &s, None, 10).unwrap();
    assert_eq!(page.next_cursor.as_deref(), Some("IGC"));
    // VIDEO entry skipped.
    assert_eq!(page.posts.len(), 1);
    let p = &page.posts[0];
    assert_eq!(p.external_id, "ig1");
    assert_eq!(p.content, "sunset pic");
    assert_eq!(p.url, "https://instagr.am/p/ig1");
    assert_eq!(p.created_at.timestamp(), 1_717_243_200);
    // Duplicate media_url/thumbnail_url collapses to one image.
    assert_eq!(p.images.len(), 1);
    assert_eq!(p.images[0].url, "https://cdn/img.jpg");

    let req = server.last_request();
    assert!(req.contains("GET /1789/media"), "{req}");
    assert!(req.contains("access_token=test-token"), "{req}");
}

#[test]
fn instagram_requires_token() {
    let server = TestServer::start(vec![]);
    let adapter = social_media_aggregator::network::InstagramAdapter::test_with_base(server.base.clone());
    let acc = account(Network::Instagram, "1789", "SMA_IG_MISSING");
    assert!(matches!(
        adapter.fetch(&client(), &acc, &secrets(), None, 10).unwrap_err(),
        NetworkError::CredentialsRequired(Network::Instagram, _)
    ));
}

// ---- threads -------------------------------------------------------------

#[test]
fn threads_parses_text_media_and_skips_video() {
    let body = r#"{
        "data": [
            {"id": "th1", "text": "threads post", "permalink": "https://threads.net/th1",
             "timestamp": "2024-03-01T00:00:00+0000", "media_type": "IMAGE",
             "media_url": "https://cdn/t.jpg", "thumbnail_url": "https://cdn/t2.jpg"},
            {"id": "th2", "media_type": "VIDEO"}
        ],
        "paging": {"cursors": {"after": "THC"}}
    }"#;
    let server = TestServer::start(vec![(200, body.to_string())]);
    let adapter = social_media_aggregator::network::ThreadsAdapter::test_with_base(server.base.clone());
    let acc = account(Network::Threads, "thuser", "SMA_TH_TOKEN");
    let s = secrets_with("SMA_TH_TOKEN");

    let page = adapter.fetch(&client(), &acc, &s, None, 10).unwrap();
    assert_eq!(page.posts.len(), 1);
    assert_eq!(page.next_cursor.as_deref(), Some("THC"));
    let p = &page.posts[0];
    assert_eq!(p.external_id, "th1");
    assert_eq!(p.content, "threads post");
    assert_eq!(p.url, "https://threads.net/th1");
    assert_eq!(p.created_at.timestamp(), 1_709_251_200);
    assert_eq!(p.images.len(), 2);

    let req = server.last_request();
    assert!(req.contains("GET /thuser/threads"), "{req}");
}

#[test]
fn threads_requires_token() {
    let server = TestServer::start(vec![]);
    let adapter = social_media_aggregator::network::ThreadsAdapter::test_with_base(server.base.clone());
    let acc = account(Network::Threads, "u", "SMA_TH_MISSING");
    assert!(matches!(
        adapter.fetch(&client(), &acc, &secrets(), None, 10).unwrap_err(),
        NetworkError::CredentialsRequired(Network::Threads, _)
    ));
}

// ---- tiktok --------------------------------------------------------------

#[test]
fn tiktok_posts_json_and_parses_videos() {
    let body = r#"{
        "error": {"code": "ok", "message": ""},
        "data": {
            "cursor": "TT10",
            "has_more": true,
            "videos": [
                {"id": "v1", "title": "my video", "share_url": "https://tt/v1",
                 "create_time": 1700000000, "cover_image_url": "https://cdn/cover.jpg"}
            ]
        }
    }"#;
    let server = TestServer::start(vec![(200, body.to_string())]);
    let adapter = social_media_aggregator::network::TikTokAdapter::test_with_base(server.base.clone());
    let acc = account(Network::TikTok, "ttuser", "SMA_TT_TOKEN");
    let s = secrets_with("SMA_TT_TOKEN");

    let page = adapter.fetch(&client(), &acc, &s, Some("TT0"), 20).unwrap();
    let p = &page.posts[0];
    assert_eq!(p.external_id, "v1");
    assert_eq!(p.content, "my video");
    assert_eq!(p.url, "https://tt/v1");
    assert_eq!(p.created_at.timestamp(), 1_700_000_000);
    assert_eq!(p.images.len(), 1);
    assert_eq!(p.images[0].url, "https://cdn/cover.jpg");
    // has_more=true -> cursor returned.
    assert_eq!(page.next_cursor.as_deref(), Some("TT10"));

    let req = server.last_request();
    assert!(req.starts_with("POST"), "{req}");
    assert!(req.contains("cursor\":\"TT0"), "{req}");
}

#[test]
fn tiktok_has_more_false_clears_cursor() {
    let body = r#"{
        "error": {"code": "ok"},
        "data": {"cursor": "TT2", "has_more": false, "videos": [
            {"id": "v2", "video_description": "desc only", "create_time": 123}
        ]}
    }"#;
    let server = TestServer::start(vec![(200, body.to_string())]);
    let adapter = social_media_aggregator::network::TikTokAdapter::test_with_base(server.base.clone());
    let acc = account(Network::TikTok, "ttuser", "SMA_TT_TOKEN");
    let s = secrets_with("SMA_TT_TOKEN");
    let page = adapter.fetch(&client(), &acc, &s, None, 20).unwrap();
    assert!(page.next_cursor.is_none());
    // title missing -> falls back to video_description.
    assert_eq!(page.posts[0].content, "desc only");
}

#[test]
fn tiktok_maps_api_error_code_to_parse_error() {
    let body = r#"{"error": {"code": "access_token_invalid", "message": "bad token"}}"#;
    let server = TestServer::start(vec![(200, body.to_string())]);
    let adapter = social_media_aggregator::network::TikTokAdapter::test_with_base(server.base.clone());
    let acc = account(Network::TikTok, "u", "SMA_TT_TOKEN");
    let s = secrets_with("SMA_TT_TOKEN");
    match adapter.fetch(&client(), &acc, &s, None, 20).unwrap_err() {
        NetworkError::Parse(Network::TikTok, msg) => assert_eq!(msg, "bad token"),
        other => panic!("wrong error: {other}"),
    }
}

#[test]
fn tiktok_requires_token() {
    let server = TestServer::start(vec![]);
    let adapter = social_media_aggregator::network::TikTokAdapter::test_with_base(server.base.clone());
    let acc = account(Network::TikTok, "u", "SMA_TT_MISSING");
    assert!(matches!(
        adapter.fetch(&client(), &acc, &secrets(), None, 20).unwrap_err(),
        NetworkError::CredentialsRequired(Network::TikTok, _)
    ));
}

// ---- twitter / X ---------------------------------------------------------

#[test]
fn twitter_resolves_username_then_fetches_tweets_with_media() {
    let user_body = r#"{"data": {"id": "999"}}"#;
    let tweets_body = r#"{
        "data": [
            {"id": "tw1", "text": "hello x",
             "created_at": "2024-01-01T00:00:00.000Z",
             "attachments": {"media_keys": ["k1", "k_missing"]}},
            {"id": "tw2", "text": "no media", "created_at": "2024-01-02T00:00:00.000Z"}
        ],
        "includes": {"media": [
            {"media_key": "k1", "url": "https://cdn/pic.png", "alt_text": "alt!"},
            {"media_key": "k9", "preview_image_url": "https://cdn/prev.jpg"}
        ]},
        "meta": {"next_token": "NEXT1"}
    }"#;
    let server = TestServer::start(vec![
        (200, user_body.to_string()),
        (200, tweets_body.to_string()),
    ]);
    let adapter = social_media_aggregator::network::TwitterAdapter::test_with_base(server.base.clone());
    let acc = account(Network::X, "@alice", "SMA_X_TOKEN");
    let s = secrets_with("SMA_X_TOKEN");

    let page = adapter.fetch(&client(), &acc, &s, None, 30).unwrap();
    assert_eq!(page.posts.len(), 2);
    assert_eq!(page.next_cursor.as_deref(), Some("NEXT1"));

    let p0 = &page.posts[0];
    assert_eq!(p0.external_id, "tw1");
    assert_eq!(p0.author, "alice");
    assert_eq!(p0.url, "https://x.com/alice/status/tw1");
    assert_eq!(p0.created_at.timestamp(), 1_704_067_200);
    // k1 resolves with alt text; k_missing is skipped.
    assert_eq!(p0.images.len(), 1);
    assert_eq!(p0.images[0].url, "https://cdn/pic.png");
    assert_eq!(p0.images[0].alt_text.as_deref(), Some("alt!"));

    // Second call: numeric handle skips user resolution.
    let server2 = TestServer::start(vec![(200, tweets_body.to_string())]);
    let adapter2 = social_media_aggregator::network::TwitterAdapter::test_with_base(server2.base.clone());
    let acc2 = account(Network::X, "777", "SMA_X_TOKEN");
    let page2 = adapter2.fetch(&client(), &acc2, &s, Some("TOK"), 30).unwrap();
    assert_eq!(page2.posts.len(), 2);
    let req = server2.last_request();
    assert!(req.contains("users/777/tweets"), "{req}");
    assert!(req.contains("pagination_token=TOK"), "{req}");
}

#[test]
fn twitter_requires_token_and_maps_user_lookup_failure() {
    let server = TestServer::start(vec![]);
    let adapter = social_media_aggregator::network::TwitterAdapter::test_with_base(server.base.clone());
    let acc = account(Network::X, "alice", "SMA_X_MISSING");
    assert!(matches!(
        adapter.fetch(&client(), &acc, &secrets(), None, 30).unwrap_err(),
        NetworkError::CredentialsRequired(Network::X, _)
    ));

    // Non-numeric handle + user response missing data.id -> Parse error.
    let server = TestServer::start(vec![(200, r#"{"errors":[]}"#.to_string())]);
    let adapter = social_media_aggregator::network::TwitterAdapter::test_with_base(server.base.clone());
    let acc = account(Network::X, "alice", "SMA_X_TOKEN");
    let s = secrets_with("SMA_X_TOKEN");
    match adapter.fetch(&client(), &acc, &s, None, 30).unwrap_err() {
        NetworkError::Parse(Network::X, msg) => {
            assert!(msg.contains("no user id"), "{msg}");
        }
        other => panic!("wrong error: {other}"),
    }
}

// ---- reddit ---------------------------------------------------------------

#[test]
fn reddit_parses_listings_images_and_cursor_without_auth() {
    let body = r#"{
        "data": {
            "after": "t3_abc",
            "children": [
                {"data": {
                    "id": "r1", "author": "speaker", "title": "my title",
                    "selftext": "body text", "permalink": "/r/rust/comments/r1/x/",
                    "created_utc": 1700000123.0,
                    "preview": {"images": [{"source": {"url": "https://preview/a.png"}}]}
                }},
                {"data": {
                    "id": "r2", "author": "other", "title": "gallery",
                    "selftext": "", "permalink": "/r/pics/r2/",
                    "created_utc": 1700000456.0,
                    "media_metadata": {"m1": {"s": {"u": "https://g/1.png"}},
                                        "m2": {"s": {"u": "https://g/2.png"}}}
                }},
                {"data": {"id": "r3", "title": "link only", "selftext": "",
                          "permalink": "", "created_utc": 1700000789.0}},
                {"data": {}},
                {}
            ]
        }
    }"#;
    let server = TestServer::start(vec![(200, body.to_string())]);
    let adapter = social_media_aggregator::network::RedditAdapter::test_with_base(server.base.clone());
    let acc = account(Network::Reddit, "speaker", "");
    // Reddit needs no secret; empty resolver must still work.
    let page = adapter.fetch(&client(), &acc, &secrets(), None, 100).unwrap();

    assert_eq!(page.next_cursor.as_deref(), Some("t3_abc"));
    // children 4/5 have no id -> skipped.
    assert_eq!(page.posts.len(), 3);

    let p0 = &page.posts[0];
    assert_eq!(p0.external_id, "r1");
    let base = &server.base;
    assert_eq!(p0.url, format!("{base}/r/rust/comments/r1/x/"));
    assert_eq!(p0.created_at.timestamp(), 1_700_000_123);
    assert_eq!(p0.images.len(), 1);

    // Gallery images unescaped and merged after preview image.
    let p1 = &page.posts[1];
    assert_eq!(p1.content, "gallery");
    assert_eq!(p1.images.len(), 2);
    assert_eq!(p1.images[0].url, "https://g/1.png");

    // Title-only post has no images.
    assert_eq!(page.posts[2].content, "link only");
    assert!(page.posts[2].images.is_empty());

    let req = server.last_request();
    assert!(req.contains("/user/speaker/submitted.json"), "{req}");
    assert!(req.contains("linux:sma:0.1"), "{req}");
}

#[test]
fn reddit_title_empty_uses_selftext_and_html_unescape_applies() {
    let body = r#"{
        "data": {"after": null, "children": [
            {"data": {"id": "r9", "author": "a", "title": "", "selftext": "only body",
                      "permalink": "/x/", "created_utc": 1.5,
                      "preview": {"images": [{"source": {"url": "https://p/b.png?w=1&amp;h=2"}}]}}
            }
        ]}
    }"#;
    let server = TestServer::start(vec![(200, body.to_string())]);
    let adapter = social_media_aggregator::network::RedditAdapter::test_with_base(server.base.clone());
    let acc = account(Network::Reddit, "a", "");
    let page = adapter.fetch(&client(), &acc, &secrets(), None, 100).unwrap();
    assert_eq!(page.posts[0].content, "only body");
    assert_eq!(page.posts[0].images[0].url, "https://p/b.png?w=1&h=2");
    assert!(page.next_cursor.is_none());
}

#[test]
fn reddit_missing_data_is_parse_error_and_cursor_passes_through() {
    let server = TestServer::start(vec![(200, r#"{"error": 1}"#.to_string())]);
    let adapter = social_media_aggregator::network::RedditAdapter::test_with_base(server.base.clone());
    let acc = account(Network::Reddit, "a", "");
    match adapter.fetch(&client(), &acc, &secrets(), None, 10).unwrap_err() {
        NetworkError::Parse(Network::Reddit, msg) => assert!(msg.contains("missing `data`"), "{msg}"),
        other => panic!("wrong error: {other}"),
    }

    let server = TestServer::start(vec![(200, r#"{"data":{"children":[]}}"#.to_string())]);
    let adapter = social_media_aggregator::network::RedditAdapter::test_with_base(server.base.clone());
    let page = adapter.fetch(&client(), &acc, &secrets(), Some("AFT"), 10).unwrap();
    assert!(page.posts.is_empty());
    assert!(page.next_cursor.is_none());
}

// ---- shared HTTP error mapping -------------------------------------------

#[test]
fn check_status_maps_429_with_retry_after_header() {
    let body = r#"{"data": {"children": []}}"#;
    let server = TestServer::start(vec![(429, body.to_string())]);
    let adapter = social_media_aggregator::network::RedditAdapter::test_with_base(server.base.clone());
    let acc = account(Network::Reddit, "a", "");
    match adapter.fetch(&client(), &acc, &secrets(), None, 10).unwrap_err() {
        NetworkError::RateLimited { network, retry_after } => {
            assert_eq!(network, Network::Reddit);
            // Our tiny server always sends retry-after: 42.
            assert_eq!(retry_after, 42);
        }
        other => panic!("wrong error: {other}"),
    }
}

#[test]
fn check_status_truncates_long_error_bodies() {
    let long = "x".repeat(500);
    let server = TestServer::start(vec![(404, long.clone())]);
    let adapter = social_media_aggregator::network::RedditAdapter::test_with_base(server.base.clone());
    let acc = account(Network::Reddit, "a", "");
    match adapter.fetch(&client(), &acc, &secrets(), None, 10).unwrap_err() {
        NetworkError::HttpStatus { status, body, .. } => {
            assert_eq!(status, 404);
            assert_eq!(body.len(), 200);
        }
        other => panic!("wrong error: {other}"),
    }
}

#[test]
fn adapter_for_defaults_point_at_production_hosts() {
    let fb = social_media_aggregator::network::FacebookAdapter::default();
    let r = social_media_aggregator::network::RedditAdapter::default();
    assert_eq!(fb.network(), Network::Facebook);
    assert_eq!(r.network(), Network::Reddit);
    // Every network resolves to an adapter of the right network.
    for n in Network::ALL {
        assert_eq!(adapter_for(n).network(), n);
    }
}
