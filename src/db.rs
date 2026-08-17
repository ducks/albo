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
    -- Shop/studio address the admin types; geocoded to lat/lng once. An
    -- entry appears on the map only when it has coordinates.
    address TEXT NOT NULL DEFAULT '',
    lat REAL,
    lng REAL,
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
    migrate(&conn)?;
    Ok(conn)
}

/// Idempotent, additive schema evolution: bring an older database up to the
/// current shape by adding any columns it's missing. Since CREATE TABLE IF
/// NOT EXISTS won't alter an existing table, new columns are applied here.
/// Only additive changes belong here - never a destructive one.
fn migrate(conn: &Connection) -> Result<()> {
    let have: std::collections::HashSet<String> = {
        let mut stmt = conn.prepare("PRAGMA table_info(entries)")?;
        let cols = stmt.query_map([], |row| row.get::<_, String>(1))?;
        cols.collect::<rusqlite::Result<_>>()?
    };
    // (column, DDL type) additive migrations.
    for (col, decl) in [
        ("address", "TEXT NOT NULL DEFAULT ''"),
        ("lat", "REAL"),
        ("lng", "REAL"),
    ] {
        if !have.contains(col) {
            conn.execute(&format!("ALTER TABLE entries ADD COLUMN {col} {decl}"), [])?;
        }
    }
    Ok(())
}

#[cfg(test)]
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(SCHEMA)?;
    migrate(&conn)?;
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
    fn migrate_adds_missing_columns_idempotently() {
        // Simulate a pre-map database: the old schema without location cols.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE entries (id INTEGER PRIMARY KEY, handle TEXT NOT NULL UNIQUE);",
        )
        .unwrap();
        migrate(&conn).unwrap();
        // Columns now exist and are writable.
        conn.execute(
            "INSERT INTO entries (handle, address, lat, lng) VALUES ('a', '1 Main St', 45.5, -122.6)",
            [],
        )
        .unwrap();
        // Running it again is a no-op, not an error.
        migrate(&conn).unwrap();
        let (lat, lng): (f64, f64) = conn
            .query_row("SELECT lat, lng FROM entries WHERE handle='a'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((lat, lng), (45.5, -122.6));
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
