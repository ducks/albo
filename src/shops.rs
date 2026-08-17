//! Shops are first-class: an artist can work at several (guest spots), a
//! shop has many artists. Location lives on the shop, geocoded once; a
//! shop-affiliated artist inherits it. Shopless artists fall back to their
//! own entry-level address.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, Clone)]
pub struct Shop {
    pub id: i64,
    pub name: String,
    pub address: String,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
}

impl Shop {
    pub fn located(&self) -> bool {
        self.lat.is_some() && self.lng.is_some()
    }
}

fn row_to_shop(row: &rusqlite::Row) -> rusqlite::Result<Shop> {
    Ok(Shop {
        id: row.get("id")?,
        name: row.get("name")?,
        address: row.get("address")?,
        lat: row.get("lat")?,
        lng: row.get("lng")?,
    })
}

pub fn list(conn: &Connection) -> Result<Vec<Shop>> {
    let mut stmt = conn.prepare("SELECT * FROM shops ORDER BY name")?;
    let rows = stmt.query_map([], row_to_shop)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn get(conn: &Connection, id: i64) -> Result<Option<Shop>> {
    let mut stmt = conn.prepare("SELECT * FROM shops WHERE id = ?1")?;
    Ok(stmt.query_row(params![id], row_to_shop).optional()?)
}

/// Create a shop by name (or return the existing one's id). Name is unique.
pub fn add(conn: &Connection, name: &str) -> Result<Option<i64>> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(None);
    }
    conn.execute(
        "INSERT OR IGNORE INTO shops (name) VALUES (?1)",
        params![name],
    )?;
    let id = conn.query_row("SELECT id FROM shops WHERE name = ?1", params![name], |r| {
        r.get(0)
    })?;
    Ok(Some(id))
}

/// Update a shop's name/address/coords (coords from geocoding the address).
pub fn update(
    conn: &Connection,
    id: i64,
    name: &str,
    address: &str,
    lat: Option<f64>,
    lng: Option<f64>,
) -> Result<bool> {
    let n = conn.execute(
        "UPDATE shops SET name=?2, address=?3, lat=?4, lng=?5,
         updated_at=datetime('now') WHERE id=?1",
        params![id, name.trim(), address.trim(), lat, lng],
    )?;
    Ok(n > 0)
}

pub fn delete(conn: &Connection, id: i64) -> Result<bool> {
    // entry_shops rows cascade away via the FK.
    let n = conn.execute("DELETE FROM shops WHERE id = ?1", params![id])?;
    Ok(n > 0)
}

// --- artist <-> shop links --------------------------------------------------

/// Replace an artist's shop links with exactly `shop_ids`.
pub fn set_entry_shops(conn: &Connection, entry_id: i64, shop_ids: &[i64]) -> Result<()> {
    conn.execute(
        "DELETE FROM entry_shops WHERE entry_id = ?1",
        params![entry_id],
    )?;
    for sid in shop_ids {
        conn.execute(
            "INSERT OR IGNORE INTO entry_shops (entry_id, shop_id) VALUES (?1, ?2)",
            params![entry_id, sid],
        )?;
    }
    Ok(())
}

