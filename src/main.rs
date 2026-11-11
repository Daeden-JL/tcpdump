mod builder;
mod config;

use axum::extract::{RawQuery, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use config::Config;
use notify::{RecursiveMode, Watcher};
use serde_json::json;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, RwLock};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::WatchStream;
use tokio_stream::{Stream, StreamExt};

struct Live {
    config: Arc<Config>,
    /// Why the file on disk is not the config being served, if it is not.
    error: Option<String>,
}

struct App {
    path: PathBuf,
    live: RwLock<Live>,
    /// Bumped on every reload attempt; pages listen for it over SSE.
    generation: watch::Sender<u64>,
}

impl App {
    fn reload(&self) {
        let result = Config::load(&self.path);
        {
            let mut live = self.live.write().unwrap();
            match result {
                Ok(cfg) => {
                    println!("config reloaded from {}", self.path.display());
                    live.config = Arc::new(cfg);
                    live.error = None;
                }
                Err(e) => {
                    eprintln!("config rejected, keeping the previous version: {e}");
                    live.error = Some(e);
                }
            }
        }
        self.generation.send_modify(|g| *g += 1);
    }

    fn config(&self) -> Arc<Config> {
        self.live.read().unwrap().config.clone()
    }
}

/// Watch the config's directory (editors replace files rather than write in
/// place) and reload shortly after the file changes.
fn watch_config(app: Arc<App>) -> notify::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<()>();
    let name = app.path.file_name().map(|n| n.to_os_string());
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            let ours = ev.paths.iter().any(|p| p.file_name() == name.as_deref());
            if ours && !ev.kind.is_access() {
                let _ = tx.send(());
            }
        }
    })?;
    let dir = app.path.parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    watcher.watch(&dir, RecursiveMode::NonRecursive)?;
    tokio::spawn(async move {
        let _watcher = watcher; // keep alive for the life of the task
        while rx.recv().await.is_some() {
            // Let a burst of write events settle into one reload.
            tokio::time::sleep(Duration::from_millis(150)).await;
            while rx.try_recv().is_ok() {}
            app.reload();
        }
    });
    Ok(())
}

async fn api_config(State(app): State<Arc<App>>) -> impl IntoResponse {
    let (cfg, error) = {
        let live = app.live.read().unwrap();
        (live.config.clone(), live.error.clone())
    };
    let examples: Vec<_> = cfg
        .examples
        .iter()
        .map(|ex| {
            let out = builder::build(&cfg, &builder::parse_query(&ex.query));
            json!({ "title": ex.title, "description": ex.description, "query": ex.query, "command": out.command })
        })
        .collect();
    no_cache(Json(json!({
        "generation": *app.generation.borrow(),
        "error": error,
        "version": cfg.version,
        "site": cfg.site,
        "targets": cfg.targets,
        "flags": cfg.flags,
        "filter": cfg.filter,
        "examples": examples,
    })))
}

async fn api_build(State(app): State<Arc<App>>, RawQuery(query): RawQuery) -> impl IntoResponse {
    let pairs = builder::parse_query(query.as_deref().unwrap_or(""));
    no_cache(Json(builder::build(&app.config(), &pairs)))
}

async fn api_events(State(app): State<Arc<App>>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = WatchStream::new(app.generation.subscribe())
        .map(|g| Ok(Event::default().event("config").data(g.to_string())));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn no_cache(body: impl IntoResponse) -> impl IntoResponse {
    ([(CACHE_CONTROL, "no-store")], body)
}

const INDEX_HTML: &str = include_str!("../assets/index.html");
const APP_JS: &str = include_str!("../assets/app.js");
const STYLE_CSS: &str = include_str!("../assets/style.css");
const ICON_SVG: &str = include_str!("../assets/icon.svg");

/// Fingerprint of the embedded assets (FNV-1a). It is part of their URL, so
/// a new build is never paired with a stylesheet or script that a browser or
/// CDN cached from an older one.
fn asset_version() -> &'static str {
    static VERSION: LazyLock<String> = LazyLock::new(|| {
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in [APP_JS, STYLE_CSS, ICON_SVG].iter().flat_map(|s| s.bytes()) {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
        format!("{hash:016x}")
    });
    &VERSION
}

/// The page, with `{{v}}` in asset URLs replaced by the fingerprint.
fn index_html() -> &'static str {
    static PAGE: LazyLock<String> = LazyLock::new(|| INDEX_HTML.replace("{{v}}", asset_version()));
    &PAGE
}

