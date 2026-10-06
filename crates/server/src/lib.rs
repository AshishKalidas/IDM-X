//! The HTTP face of the engine: a JSON API, a live event stream, and the embedded web UI.
//!
//! The desktop app, the CLI, the browser extension and the web dashboard all use this one API.
//! Every `/api` route needs the access token. Because a website can make a browser send
//! requests to `localhost`, the token is required even on loopback, and cross-origin requests
//! are only honoured from browser extensions and the app's own pages.

pub mod browser;

use anyhow::{Context, Result};
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as Sse, KeepAlive};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use idmx_core::error::Error as CoreError;
use idmx_core::types::*;
use idmx_core::{Engine, JobPatch};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::oneshot;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use tower_http::cors::{AllowOrigin, CorsLayer};

#[derive(RustEmbed)]
#[folder = "../../ui/dist"]
struct Assets;

#[derive(Clone)]
struct App {
    engine: Engine,
    token: Arc<String>,
}

#[derive(Clone, Debug)]
pub struct ServeConfig {
    pub bind: IpAddr,
    /// 0 picks a free port.
    pub port: u16,
    pub token: String,
}

impl Default for ServeConfig {
    fn default() -> Self {
        Self { bind: IpAddr::V4(Ipv4Addr::LOCALHOST), port: 0, token: String::new() }
    }
}

pub struct ServerHandle {
    pub addr: SocketAddr,
    pub token: String,
    runtime_file: Option<PathBuf>,
    stop: Option<oneshot::Sender<()>>,
}

impl ServerHandle {
    /// The address to put in a browser, with the token in the fragment (never sent to a server).
    pub fn dashboard_url(&self) -> String {
        let host = if self.addr.ip().is_unspecified() { "localhost".to_string() } else { self.addr.ip().to_string() };
        format!("http://{}:{}/#token={}", host, self.addr.port(), self.token)
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        if let Some(p) = &self.runtime_file {
            let _ = std::fs::remove_file(p);
        }
        if let Some(s) = self.stop.take() {
            let _ = s.send(());
        }
    }
}

/// How other processes find a running instance.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RuntimeInfo {
    pub port: u16,
    pub token: String,
    pub pid: u32,
}

pub fn runtime_info(data_dir: &std::path::Path) -> Option<RuntimeInfo> {
    let info: RuntimeInfo = serde_json::from_slice(&std::fs::read(data_dir.join("runtime.json")).ok()?).ok()?;
    // A stale file from a crash is worthless: make sure something is really listening.
    std::net::TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, info.port)), std::time::Duration::from_millis(300)).ok()?;
    Some(info)
}

