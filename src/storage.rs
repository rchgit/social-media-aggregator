use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{Error, Result};
use crate::models::{Account, Network, Post, PostImage, SyncCursor, Topic};

const SCHEMA_VERSION: i64 = 1;

/// Owns the SQLite connection and all persistence queries.
pub struct Storage {
    conn: Connection,
}

impl Storage {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        let storage = Self { conn };
        storage.migrate()?;
        Ok(storage)
    }

    /// In-memory database, used by tests.
    pub fn in_memory() -> Result<Self> {
        let storage = Self {
            conn: Connection::open_in_memory()?,
        };
        storage.migrate()?;
        Ok(storage)
    }

    fn migrate(&self) -> Result<()> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?;

        if version < 1 {
            self.conn.execute_batch(
                r#"
                PRAGMA journal_mode = WAL;
                PRAGMA foreign_keys = ON;

                CREATE TABLE accounts (
                    id           INTEGER PRIMARY KEY AUTOINCREMENT,
                    network      TEXT    NOT NULL,
                    handle       TEXT    NOT NULL,
                    display_name TEXT    NOT NULL DEFAULT '',
                    secret_ref   TEXT    NOT NULL DEFAULT '',
                    enabled      INTEGER NOT NULL DEFAULT 1,
                    UNIQUE (network, handle)
                );

                CREATE TABLE topics (
                    id       INTEGER PRIMARY KEY AUTOINCREMENT,
                    name     TEXT    NOT NULL UNIQUE,
                    keywords TEXT    NOT NULL DEFAULT '[]',
                    enabled  INTEGER NOT NULL DEFAULT 1
                );

                CREATE TABLE posts (
                    id             INTEGER PRIMARY KEY AUTOINCREMENT,
                    network        TEXT    NOT NULL,
                    account_handle TEXT    NOT NULL,
                    external_id    TEXT    NOT NULL,
                    author         TEXT    NOT NULL DEFAULT '',
                    content        TEXT    NOT NULL DEFAULT '',
                    url            TEXT    NOT NULL DEFAULT '',
                    created_at     INTEGER NOT NULL,
                    fetched_at     INTEGER NOT NULL,
                    UNIQUE (network, external_id)
                );

                CREATE TABLE post_images (
                    id         INTEGER PRIMARY KEY AUTOINCREMENT,
                    post_id    INTEGER NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
                    url        TEXT    NOT NULL,
                    local_path TEXT,
                    width      INTEGER,
                    height     INTEGER,
                    alt_text   TEXT
                );

                CREATE TABLE post_topics (
                    post_id  INTEGER NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
                    topic_id INTEGER NOT NULL REFERENCES topics(id) ON DELETE CASCADE,
                    PRIMARY KEY (post_id, topic_id)
                );

                CREATE TABLE cursors (
                    account_id      INTEGER PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
                    cursor          TEXT,
                    last_synced_at  INTEGER
                );

                CREATE INDEX idx_posts_created ON posts(created_at DESC);
                CREATE INDEX idx_posts_network ON posts(network, external_id);
                "#,
            )?;
            self.conn
                .execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
        }

        Ok(())
    }

    // ---- accounts ----

    pub fn add_account(
        &self,
        network: Network,
        handle: &str,
        display_name: &str,
        secret_ref: &str,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO accounts (network, handle, display_name, secret_ref, enabled)
             VALUES (?1, ?2, ?3, ?4, 1)
             ON CONFLICT(network, handle) DO UPDATE SET
               display_name = excluded.display_name,
               secret_ref   = excluded.secret_ref",
            params![network.as_str(), handle, display_name, secret_ref],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_accounts(&self) -> Result<Vec<Account>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, network, handle, display_name, secret_ref, enabled
             FROM accounts ORDER BY network, handle",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, bool>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, network, handle, display_name, secret_ref, enabled) = row?;
            out.push(Account {
                id,
                network: parse_network(&network)?,
                handle,
                display_name,
                secret_ref,
                enabled,
            });
        }
        Ok(out)
    }

    pub fn get_account(&self, id: i64) -> Result<Option<Account>> {
        self.list_accounts()
            .map(|a| a.into_iter().find(|a| a.id == id))
    }

    pub fn remove_account(&self, id: i64) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM accounts WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    pub fn set_account_enabled(&self, id: i64, enabled: bool) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE accounts SET enabled = ?1 WHERE id = ?2",
            params![enabled, id],
        )?;
        Ok(n > 0)
    }

    // ---- topics ----

    pub fn add_topic(&self, name: &str, keywords: &[String]) -> Result<i64> {
        let json = serde_json::to_string(keywords)?;
        self.conn.execute(
            "INSERT INTO topics (name, keywords, enabled) VALUES (?1, ?2, 1)
             ON CONFLICT(name) DO UPDATE SET keywords = excluded.keywords",
            params![name, json],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_topics(&self) -> Result<Vec<Topic>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, keywords, enabled FROM topics ORDER BY name")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, bool>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, name, keywords, enabled) = row?;
            out.push(Topic {
                id,
                name,
                keywords: serde_json::from_str(&keywords).unwrap_or_default(),
                enabled,
            });
        }
        Ok(out)
    }

    pub fn get_topic(&self, id: i64) -> Result<Option<Topic>> {
        self.list_topics()
            .map(|t| t.into_iter().find(|t| t.id == id))
    }

    pub fn remove_topic(&self, id: i64) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM topics WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    pub fn set_topic_enabled(&self, id: i64, enabled: bool) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE topics SET enabled = ?1 WHERE id = ?2",
            params![enabled, id],
        )?;
        Ok(n > 0)
    }

    // ---- posts ----

    /// Inserts a post if not already present, then links it to matching topics.
    /// Returns `Some(id)` for a newly inserted post, `None` if it already
    /// existed (deduplicated).
    pub fn insert_post(&self, post: &Post, topic_ids: &[i64]) -> Result<Option<i64>> {
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO posts
               (network, account_handle, external_id, author, content, url, created_at, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                post.network.as_str(),
                post.account_handle,
                post.external_id,
                post.author,
                post.content,
                post.url,
                post.created_at.timestamp_millis(),
                post.fetched_at.timestamp_millis(),
            ],
        )?;
        if n == 0 {
            return Ok(None);
        }
        let post_id = self.conn.last_insert_rowid();

        for img in &post.images {
            self.conn.execute(
                "INSERT INTO post_images (post_id, url, local_path, width, height, alt_text)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    post_id,
                    img.url,
                    img.local_path,
                    img.width,
                    img.height,
                    img.alt_text,
                ],
            )?;
        }

        for tid in topic_ids {
            self.conn.execute(
                "INSERT OR IGNORE INTO post_topics (post_id, topic_id) VALUES (?1, ?2)",
                params![post_id, tid],
            )?;
        }

        Ok(Some(post_id))
    }

    /// Feed for one topic, newest first with deterministic tie-breaking.
    pub fn feed(&self, topic_id: i64, limit: usize, offset: usize) -> Result<Vec<Post>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.network, p.account_handle, p.external_id, p.author,
                    p.content, p.url, p.created_at, p.fetched_at
             FROM posts p
             JOIN post_topics pt ON pt.post_id = p.id
             WHERE pt.topic_id = ?1
             ORDER BY p.created_at DESC, p.network, p.external_id
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(params![topic_id, limit as i64, offset as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, i64>(7)?,
                r.get::<_, i64>(8)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, network, account_handle, external_id, author, content, url, ca, fa) = row?;
            out.push(Post {
                id,
                network: parse_network(&network)?,
                account_handle,
                external_id,
                author,
                content,
                url,
                created_at: DateTime::<Utc>::from_timestamp_millis(ca)
                    .unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
                fetched_at: DateTime::<Utc>::from_timestamp_millis(fa)
                    .unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
                images: self.images_for(id)?,
            });
        }
        Ok(out)
    }

    pub fn post(&self, id: i64) -> Result<Option<Post>> {
        let row = self
            .conn
            .query_row(
                "SELECT id, network, account_handle, external_id, author, content, url,
                        created_at, fetched_at
                 FROM posts WHERE id = ?1",
                params![id],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, i64>(7)?,
                        r.get::<_, i64>(8)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, network, account_handle, external_id, author, content, url, ca, fa)) = row
        else {
            return Ok(None);
        };
        Ok(Some(Post {
            id,
            network: parse_network(&network)?,
            account_handle,
            external_id,
            author,
            content,
            url,
            created_at: DateTime::<Utc>::from_timestamp_millis(ca)
                .unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
            fetched_at: DateTime::<Utc>::from_timestamp_millis(fa)
                .unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
            images: self.images_for(id)?,
        }))
    }

    fn images_for(&self, post_id: i64) -> Result<Vec<PostImage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, post_id, url, local_path, width, height, alt_text
             FROM post_images WHERE post_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![post_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, post_id, url, local_path, w, h, alt_text) = row?;
            out.push(PostImage {
                id,
                post_id,
                url,
                local_path,
                width: w.map(|v| v as u32),
                height: h.map(|v| v as u32),
                alt_text,
            });
        }
        Ok(out)
    }

    pub fn set_image_local_path(&self, image_id: i64, path: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE post_images SET local_path = ?1 WHERE id = ?2",
            params![path, image_id],
        )?;
        Ok(())
    }

    /// Counts the number of posts for a topic.
    pub fn feed_count(&self, topic_id: i64) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM post_topics WHERE topic_id = ?1",
            params![topic_id],
            |r| r.get(0),
        )?)
    }

    // ---- cursors ----

    pub fn get_cursor(&self, account_id: i64) -> Result<Option<SyncCursor>> {
        let row = self
            .conn
            .query_row(
                "SELECT account_id, cursor, last_synced_at FROM cursors WHERE account_id = ?1",
                params![account_id],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                    ))
                },
            )
            .optional()?;
        Ok(row.map(|(account_id, cursor, last)| SyncCursor {
            account_id,
            cursor,
            last_synced_at: last.and_then(DateTime::<Utc>::from_timestamp_millis),
        }))
    }

    pub fn set_cursor(&self, account_id: i64, cursor: Option<&str>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO cursors (account_id, cursor, last_synced_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(account_id) DO UPDATE SET
               cursor = excluded.cursor,
               last_synced_at = excluded.last_synced_at",
            params![account_id, cursor, Utc::now().timestamp_millis()],
        )?;
        Ok(())
    }
}