fn asset(content_type: &'static str, cache: &'static str, body: &'static str) -> impl IntoResponse {
    (
        [
            (CONTENT_TYPE, content_type),
            (CACHE_CONTROL, cache),
            (X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (CONTENT_SECURITY_POLICY, "default-src 'self'; frame-ancestors 'none'"),
        ],
        body,
    )
}

/// The page itself must always be fetched fresh; fingerprinted assets never change.
const FRESH: &str = "no-store";
const FOREVER: &str = "public, max-age=31536000, immutable";

async fn shutdown_signal() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

#[tokio::main]
async fn main() {
    let path = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("TCPDUMP_DEV_CONFIG").ok())
        .unwrap_or_else(|| "tcpdump.toml".into());
    let path = match std::fs::canonicalize(&path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("cannot open config file {path}: {e}");
            std::process::exit(1);
        }
    };
    let cfg = match Config::load(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("invalid config {}: {e}", path.display());
            std::process::exit(1);
        }
    };
    let listen = std::env::var("LISTEN")
        .ok()
        .or_else(|| Some(cfg.site.listen.clone()).filter(|l| !l.is_empty()))
        .unwrap_or_else(|| "127.0.0.1:8080".into());

    let app = Arc::new(App {
        path,
        live: RwLock::new(Live { config: Arc::new(cfg), error: None }),
        generation: watch::channel(0).0,
    });
    if let Err(e) = watch_config(app.clone()) {
        eprintln!("warning: cannot watch the config file, live edits are off: {e}");
    }

    let router = Router::new()
        .route("/", get(|| async { asset("text/html; charset=utf-8", FRESH, index_html()) }))
        .route("/static/{v}/app.js", get(|| async { asset("text/javascript; charset=utf-8", FOREVER, APP_JS) }))
        .route("/static/{v}/style.css", get(|| async { asset("text/css; charset=utf-8", FOREVER, STYLE_CSS) }))
        .route("/static/{v}/icon.svg", get(|| async { asset("image/svg+xml", FOREVER, ICON_SVG) }))
        // Un-fingerprinted addresses used by earlier builds; kept so an old cached page still works.
        .route("/app.js", get(|| async { asset("text/javascript; charset=utf-8", FRESH, APP_JS) }))
        .route("/style.css", get(|| async { asset("text/css; charset=utf-8", FRESH, STYLE_CSS) }))
        .route("/icon.svg", get(|| async { asset("image/svg+xml", FRESH, ICON_SVG) }))
        .route("/favicon.ico", get(|| async { asset("image/svg+xml", FRESH, ICON_SVG) }))
        .route("/api/config", get(api_config))
        .route("/api/build", get(api_build))
        .route("/api/events", get(api_events))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(app.clone());

    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot listen on {listen}: {e}");
            std::process::exit(1);
        }
    };
    println!("serving {} on http://{listen}", app.path.display());
    tokio::select! {
        r = axum::serve(listener, router) => { if let Err(e) = r { eprintln!("server error: {e}"); } }
        _ = shutdown_signal() => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_references_fingerprinted_assets_only() {
        let page = index_html();
        let v = asset_version();
        assert_eq!(v.len(), 16);
        assert!(!page.contains("{{v}}"));
        for file in ["style.css", "app.js", "icon.svg"] {
            assert!(page.contains(&format!("static/{v}/{file}")), "{file} is not fingerprinted");
        }
    }
}
