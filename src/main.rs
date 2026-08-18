//! albo - a curated directory engine. One binary, one directory instance,
//! configured by directory.toml. Customer #1: a Portland tattooer directory.

mod admin;
mod admin_users;
mod auth;
mod config;
mod db;
mod entries;
mod geocode;
mod instagram;
mod shops;

use anyhow::{Context, Result};
use askama::Template;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use clap::{Parser, Subcommand};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub struct AppState {
    pub config: config::Config,
    pub db: Mutex<Connection>,
    pub sessions: auth::Sessions,
}

#[derive(Parser)]
#[command(
    name = "albo",
    about = "A curated directory of skilled people, built from Instagram handles",
    version
)]
struct Cli {
    /// Path to the instance config
    #[arg(long, default_value = "directory.toml", global = true)]
    config: PathBuf,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the web server (default when no subcommand is given)
    Serve,
    /// Create an admin, or reset an existing admin's password (prompts
    /// for the password; never pass it on the command line)
    AdminAdd { username: String },
    /// Remove an admin account
    AdminRemove { username: String },
    /// List admin accounts
    AdminList,
}

/// Open the instance DB from the config's database path.
fn open_db(config: &config::Config) -> Result<Connection> {
    db::open(Path::new(&config.server.database))
}

/// Read a password for admin creation. On a real terminal, prompt twice
/// without echo. When stdin is piped (automation, tests), read one line -
/// this is how a self-hoster scripts admin creation:
///   echo "$PASS" | albo admin-add jake
fn prompt_password() -> Result<String> {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        let p = rpassword::prompt_password("Password: ")?;
        let again = rpassword::prompt_password("Confirm: ")?;
        if p != again {
            anyhow::bail!("passwords did not match");
        }
        Ok(p)
    } else {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        let p = line.trim_end_matches(['\n', '\r']).to_string();
        if p.is_empty() {
            anyhow::bail!("no password provided on stdin");
        }
        Ok(p)
    }
}

/// One row of the public list: the entry plus the shops it's linked to, so
/// the table can link the shop to its page instead of showing the free-text
/// fallback. `shops` is empty when the artist has no linked shop.
struct IndexRow {
    entry: entries::Entry,
    shops: Vec<shops::Shop>,
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexPage<'a> {
    site_name: &'a str,
    tagline: &'a str,
    entities: &'a str,
    entries: Vec<IndexRow>,
    /// (tag, count) pairs for the filter bar.
    tags: Vec<(String, usize)>,
    /// The currently-selected tag, empty when showing all.
    active_tag: String,
    /// The current search query, echoed back into the search box.
    active_query: String,
    /// The search query URL-encoded, for safely carrying it in link hrefs
    /// (tag links, "All", "clear"). Empty when there's no query.
    encoded_query: String,
    /// Pre-rendered JSON array of map pins for the located entries, so the
    /// template doesn't do JSON. Empty array when nothing is located.
    pins_json: String,
    /// Whether any entry has a location - controls whether the map toggle shows.
    has_map: bool,
    /// Whether the viewer is a logged-in admin (drives the header nav).
    authed: bool,
}

/// JSON-escape a string for embedding in the pins array. Handles the
/// characters that would break a JSON string; enough for names/handles.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Build the map-pin JSON array. Each pin is a shop (with its roster) or a
/// solo shopless artist; the popup shows the shop link and every artist.
fn pins_json(pins: &[shops::MapPin]) -> String {
    let mut items: Vec<String> = Vec::new();
    for p in pins {
        let (shop_id, shop_name) = match &p.shop {
            Some(s) => (s.id.to_string(), json_escape(&s.name)),
            None => ("0".to_string(), String::new()),
        };
        let artists: Vec<String> = p
            .artists
            .iter()
            .map(|a| {
                format!(
                    "{{\"name\":\"{}\",\"handle\":\"{}\"}}",
                    json_escape(&a.display_name),
                    json_escape(&a.handle),
                )
            })
            .collect();
        items.push(format!(
            "{{\"lat\":{},\"lng\":{},\"shop_id\":{},\"shop\":\"{}\",\"artists\":[{}]}}",
            p.lat,
            p.lng,
            shop_id,
            shop_name,
            artists.join(","),
        ));
    }
    format!("[{}]", items.join(","))
}

#[derive(serde::Deserialize)]
struct IndexQuery {
    #[serde(default)]
    tag: String,
    /// Free-text search across entry fields. Empty means no search.
    #[serde(default)]
    q: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = config::Config::load(&cli.config)?;

    match cli.command.unwrap_or(Command::Serve) {
        Command::AdminAdd { username } => {
            let conn = open_db(&config)?;
            let password = prompt_password()?;
            admin_users::set(&conn, &username, &password)?;
            println!("admin '{username}' saved");
            return Ok(());
        }
        Command::AdminRemove { username } => {
            let conn = open_db(&config)?;
            if admin_users::remove(&conn, &username)? {
                println!("removed admin '{username}'");
            } else {
                println!("no admin named '{username}'");
            }
            return Ok(());
        }
        Command::AdminList => {
            let conn = open_db(&config)?;
            for u in admin_users::list(&conn)? {
                println!("{u}");
            }
            return Ok(());
        }
        Command::Serve => {}
    }

