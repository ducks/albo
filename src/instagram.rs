//! Best-effort Instagram integration. Two jobs, both allowed to fail
//! without consequence:
//!
//! 1. Profile prefill: one server-side GET of the public profile page,
//!    reading og: meta tags for display name + avatar. Empirically
//!    (2026-08-15, tested against a real profile): Instagram serves real
//!    SSR og: tags to bot-looking user agents (curl's default) and an
//!    empty JS shell to browser UAs - so we deliberately identify as curl.
//!    og:description is a follower-count blurb, not the bio, so bio stays
//!    manual. Any failure returns None; the admin just types the fields.
//!
//! 2. Featured-post embeds: we never scrape posts. We validate that a URL
//!    is an instagram.com/p/... or /reel/... link and render Instagram's
//!    own /embed/ iframe for it on the public page. If Meta changes embed
//!    behavior, the iframe degrades; the profile link next to it still
//!    works.
//!
//! Nothing in albo may *depend* on this module succeeding.

use std::path::Path;
use std::time::Duration;

/// What a successful profile fetch yields. All fields optional in spirit;
/// present fields are still admin-overridable.
#[derive(Debug, PartialEq)]
pub struct ProfilePrefill {
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

/// Fetch the public profile page and extract og: metadata. Returns None on
/// any failure (network, login wall, markup change) - callers treat that
/// as "no prefill available", never as an error.
pub fn fetch_profile_prefill(handle: &str) -> Option<ProfilePrefill> {
    let url = format!("https://www.instagram.com/{handle}/");
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        // The empirically-working identity; see module docs.
        .user_agent("curl/8.9.1")
        .build();
    let body = agent.get(&url).call().ok()?.into_string().ok()?;
    Some(parse_profile_html(&body, handle))
}

/// Extract prefill fields from profile HTML. Separated from fetching so it
/// is testable against fixture markup.
pub fn parse_profile_html(html: &str, handle: &str) -> ProfilePrefill {
    let og_title = meta_content(html, "og:title");
    let display_name = og_title.as_deref().and_then(|t| {
        // "Yam Yippy (@damn_zippy) • Instagram photos and videos"
        let decoded = decode_entities(t);
        let name = decoded.split(" (@").next()?.trim().to_string();
        // A title that's just "Instagram" (the shell) or contains the bare
        // handle only isn't a real name.
        (!name.is_empty() && name != "Instagram" && name.to_lowercase() != handle.to_lowercase())
            .then_some(name)
    });
    let avatar_url = meta_content(html, "og:image").map(|u| decode_entities(&u));
    ProfilePrefill {
        display_name,
        avatar_url,
    }
}

/// Pull content="..." for a <meta property="NAME" .../> tag without an HTML
/// parser dependency. Instagram's SSR markup is machine-generated and
/// stable enough for this; if it ever isn't, we return None and the admin
/// types the field - the failure mode is the designed one.
fn meta_content(html: &str, property: &str) -> Option<String> {
    let needle = format!("property=\"{property}\"");
    let tag_start = html.find(&needle)?;
    let rest = &html[tag_start..];
    let content_start = rest.find("content=\"")? + "content=\"".len();
    let rest = &rest[content_start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Minimal HTML entity decoding for the entities Instagram actually emits
/// in og: tags (&#064; for @, &#x2022; for the bullet, plus the basics).
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        if c != '&' {
            out.push(c);
            continue;
        }
        let rest = &s[i..];
        let Some(semi) = rest.find(';') else {
            out.push(c);
            continue;
        };
        let entity = &rest[1..semi];
        let decoded: Option<char> = match entity {
            "amp" => Some('&'),
            "quot" => Some('"'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "#39" | "apos" => Some('\''),
            _ => {
                if let Some(hex) = entity
                    .strip_prefix("#x")
                    .or_else(|| entity.strip_prefix("#X"))
                {
                    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
                } else if let Some(dec) = entity.strip_prefix('#') {
                    dec.parse::<u32>().ok().and_then(char::from_u32)
                } else {
                    None
                }
            }
        };
        match decoded {
            Some(d) => {
                out.push(d);
                // Skip the rest of the entity.
                for _ in 0..semi {
                    chars.next();
                }
            }
            None => out.push(c),
        }
    }
    out
}

/// Download an avatar image to `avatars/<handle>.jpg` under `data_dir`.
/// Returns the relative path stored in the DB, or None on any failure.
/// We cache locally because Instagram CDN URLs are signed and expire -
/// hotlinking them is the rot we're avoiding.
pub fn download_avatar(data_dir: &Path, handle: &str, avatar_url: &str) -> Option<String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .user_agent("curl/8.9.1")
        .build();
    let resp = agent.get(avatar_url).call().ok()?;
    let mut bytes: Vec<u8> = Vec::new();
    use std::io::Read;
    resp.into_reader()
        .take(5 * 1024 * 1024) // an avatar over 5MB is not an avatar
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.is_empty() {
        return None;
    }
    let dir = data_dir.join("avatars");
    std::fs::create_dir_all(&dir).ok()?;
    let rel = format!("avatars/{handle}.jpg");
    std::fs::write(data_dir.join(&rel), &bytes).ok()?;
    Some(rel)
}

/// Validate a featured-post URL and return its official embed URL.
/// Accepts instagram.com/p/<code>/ and /reel/<code>/ forms; anything else
/// is rejected (this is also the XSS boundary for admin-entered URLs -
/// only URLs we reconstructed ourselves reach the template).
pub fn embed_url(post_url: &str) -> Option<String> {
    let trimmed = post_url.trim();
    let rest = trimmed
        .strip_prefix("https://www.instagram.com/")
        .or_else(|| trimmed.strip_prefix("https://instagram.com/"))
        .or_else(|| trimmed.strip_prefix("http://www.instagram.com/"))
        .or_else(|| trimmed.strip_prefix("http://instagram.com/"))
        .or_else(|| trimmed.strip_prefix("www.instagram.com/"))
        .or_else(|| trimmed.strip_prefix("instagram.com/"))?;
    let mut parts = rest.split('/').filter(|p| !p.is_empty());
    let kind = parts.next()?;
    if kind != "p" && kind != "reel" {
        return None;
    }
    let code = parts.next()?;
    // Shortcodes are URL-safe base64-ish; reject anything else.
    if code.is_empty()
        || !code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    Some(format!("https://www.instagram.com/{kind}/{code}/embed/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
<meta property="og:type" content="profile" />
<meta property="og:image" content="https://scontent.cdninstagram.com/v/t51/avatar.jpg?stp=x&amp;ccb=7-5" />
<meta property="og:title" content="Yam Yippy (&#064;damn_zippy) &#x2022; Instagram photos and videos" />
<meta property="og:description" content="38K Followers, 4,909 Following - See photos" />
"#;

    #[test]
    fn parses_real_profile_markup() {
        let p = parse_profile_html(FIXTURE, "damn_zippy");
        assert_eq!(p.display_name.as_deref(), Some("Yam Yippy"));
        let avatar = p.avatar_url.unwrap();
        assert!(avatar.starts_with("https://scontent.cdninstagram.com/"));
        // &amp; must decode so the signed URL survives.
        assert!(avatar.contains("stp=x&ccb=7-5"));
    }

    #[test]
    fn shell_page_yields_no_name() {
        let shell = r#"<meta property="og:title" content="Instagram" />"#;
        let p = parse_profile_html(shell, "whoever");
        assert_eq!(p.display_name, None);
    }

    #[test]
    fn missing_tags_yield_none_not_error() {
        let p = parse_profile_html("<html><body>login wall</body></html>", "x");
        assert_eq!(p.display_name, None);
        assert_eq!(p.avatar_url, None);
    }

    #[test]
    fn decode_entities_covers_instagrams_repertoire() {
        assert_eq!(decode_entities("&#064;handle"), "@handle");
        assert_eq!(decode_entities("a &#x2022; b"), "a \u{2022} b");
        assert_eq!(decode_entities("x&amp;y"), "x&y");
        assert_eq!(decode_entities("no entities"), "no entities");
        assert_eq!(decode_entities("broken &#"), "broken &#");
    }

    #[test]
    fn embed_url_validation() {
        assert_eq!(
            embed_url("https://www.instagram.com/p/Cxy-Z_1/"),
            Some("https://www.instagram.com/p/Cxy-Z_1/embed/".into())
        );
        assert_eq!(
            embed_url("instagram.com/reel/AbC123/"),
            Some("https://www.instagram.com/reel/AbC123/embed/".into())
        );
        // Profile links, other hosts, and injection attempts are rejected.
        assert_eq!(embed_url("https://www.instagram.com/damn_zippy/"), None);
        assert_eq!(embed_url("https://evil.com/p/AbC123/"), None);
        assert_eq!(embed_url("https://www.instagram.com/p/\"><script>/"), None);
        assert_eq!(embed_url(""), None);
    }
}
