//! Admin account storage. A normal login system: credentials live in the
//! database (username + argon2 hash), created and changed at runtime via the
//! CLI - never in config, never in the Nix store. Login verifies against
//! this table.

use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, params};

use crate::auth;

/// Create or update an admin. If the username exists, the password is
/// replaced (so this doubles as password reset).
pub fn set(conn: &Connection, username: &str, password: &str) -> Result<()> {
    let username = username.trim();
    if username.is_empty() {
        bail!("username cannot be empty");
    }
    if password.len() < 8 {
        bail!("password must be at least 8 characters");
    }
    let hash = auth::hash_password(password).map_err(|e| anyhow::anyhow!("hashing failed: {e}"))?;
    conn.execute(
        "INSERT INTO admin_users (username, password_hash) VALUES (?1, ?2)
         ON CONFLICT(username) DO UPDATE SET password_hash = excluded.password_hash",
        params![username, hash],
    )?;
    Ok(())
}

/// Verify a login attempt against the stored hash. Constant-ish time: we
/// always run argon2, even for an unknown user, so timing doesn't leak
/// which usernames exist.
pub fn verify(conn: &Connection, username: &str, password: &str) -> Result<bool> {
    let hash: Option<String> = conn
        .query_row(
            "SELECT password_hash FROM admin_users WHERE username = ?1",
            params![username.trim()],
            |row| row.get(0),
        )
        .optional()?;
    // A real argon2 hash (of a throwaway value) for the no-such-user case,
    // so we always run a full verify and timing doesn't leak which
    // usernames exist.
    const DUMMY: &str = "$argon2id$v=19$m=19456,t=2,p=1$iNH5NMN+AKCkAwV/R/DiAA$\
                         F7omjvmfg1dH+K4dEzkFJ2TzBodjtfvvnh2QzTCFjeI";
    let target = hash.as_deref().unwrap_or(DUMMY);
    Ok(auth::verify_password(password, target) && hash.is_some())
}

pub fn count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM admin_users", [], |r| r.get(0))?)
}

pub fn remove(conn: &Connection, username: &str) -> Result<bool> {
    let n = conn.execute(
        "DELETE FROM admin_users WHERE username = ?1",
        params![username.trim()],
    )?;
    Ok(n > 0)
}

pub fn list(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT username FROM admin_users ORDER BY username")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn set_verify_roundtrip() {
        let conn = open_in_memory().unwrap();
        set(&conn, "jake", "correcthorse").unwrap();
        assert!(verify(&conn, "jake", "correcthorse").unwrap());
        assert!(!verify(&conn, "jake", "wrongpass").unwrap());
        // Unknown user never verifies (and doesn't panic on the dummy hash).
        assert!(!verify(&conn, "nobody", "correcthorse").unwrap());
    }

    #[test]
    fn set_is_upsert_password_reset() {
        let conn = open_in_memory().unwrap();
        set(&conn, "jake", "firstpass").unwrap();
        set(&conn, "jake", "secondpass").unwrap();
        assert!(!verify(&conn, "jake", "firstpass").unwrap());
        assert!(verify(&conn, "jake", "secondpass").unwrap());
        assert_eq!(count(&conn).unwrap(), 1); // upsert, not a second row
    }

    #[test]
    fn rejects_empty_user_and_short_password() {
        let conn = open_in_memory().unwrap();
        assert!(set(&conn, "  ", "longenough").is_err());
        assert!(set(&conn, "jake", "short").is_err());
    }

    #[test]
    fn remove_and_list() {
        let conn = open_in_memory().unwrap();
        set(&conn, "a", "password1").unwrap();
        set(&conn, "b", "password2").unwrap();
        assert_eq!(list(&conn).unwrap(), vec!["a", "b"]);
        assert!(remove(&conn, "a").unwrap());
        assert_eq!(list(&conn).unwrap(), vec!["b"]);
        assert!(!remove(&conn, "a").unwrap());
    }
}