pub async fn serve(engine: Engine, cfg: ServeConfig) -> Result<ServerHandle> {
    let token = if cfg.token.is_empty() { engine.settings().remote.token } else { cfg.token };
    let app = App { engine: engine.clone(), token: Arc::new(token.clone()) };

    let api = Router::new()
        .route("/state", get(state))
        .route("/events", get(events))
        .route("/jobs", post(add_jobs))
        .route("/jobs/{id}", get(get_job).patch(patch_job).delete(remove_job))
        .route("/jobs/{id}/pause", post(job_pause))
        .route("/jobs/{id}/resume", post(job_resume))
        .route("/jobs/{id}/cancel", post(job_cancel))
        .route("/jobs/{id}/restart", post(job_restart))
        .route("/jobs/{id}/top", post(job_top))
        .route("/jobs/{id}/map", get(job_map))
        .route("/jobs/{id}/streams", get(job_streams))
        .route("/jobs/{id}/networks/{net}", post(job_network))
        .route("/jobs/{id}/reveal", post(job_reveal))
        .route("/pause-all", post(pause_all))
        .route("/resume-all", post(resume_all))
        .route("/clear-finished", post(clear_finished))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/settings/token", post(new_token))
        .route("/interfaces/{id}/rule", put(put_rule).delete(delete_rule))
        .route("/ping", post(ping))
        .route("/tools", get(get_tools))
        .route("/tools/refresh", post(refresh_tools))
        .route("/tools/install/{which}", post(install_tool))
        .route("/media/probe", post(media_probe))
        .route("/torrent/inspect", post(torrent_inspect))
        .route("/jobs/{id}/torrent-files", post(torrent_files))
        .route("/extension/hello", post(extension_hello))
        .route("/torrent/stage", post(torrent_stage))
        .route("/pending-add", get(pending_add))
        .route("/cache", get(cache_info))
        .route("/cache/clear", post(cache_clear))
        .route("/browser", get(browser_info))
        .route("/browser/setup", post(browser_setup))
        .route("/browser/open/{id}", post(browser_open))
        .route("/browser/reveal", post(browser_reveal))
        .route("/networks/refresh", post(refresh_networks))
        .route("/open-network-settings", post(open_network_settings))
        .route("/update", get(get_update))
        .route("/update/check", post(check_update))
        .route("/update/dismiss", post(dismiss_update))
        .route("/expand", post(expand))
        .route("/probe", post(probe))
        .route("/speedtest", post(speedtest))
        .route("/endurance", get(endurance_status))
        .route("/endurance/start", post(endurance_start))
        .route("/endurance/stop", post(endurance_stop))
        .route("/endurance/logs", get(endurance_logs))
        .route("/endurance/logs/{name}", axum::routing::delete(endurance_delete))
        .route("/endurance/logs/{name}/reveal", post(endurance_reveal))
        .route_layer(middleware::from_fn_with_state(app.clone(), auth));

    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|o: &HeaderValue, _| origin_allowed(o)))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE, Method::OPTIONS])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, "x-idmx-token".parse().unwrap()]);

    let router = Router::new()
        .route("/api/health", get(|| async { Json(json!({ "ok": true, "app": "idmx", "version": env!("CARGO_PKG_VERSION") })) }))
        .nest("/api", api)
        .fallback(static_files)
        .layer(cors)
        .with_state(app);

    let listener = tokio::net::TcpListener::bind(SocketAddr::new(cfg.bind, cfg.port))
        .await
        .with_context(|| format!("couldn't listen on {}:{}", cfg.bind, cfg.port))?;
    let addr = listener.local_addr()?;
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
            .with_graceful_shutdown(async {
                let _ = rx.await;
            })
            .await;
    });

    let runtime_file = engine.data_dir().join("runtime.json");
    let info = RuntimeInfo { port: addr.port(), token: token.clone(), pid: std::process::id() };
    let _ = std::fs::write(&runtime_file, serde_json::to_vec(&info)?);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&runtime_file, std::fs::Permissions::from_mode(0o600));
    }
    Ok(ServerHandle { addr, token, runtime_file: Some(runtime_file), stop: Some(tx) })
}

fn origin_allowed(o: &HeaderValue) -> bool {
    let Ok(o) = o.to_str() else { return false };
    o.starts_with("chrome-extension://")
        || o.starts_with("moz-extension://")
        || o.starts_with("safari-web-extension://")
        || o == "tauri://localhost"
        || o == "http://tauri.localhost"
        || o == "https://tauri.localhost"
        || o.starts_with("http://localhost:")
        || o.starts_with("http://127.0.0.1:")
        || o.starts_with("http://[::1]:")
}

// ───────────────────────────── auth ─────────────────────────────

async fn auth(State(app): State<App>, req: Request, next: Next) -> Response {
    if req.method() == Method::OPTIONS {
        return next.run(req).await;
    }
    let from_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| req.headers().get("x-idmx-token").and_then(|v| v.to_str().ok()))
        .map(str::to_string);
    let from_query = || {
        req.uri().query().and_then(|q| {
            q.split('&').find_map(|kv| kv.strip_prefix("token=").map(str::to_string))
        })
    };
    let given = from_header.or_else(from_query).unwrap_or_default();
    let ok = constant_time_eq::constant_time_eq(given.as_bytes(), app.token.as_bytes());
    if !ok {
        return err(StatusCode::UNAUTHORIZED, "missing or wrong access token");
    }
    next.run(req).await
}

// ───────────────────────────── helpers ─────────────────────────────

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({ "error": msg.into() }))).into_response()
}

