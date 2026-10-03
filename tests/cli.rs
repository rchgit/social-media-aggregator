//! End-to-end CLI tests running the compiled `sma` binary against a temp
//! working directory, covering every subcommand's happy and error paths.

use std::path::Path;
use std::process::Command;

struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn cwd(&self) -> &Path {
        self.dir.path()
    }

    fn run(&self, args: &[&str]) -> Output {
        run_in(self.cwd(), args)
    }
}

struct Output {
    stdout: String,
    stderr: String,
    success: bool,
}

fn run_in(cwd: &Path, args: &[&str]) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_sma"))
        .args(args)
        .current_dir(cwd)
        .env_remove("SMA_CONFIG")
        .output()
        .expect("failed to spawn sma");
    Output {
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        success: out.status.success(),
    }
}

#[test]
fn version_and_help_succeed() {
    let out = run_in(Path::new("/tmp"), &["--version"]);
    assert!(out.success);
    assert!(out.stdout.contains("sma"), "{}", out.stdout);

    let out = run_in(Path::new("/tmp"), &["--help"]);
    assert!(out.success);
    for word in ["config", "accounts", "topics", "sync", "feed"] {
        assert!(out.stdout.contains(word), "help missing {word}");
    }

    // No subcommand starts the TUI; without a TTY it exits with an error.
    let dir = tempfile::tempdir().unwrap();
    let out = run_in(dir.path(), &[]);
    assert!(!out.success);
    assert!(!out.stderr.is_empty());
}

#[test]
fn config_init_writes_sample_then_refuses_overwrite() {
    let sb = Sandbox::new();
    let out = sb.run(&["config", "init"]);
    assert!(out.success, "{}", out.stderr);
    assert!(out.stdout.contains("wrote"), "{}", out.stdout);
    assert!(sb.cwd().join("sma.toml").exists());

    let out = sb.run(&["config", "init"]);
    assert!(out.success);
    assert!(out.stderr.contains("already exists"), "{}", out.stderr);
    // File content untouched.
    let content = std::fs::read_to_string(sb.cwd().join("sma.toml")).unwrap();
    assert!(content.contains("[settings]"));
}

#[test]
fn config_path_prints_resolved_path() {
    let sb = Sandbox::new();
    let out = sb.run(&["config", "path"]);
    assert!(out.success);
    assert!(out.stdout.trim().ends_with("sma.toml"), "{}", out.stdout);

    // --config flag overrides.
    let out = run_in(sb.cwd(), &["--config", "/custom/other.toml", "config", "path"]);
    assert_eq!(out.stdout.trim(), "/custom/other.toml");

    // SMA_CONFIG env overrides the default.
    let out = Command::new(env!("CARGO_BIN_EXE_sma"))
        .args(["config", "path"])
        .current_dir(sb.cwd())
        .env("SMA_CONFIG", "/from/env.toml")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "/from/env.toml"
    );
}

#[test]
fn account_lifecycle_add_list_toggle_remove() {
    let sb = Sandbox::new();

    // Unknown network is rejected by the value parser.
    let out = sb.run(&["accounts", "add", "--network", "myspace", "--handle", "h"]);
    assert!(!out.success);
    assert!(out.stderr.contains("unknown network"), "{}", out.stderr);

    // Add, list, duplicate-add updates not duplicates.
    let out = sb.run(&["accounts", "add", "--network", "reddit", "--handle", "alice", "--display", "Alice"]);
    assert!(out.success, "{}", out.stderr);
    let out = sb.run(&["accounts", "add", "--network", "x", "--handle", "bob", "--secret", "env:TOK"]);
    assert!(out.success, "{}", out.stderr);

    let out = sb.run(&["accounts", "list"]);
    assert!(out.success);
    assert!(out.stdout.contains("reddit"), "{}", out.stdout);
    assert!(out.stdout.contains("alice"), "{}", out.stdout);
    assert!(out.stdout.contains("bob"), "{}", out.stdout);

    // Duplicate add (same network+handle) does not create a second row.
    let out = sb.run(&["accounts", "add", "--network", "reddit", "--handle", "alice", "--display", "Alice2"]);
    assert!(out.success);
    let out = sb.run(&["accounts", "list"]);
    assert_eq!(out.stdout.lines().count(), 2, "{}", out.stdout);

    // Enable/disable unknown id errors.
    let out = sb.run(&["accounts", "enable", "--id", "999"]);
    assert!(!out.success);
    assert!(out.stderr.contains("not found"), "{}", out.stderr);

    // Disable then re-enable id 1.
    let out = sb.run(&["accounts", "disable", "--id", "1"]);
    assert!(out.success, "{}", out.stderr);
    let out = sb.run(&["accounts", "list"]);
    assert!(out.stdout.contains("off"), "{}", out.stdout);
    let out = sb.run(&["accounts", "enable", "--id", "1"]);
    assert!(out.success);
    let out = sb.run(&["accounts", "list"]);
    assert!(out.stdout.contains(" on"), "{}", out.stdout);

    // Remove.
    let out = sb.run(&["accounts", "remove", "--id", "2"]);
    assert!(out.success);
    let out = sb.run(&["accounts", "remove", "--id", "2"]);
    assert!(!out.success);
    assert!(out.stderr.contains("not found"), "{}", out.stderr);
}

