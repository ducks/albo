//! albo - a curated directory engine. One binary, one directory instance,
//! configured by directory.toml. Customer #1: a Portland tattooer directory.

mod config;
mod db;

use anyhow::{Context, Result};
use axum::{extract::State, response::Html, routing::get, Router};
use std::path::Path;
use std::sync::Arc;

struct AppState {
    config: config::Config,
}

#[tokio::main]
async fn main() -> Result<()> {
    let config_path = std::env::args().nth(1).unwrap_or_else(|| "directory.toml".into());
    let config = config::Config::load(Path::new(&config_path))?;

    // Open (and initialize) the database up front so a broken path fails at
    // startup, not on first request.
    let _conn = db::open(Path::new(&config.server.database))?;

    let bind = config.server.bind.clone();
    let state = Arc::new(AppState { config });

    let app = Router::new()
        .route("/", get(index))
        .route("/health", get(|| async { "ok" }))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("could not bind {bind}"))?;
    println!("albo serving on http://{bind}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index(State(state): State<Arc<AppState>>) -> Html<String> {
    let d = &state.config.directory;
    Html(format!(
        "<!doctype html><html><head><title>{name}</title></head>\
         <body><h1>{name}</h1><p>{tagline}</p>\
         <p>A directory of {entities}. Coming soon.</p></body></html>",
        name = d.name,
        tagline = d.tagline,
        entities = d.entities,
    ))
}
