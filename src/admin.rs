//! Admin routes: login/logout, list, add-by-handle, edit, delete. All
//! server-rendered forms; the bar is "the tattooer never needs it explained
//! twice".

use askama::Template;
use axum::body::Bytes;
use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::{self, SESSION_COOKIE};
use crate::entries::{self, Entry};

// --- templates --------------------------------------------------------------

#[derive(Template)]
#[template(path = "login.html")]
struct LoginPage<'a> {
    site_name: &'a str,
    tagline: &'a str,
    failed: bool,
    /// The login page is the one place the viewer is never authed.
    authed: bool,
}

#[derive(Template)]
#[template(path = "admin.html")]
struct AdminPage<'a> {
    site_name: &'a str,
    tagline: &'a str,
    entity: &'a str,
    entities: &'a str,
    entries: Vec<Entry>,
    message: String,
    /// Always true here (behind the admin gate); present so base.html's nav
    /// can render uniformly across every page.
    authed: bool,
}

#[derive(Template)]
#[template(path = "edit.html")]
struct EditPage<'a> {
    site_name: &'a str,
    tagline: &'a str,
    entry: Entry,
    available_tags: String,
    /// All shops, with a flag for whether this entry is linked to each.
    shops: Vec<(crate::shops::Shop, bool)>,
    /// Always true here (behind the admin gate); present for base.html's nav.
    authed: bool,
}

#[derive(Template)]
#[template(path = "shops.html")]
struct ShopsPage<'a> {
    site_name: &'a str,
    tagline: &'a str,
    shops: Vec<crate::shops::Shop>,
    message: String,
    /// Always true here (behind the admin gate); present for base.html's nav.
    authed: bool,
}

#[derive(Template)]
#[template(path = "shop_edit.html")]
struct ShopEditPage<'a> {
    site_name: &'a str,
    tagline: &'a str,
    shop: crate::shops::Shop,
    /// Active artists linked to this shop, shown as read-only context.
    artists: Vec<Entry>,
    /// Always true here (behind the admin gate); present for base.html's nav.
    authed: bool,
}

fn render<T: Template>(t: T) -> Response {
    match t.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("template error: {e}"),
        )
            .into_response(),
    }
}

// --- auth gate --------------------------------------------------------------

pub fn is_authed(state: &AppState, headers: &HeaderMap) -> bool {
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(auth::token_from_cookie_header)
        .is_some_and(|t| state.sessions.is_valid(&t))
}

macro_rules! require_admin {
    ($state:expr, $headers:expr) => {
        if !is_authed(&$state, &$headers) {
            return Redirect::to("/admin/login").into_response();
        }
    };
}

// --- handlers ---------------------------------------------------------------

pub async fn login_page(State(state): State<Arc<AppState>>) -> Response {
    let d = &state.config.directory;
    render(LoginPage {
        site_name: &d.name,
        tagline: &d.tagline,
        failed: false,
        authed: false,
    })
}

#[derive(Deserialize)]
pub struct LoginForm {
    username: String,
    password: String,
}

pub async fn login_submit(
    State(state): State<Arc<AppState>>,
    Form(form): Form<LoginForm>,
) -> Response {
    let ok = {
        let conn = state.db.lock().unwrap();
        crate::admin_users::verify(&conn, &form.username, &form.password).unwrap_or(false)
    };
    if !ok {
        let d = &state.config.directory;
        return render(LoginPage {
            site_name: &d.name,
            tagline: &d.tagline,
            failed: true,
            authed: false,
        });
    }
    let token = state.sessions.create();
    let cookie = format!("{SESSION_COOKIE}={token}; HttpOnly; SameSite=Lax; Path=/");
    ([(header::SET_COOKIE, cookie)], Redirect::to("/admin")).into_response()
}

pub async fn logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(token) = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(auth::token_from_cookie_header)
    {
        state.sessions.revoke(&token);
    }
    let clear = format!("{SESSION_COOKIE}=; Max-Age=0; Path=/");
    ([(header::SET_COOKIE, clear)], Redirect::to("/")).into_response()
}

