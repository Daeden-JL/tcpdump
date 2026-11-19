mod builder;
mod config;
mod parse;

use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use config::{Config, FeedbackMode};
use notify::{RecursiveMode, Watcher};
use serde_json::json;
use std::convert::Infallible;
use std::path::PathBuf;
use std::io::Write;
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
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
    /// Start of the current minute and how many feedback reports it has taken.
    feedback_gate: Mutex<(Instant, u32)>,
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

/// The app version shown on the page: the Cargo version, plus the commit the
/// binary was built from when that was known at build time.
fn app_version() -> String {
    match env!("TCPDUMP_DEV_COMMIT") {
        "" => format!("v{}", env!("CARGO_PKG_VERSION")),
        commit => format!("v{} ({commit})", env!("CARGO_PKG_VERSION")),
    }
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
        "app_version": app_version(),
        "error": error,
        "version": cfg.version,
        "site": cfg.site,
        "targets": cfg.targets,
        "flags": cfg.flags,
        "filter": cfg.filter,
        "estimate": cfg.estimate,
        "feedback": {
            "mode": cfg.feedback.mode,
            "github_repo": if cfg.feedback.github_repo.is_empty() { &cfg.site.repo.url } else { &cfg.feedback.github_repo },
            "email": cfg.feedback.email,
        },
        "examples": examples,
    })))
}

async fn api_build(State(app): State<Arc<App>>, RawQuery(query): RawQuery) -> impl IntoResponse {
    let pairs = builder::parse_query(query.as_deref().unwrap_or(""));
    no_cache(Json(builder::build(&app.config(), &pairs)))
}

/// Limits on feedback saved to the server's file.
const FEEDBACK_MAX_BODY: usize = 16 * 1024;
const FEEDBACK_MAX_FIELD: usize = 8000;
const FEEDBACK_PER_MINUTE: u32 = 20;
const FEEDBACK_MAX_FILE: u64 = 5 * 1024 * 1024;

/// Seconds since 1970 as `2026-11-19T22:15:03Z`.
fn utc_timestamp(secs: u64) -> String {
    let (days, rest) = (secs / 86400, secs % 86400);
    // Days to a civil date (proleptic Gregorian), after Howard Hinnant.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (day, month) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}

/// One line of the feedback file. Control characters are dropped and long
/// text is cut, so a report cannot forge or bloat the file.
fn feedback_line(expected: &str, report: &str, secs: u64) -> String {
    let clean = |s: &str| -> String { s.chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\t').take(FEEDBACK_MAX_FIELD).collect() };
    json!({ "time": utc_timestamp(secs), "version": app_version(), "expected": clean(expected), "report": clean(report) }).to_string()
}

/// Save a parse feedback report to the server's private file.
async fn api_feedback(State(app): State<Arc<App>>, headers: HeaderMap, body: String) -> (StatusCode, Json<serde_json::Value>) {
    let reply = |code: StatusCode, message: &str| (code, Json(json!({ "ok": code == StatusCode::OK, "message": message })));
    let cfg = app.config();
    if cfg.feedback.mode != FeedbackMode::File {
        return reply(StatusCode::NOT_FOUND, "Reports are not saved on this server.");
    }
    // Requiring JSON keeps other websites from posting here through a visitor's browser.
    let is_json = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with("application/json"));
    if !is_json {
        return reply(StatusCode::UNSUPPORTED_MEDIA_TYPE, "The report must be sent as JSON.");
    }
    if body.len() > FEEDBACK_MAX_BODY {
        return reply(StatusCode::PAYLOAD_TOO_LARGE, "The report is too long. Shorten it and try again.");
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) else {
        return reply(StatusCode::BAD_REQUEST, "The report could not be read.");
    };
    let field = |name: &str| value.get(name).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let (expected, report) = (field("expected"), field("report"));
    if expected.is_empty() && report.is_empty() {
        return reply(StatusCode::BAD_REQUEST, "The report is empty.");
    }
    {
        let mut gate = app.feedback_gate.lock().unwrap();
        if gate.0.elapsed() >= Duration::from_secs(60) {
            *gate = (Instant::now(), 0);
        }
        if gate.1 >= FEEDBACK_PER_MINUTE {
            return reply(StatusCode::TOO_MANY_REQUESTS, "Too many reports just now. Please try again in a minute.");
        }
        gate.1 += 1;
    }
    let dir = app.path.parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let path = dir.join(&cfg.feedback.file);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() >= FEEDBACK_MAX_FILE) {
        eprintln!("feedback file {} is full; report not saved", path.display());
        return reply(StatusCode::INSUFFICIENT_STORAGE, "The report could not be saved: the feedback file is full.");
    }
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let saved = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| writeln!(f, "{}", feedback_line(&expected, &report, secs)));
    match saved {
        Ok(()) => reply(StatusCode::OK, "Thank you. Your report was sent."),
        Err(e) => {
            eprintln!("cannot write feedback to {}: {e}", path.display());
            reply(StatusCode::INTERNAL_SERVER_ERROR, "The report could not be saved on the server.")
        }
    }
}