fn parse_network(s: &str) -> Result<Network> {
    s.parse().map_err(|_| Error::Config(format!("unknown network `{s}`")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(network: Network, external_id: &str, created: i64) -> Post {
        Post {
            id: 0,
            network,
            account_handle: "u".to_string(),
            external_id: external_id.to_string(),
            author: "a".to_string(),
            content: "hello".to_string(),
            url: "https://example.com".to_string(),
            created_at: DateTime::<Utc>::from_timestamp_millis(created).unwrap(),
            fetched_at: Utc::now(),
            images: vec![],
        }
    }

    #[test]
    fn dedups_identical_external_ids() {
        let s = Storage::in_memory().unwrap();
        let t = s.add_topic("tech", &["rust".into()]).unwrap();
        let p = post(Network::Reddit, "abc", 1000);
        assert!(s.insert_post(&p, &[t]).unwrap().is_some());
        assert!(s.insert_post(&p, &[t]).unwrap().is_none());
        assert_eq!(s.feed_count(t).unwrap(), 1);
    }

    #[test]
    fn feed_is_newest_first_with_tie_break() {
        let s = Storage::in_memory().unwrap();
        let t = s.add_topic("news", &["x".into()]).unwrap();
        s.insert_post(&post(Network::Reddit, "a", 1000), &[t])
            .unwrap();
        s.insert_post(&post(Network::X, "b", 2000), &[t]).unwrap();
        s.insert_post(&post(Network::Reddit, "c", 2000), &[t])
            .unwrap();
        let feed = s.feed(t, 10, 0).unwrap();
        let order: Vec<String> = feed.iter().map(|p| p.external_id.clone()).collect();
        // newest (2000) first; within equal timestamps network sorts reddit < x,
        // so "c" (reddit) precedes "b" (x), then "a".
        assert_eq!(order, vec!["c", "b", "a"]);
    }

    #[test]
    fn topics_are_isolated() {
        let s = Storage::in_memory().unwrap();
        let a = s.add_topic("a", &["rust".into()]).unwrap();
        let b = s.add_topic("b", &["go".into()]).unwrap();
        let mut p = post(Network::Reddit, "x", 1000);
        p.content = "I love rust".to_string();
        // Only topic "a" matches.
        s.insert_post(&p, &[a]).unwrap();
        assert_eq!(s.feed_count(a).unwrap(), 1);
        assert_eq!(s.feed_count(b).unwrap(), 0);
    }
}
