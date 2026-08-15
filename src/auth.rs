//! Admin authentication: argon2 password verification + in-memory session
//! tokens carried in a cookie. Single-admin, single-process by design; if
//! albo ever grows multi-admin or multi-process, sessions move to the DB.

use argon2::password_hash::{PasswordHash, PasswordHasher, SaltString};
use argon2::{Argon2, PasswordVerifier};
use std::collections::HashSet;
use std::sync::Mutex;

/// Fill a buffer from the OS entropy source, panicking only on a broken OS
/// RNG (unrecoverable anyway).
fn os_random(buf: &mut [u8]) {
    getrandom::fill(buf).expect("OS random source unavailable");
}

pub const SESSION_COOKIE: &str = "albo_session";

#[derive(Default)]
pub struct Sessions {
    tokens: Mutex<HashSet<String>>,
}

impl Sessions {
    pub fn create(&self) -> String {
        let mut bytes = [0u8; 32];
        os_random(&mut bytes);
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        self.tokens.lock().unwrap().insert(token.clone());
        token
    }

    pub fn is_valid(&self, token: &str) -> bool {
        self.tokens.lock().unwrap().contains(token)
    }

    pub fn revoke(&self, token: &str) {
        self.tokens.lock().unwrap().remove(token);
    }
}

pub fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let mut salt_bytes = [0u8; 16];
    os_random(&mut salt_bytes);
    let salt = SaltString::encode_b64(&salt_bytes)?;
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)?
        .to_string())
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Pull the session token out of a Cookie header value, if present.
pub fn token_from_cookie_header(header: &str) -> Option<String> {
    header.split(';').find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        (k == SESSION_COOKIE).then(|| v.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_roundtrip() {
        let hash = hash_password("hunter2").unwrap();
        assert!(verify_password("hunter2", &hash));
        assert!(!verify_password("hunter3", &hash));
        assert!(!verify_password("hunter2", "not-a-hash"));
    }

    #[test]
    fn sessions_lifecycle() {
        let s = Sessions::default();
        let t = s.create();
        assert!(s.is_valid(&t));
        s.revoke(&t);
        assert!(!s.is_valid(&t));
        assert!(!s.is_valid("forged"));
    }

    #[test]
    fn cookie_parsing() {
        assert_eq!(
            token_from_cookie_header("foo=bar; albo_session=abc123; x=y"),
            Some("abc123".into())
        );
        assert_eq!(token_from_cookie_header("foo=bar"), None);
        assert_eq!(token_from_cookie_header(""), None);
    }
}
