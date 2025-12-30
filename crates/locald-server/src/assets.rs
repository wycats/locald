use axum::{http::Uri, response::IntoResponse};
use hyper::StatusCode;

#[cfg(feature = "ui")]
use include_dir::{Dir, include_dir};

#[cfg(feature = "ui")]
static ASSETS: Dir<'_> = include_dir!("$OUT_DIR/assets");

pub fn handle_docs(uri: &Uri) -> impl IntoResponse {
    if !cfg!(feature = "ui") {
        let html = r#"<!doctype html>
<html lang=\"en\">
    <head>
        <meta charset=\"utf-8\" />
        <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />
        <title>locald docs</title>
        <style>
            body { font-family: system-ui, -apple-system, Segoe UI, Roboto, sans-serif; margin: 3rem; line-height: 1.5; }
            code { background: #f3f4f6; padding: 0.1rem 0.25rem; border-radius: 4px; }
        </style>
    </head>
    <body>
        <h1>locald docs</h1>
        <p>This build was compiled without embedded UI assets.</p>
        <p>Build with the default features (or enable <code>ui</code>) to embed the docs site.</p>
    </body>
</html>"#;

        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(axum::http::header::CONTENT_TYPE, "text/html")],
            html,
        )
            .into_response();
    }

    let path = uri.path().trim_start_matches('/');
    // If path is empty or ends with /, append index.html
    let effective_path = if path.is_empty() || path.ends_with('/') {
        format!("{}{}", path, "index.html")
    } else {
        path.to_string()
    };

    let doc_path = format!("docs/{effective_path}");

    // If docs assets were not built/embedded, still serve a helpful landing page
    // for the docs root so the proxy behaves sensibly in CI and fresh checkouts.
    #[cfg(feature = "ui")]
    if path.is_empty() && ASSETS.get_file("docs/index.html").is_none() {
        let html = r#"<!doctype html>
<html lang=\"en\">
    <head>
        <meta charset=\"utf-8\" />
        <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />
        <title>locald docs</title>
        <style>
            body { font-family: system-ui, -apple-system, Segoe UI, Roboto, sans-serif; margin: 3rem; line-height: 1.5; }
            code { background: #f3f4f6; padding: 0.1rem 0.25rem; border-radius: 4px; }
        </style>
    </head>
    <body>
        <h1>locald docs</h1>
        <p>The documentation site is not embedded in this build.</p>
        <p>To build and embed it for distribution, run <code>pnpm build</code> in <code>locald-docs</code>, then rebuild <code>locald-server</code> (or use <code>scripts/build-assets.sh</code>).</p>
    </body>
</html>"#;

        return ([(axum::http::header::CONTENT_TYPE, "text/html")], html).into_response();
    }

    // Try exact match
    #[cfg(feature = "ui")]
    if let Some(file) = ASSETS.get_file(&doc_path) {
        let mime = mime_guess::from_path(&effective_path).first_or_octet_stream();
        return (
            [(axum::http::header::CONTENT_TYPE, mime.essence_str())],
            file.contents(),
        )
            .into_response();
    }

    // Fallback: try appending /index.html if it didn't end in slash but might be a directory
    // e.g. /guides/getting-started -> /guides/getting-started/index.html
    let index_path = format!("docs/{path}/index.html");
    #[cfg(feature = "ui")]
    if let Some(file) = ASSETS.get_file(&index_path) {
        let mime = mime_guess::from_path(&index_path).first_or_octet_stream();
        return (
            [(axum::http::header::CONTENT_TYPE, mime.essence_str())],
            file.contents(),
        )
            .into_response();
    }

    // Fallback for 404 page
    #[cfg(feature = "ui")]
    if let Some(file) = ASSETS.get_file("docs/404.html") {
        let mime = mime_guess::from_path("docs/404.html").first_or_octet_stream();
        return (
            StatusCode::NOT_FOUND,
            [(axum::http::header::CONTENT_TYPE, mime.essence_str())],
            file.contents(),
        )
            .into_response();
    }

    (StatusCode::NOT_FOUND, "404 Not Found").into_response()
}

pub fn handle_dashboard(uri: &Uri) -> impl IntoResponse {
    if !cfg!(feature = "ui") {
        let html = r#"<!doctype html>
<html lang=\"en\">
    <head>
        <meta charset=\"utf-8\" />
        <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />
        <title>locald dashboard</title>
        <style>
            body { font-family: system-ui, -apple-system, Segoe UI, Roboto, sans-serif; margin: 3rem; line-height: 1.5; }
            code { background: #f3f4f6; padding: 0.1rem 0.25rem; border-radius: 4px; }
        </style>
    </head>
    <body>
        <h1>locald dashboard</h1>
        <p>This build was compiled without embedded UI assets.</p>
        <p>Build with the default features (or enable <code>ui</code>) to embed the dashboard.</p>
    </body>
</html>"#;

        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(axum::http::header::CONTENT_TYPE, "text/html")],
            html,
        )
            .into_response();
    }

    let path = uri.path().trim_start_matches('/');
    // If path is empty or ends with /, append index.html
    let effective_path = if path.is_empty() || path.ends_with('/') {
        format!("{}{}", path, "index.html")
    } else {
        path.to_string()
    };

    // Try exact match in root assets
    #[cfg(feature = "ui")]
    if let Some(file) = ASSETS.get_file(&effective_path) {
        let mime = mime_guess::from_path(&effective_path).first_or_octet_stream();
        return (
            [(axum::http::header::CONTENT_TYPE, mime.essence_str())],
            file.contents(),
        )
            .into_response();
    }

    // Fallback: try appending /index.html
    let index_path = format!("{}/index.html", path.trim_end_matches('/'));
    #[cfg(feature = "ui")]
    if let Some(file) = ASSETS.get_file(&index_path) {
        let mime = mime_guess::from_path(&index_path).first_or_octet_stream();
        return (
            [(axum::http::header::CONTENT_TYPE, mime.essence_str())],
            file.contents(),
        )
            .into_response();
    }

    (StatusCode::NOT_FOUND, "404 Not Found").into_response()
}