/// Shops an artist is linked to.
pub fn shops_for_entry(conn: &Connection, entry_id: i64) -> Result<Vec<Shop>> {
    let mut stmt = conn.prepare(
        "SELECT s.* FROM shops s
         JOIN entry_shops es ON es.shop_id = s.id
         WHERE es.entry_id = ?1 ORDER BY s.name",
    )?;
    let rows = stmt.query_map(params![entry_id], row_to_shop)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The shop ids an artist is linked to (for pre-checking the edit form).
pub fn shop_ids_for_entry(conn: &Connection, entry_id: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT shop_id FROM entry_shops WHERE entry_id = ?1")?;
    let rows = stmt.query_map(params![entry_id], |r| r.get::<_, i64>(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// A single point on the public map. A shop pin carries the shop and its
/// active artists; a solo pin is one shopless, self-located artist.
pub struct MapPin {
    pub lat: f64,
    pub lng: f64,
    /// The shop this pin represents, if any (solo artists have none).
    pub shop: Option<Shop>,
    /// Artists shown in the popup (a shop's roster, or the one solo artist).
    pub artists: Vec<crate::entries::Entry>,
}

/// Build the public map: one pin per located shop (with its active artists),
/// plus one pin per located artist who is not linked to any shop. A shop
/// artist's own address is ignored - the shop's location wins - matching the
/// edit-form contract. Only active artists appear.
pub fn map_pins(conn: &Connection) -> Result<Vec<MapPin>> {
    let mut pins = Vec::new();

    // Shop pins: every located shop that has at least one active artist.
    for shop in list(conn)? {
        if !shop.located() {
            continue;
        }
        let artists = entries_for_shop(conn, shop.id)?;
        if artists.is_empty() {
            continue;
        }
        pins.push(MapPin {
            lat: shop.lat.unwrap(),
            lng: shop.lng.unwrap(),
            shop: Some(shop),
            artists,
        });
    }

    // Solo pins: active, located artists with no shop link at all.
    let mut stmt = conn.prepare(
        "SELECT e.* FROM entries e
         WHERE e.active = 1 AND e.lat IS NOT NULL AND e.lng IS NOT NULL
           AND NOT EXISTS (SELECT 1 FROM entry_shops es WHERE es.entry_id = e.id)
         ORDER BY e.display_name, e.handle",
    )?;
    let solos = stmt.query_map([], crate::entries::row_to_entry)?;
    for artist in solos {
        let a = artist?;
        pins.push(MapPin {
            lat: a.lat.unwrap(),
            lng: a.lng.unwrap(),
            shop: None,
            artists: vec![a],
        });
    }
    Ok(pins)
}

/// Active artists linked to a shop.
pub fn entries_for_shop(conn: &Connection, shop_id: i64) -> Result<Vec<crate::entries::Entry>> {
    let mut stmt = conn.prepare(
        "SELECT e.* FROM entries e
         JOIN entry_shops es ON es.entry_id = e.id
         WHERE es.shop_id = ?1 AND e.active = 1
         ORDER BY e.display_name, e.handle",
    )?;
    let rows = stmt.query_map(params![shop_id], crate::entries::row_to_entry)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::entries;

    #[test]
    fn shop_crud_and_uniqueness() {
        let conn = open_in_memory().unwrap();
        let id = add(&conn, "Heart Eyes").unwrap().unwrap();
        // Re-adding the same name returns the same id, no duplicate.
        assert_eq!(add(&conn, " Heart Eyes ").unwrap(), Some(id));
        assert_eq!(list(&conn).unwrap().len(), 1);
        assert!(add(&conn, "   ").unwrap().is_none());

        update(
            &conn,
            id,
            "Heart Eyes",
            "1 Main St",
            Some(45.5),
            Some(-122.6),
        )
        .unwrap();
        let s = get(&conn, id).unwrap().unwrap();
        assert_eq!(s.address, "1 Main St");
        assert!(s.located());

        assert!(delete(&conn, id).unwrap());
        assert!(list(&conn).unwrap().is_empty());
    }

    #[test]
    fn many_to_many_links() {
        let conn = open_in_memory().unwrap();
        let a = entries::add_by_handle(&conn, "artist_a").unwrap().unwrap();
        let s1 = add(&conn, "Shop One").unwrap().unwrap();
        let s2 = add(&conn, "Shop Two").unwrap().unwrap();

        set_entry_shops(&conn, a, &[s1, s2]).unwrap();
        assert_eq!(shops_for_entry(&conn, a).unwrap().len(), 2);
        assert_eq!(entries_for_shop(&conn, s1).unwrap().len(), 1);

        // Replacing links is a full replace, not append.
        set_entry_shops(&conn, a, &[s2]).unwrap();
        let ids = shop_ids_for_entry(&conn, a).unwrap();
        assert_eq!(ids, vec![s2]);

        // Deleting a shop cascades the link away.
        delete(&conn, s2).unwrap();
        assert!(shops_for_entry(&conn, a).unwrap().is_empty());
    }

    #[test]
    fn map_pins_prefers_shop_location_and_includes_solos() {
        let conn = open_in_memory().unwrap();

        // A located shop with two active artists -> one shop pin.
        let shop = add(&conn, "Heart Eyes").unwrap().unwrap();
        update(
            &conn,
            shop,
            "Heart Eyes",
            "1 Main St",
            Some(45.5),
            Some(-122.6),
        )
        .unwrap();
        let a = entries::add_by_handle(&conn, "a").unwrap().unwrap();
        let b = entries::add_by_handle(&conn, "b").unwrap().unwrap();
        for id in [a, b] {
            entries::update(
                &conn,
                id,
                &entries::EntryEdit {
                    display_name: "x".into(),
                    active: true,
                    // A shop artist's own address must be ignored on the map.
                    address: "999 Ignored Ave".into(),
                    lat: Some(1.0),
                    lng: Some(1.0),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        set_entry_shops(&conn, a, &[shop]).unwrap();
        set_entry_shops(&conn, b, &[shop]).unwrap();

        // A shopless, located artist -> one solo pin.
        let solo = entries::add_by_handle(&conn, "solo").unwrap().unwrap();
        entries::update(
            &conn,
            solo,
            &entries::EntryEdit {
                display_name: "Solo".into(),
                active: true,
                lat: Some(40.0),
                lng: Some(-70.0),
                ..Default::default()
            },
        )
        .unwrap();

        let pins = map_pins(&conn).unwrap();
        assert_eq!(pins.len(), 2);

        let shop_pin = pins.iter().find(|p| p.shop.is_some()).unwrap();
        // The shop's coords win over the artists' own (1.0, 1.0).
        assert_eq!((shop_pin.lat, shop_pin.lng), (45.5, -122.6));
        assert_eq!(shop_pin.artists.len(), 2);

        let solo_pin = pins.iter().find(|p| p.shop.is_none()).unwrap();
        assert_eq!((solo_pin.lat, solo_pin.lng), (40.0, -70.0));
        assert_eq!(solo_pin.artists.len(), 1);
    }

    #[test]
    fn backfill_creates_shops_from_legacy_text() {
        // Two artists sharing a shop string, one solo, via the migration path.
        let conn = open_in_memory().unwrap();
        let a = entries::add_by_handle(&conn, "a").unwrap().unwrap();
        let b = entries::add_by_handle(&conn, "b").unwrap().unwrap();
        for id in [a, b] {
            entries::update(
                &conn,
                id,
                &entries::EntryEdit {
                    display_name: "x".into(),
                    shop: "Heart Eyes".into(),
                    active: true,
                    ..Default::default()
                },
            )
            .unwrap();
        }
        // open_in_memory already ran migrate once (no shops then); run the
        // backfill again now that shop strings exist.
        crate::db::backfill_shops_for_test(&conn).unwrap();

        // One shop created, both artists linked to it.
        let shops = list(&conn).unwrap();
        assert_eq!(shops.len(), 1);
        assert_eq!(shops[0].name, "Heart Eyes");
        assert_eq!(entries_for_shop(&conn, shops[0].id).unwrap().len(), 2);

        // Idempotent: running again changes nothing.
        crate::db::backfill_shops_for_test(&conn).unwrap();
        assert_eq!(list(&conn).unwrap().len(), 1);
    }
}
