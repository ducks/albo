//! CRUD over directory entries. All functions take a &Connection so they
//! stay synchronous and testable; the server wraps the connection in a
//! Mutex (directory-scale traffic, not a pool workload).

use anyhow::Result;
use rusqlite::{Connection, params};

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: i64,
    pub handle: String,
    pub display_name: String,
    pub shop: String,
    pub bio: String,
    pub avatar_path: String,
    pub tags: Vec<String>,
    pub featured_posts: Vec<String>,
    pub booking_url: String,
    pub address: String,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub active: bool,
}

impl Entry {
    /// True when the entry has coordinates and can be placed on the map.
    pub fn located(&self) -> bool {
        self.lat.is_some() && self.lng.is_some()
    }
}

fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<Entry> {
    let tags: String = row.get("tags")?;
    let posts: String = row.get("featured_posts")?;
    Ok(Entry {
        id: row.get("id")?,
        handle: row.get("handle")?,
        display_name: row.get("display_name")?,
        shop: row.get("shop")?,
        bio: row.get("bio")?,
        avatar_path: row.get("avatar_path")?,
        tags: tags
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect(),
        featured_posts: posts
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect(),
        booking_url: row.get("booking_url")?,
        address: row.get("address")?,
        lat: row.get("lat")?,
        lng: row.get("lng")?,
        active: row.get::<_, i64>("active")? != 0,
    })
}

/// Normalize a user-typed handle: strip whitespace, a leading @, and any
/// instagram.com URL prefix, lowercase the rest.
pub fn normalize_handle(raw: &str) -> String {
    let mut h = raw.trim().to_lowercase();
    for prefix in [
        "https://www.instagram.com/",
        "https://instagram.com/",
        "http://www.instagram.com/",
        "http://instagram.com/",
        "www.instagram.com/",
        "instagram.com/",
    ] {
        if let Some(rest) = h.strip_prefix(prefix) {
            h = rest.to_string();
            break;
        }
    }
    h.trim_start_matches('@').trim_end_matches('/').to_string()
}

pub fn list(conn: &Connection, include_inactive: bool) -> Result<Vec<Entry>> {
    let sql = if include_inactive {
        "SELECT * FROM entries ORDER BY sort_order, display_name, handle"
    } else {
        "SELECT * FROM entries WHERE active = 1 ORDER BY sort_order, display_name, handle"
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], row_to_entry)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn get(conn: &Connection, id: i64) -> Result<Option<Entry>> {
    let mut stmt = conn.prepare("SELECT * FROM entries WHERE id = ?1")?;
    let mut rows = stmt.query_map(params![id], row_to_entry)?;
    Ok(rows.next().transpose()?)
}

pub fn get_by_handle(conn: &Connection, handle: &str) -> Result<Option<Entry>> {
    let mut stmt = conn.prepare("SELECT * FROM entries WHERE handle = ?1 AND active = 1")?;
    let mut rows = stmt.query_map(params![handle], row_to_entry)?;
    Ok(rows.next().transpose()?)
}

/// Active entries carrying `tag` (case-insensitive, exact tag match).
pub fn list_by_tag(conn: &Connection, tag: &str) -> Result<Vec<Entry>> {
    let want = tag.trim().to_lowercase();
    Ok(list(conn, false)?
        .into_iter()
        .filter(|e| e.tags.iter().any(|t| t.to_lowercase() == want))
        .collect())
}

/// Tags actually in use across active entries, with counts, sorted by
/// count desc then name. Drives the public filter bar - we show tags that
/// exist on real entries, not the full config taxonomy, so empty tags
/// never appear as dead filters.
pub fn tags_in_use(conn: &Connection) -> Result<Vec<(String, usize)>> {
    use std::collections::BTreeMap;
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for e in list(conn, false)? {
        for t in e.tags {
            *counts.entry(t).or_default() += 1;
        }
    }
    let mut v: Vec<(String, usize)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(v)
}

