//! The web UI: the SvelteKit build from `web/build`, embedded into the
//! binary in release builds (debug builds read it from disk, so a rebuilt UI
//! shows up without recompiling). Every path that is not a file gets the
//! app shell, so client-side routes like `/services/web01/disk` work.

use crate::api::ApiError;
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{Html, IntoResponse, Response};

#[derive(rust_embed::Embed)]
#[folder = "../../web/build"]
#[allow_missing = true]
struct Assets;

/// The app shell written by adapter-static (`fallback` in vite.config.ts).
const SHELL: &str = "200.html";

const NOT_BUILT: &str = r#"<!doctype html>
<meta charset="utf-8"><title>Watcher</title>
<body style="font-family: system-ui, sans-serif; max-width: 40rem; margin: 4rem auto; padding: 0 1rem">
<h1>The web UI is not built into this binary</h1>
<p>The API is available under <a href="/api/v1/health">/api/v1</a>. To add the UI, build it and then Watcher:</p>
<pre>cd web &amp;&amp; npm ci &amp;&amp; npm run build
cargo build --release</pre>
"#;

pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path == "api" || path.starts_with("api/") {
        return ApiError::NotFound("no such endpoint".into()).into_response();
    }
    if let Some(file) = (!path.is_empty()).then(|| Assets::get(path)).flatten() {
        // Hashed build output never changes under the same name.
        let cache = if path.starts_with("_app/immutable/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        return file_response(file, cache);
    }
    match Assets::get(SHELL) {
        Some(shell) => file_response(shell, "no-cache"),
        None => (StatusCode::OK, Html(NOT_BUILT)).into_response(),
    }
}

fn file_response(file: rust_embed::EmbeddedFile, cache: &'static str) -> Response {
    let mime = file.metadata.mimetype().to_string();
    let mut response = file.data.into_owned().into_response();
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(&mime) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}
