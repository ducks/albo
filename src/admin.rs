//! Admin routes: login/logout, list, add-by-handle, edit, delete. All
//! server-rendered forms; the bar is "the tattooer never needs it explained
//! twice".

use askama::Template;
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
}

#[derive(Template)]
#[template(path = "edit.html")]
struct EditPage<'a> {
    site_name: &'a str,
    tagline: &'a str,
    entry: Entry,
    available_tags: String,
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

fn is_authed(state: &AppState, headers: &HeaderMap) -> bool {
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
    let conn = state.db.lock().unwrap();
    let entry = entries::get(&conn, id).ok().flatten();
    drop(conn);
    let Some(entry) = entry else {
        return (StatusCode::NOT_FOUND, "no such entry").into_response();
    };
    let d = &state.config.directory;
    render(EditPage {
        site_name: &d.name,
        tagline: &d.tagline,
        entry,
        available_tags: state.config.tags.available.join(", "),
    })
}

#[derive(Deserialize)]
pub struct EditForm {
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    shop: String,
    #[serde(default)]
    bio: String,
    #[serde(default)]
    tags: String,
    #[serde(default)]
    featured_posts: String,
    #[serde(default)]
    booking_url: String,
    /// Checkboxes are absent when unchecked; presence means true.
    #[serde(default)]
    active: Option<String>,
}

pub async fn edit_submit(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(form): Form<EditForm>,
) -> Response {
    require_admin!(state, headers);
    let conn = state.db.lock().unwrap();
    let res = entries::update(
        &conn,
        id,
        form.display_name.trim(),
        form.shop.trim(),
        form.bio.trim(),
        form.tags.trim(),
        form.featured_posts.trim(),
        form.booking_url.trim(),
        form.active.is_some(),
    );
    drop(conn);
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