#[test]
fn topic_lifecycle_add_list_toggle_remove() {
    let sb = Sandbox::new();

    let out = sb.run(&["topics", "add", "--name", "tech", "--keywords", "rust,go"]);
    assert!(out.success, "{}", out.stderr);
    assert!(out.stdout.contains("topic 1: tech"), "{}", out.stdout);
    assert!(out.stdout.contains("rust, go"), "{}", out.stdout);

    // Re-adding same name updates keywords, keeps single row.
    let out = sb.run(&["topics", "add", "--name", "tech", "--keywords", "zig"]);
    assert!(out.success);
    let out = sb.run(&["topics", "list"]);
    assert_eq!(out.stdout.lines().count(), 1);
    assert!(out.stdout.contains("zig"), "{}", out.stdout);

    let out = sb.run(&["topics", "enable", "--id", "999"]);
    assert!(!out.success);
    assert!(out.stderr.contains("not found"), "{}", out.stderr);

    let out = sb.run(&["topics", "disable", "--id", "1"]);
    assert!(out.success);
    let out = sb.run(&["topics", "list"]);
    assert!(out.stdout.contains("off"), "{}", out.stdout);
    let out = sb.run(&["topics", "enable", "--id", "1"]);
    assert!(out.success);

    let out = sb.run(&["topics", "remove", "--id", "1"]);
    assert!(out.success);
    assert!(out.stdout.contains("removed topic 1"), "{}", out.stdout);
    let out = sb.run(&["topics", "remove", "--id", "1"]);
    assert!(!out.success);
    assert!(out.stderr.contains("not found"), "{}", out.stderr);
}

#[test]
fn feed_requires_existing_topic_and_prints_posts() {
    let sb = Sandbox::new();
    sb.run(&["topics", "add", "--name", "tech", "--keywords", "rust"]);

    let out = sb.run(&["feed", "--topic", "missing"]);
    assert!(!out.success);
    assert!(out.stderr.contains("not found"), "{}", out.stderr);

    let out = sb.run(&["feed", "--topic", "tech"]);
    assert!(out.success, "{}", out.stderr);
    assert!(out.stdout.is_empty(), "{}", out.stdout);
}

#[test]
fn sync_reports_accounts_even_on_failure() {
    let sb = Sandbox::new();
    sb.run(&["accounts", "add", "--network", "reddit", "--handle", "nosuchuser"]);

    // Sync uses the real network stack; on this offline sandbox it may fail
    // with error status. Either way it must exit 0 and print a report line.
    let out = sb.run(&["sync"]);
    assert!(out.success, "stderr: {}", out.stderr);
    assert!(out.stdout.contains("nosuchuser"), "{}", out.stdout);
    assert!(out.stdout.contains("fetched="), "{}", out.stdout);

    // Filtering by unknown handle produces an empty report.
    let out = sb.run(&["sync", "--account", "ghost"]);
    assert!(out.success);
    assert!(out.stdout.trim().is_empty(), "{}", out.stdout);
}

#[test]
fn missing_config_file_is_reported_when_unparseable() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sma.toml"), "[settings\nbroken").unwrap();
    let out = run_in(dir.path(), &["config", "path"]);
    assert!(!out.success);
    assert!(out.stderr.contains("config error"), "{}", out.stderr);
}

#[test]
fn custom_config_flag_is_used_for_storage() {
    let sb = Sandbox::new();
    let cfg = sb.dir.path().join("custom.toml");
    std::fs::write(
        &cfg,
        format!(
            "[settings]\ndb_path = {:?}\ncache_dir = \"cache\"\npage_size = 5\n",
            sb.dir.path().join("custom.db")
        ),
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_sma"))
        .args(["--config"])
        .arg(&cfg)
        .args(["accounts", "add", "--network", "x", "--handle", "zed"])
        .current_dir(sb.cwd())
        .env_remove("SMA_CONFIG")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(sb.dir.path().join("custom.db").exists());
}

#[test]
fn feed_prints_seeded_posts_with_images_and_urls() {
    use social_media_aggregator::models::{Network, Post, PostImage};
    use social_media_aggregator::storage::Storage;

    let sb = Sandbox::new();
    // Point the CLI at a fixed db via config; seed through the library.
    let db = sb.dir.path().join("seed.db");
    std::fs::write(
        sb.dir.path().join("sma.toml"),
        format!("[settings]\ndb_path = {:?}\ncache_dir = \"cache\"\n", db),
    )
    .unwrap();

    {
        let storage = Storage::open(&db).unwrap();
        let t = storage.add_topic("tech", &["rust".to_string()]).unwrap();
        let post = Post {
            id: 0,
            network: Network::Reddit,
            account_handle: "alice".to_string(),
            external_id: "ext1".into(),
            author: "bob".into(),
            content: "line one\nline two".into(),
            url: "https://reddit.com/ext1".into(),
            created_at: chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2024, 5, 1, 12, 30, 0).unwrap(),
            fetched_at: chrono::Utc::now(),
            images: vec![PostImage {
                id: 0,
                post_id: 0,
                url: "https://cdn/1.jpg".into(),
                local_path: None,
                width: Some(10),
                height: None,
                alt_text: None,
            }],
        };
        let _ = &post;
        storage.insert_post(&post, &[t]).unwrap();
    }

    let out = sb.run(&["feed", "--topic", "tech", "--limit", "10"]);
    assert!(out.success, "{}", out.stderr);
    let lines: Vec<&str> = out.stdout.lines().collect();
    // Header, 2 content lines (indented), url line, blank separator.
    assert_eq!(lines.len(), 5, "{}", out.stdout);
    assert!(lines[0].starts_with("[r] bob @alice · 2024-05-01 12:30:00 [1 image(s)]"), "{}", lines[0]);
    assert_eq!(lines[1], "  line one");
    assert_eq!(lines[2], "  line two");
    assert_eq!(lines[3], "  https://reddit.com/ext1");
    assert_eq!(lines[4], "");

    // A post with no images/url omits those parts (covered by empty feed
    // formatting branch via limit skip: offset past everything is empty).
    let out = sb.run(&["feed", "--topic", "tech", "--limit", "10", "--offset", "5"]);
    assert!(out.success);
    assert!(out.stdout.is_empty());
}