/// Apply best-effort Instagram prefill without clobbering admin edits:
/// display_name only replaces the insert-time default (the handle itself),
/// and avatar_path only fills if currently empty.
pub fn apply_prefill(
    conn: &Connection,
    id: i64,
    display_name: Option<&str>,
    avatar_path: Option<&str>,
) -> Result<()> {
    if let Some(name) = display_name {
        conn.execute(
            "UPDATE entries SET display_name = ?2, updated_at = datetime('now')
             WHERE id = ?1 AND display_name = handle",
            params![id, name],
        )?;
    }
    if let Some(avatar) = avatar_path {
        conn.execute(
            "UPDATE entries SET avatar_path = ?2, updated_at = datetime('now')
             WHERE id = ?1 AND avatar_path = ''",
            params![id, avatar],
        )?;
    }
    Ok(())
}

/// Insert a new entry by handle. Returns the new id, or None if the handle
/// already exists (the admin UI treats that as "already listed").
pub fn add_by_handle(conn: &Connection, raw_handle: &str) -> Result<Option<i64>> {
    let handle = normalize_handle(raw_handle);
    if handle.is_empty() {
        return Ok(None);
    }
    let res = conn.execute(
        "INSERT INTO entries (handle, display_name) VALUES (?1, ?1)",
        params![handle],
    );
    match res {
        Ok(_) => Ok(Some(conn.last_insert_rowid())),
        Err(rusqlite::Error::SqliteFailure(e, _))
            if e.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}

/// The admin-editable fields of an entry. Grouped into a struct so the
/// update signature doesn't grow a positional argument per field.
#[derive(Debug, Default)]
pub struct EntryEdit {
    pub display_name: String,
    pub shop: String,
    pub bio: String,
    pub tags: String,
    pub featured_posts: String,
    pub booking_url: String,
    pub address: String,
    /// Geocoded coordinates for the address, if geocoding succeeded.
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub active: bool,
}

pub fn update(conn: &Connection, id: i64, e: &EntryEdit) -> Result<bool> {
    let n = conn.execute(
        "UPDATE entries SET display_name=?2, shop=?3, bio=?4, tags=?5,
         featured_posts=?6, booking_url=?7, address=?8, lat=?9, lng=?10,
         active=?11, updated_at=datetime('now') WHERE id=?1",
        params![
            id,
            e.display_name,
            e.shop,
            e.bio,
            e.tags,
            e.featured_posts,
            e.booking_url,
            e.address,
            e.lat,
            e.lng,
            e.active as i64,
        ],
    )?;
    Ok(n > 0)
}


pub fn delete(conn: &Connection, id: i64) -> Result<bool> {
    let n = conn.execute("DELETE FROM entries WHERE id = ?1", params![id])?;
    Ok(n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    /// Test helper: update just the commonly-set fields, defaulting the rest.
    fn edit(
        conn: &Connection,
        id: i64,
        display_name: &str,
        tags: &str,
        active: bool,
    ) -> Result<bool> {
        update(
            conn,
            id,
            &EntryEdit {
                display_name: display_name.into(),
                tags: tags.into(),
                active,
                ..Default::default()
            },
        )
    }

    #[test]
    fn normalize_strips_at_urls_and_case() {
        assert_eq!(normalize_handle("@InkByExample"), "inkbyexample");
        assert_eq!(
            normalize_handle("https://www.instagram.com/inkbyexample/"),
            "inkbyexample"
        );
        assert_eq!(
            normalize_handle("  instagram.com/ink.by.example "),
            "ink.by.example"
        );
        assert_eq!(normalize_handle(""), "");
        assert_eq!(normalize_handle("@"), "");
    }

    #[test]
    fn add_normalizes_and_rejects_duplicates() {
        let conn = open_in_memory().unwrap();
        let id = add_by_handle(&conn, "@InkByExample").unwrap();
        assert!(id.is_some());
        // Same handle in different dress = duplicate.
        let dup = add_by_handle(&conn, "https://instagram.com/inkbyexample/").unwrap();
        assert!(dup.is_none());
        // Empty input never inserts.
        assert!(add_by_handle(&conn, "  @ ").unwrap().is_none());
    }

    #[test]
    fn update_and_list_roundtrip() {
        let conn = open_in_memory().unwrap();
        let id = add_by_handle(&conn, "artist").unwrap().unwrap();
        update(
            &conn,
            id,
            &EntryEdit {
                display_name: "Artist Name".into(),
                shop: "Good Shop".into(),
                bio: "bio here".into(),
                tags: "blackwork, fine line".into(),
                featured_posts:
                    "https://www.instagram.com/p/AAA/\nhttps://www.instagram.com/p/BBB/\n".into(),
                booking_url: "https://example.com/book".into(),
                address: "123 Ink St, Portland".into(),
                lat: Some(45.52),
                lng: Some(-122.67),
                active: true,
            },
        )
        .unwrap();
        let e = get(&conn, id).unwrap().unwrap();
        assert_eq!(e.display_name, "Artist Name");
        assert_eq!(e.tags, vec!["blackwork", "fine line"]);
        assert_eq!(e.featured_posts.len(), 2);
        assert_eq!(e.address, "123 Ink St, Portland");
        assert!(e.located());

        // Deactivated entries drop from the public list but stay in admin.
        edit(&conn, id, "Artist Name", "", false).unwrap();
        assert!(list(&conn, false).unwrap().is_empty());
        assert_eq!(list(&conn, true).unwrap().len(), 1);
    }

    #[test]
    fn prefill_never_clobbers_admin_edits() {
        let conn = open_in_memory().unwrap();
        let id = add_by_handle(&conn, "artist").unwrap().unwrap();

        // Fresh entry: display_name defaults to the handle, so prefill lands.
        apply_prefill(&conn, id, Some("Real Name"), Some("avatars/artist.jpg")).unwrap();
        let e = get(&conn, id).unwrap().unwrap();
        assert_eq!(e.display_name, "Real Name");
        assert_eq!(e.avatar_path, "avatars/artist.jpg");

        // A second prefill (e.g. re-add attempt) must not overwrite either.
        apply_prefill(&conn, id, Some("Wrong Name"), Some("avatars/other.jpg")).unwrap();
        let e = get(&conn, id).unwrap().unwrap();
        assert_eq!(e.display_name, "Real Name");
        assert_eq!(e.avatar_path, "avatars/artist.jpg");
    }

    #[test]
    fn tag_filtering_and_usage_counts() {
        let conn = open_in_memory().unwrap();
        let a = add_by_handle(&conn, "a").unwrap().unwrap();
        let b = add_by_handle(&conn, "b").unwrap().unwrap();
        let c = add_by_handle(&conn, "c").unwrap().unwrap();
        edit(&conn, a, "A", "blackwork, fine line", true).unwrap();
        edit(&conn, b, "B", "blackwork", true).unwrap();
        // Inactive entry's tags must not count or appear.
        edit(&conn, c, "C", "blackwork, color", false).unwrap();

        // Case-insensitive tag match, active only.
        let bw = list_by_tag(&conn, "BlackWork").unwrap();
        let names: Vec<&str> = bw.iter().map(|e| e.display_name.as_str()).collect();
        assert_eq!(names, vec!["A", "B"]); // not C (inactive)

        assert!(list_by_tag(&conn, "color").unwrap().is_empty()); // only on inactive C
        assert!(list_by_tag(&conn, "nope").unwrap().is_empty());

        // Usage counts exclude inactive; sorted count desc then name.
        let used = tags_in_use(&conn).unwrap();
        assert_eq!(used, vec![("blackwork".into(), 2), ("fine line".into(), 1)]);
    }

    #[test]
    fn get_by_handle_only_finds_active() {
        let conn = open_in_memory().unwrap();
        let id = add_by_handle(&conn, "artist").unwrap().unwrap();
        assert!(get_by_handle(&conn, "artist").unwrap().is_some());
        edit(&conn, id, "artist", "", false).unwrap();
        assert!(get_by_handle(&conn, "artist").unwrap().is_none());
    }

    #[test]
    fn delete_removes() {
        let conn = open_in_memory().unwrap();
        let id = add_by_handle(&conn, "gone").unwrap().unwrap();
        assert!(delete(&conn, id).unwrap());
        assert!(get(&conn, id).unwrap().is_none());
    }
}