struct ApiError(CoreError);

impl From<CoreError> for ApiError {
    fn from(e: CoreError) -> Self {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let code = match &self.0 {
            CoreError::NotFound => StatusCode::NOT_FOUND,
            CoreError::BadUrl(_) | CoreError::Msg(_) | CoreError::Json(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        err(code, self.0.to_string())
    }
}

type Res<T> = std::result::Result<T, ApiError>;

fn is_local(addr: &SocketAddr) -> bool {
    addr.ip().is_loopback()
}

fn forbidden_remote() -> ApiError {
    ApiError(CoreError::msg("this can only be changed from this computer"))
}

// ───────────────────────────── handlers ─────────────────────────────

async fn state(State(app): State<App>, ConnectInfo(peer): ConnectInfo<SocketAddr>) -> Json<Value> {
    Json(snapshot(&app.engine, is_local(&peer)))
}

fn snapshot(engine: &Engine, local: bool) -> Value {
    let mut settings = serde_json::to_value(engine.settings()).unwrap_or_default();
    // Remote viewers already hold the token; there is no reason to echo it back around.
    if !local {
        if let Some(r) = settings.get_mut("remote").and_then(|r| r.as_object_mut()) {
            r.remove("token");
        }
    }
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "local": local,
        "jobs": engine.list(),
        "interfaces": engine.interfaces(),
        "settings": settings,
        "stats": engine.stats(),
        "update": engine.update_info(),
        "extension": engine.extension_info(),
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
    })
}

async fn events(State(app): State<App>, ConnectInfo(peer): ConnectInfo<SocketAddr>) -> impl IntoResponse {
    let local = is_local(&peer);
    let engine = app.engine.clone();
    let first = futures_util::stream::once(async move {
        Ok::<_, std::convert::Infallible>(Sse::default().event("snapshot").data(snapshot(&engine, local).to_string()))
    });
    let rest = BroadcastStream::new(app.engine.subscribe()).map(move |m| {
        Ok(match m {
            Ok(Event::Settings(s)) if !local => {
                let mut s = *s;
                s.remote.token.clear();
                Sse::default().data(serde_json::to_string(&Event::Settings(Box::new(s))).unwrap_or_default())
            }
            Ok(ev) => Sse::default().data(serde_json::to_string(&ev).unwrap_or_default()),
            // The client fell behind: tell it to reload the snapshot instead of guessing.
            Err(BroadcastStreamRecvError::Lagged(_)) => Sse::default().event("resync").data("{}"),
        })
    });
    axum::response::Sse::new(futures_util::StreamExt::chain(first, rest)).keep_alive(KeepAlive::default())
}

async fn add_jobs(State(app): State<App>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Json(mut req): Json<AddRequest>) -> Res<Json<Vec<Job>>> {
    if !is_local(&peer) {
        // A remote client may queue downloads, but only into the configured download folder.
        let base = std::path::PathBuf::from(&app.engine.settings().download_dir);
        if let Some(d) = &req.dest_dir {
            let p = std::path::PathBuf::from(d);
            if !p.starts_with(&base) || p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
                req.dest_dir = None;
            }
        }
    }
    Ok(Json(app.engine.add(req)?))
}

async fn get_job(State(app): State<App>, Path(id): Path<String>) -> Res<Json<Job>> {
    app.engine.get(&id).map(Json).ok_or(ApiError(CoreError::NotFound))
}

