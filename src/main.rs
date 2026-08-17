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

use anyhow::{Context, Result};
use askama::Template;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
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

#[derive(Template)]
#[template(path = "index.html")]
struct IndexPage<'a> {
    site_name: &'a str,
    tagline: &'a str,
    entities: &'a str,
    entries: Vec<entries::Entry>,
    /// (tag, count) pairs for the filter bar.
    tags: Vec<(String, usize)>,
    /// The currently-selected tag, empty when showing all.
    active_tag: String,
    /// Pre-rendered JSON array of map pins for the located entries, so the
    /// template doesn't do JSON. Empty array when nothing is located.
    pins_json: String,
    /// Whether any entry has a location - controls whether the map toggle shows.
    has_map: bool,
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

/// Build the map-pin JSON array for a set of entries. Only located entries
/// are included; each pin carries what the popup needs.
fn pins_json(entries: &[entries::Entry]) -> String {
    let mut items: Vec<String> = Vec::new();
    for e in entries.iter().filter(|e| e.located()) {
        items.push(format!(
            "{{\"name\":\"{}\",\"handle\":\"{}\",\"shop\":\"{}\",\"lat\":{},\"lng\":{}}}",
            json_escape(&e.display_name),
            json_escape(&e.handle),
            json_escape(&e.shop),
            e.lat.unwrap(),
            e.lng.unwrap(),
        ));
    }
    format!("[{}]", items.join(","))
}

#[derive(serde::Deserialize)]
struct IndexQuery {
    #[serde(default)]
    tag: String,
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
}

async fn artist(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(handle): axum::extract::Path<String>,
) -> Response {
    let normalized = entries::normalize_handle(&handle);
    let conn = state.db.lock().unwrap();
    let entry = entries::get_by_handle(&conn, &normalized).ok().flatten();
    drop(conn);
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
    axum::extract::Query(q): axum::extract::Query<IndexQuery>,
) -> Response {
    let tag = q.tag.trim().to_string();
    let (listed, tags) = {
        let conn = state.db.lock().unwrap();
        let listed = if tag.is_empty() {
            entries::list(&conn, false)
        } else {
            entries::list_by_tag(&conn, &tag)
        }
        .unwrap_or_default();
        let tags = entries::tags_in_use(&conn).unwrap_or_default();
        (listed, tags)
    };
    let pins = pins_json(&listed);
    let has_map = listed.iter().any(entries::Entry::located);
    let d = &state.config.directory;
    let page = IndexPage {
        site_name: &d.name,
        tagline: &d.tagline,
        entities: &d.entities,
        entries: listed,
        tags,
        active_tag: tag,
        pins_json: pins,
        has_map,
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
