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
    -- Personal address for shopless artists (freelancers, home studios).
    -- Shop-affiliated artists get their location from the shop instead.
    -- Geocoded to lat/lng once; an entry maps here only when it has coords
    -- AND no shop provides a location.
    address TEXT NOT NULL DEFAULT '',
    lat REAL,
    lng REAL,
    active INTEGER NOT NULL DEFAULT 1,
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS shops (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    address TEXT NOT NULL DEFAULT '',
    lat REAL,
    lng REAL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Many-to-many: an artist can work at several shops (guest spots), a shop
-- has many artists. Rows vanish with either side.
CREATE TABLE IF NOT EXISTS entry_shops (
    entry_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
    shop_id INTEGER NOT NULL REFERENCES shops(id) ON DELETE CASCADE,
    PRIMARY KEY (entry_id, shop_id)
);
"#;

pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)
        .with_context(|| format!("could not open database at {}", path.display()))?;
    // Enforce the entry_shops foreign-key cascades.
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.execute_batch(SCHEMA)?;
    migrate(&conn)?;
    Ok(conn)
}

/// Idempotent, additive schema evolution: bring an older database up to the
/// current shape by adding missing columns and backfilling shop entities
/// from the legacy free-text shop field. Only additive changes belong here.
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
    backfill_shops(conn)?;
    Ok(())
}

/// Turn distinct legacy free-text `entries.shop` values into shop rows and
/// link the artists to them. Idempotent: the shop `name` is unique and the
/// join uses INSERT OR IGNORE, so re-running links nothing twice and creates
/// no duplicate shops. Runs only for entries that have a shop string but no
/// shop link yet, so it never re-creates a shop the admin has since renamed
/// or an entry the admin has re-linked.
fn backfill_shops(conn: &Connection) -> Result<()> {
    let rows: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT e.id, e.shop FROM entries e
             WHERE TRIM(e.shop) <> ''
               AND NOT EXISTS (SELECT 1 FROM entry_shops es WHERE es.entry_id = e.id)",
        )?;
        let mapped = stmt.query_map([], |r| Ok((r.get(0)?, r.get::<_, String>(1)?)))?;
        mapped.collect::<rusqlite::Result<_>>()?
    };
    for (entry_id, shop_name) in rows {
        let name = shop_name.trim();
        conn.execute(
            "INSERT OR IGNORE INTO shops (name) VALUES (?1)",
            rusqlite::params![name],
        )?;
        let shop_id: i64 = conn.query_row(
            "SELECT id FROM shops WHERE name = ?1",
            rusqlite::params![name],
            |r| r.get(0),
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO entry_shops (entry_id, shop_id) VALUES (?1, ?2)",
            rusqlite::params![entry_id, shop_id],
        )?;
    }
    Ok(())
}

#[cfg(test)]
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.execute_batch(SCHEMA)?;
    migrate(&conn)?;
    Ok(conn)
}

/// Test hook so the shops module can exercise the backfill after inserting
/// legacy shop strings (open_in_memory runs it once when no shops exist yet).
#[cfg(test)]
pub fn backfill_shops_for_test(conn: &Connection) -> Result<()> {
    backfill_shops(conn)
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
        // Simulate a pre-map database: old entries table without location
        // cols, but with the shop tables present (SCHEMA always runs before
        // migrate in open(), and CREATE IF NOT EXISTS is harmless).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE entries (id INTEGER PRIMARY KEY, handle TEXT NOT NULL UNIQUE, shop TEXT NOT NULL DEFAULT '');
             CREATE TABLE shops (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, address TEXT NOT NULL DEFAULT '', lat REAL, lng REAL);
             CREATE TABLE entry_shops (entry_id INTEGER, shop_id INTEGER, PRIMARY KEY (entry_id, shop_id));",
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