async fn patch_job(State(app): State<App>, Path(id): Path<String>, Json(p): Json<JobPatch>) -> Res<Json<Job>> {
    Ok(Json(app.engine.update_job(&id, p)?))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RemoveQuery {
    #[serde(default)]
    delete_file: bool,
}

async fn remove_job(State(app): State<App>, Path(id): Path<String>, Query(q): Query<RemoveQuery>) -> Res<Json<Value>> {
    app.engine.remove(&id, q.delete_file)?;
    Ok(Json(json!({ "ok": true })))
}

macro_rules! simple {
    ($name:ident, $method:ident) => {
        async fn $name(State(app): State<App>, Path(id): Path<String>) -> Res<Json<Value>> {
            app.engine.$method(&id)?;
            Ok(Json(json!({ "ok": true })))
        }
    };
}
simple!(job_pause, pause);
simple!(job_resume, resume);
simple!(job_cancel, cancel);
simple!(job_restart, restart);
simple!(job_top, move_to_top);

async fn job_map(State(app): State<App>, Path(id): Path<String>) -> Res<Json<Option<BlockMap>>> {
    if app.engine.get(&id).is_none() {
        return Err(ApiError(CoreError::NotFound));
    }
    Ok(Json(app.engine.block_map(&id)))
}

async fn job_streams(State(app): State<App>, Path(id): Path<String>) -> Res<Json<Vec<StreamStat>>> {
    Ok(Json(app.engine.streams(&id)?))
}

#[derive(Deserialize)]
struct NetSwitch {
    enabled: bool,
}

async fn job_network(State(app): State<App>, Path((id, net)): Path<(String, String)>, Json(b): Json<NetSwitch>) -> Res<Json<Value>> {
    app.engine.set_job_network(&id, &net, b.enabled)?;
    Ok(Json(json!({ "ok": true })))
}

async fn ping(State(app): State<App>) -> Json<Vec<InterfaceInfo>> {
    Json(app.engine.ping_now().await)
}

async fn refresh_networks(State(app): State<App>) -> Res<Json<Vec<InterfaceInfo>>> {
    Ok(Json(app.engine.refresh_networks().await?))
}

async fn open_network_settings(ConnectInfo(peer): ConnectInfo<SocketAddr>) -> Res<Json<Value>> {
    if !is_local(&peer) {
        return Err(forbidden_remote());
    }
    idmx_core::os::open_network_settings();
    Ok(Json(json!({ "ok": true })))
}

async fn get_update(State(app): State<App>) -> Json<Option<UpdateInfo>> {
    Json(app.engine.update_info())
}

async fn check_update(State(app): State<App>) -> Res<Json<Option<UpdateInfo>>> {
    Ok(Json(app.engine.check_update().await?))
}

async fn dismiss_update(State(app): State<App>) -> Json<Value> {
    app.engine.dismiss_update();
    Json(json!({ "ok": true }))
}

async fn get_tools(State(app): State<App>) -> Json<ToolsStatus> {
    Json(app.engine.tools_status(false).await)
}

async fn refresh_tools(State(app): State<App>) -> Json<ToolsStatus> {
    Json(app.engine.tools_status(true).await)
}

async fn install_tool(State(app): State<App>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Path(which): Path<String>) -> Res<Json<Value>> {
    // Downloads and runs a program: only from the machine itself.
    if !is_local(&peer) {
        return Err(forbidden_remote());
    }
    app.engine.install_tool(&which)?;
    Ok(Json(json!({ "started": true })))
}

#[derive(Deserialize)]
struct UrlReq {
    url: String,
    #[serde(default)]
    options: JobOptions,
}

async fn media_probe(State(app): State<App>, Json(r): Json<UrlReq>) -> Res<Json<MediaProbe>> {
    Ok(Json(app.engine.media_probe(&r.url, &r.options).await?))
}

async fn torrent_inspect(State(app): State<App>, Json(r): Json<UrlReq>) -> Res<Json<TorrentMeta>> {
    Ok(Json(app.engine.torrent_inspect(&r.url).await?))
}

#[derive(Deserialize)]
struct FilesReq {
    files: Vec<u32>,
}

async fn torrent_files(State(app): State<App>, Path(id): Path<String>, Json(r): Json<FilesReq>) -> Res<Json<Value>> {
    app.engine.set_torrent_files(&id, r.files).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct HelloReq {
    #[serde(default)]
    version: String,
    #[serde(default)]
    browser: String,
}

async fn extension_hello(State(app): State<App>, Json(r): Json<HelloReq>) -> Json<Value> {
    app.engine.extension_hello(r.version, r.browser);
    let s = app.engine.settings();
    Json(json!({ "ok": true, "app": "idmx", "mediaEnabled": s.media.enabled, "torrentEnabled": s.torrent.enabled }))
}

async fn job_reveal(State(app): State<App>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Path(id): Path<String>) -> Res<Json<Value>> {
    if !is_local(&peer) {
        return Err(forbidden_remote());
    }
    let job = app.engine.get(&id).ok_or(CoreError::NotFound)?;
    let path = job.path.ok_or_else(|| CoreError::msg("this download isn't finished yet"))?;
    idmx_core::os::reveal(std::path::Path::new(&path));
    Ok(Json(json!({ "ok": true })))
}

async fn pause_all(State(app): State<App>) -> Json<Value> {
    app.engine.pause_all();
    Json(json!({ "ok": true }))
}
async fn resume_all(State(app): State<App>) -> Json<Value> {
    app.engine.resume_all();
    Json(json!({ "ok": true }))
}
async fn clear_finished(State(app): State<App>) -> Json<Value> {
    app.engine.clear_finished();
    Json(json!({ "ok": true }))
}

async fn get_settings(State(app): State<App>, ConnectInfo(peer): ConnectInfo<SocketAddr>) -> Json<Value> {
    Json(snapshot(&app.engine, is_local(&peer))["settings"].clone())
}

/// Settings that can run code or widen access are for the person at the keyboard only.
const LOCAL_ONLY: &[&str] = &["postRules", "remote", "onQueueEmpty", "downloadDir", "proxy", "media", "torrent"];

async fn put_settings(State(app): State<App>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Json(patch): Json<Value>) -> Res<Json<Settings>> {
    if !is_local(&peer) {
        if let Some(o) = patch.as_object() {
            if o.keys().any(|k| LOCAL_ONLY.contains(&k.as_str())) {
                return Err(forbidden_remote());
            }
        }
    }
    Ok(Json(app.engine.update_settings(patch)?))
}

async fn new_token(State(app): State<App>, ConnectInfo(peer): ConnectInfo<SocketAddr>) -> Res<Json<Value>> {
    if !is_local(&peer) {
        return Err(forbidden_remote());
    }
    let s = app.engine.regenerate_token()?;
    // The old token stops working at the next restart of the listener; say so.
    Ok(Json(json!({ "token": s.remote.token, "note": "restart IDMX to apply the new token" })))
}

async fn put_rule(State(app): State<App>, Path(id): Path<String>, Json(rule): Json<InterfaceRule>) -> Res<Json<Settings>> {
    Ok(Json(app.engine.set_interface_rule(&id, Some(rule))))
}

async fn delete_rule(State(app): State<App>, Path(id): Path<String>) -> Res<Json<Settings>> {
    Ok(Json(app.engine.set_interface_rule(&id, None)))
}

#[derive(Deserialize)]
struct ExpandReq {
    text: String,
}

/// What pasted text will turn into: patterns and lists expanded, nothing queued.
async fn expand(Json(r): Json<ExpandReq>) -> Json<Value> {
    let items = idmx_core::batch::expand_items(&r.text);
    let total = items.len();
    let sample: Vec<Value> = items.into_iter().take(200).map(|i| json!({ "url": i.url, "name": i.name, "checksum": i.checksum })).collect();
    Json(json!({ "total": total, "items": sample }))
}

#[derive(Deserialize)]
struct ProbeReq {
    url: String,
    #[serde(default)]
    options: JobOptions,
}

async fn probe(State(app): State<App>, Json(r): Json<ProbeReq>) -> Res<Json<ProbeInfo>> {
    Ok(Json(app.engine.probe(&r.url, &r.options).await?))
}

#[derive(Deserialize, Default)]
struct SpeedReq {
    url: Option<String>,
}

async fn speedtest(State(app): State<App>, body: Option<Json<SpeedReq>>) -> Res<Json<Value>> {
    app.engine.speed_test(body.and_then(|b| b.0.url))?;
    Ok(Json(json!({ "started": true })))
}

#[derive(Deserialize, Default)]
struct EnduranceReq {
    url: Option<String>,
    #[serde(default)]
    nets: Vec<String>,
}

async fn endurance_start(State(app): State<App>, body: Option<Json<EnduranceReq>>) -> Res<Json<Value>> {
    let r = body.map(|b| b.0).unwrap_or_default();
    app.engine.endurance_start(r.url, r.nets)?;
    Ok(Json(json!({ "started": true })))
}

async fn endurance_stop(State(app): State<App>) -> Json<Value> {
    app.engine.endurance_stop();
    Json(json!({ "stopping": true }))
}

async fn endurance_status(State(app): State<App>) -> Json<Value> {
    Json(json!({ "status": app.engine.endurance_status() }))
}

async fn endurance_logs(State(app): State<App>) -> Json<Value> {
    Json(json!({ "logs": app.engine.endurance_logs() }))
}

async fn endurance_delete(State(app): State<App>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Path(name): Path<String>) -> Res<Json<Value>> {
    if !is_local(&peer) {
        return Err(forbidden_remote());
    }
    app.engine.endurance_delete_log(&name)?;
    Ok(Json(json!({ "ok": true })))
}

async fn endurance_reveal(State(app): State<App>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Path(name): Path<String>) -> Res<Json<Value>> {
    if !is_local(&peer) {
        return Err(forbidden_remote());
    }
    idmx_core::os::reveal(&app.engine.endurance_log_path(&name)?);
    Ok(Json(json!({ "ok": true })))
}

// ───────────────────────────── the web UI ─────────────────────────────

async fn static_files(uri: Uri, headers: HeaderMap) -> Response {
    let path = uri.path().trim_start_matches('/');
    let (file, is_index) = match Assets::get(if path.is_empty() { "index.html" } else { path }) {
        Some(f) => (f, path.is_empty() || path == "index.html"),
        // Single-page app: unknown paths get the shell, but asset-looking ones are real 404s.
        None if path.contains('.') => return StatusCode::NOT_FOUND.into_response(),
        None => match Assets::get("index.html") {
            Some(f) => (f, true),
            None => return (StatusCode::NOT_FOUND, "UI not built — run `npm run build` in ui/").into_response(),
        },
    };
    let name = if is_index { "index.html" } else { path };
    let mime = mime_guess::from_path(name).first_or_octet_stream();
    let etag = format!("\"{}\"", hex::encode(file.metadata.sha256_hash()));
    if headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) == Some(etag.as_str()) {
        return StatusCode::NOT_MODIFIED.into_response();
    }
    let cache = if path.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
    ([(header::CONTENT_TYPE, mime.to_string()), (header::CACHE_CONTROL, cache.to_string()), (header::ETAG, etag)], file.data.into_owned()).into_response()
}

fn browser_state(app: &App) -> Value {
    let dir = app.engine.data_dir().join("extension");
    json!({
        "browsers": browser::list(),
        "extensionDir": dir.exists().then(|| dir.display().to_string()),
        "extensionId": browser::EXTENSION_ID,
        "extension": app.engine.extension_info(),
    })
}

async fn browser_info(State(app): State<App>) -> Json<Value> {
    Json(browser_state(&app))
}

/// Unpack the extension and register the pairing host with every installed browser.
async fn browser_setup(State(app): State<App>, ConnectInfo(peer): ConnectInfo<SocketAddr>) -> Res<Json<Value>> {
    if !is_local(&peer) {
        return Err(forbidden_remote());
    }
    let data = app.engine.data_dir();
    browser::extract_extension(&data).map_err(|e| CoreError::msg(e.to_string()))?;
    let exe = std::env::current_exe().map_err(|e| CoreError::msg(e.to_string()))?;
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
    let registered = browser::register_host(&home, &data, &exe).map_err(|e| CoreError::msg(e.to_string()))?;
    let mut v = browser_state(&app);
    v["registered"] = json!(registered);
    Ok(Json(v))
}

async fn browser_open(State(_app): State<App>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Path(id): Path<String>) -> Res<Json<Value>> {
    if !is_local(&peer) {
        return Err(forbidden_remote());
    }
    browser::open_extensions_page(&id).map_err(|e| CoreError::msg(e.to_string()))?;
    Ok(Json(json!({ "ok": true })))
}

async fn browser_reveal(State(app): State<App>, ConnectInfo(peer): ConnectInfo<SocketAddr>) -> Res<Json<Value>> {
    if !is_local(&peer) {
        return Err(forbidden_remote());
    }
    let dir = app.engine.data_dir().join("extension");
    if !dir.exists() {
        browser::extract_extension(&app.engine.data_dir()).map_err(|e| CoreError::msg(e.to_string()))?;
    }
    // Select the folder itself (not open it) so it can be dragged straight onto the extensions page.
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(&dir).status();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(dir.parent().unwrap_or(&dir)).status();
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(format!("/select,{}", dir.display())).status();
    Ok(Json(json!({ "path": dir.display().to_string() })))
}

// ---- cache: .torrent files dropped into the app, plus the torrent engine's scratch space ----

/// Where IDMX keeps its leftovers: a hidden `.idmx-cache` inside the folder downloads are saved to
/// (so they stay with the downloads, on the same disk), plus the torrent engine's scratch space.
fn cache_dirs(app: &App) -> Vec<PathBuf> {
    vec![PathBuf::from(app.engine.settings().download_dir).join(".idmx-cache"), app.engine.data_dir().join("cache"), std::env::temp_dir().join("idmx-torrent-default")]
}

fn dir_size(p: &std::path::Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    rd.flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

async fn cache_info(State(app): State<App>) -> Json<Value> {
    let bytes: u64 = cache_dirs(&app).iter().map(|d| dir_size(d)).sum();
    Json(json!({ "bytes": bytes }))
}

/// Remove cached files, keeping any `.torrent` that a download in the list still points at.
async fn cache_clear(State(app): State<App>, ConnectInfo(peer): ConnectInfo<SocketAddr>) -> Res<Json<Value>> {
    if !is_local(&peer) {
        return Err(forbidden_remote());
    }
    let in_use: Vec<String> = app.engine.list().into_iter().filter(|j| !j.status.is_finished() || j.status == JobStatus::Seeding).map(|j| j.url).collect();
    let before: u64 = cache_dirs(&app).iter().map(|d| dir_size(d)).sum();
    for d in cache_dirs(&app) {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if in_use.iter().any(|u| u.contains(&p.display().to_string())) {
                continue;
            }
            let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        }
    }
    let after: u64 = cache_dirs(&app).iter().map(|d| dir_size(d)).sum();
    Ok(Json(json!({ "freed": before.saturating_sub(after), "bytes": after })))
}

/// Receive the bytes of a dropped `.torrent` and keep them where the engine can read them.
async fn torrent_stage(State(app): State<App>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Query(q): Query<std::collections::HashMap<String, String>>, body: axum::body::Bytes) -> Res<Json<Value>> {
    if !is_local(&peer) {
        return Err(forbidden_remote());
    }
    if body.is_empty() || body.len() > 16 * 1024 * 1024 || body.first() != Some(&b'd') {
        return Err(CoreError::msg("That doesn't look like a .torrent file.").into());
    }
    // Keep it in the folder the user saves to, unless that can't be written.
    let wanted = q.get("dir").filter(|d| !d.trim().is_empty()).cloned().unwrap_or_else(|| app.engine.settings().download_dir);
    let mut dir = PathBuf::from(&wanted).join(".idmx-cache");
    if !dir.is_absolute() || std::fs::create_dir_all(&dir).is_err() {
        dir = app.engine.data_dir().join("cache").join("torrents");
    }
    std::fs::create_dir_all(&dir).map_err(|e| CoreError::msg(e.to_string()))?;
    let name = format!("{}.torrent", hex::encode(&rand::random::<[u8; 6]>()));
    let path = dir.join(name);
    std::fs::write(&path, &body).map_err(|e| CoreError::msg(e.to_string()))?;
    Ok(Json(json!({ "path": path.display().to_string() })))
}

/// Things the operating system asked IDMX to open, for the window's Add sheet.
async fn pending_add(State(app): State<App>) -> Json<Value> {
    Json(json!({ "items": app.engine.take_pending_add() }))
}
