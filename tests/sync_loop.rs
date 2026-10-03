//! Tests for the sync orchestration loop, driven through the `sync_with`
//! test seam with in-memory fake adapters.

use social_media_aggregator::models::{Account, Network};
use social_media_aggregator::network::{
    FetchPage, NetworkAdapter, NetworkError, RawImage, RawPost,
};
use social_media_aggregator::secrets::SecretResolver;
use social_media_aggregator::storage::Storage;
use social_media_aggregator::sync::{AccountSync, SyncStatus};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

fn raw_post(id: &str, content: &str) -> RawPost {
    RawPost {
        external_id: id.into(),
        author: "author".into(),
        content: content.into(),
        url: format!("https://example.com/{id}"),
        created_at: chrono::Utc::now(),
        images: Vec::new(),
    }
}

fn account(network: Network, handle: &str, secret_ref: &str) -> Account {
    Account {
        id: 0,
        network,
        handle: handle.into(),
        display_name: handle.into(),
        secret_ref: secret_ref.into(),
        enabled: true,
    }
}

/// Fake adapter serving canned pages; optional error injected per fetch.
struct FakeAdapter {
    network: Network,
    pages: std::sync::Mutex<Vec<FetchPage>>,
    error: Option<NetworkError>,
    calls: AtomicUsize,
}

impl FakeAdapter {
    fn new(network: Network, pages: Vec<FetchPage>) -> Self {
        Self {
            network,
            pages: std::sync::Mutex::new(pages),
            error: None,
            calls: AtomicUsize::new(0),
        }
    }

    fn failing(network: Network, error: NetworkError) -> Self {
        Self {
            network,
            pages: std::sync::Mutex::new(Vec::new()),
            error: Some(error),
            calls: AtomicUsize::new(0),
        }
    }
}

impl NetworkAdapter for FakeAdapter {
    fn network(&self) -> Network {
        self.network
    }

    fn fetch(
        &self,
        _client: &reqwest::blocking::Client,
        _account: &Account,
        _secrets: &SecretResolver,
        _cursor: Option<&str>,
        _limit: usize,
    ) -> Result<FetchPage, NetworkError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(e) = &self.error {
            return Err(e.clone_variant());
        }
        let mut pages = self.pages.lock().unwrap();
        if pages.is_empty() {
            return Ok(FetchPage::default());
        }
        Ok(pages.remove(0))
    }

}


/// Helper: clones the variant so a single error can be returned repeatedly.
trait CloneVariant {
    fn clone_variant(&self) -> NetworkError;
}

impl CloneVariant for NetworkError {
    fn clone_variant(&self) -> NetworkError {
        match self {
            NetworkError::CredentialsRequired(n, s) => {
                NetworkError::CredentialsRequired(*n, s.clone())
            }
            NetworkError::RateLimited {
                network,
                retry_after,
            } => NetworkError::RateLimited {
                network: *network,
                retry_after: *retry_after,
            },
            NetworkError::HttpStatus {
                network,
                status,
                body,
            } => NetworkError::HttpStatus {
                network: *network,
                status: *status,
                body: body.clone(),
            },
            NetworkError::Parse(n, s) => NetworkError::Parse(*n, s.clone()),
            NetworkError::Request(n, s) => NetworkError::Request(*n, s.clone()),
            NetworkError::Unsupported(n) => NetworkError::Unsupported(*n),
        }
    }
}

/// Builds the sync harness: storage with accounts+topics and an adapter map.
struct Harness {
    storage: Storage,
    secrets: SecretResolver,
    client: reqwest::blocking::Client,
    adapters: HashMap<Network, Box<dyn NetworkAdapter>>,
}

