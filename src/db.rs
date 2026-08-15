//! SQLite storage for directory entries. Entries are *listings*, not user
//! accounts - artists never log in; only admins mutate the directory.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS admin_users (
    id INTEGER PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    -- argon2 password hash; never a plaintext password.
    password_hash TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS entries (
    id INTEGER PRIMARY KEY,
    -- Instagram handle without the @, unique key for the entry.
    handle TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL DEFAULT '',
    shop TEXT NOT NULL DEFAULT '',
    bio TEXT NOT NULL DEFAULT '',
    -- Local path to a cached/uploaded avatar; never a hotlinked IG URL.
    avatar_path TEXT NOT NULL DEFAULT '',
    -- Comma-separated tags from the instance taxonomy.
    tags TEXT NOT NULL DEFAULT '',
    -- Up to a few featured public post URLs, one per line, rendered as
    -- official Instagram embeds.
    featured_posts TEXT NOT NULL DEFAULT '',
    booking_url TEXT NOT NULL DEFAULT '',
    active INTEGER NOT NULL DEFAULT 1,
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
"#;

pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)
        .with_context(|| format!("could not open database at {}", path.display()))?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

#[cfg(test)]
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_applies_and_entries_insert() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO entries (handle, display_name) VALUES (?1, ?2)",
            rusqlite::params!["inkbyexample", "Ink By Example"],
        )
        .unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn handle_is_unique() {
        let conn = open_in_memory().unwrap();
        conn.execute("INSERT INTO entries (handle) VALUES ('dupe')", [])
            .unwrap();
        let dup = conn.execute("INSERT INTO entries (handle) VALUES ('dupe')", []);
        assert!(dup.is_err());
    }
}
