//! albo - a curated directory engine. One binary, one directory instance,
//! configured by directory.toml. Customer #1: a Portland tattooer directory.

mod admin;
mod auth;
mod config;
mod db;
mod entries;

use anyhow::{Context, Result};
use askama::Template;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};

pub struct AppState {
    pub config: config::Config,
    pub db: Mutex<Connection>,
    pub sessions: auth::Sessions,
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexPage<'a> {
    site_name: &'a str,
    tagline: &'a str,
    entities: &'a str,
    entries: Vec<entries::Entry>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let first = args.next();

    // `albo hash-password <password>` prints an argon2 hash for the config.
    if first.as_deref() == Some("hash-password") {
        let password = args
            .next()
            .context("usage: albo hash-password <password>")?;
        let hash =
            auth::hash_password(&password).map_err(|e| anyhow::anyhow!("hashing failed: {e}"))?;
        println!("{hash}");
        return Ok(());
    }

    let config_path = first.unwrap_or_else(|| "directory.toml".into());
    let config = config::Config::load(Path::new(&config_path))?;
    if config.admin.password_hash.is_empty() {
        eprintln!(
            "warning: [admin] password_hash is empty - admin login is impossible. \
             Generate one with `albo hash-password <password>`."
        );
    }

    let conn = db::open(Path::new(&config.server.database))?;
    let bind = config.server.bind.clone();
    let state = Arc::new(AppState {
        config,
        db: Mutex::new(conn),
        sessions: auth::Sessions::default(),
    });

    let app = Router::new()
        .route("/", get(index))
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

async fn index(State(state): State<Arc<AppState>>) -> Response {
    let conn = state.db.lock().unwrap();
    let listed = entries::list(&conn, false).unwrap_or_default();
    drop(conn);
    let d = &state.config.directory;
    let page = IndexPage {
        site_name: &d.name,
        tagline: &d.tagline,
        entities: &d.entities,
        entries: listed,
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