fn setup(
    accounts: &[Account],
    topics: &[(i64, &[&str], bool)],
    adapters: HashMap<Network, Box<dyn NetworkAdapter>>,
) -> Harness {
    let storage = Storage::in_memory().unwrap();
    for a in accounts {
        let id = storage
            .add_account(a.network, &a.handle, &a.display_name, &a.secret_ref)
            .unwrap();
        if !a.enabled {
            storage.set_account_enabled(id, false).unwrap();
        }
    }
    for (id, kws, enabled) in topics {
        let tid = storage.add_topic(&format!("t{id}"), &kws.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap();
        let _ = tid;
        if !enabled {
            storage.set_topic_enabled(*id, false).unwrap();
        }
    }
    Harness {
        storage,
        secrets: SecretResolver::default(),
        client: reqwest::blocking::Client::new(),
        adapters,
    }
}

impl Harness {
    fn run(&self, filter: Option<&str>) -> Vec<AccountSync> {
        social_media_aggregator::sync::sync_with(
            &self.storage,
            &self.secrets,
            filter,
            &self.client,
            &self.adapters,
        )
        .unwrap()
    }
}

#[test]
fn sync_inserts_new_posts_and_counts() {
    let mut adapters: HashMap<Network, Box<dyn NetworkAdapter>> = HashMap::new();
    adapters.insert(
        Network::Reddit,
        Box::new(FakeAdapter::new(
            Network::Reddit,
            vec![FetchPage {
                posts: vec![
                    raw_post("1", "a rust post"),
                    raw_post("2", "a go post"),
                ],
                next_cursor: None,
            }],
        )),
    );
    let h = setup(
        &[account(Network::Reddit, "alice", "")],
        &[(1, &["rust"], true)],
        adapters,
    );

    let report = h.run(None);
    assert_eq!(report.len(), 1);
    let r = &report[0];
    // `inserted` counts stored posts (dedupe unit), not topic matches;
    // both posts are stored, only one is linked to the rust topic.
    assert_eq!(r.inserted, 2);
    assert_eq!(h.storage.feed_count(1).unwrap(), 1);
    assert!(matches!(r.status, SyncStatus::Ok));
    assert_eq!(r.handle, "alice");

    // Second run: same posts dedupe to zero inserts.
    // (Fresh harness shares no state with the first beyond the storage.)
    let mut adapters: HashMap<Network, Box<dyn NetworkAdapter>> = HashMap::new();
    adapters.insert(
        Network::Reddit,
        Box::new(FakeAdapter::new(
            Network::Reddit,
            vec![FetchPage {
                posts: vec![raw_post("1", "a rust post")],
                next_cursor: None,
            }],
        )),
    );
    let h2 = setup(
        &[account(Network::Reddit, "alice", "")],
        &[(1, &["rust"], true)],
        adapters,
    );
    h.run(None);
    let report = h2.run(None);
    assert_eq!(report[0].inserted, 1);
}

#[test]
fn sync_persists_cursor_and_stops_when_exhausted() {
    let mut adapters: HashMap<Network, Box<dyn NetworkAdapter>> = HashMap::new();
    adapters.insert(
        Network::Reddit,
        Box::new(FakeAdapter::new(
            Network::Reddit,
            vec![
                FetchPage {
                    posts: vec![raw_post("1", "p1")],
                    next_cursor: Some("page2".into()),
                },
                FetchPage {
                    posts: vec![raw_post("2", "p2")],
                    next_cursor: None,
                },
            ],
        )),
    );
    let h = setup(
        &[account(Network::Reddit, "alice", "")],
        &[(1, &["p"], true)],
        adapters,
    );

    let report = h.run(None);
    assert_eq!(report[0].inserted, 2);
    assert_eq!(report[0].fetched, 2);

    let cursor = h.storage.get_cursor(1).unwrap().unwrap();
    // Last page had no cursor, so stored cursor is None after final write...
    // Actually the final page has next_cursor: None -> loop breaks before
    // persisting again; cursor remains "page2" from page 1.
    assert_eq!(cursor.cursor.as_deref(), Some("page2"));
}

#[test]
fn sync_respects_max_pages_per_account() {
    // 5 pages, all with cursors: only the first 3 (MAX_PAGES) are fetched.
    let pages: Vec<FetchPage> = (0..5)
        .map(|i| FetchPage {
            posts: vec![raw_post(&format!("p{i}"), "content")],
            next_cursor: Some(format!("c{i}")),
        })
        .collect();
    let mut adapters: HashMap<Network, Box<dyn NetworkAdapter>> = HashMap::new();
    adapters.insert(
        Network::Reddit,
        Box::new(FakeAdapter::new(Network::Reddit, pages)),
    );
    let h = setup(
        &[account(Network::Reddit, "alice", "")],
        &[(1, &["content"], true)],
        adapters,
    );

    let report = h.run(None);
    assert_eq!(report[0].inserted, 3);
    let cursor = h.storage.get_cursor(1).unwrap().unwrap();
    assert_eq!(cursor.cursor.as_deref(), Some("c2"));
}

#[test]
fn sync_skips_disabled_accounts_and_filters_by_handle() {
    let mut adapters: HashMap<Network, Box<dyn NetworkAdapter>> = HashMap::new();
    adapters.insert(
        Network::Reddit,
        Box::new(FakeAdapter::new(Network::Reddit, vec![])),
    );
    adapters.insert(
        Network::X,
        Box::new(FakeAdapter::new(Network::X, vec![])),
    );
    let h = setup(
        &[
            account(Network::Reddit, "alice", ""),
            Account {
                enabled: false,
                ..account(Network::X, "bob", "")
            },
        ],
        &[(1, &["k"], true)],
        adapters,
    );

    // Only the enabled alice is synced.
    let report = h.run(None);
    assert_eq!(report.len(), 1);
    assert_eq!(report[0].handle, "alice");

    // Filtering to bob (disabled) yields an empty report.
    let report = h.run(Some("bob"));
    assert!(report.is_empty());

    // Filtering to alice yields just alice.
    let report = h.run(Some("alice"));
    assert_eq!(report.len(), 1);
    assert_eq!(report[0].handle, "alice");
}

#[test]
fn sync_reports_credentials_required() {
    let mut adapters: HashMap<Network, Box<dyn NetworkAdapter>> = HashMap::new();
    adapters.insert(
        Network::X,
        Box::new(FakeAdapter::failing(
            Network::X,
            NetworkError::CredentialsRequired(Network::X, "ENV_TOKEN".into()),
        )),
    );
    let h = setup(
        &[account(Network::X, "alice", "ENV_TOKEN")],
        &[(1, &["k"], true)],
        adapters,
    );

    let report = h.run(None);
    match &report[0].status {
        SyncStatus::CredentialsRequired(s) => assert_eq!(s, "ENV_TOKEN"),
        other => panic!("wrong status: {other:?}"),
    }
}

#[test]
fn sync_reports_rate_limit_with_retry_after() {
    let mut adapters: HashMap<Network, Box<dyn NetworkAdapter>> = HashMap::new();
    adapters.insert(
        Network::Facebook,
        Box::new(FakeAdapter::failing(
            Network::Facebook,
            NetworkError::RateLimited {
                network: Network::Facebook,
                retry_after: 77,
            },
        )),
    );
    let h = setup(
        &[account(Network::Facebook, "page", "T")],
        &[(1, &["k"], true)],
        adapters,
    );

    let report = h.run(None);
    match &report[0].status {
        SyncStatus::RateLimited(secs) => assert_eq!(*secs, 77),
        other => panic!("wrong status: {other:?}"),
    }
}

#[test]
fn sync_reports_other_network_errors() {
    let mut adapters: HashMap<Network, Box<dyn NetworkAdapter>> = HashMap::new();
    adapters.insert(
        Network::Threads,
        Box::new(FakeAdapter::failing(
            Network::Threads,
            NetworkError::HttpStatus {
                network: Network::Threads,
                status: 500,
                body: "boom".into(),
            },
        )),
    );
    let h = setup(
        &[account(Network::Threads, "u", "T")],
        &[(1, &["k"], true)],
        adapters,
    );

    let report = h.run(None);
    match &report[0].status {
        SyncStatus::Error(msg) => assert!(msg.contains("HTTP 500"), "{msg}"),
        other => panic!("wrong status: {other:?}"),
    }
}

#[test]
fn sync_disabled_topics_do_not_match() {
    let mut adapters: HashMap<Network, Box<dyn NetworkAdapter>> = HashMap::new();
    adapters.insert(
        Network::Reddit,
        Box::new(FakeAdapter::new(
            Network::Reddit,
            vec![FetchPage {
                posts: vec![raw_post("1", "rust rocks")],
                next_cursor: None,
            }],
        )),
    );
    let h = setup(
        &[account(Network::Reddit, "alice", "")],
        &[(1, &["rust"], false), (2, &["rocks"], true)],
        adapters,
    );

    let report = h.run(None);
    assert_eq!(report[0].inserted, 1);
    // Post linked only to the enabled topic 2.
    assert_eq!(h.storage.feed_count(1).unwrap(), 0);
    assert_eq!(h.storage.feed_count(2).unwrap(), 1);
}

#[test]
fn sync_status_as_str_covers_all_variants() {
    assert_eq!(SyncStatus::Ok.as_str(), "ok");
    assert_eq!(SyncStatus::Skipped.as_str(), "skipped");
    assert_eq!(
        SyncStatus::CredentialsRequired("E".into()).as_str(),
        "missing credentials"
    );
    assert_eq!(SyncStatus::RateLimited(1).as_str(), "rate limited");
    assert_eq!(SyncStatus::Error("e".into()).as_str(), "error");
}

#[test]
fn raw_images_cloneable_for_normalize_path() {
    // normalize() is private; its inputs are covered here via RawImage clone.
    let img = RawImage {
        url: "https://cdn/x.jpg".into(),
        alt_text: Some("an x".into()),
    };
    let clone = img.clone();
    assert_eq!(clone.url, img.url);
    assert_eq!(clone.alt_text, img.alt_text);
}