    let conn = open_db(&config)?;
    if admin_users::count(&conn)? == 0 {
        eprintln!(
            "warning: no admin accounts exist - admin login is impossible. \
             Create one with `albo admin-add <username>`."
        );
    }
    let bind = config.server.bind.clone();
    let state = Arc::new(AppState {
        config,
        db: Mutex::new(conn),
        sessions: auth::Sessions::default(),
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/a/{handle}", get(artist))
        .route("/s/{id}", get(shop_page))
        .nest_service("/avatars", tower_http::services::ServeDir::new("avatars"))
        .route("/health", get(|| async { "ok" }))
        .route("/admin", get(admin::dashboard))
        .route(
            "/admin/login",
            get(admin::login_page).post(admin::login_submit),
        )
        .route("/admin/logout", post(admin::logout))
        .route("/admin/add", post(admin::add))
        .route(
            "/admin/edit/{id}",
            get(admin::edit_page).post(admin::edit_submit),
        )
        .route("/admin/delete/{id}", post(admin::delete))
        .route("/admin/shops", get(admin::shops_dashboard))
        .route("/admin/shops/add", post(admin::shop_add))
        .route(
            "/admin/shops/edit/{id}",
            get(admin::shop_edit_page).post(admin::shop_edit),
        )
        .route("/admin/shops/delete/{id}", post(admin::shop_delete))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("could not bind {bind}"))?;
    println!("albo serving on http://{bind}");
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Template)]
#[template(path = "artist.html")]
struct ArtistPage<'a> {
    site_name: &'a str,
    tagline: &'a str,
    entities: &'a str,
    entry: entries::Entry,
    embeds: Vec<String>,
    /// Shops this artist is linked to (linked, first-class location).
    shops: Vec<shops::Shop>,
    /// Whether the viewer is a logged-in admin (drives the header nav).
    authed: bool,
}

#[derive(Template)]
#[template(path = "shop.html")]
struct ShopPage<'a> {
    site_name: &'a str,
    tagline: &'a str,
    entities: &'a str,
    shop: shops::Shop,
    artists: Vec<entries::Entry>,
    /// Whether the viewer is a logged-in admin (drives the header nav).
    authed: bool,
}

async fn shop_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Response {
    let authed = admin::is_authed(&state, &headers);
    let (shop, artists) = {
        let conn = state.db.lock().unwrap();
        let shop = shops::get(&conn, id).ok().flatten();
        let artists = shops::entries_for_shop(&conn, id).unwrap_or_default();
        (shop, artists)
    };
    let Some(shop) = shop else {
        return (StatusCode::NOT_FOUND, "no such shop").into_response();
    };
    let d = &state.config.directory;
    let page = ShopPage {
        site_name: &d.name,
        tagline: &d.tagline,
        entities: &d.entities,
        shop,
        artists,
        authed,
    };
    match page.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("template error: {e}"),
        )
            .into_response(),
    }
}

async fn artist(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Path(handle): axum::extract::Path<String>,
) -> Response {
    let authed = admin::is_authed(&state, &headers);
    let normalized = entries::normalize_handle(&handle);
    let (entry, artist_shops) = {
        let conn = state.db.lock().unwrap();
        let entry = entries::get_by_handle(&conn, &normalized).ok().flatten();
        let artist_shops = match &entry {
            Some(e) => shops::shops_for_entry(&conn, e.id).unwrap_or_default(),
            None => Vec::new(),
        };
        (entry, artist_shops)
    };
    let Some(entry) = entry else {
        return (StatusCode::NOT_FOUND, "no such artist").into_response();
    };
    // Only URLs we reconstructed ourselves reach the template (XSS boundary).
    let embeds: Vec<String> = entry
        .featured_posts
        .iter()
        .filter_map(|u| instagram::embed_url(u))
        .collect();
    let d = &state.config.directory;
    let page = ArtistPage {
        site_name: &d.name,
        tagline: &d.tagline,
        entities: &d.entities,
        entry,
        embeds,
        shops: artist_shops,
        authed,
    };
    match page.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("template error: {e}"),
        )
            .into_response(),
    }
}

async fn index(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<IndexQuery>,
) -> Response {
    let authed = admin::is_authed(&state, &headers);
    let tag = q.tag.trim().to_string();
    let query = q.q.trim().to_string();
    let (rows, tags, map_pins) = {
        let conn = state.db.lock().unwrap();
        let listed: Vec<entries::Entry> = if tag.is_empty() {
            entries::list(&conn, false)
        } else {
            entries::list_by_tag(&conn, &tag)
        }
        .unwrap_or_default()
        .into_iter()
        // Search composes with the tag filter: narrow the tag-filtered set.
        .filter(|e| e.matches(&query))
        .collect();
        let tags = entries::tags_in_use(&conn).unwrap_or_default();
        // Batch-load the linked shops so each row can show the real shop
        // (linked to its page) instead of the free-text fallback.
        let ids: Vec<i64> = listed.iter().map(|e| e.id).collect();
        let mut by_entry = shops::shops_by_entry(&conn, &ids).unwrap_or_default();
        let rows: Vec<IndexRow> = listed
            .into_iter()
            .map(|entry| {
                let shops = by_entry.remove(&entry.id).unwrap_or_default();
                IndexRow { entry, shops }
            })
            .collect();
        // The map is shop-centric and shows the whole directory (not the tag
        // filter): one pin per located shop with its roster, plus solo pins.
        let map_pins = shops::map_pins(&conn).unwrap_or_default();
        (rows, tags, map_pins)
    };
    let pins = pins_json(&map_pins);
    let has_map = !map_pins.is_empty();
    let d = &state.config.directory;
    let page = IndexPage {
        site_name: &d.name,
        tagline: &d.tagline,
        entities: &d.entities,
        entries: rows,
        tags,
        active_tag: tag,
        encoded_query: geocode::urlencode(&query),
        active_query: query,
        pins_json: pins,
        has_map,
        authed,
    };
    match page.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("template error: {e}"),
        )
            .into_response(),
    }
}