/// Repair invalid settings. Nothing is applied here: the page shows the
/// list of changes and only adopts the result once the user confirms.
async fn api_fix(State(app): State<Arc<App>>, RawQuery(query): RawQuery) -> impl IntoResponse {
    let pairs = builder::parse_query(query.as_deref().unwrap_or(""));
    let fixed = builder::fix(&app.config(), &pairs);
    no_cache(Json(json!({ "changes": fixed.changes, "output": fixed.output })))
}

/// Read an edited command back into builder state. `t` is the system the
/// page currently has selected, used only to break ties.
async fn api_parse(State(app): State<Arc<App>>, RawQuery(query): RawQuery, body: String) -> impl IntoResponse {
    let cfg = app.config();
    let given = builder::parse_query(query.as_deref().unwrap_or(""));
    let hint = given.iter().find(|(k, _)| k == "t").map(|(_, v)| v.clone()).unwrap_or_default();
    no_cache(Json(match parse::parse(&cfg, &body, &hint) {
        Some(mut parsed) => {
            // Estimate inputs and fill-in values are not part of a command, so carry them over.
            parsed.pairs.extend(given.into_iter().filter(|(k, _)| ["en", "ei", "ls", "fs"].contains(&k.as_str()) || k.starts_with("ph.")));
            let output = builder::build(&cfg, &parsed.pairs);
            // The file name is always generated; say so when that changed what was typed.
            let braced = |s: &str| s.replace("${", "$").replace('}', "");
            if let Some(path) = parsed.out_path.as_ref().filter(|p| !output.command.is_empty() && !braced(&output.command).contains(&braced(p))) {
                let wrote_note = parsed.notes.iter().any(|n| n.contains("folder"));
                if !wrote_note {
                    parsed.notes.push(format!("The output file is always named by the builder, so {path} was replaced with its standard name."));
                }
            }
            json!({ "found": true, "notes": parsed.notes, "output": output })
        }
        None => json!({ "found": false }),
    }))
}

async fn api_theme(State(app): State<Arc<App>>) -> impl IntoResponse {
    (
        [(CONTENT_TYPE, "text/css; charset=utf-8"), (CACHE_CONTROL, FRESH), (X_CONTENT_TYPE_OPTIONS, "nosniff")],
        app.config().theme.css(),
    )
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
        feedback_gate: Mutex::new((Instant::now(), 0)),
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
        .route("/theme.css", get(api_theme))
        .route("/api/build", get(api_build))
        .route("/api/parse", post(api_parse))
        .route("/api/feedback", post(api_feedback))
        .route("/api/fix", get(api_fix))
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
    println!("tcpdump-dev {} serving {} on http://{listen}", app_version(), app.path.display());
    tokio::select! {
        r = axum::serve(listener, router) => { if let Err(e) = r { eprintln!("server error: {e}"); } }
        _ = shutdown_signal() => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_version_is_the_cargo_version_with_an_optional_commit() {
        let v = app_version();
        assert!(v.starts_with(concat!("v", env!("CARGO_PKG_VERSION"))), "{v}");
        let commit = v.split_once(" (").map(|(_, c)| c.trim_end_matches(')'));
        assert!(commit.is_none_or(|c| c.len() == 7 && c.chars().all(|ch| ch.is_ascii_hexdigit())), "{v}");
    }

    #[test]
    fn feedback_lines_are_single_line_json_with_a_readable_time() {
        assert_eq!(utc_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_timestamp(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc_timestamp(1_795_126_503), "2026-11-19T22:15:03Z");
        let line = feedback_line("keep -n\nplease", "tcpdump -ni x\u{0}\u{1b}[31m\n{\"time\":\"forged\"}", 0);
        assert_eq!(line.lines().count(), 1, "{line}");
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["time"], "1970-01-01T00:00:00Z");
        assert_eq!(v["expected"], "keep -n\nplease");
        assert_eq!(v["report"], "tcpdump -ni x[31m\n{\"time\":\"forged\"}");
        let long = feedback_line(&"x".repeat(50_000), "", 0);
        assert!(long.len() < FEEDBACK_MAX_FIELD + 200);
    }

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