pub async fn dashboard(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    require_admin!(state, headers);
    let conn = state.db.lock().unwrap();
    let entries = entries::list(&conn, true).unwrap_or_default();
    drop(conn);
    let d = &state.config.directory;
    render(AdminPage {
        site_name: &d.name,
        tagline: &d.tagline,
        entity: &d.entity,
        entities: &d.entities,
        entries,
        message: String::new(),
        authed: true,
    })
}

#[derive(Deserialize)]
pub struct AddForm {
    handle: String,
}

pub async fn add(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<AddForm>,
) -> Response {
    require_admin!(state, headers);
    // Block-scope the guard: an explicit drop() is not enough for the
    // async Send analysis when an await follows in the same scope.
    let added = {
        let conn = state.db.lock().unwrap();
        entries::add_by_handle(&conn, &form.handle)
    };
    match added {
        Ok(Some(id)) => {
            // Best-effort Instagram prefill: fetch off the async runtime,
            // apply only over defaults, and any failure means the edit form
            // simply comes up unprefilled. Never blocks the add.
            let handle = entries::normalize_handle(&form.handle);
            let prefill = tokio::task::spawn_blocking(move || {
                let p = crate::instagram::fetch_profile_prefill(&handle)?;
                let avatar = p.avatar_url.as_deref().and_then(|url| {
                    crate::instagram::download_avatar(std::path::Path::new("."), &handle, url)
                });
                Some((p.display_name, avatar))
            })
            .await
            .ok()
            .flatten();
            if let Some((name, avatar)) = prefill {
                let conn = state.db.lock().unwrap();
                let _ = entries::apply_prefill(&conn, id, name.as_deref(), avatar.as_deref());
            }
            Redirect::to(&format!("/admin/edit/{id}")).into_response()
        }
        Ok(None) => {
            let conn = state.db.lock().unwrap();
            let entries = entries::list(&conn, true).unwrap_or_default();
            drop(conn);
            let d = &state.config.directory;
            render(AdminPage {
                site_name: &d.name,
                tagline: &d.tagline,
                entity: &d.entity,
                entities: &d.entities,
                entries,
                message: format!("'{}' is empty or already listed.", form.handle.trim()),
                authed: true,
            })
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn edit_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    require_admin!(state, headers);
    let (entry, shops) = {
        let conn = state.db.lock().unwrap();
        let entry = entries::get(&conn, id).ok().flatten();
        let linked = crate::shops::shop_ids_for_entry(&conn, id).unwrap_or_default();
        let shops: Vec<(crate::shops::Shop, bool)> = crate::shops::list(&conn)
            .unwrap_or_default()
            .into_iter()
            .map(|s| {
                let is_linked = linked.contains(&s.id);
                (s, is_linked)
            })
            .collect();
        (entry, shops)
    };
    let Some(entry) = entry else {
        return (StatusCode::NOT_FOUND, "no such entry").into_response();
    };
    let d = &state.config.directory;
    render(EditPage {
        site_name: &d.name,
        tagline: &d.tagline,
        entry,
        available_tags: state.config.tags.available.join(", "),
        shops,
        authed: true,
    })
}

/// The artist edit form, hand-parsed from the urlencoded body so repeated
/// `shop_id` checkboxes survive (serde_urlencoded collapses repeated keys).
struct EditForm {
    display_name: String,
    shop: String,
    bio: String,
    tags: String,
    featured_posts: String,
    booking_url: String,
    address: String,
    active: bool,
}

/// Parse an `application/x-www-form-urlencoded` body into key/value pairs,
/// preserving repeated keys (checkbox groups). `+` is a space; `%XX` is a
/// byte. Hand-rolled to keep albo's dependency surface minimal.
fn parse_form(body: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for pair in body.split(|&b| b == b'&') {
        if pair.is_empty() {
            continue;
        }
        let mut it = pair.splitn(2, |&b| b == b'=');
        let key = it.next().unwrap_or(&[]);
        let val = it.next().unwrap_or(&[]);
        out.push((form_decode(key), form_decode(val)));
    }
    out
}

fn form_decode(bytes: &[u8]) -> String {
    let mut buf = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                buf.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                match (hi, lo) {
                    (Some(h), Some(l)) => {
                        buf.push((h * 16 + l) as u8);
                        i += 3;
                    }
                    _ => {
                        buf.push(b'%');
                        i += 1;
                    }
                }
            }
            b => {
                buf.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

pub async fn edit_submit(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    // Raw body so we can read both the typed fields and the repeated
    // `shop_id` checkboxes (serde_urlencoded collapses repeated keys).
    body: Bytes,
) -> Response {
    require_admin!(state, headers);
    let pairs = parse_form(&body);
    let field = |k: &str| -> String {
        pairs
            .iter()
            .find(|(pk, _)| pk == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let form = EditForm {
        display_name: field("display_name"),
        shop: field("shop"),
        bio: field("bio"),
        tags: field("tags"),
        featured_posts: field("featured_posts"),
        booking_url: field("booking_url"),
        address: field("address"),
        // A checkbox is present in the body only when checked.
        active: pairs.iter().any(|(k, _)| k == "active"),
    };
    // Checkboxes post as repeated `shop_id=<n>` pairs; collect the ids.
    let shop_ids: Vec<i64> = pairs
        .iter()
        .filter(|(k, _)| k == "shop_id")
        .filter_map(|(_, v)| v.parse::<i64>().ok())
        .collect();
    let address = form.address.trim().to_string();

    // Preserve existing coordinates if the address is unchanged; only
    // (re)geocode when the address is new or edited. Empty address clears
    // the pin. Geocoding is a blocking network call, run off the runtime,
    // and best-effort - a failure just leaves the entry off the map.
    let (prev_addr, prev_lat, prev_lng) = {
        let conn = state.db.lock().unwrap();
        match entries::get(&conn, id).ok().flatten() {
            Some(e) => (e.address, e.lat, e.lng),
            None => return (StatusCode::NOT_FOUND, "no such entry").into_response(),
        }
    };
    let (lat, lng) = if address.is_empty() {
        (None, None)
    } else if address == prev_addr && prev_lat.is_some() {
        (prev_lat, prev_lng) // unchanged, keep cached coords, no network call
    } else {
        let a = address.clone();
        match tokio::task::spawn_blocking(move || crate::geocode::geocode(&a)).await {
            Ok(Some(ll)) => (Some(ll.lat), Some(ll.lng)),
            _ => (None, None), // geocode failed: saved without a pin
        }
    };

    let edit = entries::EntryEdit {
        display_name: form.display_name.trim().to_string(),
        shop: form.shop.trim().to_string(),
        bio: form.bio.trim().to_string(),
        tags: form.tags.trim().to_string(),
        featured_posts: form.featured_posts.trim().to_string(),
        booking_url: form.booking_url.trim().to_string(),
        address,
        lat,
        lng,
        active: form.active,
    };
    let res = {
        let conn = state.db.lock().unwrap();
        let updated = entries::update(&conn, id, &edit);
        // Set shop links regardless (an empty selection clears them). Only
        // when the entry actually exists, matched by the update result.
        if let Ok(true) = updated {
            let _ = crate::shops::set_entry_shops(&conn, id, &shop_ids);
        }
        updated
    };
    match res {
        Ok(true) => Redirect::to("/admin").into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such entry").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    require_admin!(state, headers);
    let conn = state.db.lock().unwrap();
    let _ = entries::delete(&conn, id);
    drop(conn);
    Redirect::to("/admin").into_response()
}

// --- shop management --------------------------------------------------------

fn shops_page(state: &AppState, message: String) -> Response {
    let shops = {
        let conn = state.db.lock().unwrap();
        crate::shops::list(&conn).unwrap_or_default()
    };
    let d = &state.config.directory;
    render(ShopsPage {
        site_name: &d.name,
        tagline: &d.tagline,
        shops,
        message,
        authed: true,
    })
}

pub async fn shops_dashboard(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    require_admin!(state, headers);
    shops_page(&state, String::new())
}

#[derive(Deserialize)]
pub struct ShopAddForm {
    name: String,
}

pub async fn shop_add(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<ShopAddForm>,
) -> Response {
    require_admin!(state, headers);
    let name = form.name.trim().to_string();
    if name.is_empty() {
        return shops_page(&state, "Shop name cannot be empty.".into());
    }
    {
        let conn = state.db.lock().unwrap();
        let _ = crate::shops::add(&conn, &name);
    }
    Redirect::to("/admin/shops").into_response()
}

pub async fn shop_edit_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    require_admin!(state, headers);
    let (shop, artists) = {
        let conn = state.db.lock().unwrap();
        let shop = crate::shops::get(&conn, id).ok().flatten();
        let artists = crate::shops::entries_for_shop(&conn, id).unwrap_or_default();
        (shop, artists)
    };
    let Some(shop) = shop else {
        return (StatusCode::NOT_FOUND, "no such shop").into_response();
    };
    let d = &state.config.directory;
    render(ShopEditPage {
        site_name: &d.name,
        tagline: &d.tagline,
        shop,
        artists,
        authed: true,
    })
}

#[derive(Deserialize)]
pub struct ShopEditForm {
    name: String,
    #[serde(default)]
    address: String,
}

pub async fn shop_edit(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(form): Form<ShopEditForm>,
) -> Response {
    require_admin!(state, headers);
    let name = form.name.trim().to_string();
    if name.is_empty() {
        return shops_page(&state, "Shop name cannot be empty.".into());
    }
    let address = form.address.trim().to_string();

    // Same geocoding contract as artists: only (re)geocode when the address
    // changed, keep cached coords otherwise, best-effort (no pin on failure).
    let (prev_addr, prev_lat, prev_lng) = {
        let conn = state.db.lock().unwrap();
        match crate::shops::get(&conn, id).ok().flatten() {
            Some(s) => (s.address, s.lat, s.lng),
            None => return (StatusCode::NOT_FOUND, "no such shop").into_response(),
        }
    };
    let (lat, lng) = if address.is_empty() {
        (None, None)
    } else if address == prev_addr && prev_lat.is_some() {
        (prev_lat, prev_lng)
    } else {
        let a = address.clone();
        match tokio::task::spawn_blocking(move || crate::geocode::geocode(&a)).await {
            Ok(Some(ll)) => (Some(ll.lat), Some(ll.lng)),
            _ => (None, None),
        }
    };
    let res = {
        let conn = state.db.lock().unwrap();
        crate::shops::update(&conn, id, &name, &address, lat, lng)
    };
    match res {
        Ok(true) => Redirect::to("/admin/shops").into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such shop").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn shop_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    require_admin!(state, headers);
    {
        let conn = state.db.lock().unwrap();
        let _ = crate::shops::delete(&conn, id);
    }
    Redirect::to("/admin/shops").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_form_keeps_repeated_keys_and_decodes() {
        let body = b"display_name=Jane+Doe&shop_id=3&shop_id=7&address=1+Main+St%2C+PDX&active=1";
        let pairs = parse_form(body);
        // Repeated shop_id survives as two entries.
        let shop_ids: Vec<&str> = pairs
            .iter()
            .filter(|(k, _)| k == "shop_id")
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(shop_ids, vec!["3", "7"]);
        // '+' decodes to space, %2C to a comma.
        let addr = pairs.iter().find(|(k, _)| k == "address").unwrap();
        assert_eq!(addr.1, "1 Main St, PDX");
        let name = pairs.iter().find(|(k, _)| k == "display_name").unwrap();
        assert_eq!(name.1, "Jane Doe");
        // A present checkbox key has an empty-ish value but exists.
        assert!(pairs.iter().any(|(k, _)| k == "active"));
    }

    #[test]
    fn parse_form_handles_empty_and_valueless() {
        let pairs = parse_form(b"a=&b&=c");
        assert_eq!(pairs.iter().find(|(k, _)| k == "a").unwrap().1, "");
        assert_eq!(pairs.iter().find(|(k, _)| k == "b").unwrap().1, "");
        assert!(parse_form(b"").is_empty());
    }

    #[test]
    fn form_decode_leaves_bad_escapes_intact() {
        // A stray percent with no following hex digits stays literal.
        assert_eq!(form_decode(b"100%"), "100%");
        assert_eq!(form_decode(b"a%zz"), "a%zz");
    }
}
