//! The remote bridge (ADR-0021, spec `docs/SPECS/active/2026-08-31-remote-control.md`).
//!
//! An HTTP + WebSocket server inside the desktop app. A phone scans the QR the window
//! shows, its browser loads the *same* `dist/` frontend through this server, and a
//! small shim (`remote.js`) redirects the three Tauri touchpoints onto the socket.
//! The bridge dispatches commands through the **same functions the window invokes**
//! — the registry below names every command with its exact parameters, so the
//! compiler rejects an adapter that drifts from the command's real signature, and a
//! test compares the registry against what `app.js` and `generate_handler!` know.
//!
//! Security posture (ADR-0021): nothing listens until the user toggles it on; a fresh
//! 128-bit token gates every route; ten failed pair attempts disable the server until
//! it is toggled again; the phone is a *paired device*, not a public endpoint.

use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use tauri::{Emitter, Manager};

type CmdResult<T> = Result<T, String>;
fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

// ---------------------------------------------------------------- events

/// Events pushed to every connected device.
///
/// Emission is funnelled through [`emit_all`] (the window's webview *and* this
/// channel), so an event added for the window reaches the phone with no bridge
/// change — the generic half of the parity rule.
static EVENTS: LazyLock<tokio::sync::broadcast::Sender<String>> =
    LazyLock::new(|| tokio::sync::broadcast::channel(256).0);

/// Emit to the window's webview and, as a WS frame, to every paired device.
pub fn emit_all<T: Serialize>(app: &tauri::AppHandle, event: &str, payload: &T) {
    let _ = app.emit(event, payload);
    if let Ok(frame) = serde_json::to_string(&json!({ "ev": event, "payload": payload })) {
        let _ = EVENTS.send(frame);
    }
}

// ---------------------------------------------------------------- the command registry

/// One bridge dispatch: an app handle and the raw JSON arguments in, the command's
/// serialised result (or message) out.
pub type BridgeFut =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>>;
pub type BridgeHandler = Arc<dyn Fn(tauri::AppHandle, serde_json::Value) -> BridgeFut + Send + Sync>;

/// Commands with no browser equivalent (ADR-0021): they exist to hand files to macOS
/// and cannot be dispatched to a phone. The parity test exempts exactly this list and
/// no other.
#[cfg(test)]
pub const NATIVE_ONLY: &[&str] = &["share_photos", "start_file_drag"];

/// Normalises what a command returns into a bridge reply. The window's commands are
/// mixed: most return `Result<T, String>`, but a few — `cancel_survey` — return
/// nothing at all, and both are legal answers over the socket.
trait IntoBridgeOut {
    fn into_out(self) -> Result<serde_json::Value, String>;
}

impl IntoBridgeOut for () {
    fn into_out(self) -> Result<serde_json::Value, String> {
        Ok(serde_json::Value::Null)
    }
}

impl<T: Serialize> IntoBridgeOut for Result<T, String> {
    fn into_out(self) -> Result<serde_json::Value, String> {
        match self {
            Ok(v) => serde_json::to_value(v).map_err(err),
            Err(e) => Err(e),
        }
    }
}

/// Commands that operate the bridge itself; the device must not toggle its own door.
#[cfg(test)]
pub const WINDOW_ONLY: &[&str] = &["remote_start", "remote_stop", "remote_status"];

/// The arguments object of one request, or `None` when the caller sent none.
fn remote_args(v: &serde_json::Value) -> Result<Option<&serde_json::Map<String, serde_json::Value>>, String> {
    match v {
        serde_json::Value::Null => Ok(None),
        v => Ok(Some(v.as_object().ok_or("arguments must be an object")?)),
    }
}

/// Extract one named argument. Absent deserialises as `null`, which is what makes
/// `Option<T>` parameters work the way Tauri's own bridge does.
fn remote_arg<T: serde::de::DeserializeOwned>(
    args: Option<&serde_json::Map<String, serde_json::Value>>,
    name: &str,
) -> Result<T, String> {
    let raw = args.and_then(|m| m.get(name)).cloned().unwrap_or(serde_json::Value::Null);
    serde_json::from_value(raw).map_err(|e| format!("argument {name}: {e}"))
}

/// Every command the window can invoke, paired with its exact parameter list.
///
/// One list, read by both transports: `generate_handler!` in `lib.rs` takes the
/// command names, and the `bridge!` macro here expands each entry into an adapter
/// that deserialises *these* named parameters and calls the *real* command function.
/// Adding a parameter to a command without updating its entry is a compile error in
/// the adapter's call; adding a command without an entry at all trips the parity test.
macro_rules! bridge {
    (
        $( #app: [$($aname:ident ( $($aarg:ident : $aty:ty),* $(,)? )),* $(,)?])*
        $( #state: [$($sname:ident ( $($sarg:ident : $sty:ty),* $(,)? )),* $(,)?])*
        $( #plain: [$($pname:ident ( $($parg:ident : $pty:ty),* $(,)? )),* $(,)?])*
        $( #dev: [$($dname:ident ( $($darg:ident : $dty:ty),* $(,)? )),* $(,)?])*
        $( #sync: [$($yname:ident ()),* $(,)?])*
        $(,)?
    ) => {
        fn build_registry() -> HashMap<&'static str, BridgeHandler> {
            let mut m: HashMap<&'static str, BridgeHandler> = HashMap::new();
            let mut add = |k: &'static str, h: BridgeHandler| { m.insert(k, h); };
            $(
                $(
                    add(stringify!($aname), Arc::new(move |app: tauri::AppHandle, v: serde_json::Value| -> BridgeFut {
                        Box::pin(async move {
                            let args = remote_args(&v)?;
                            let _ = args;
                            $( let $aarg: $aty = remote_arg(args, stringify!($aarg))?; )*
                            let handle = app.clone();
                            let out = crate::$aname(handle, app.state::<crate::AppState>(), $($aarg),*).await?;
                            serde_json::to_value(out).map_err(err)
                        })
                    }));
                )*
            )*
            $(
                $(
                    add(stringify!($sname), Arc::new(move |app: tauri::AppHandle, v: serde_json::Value| -> BridgeFut {
                        Box::pin(async move {
                            let args = remote_args(&v)?;
                            $( let $sarg: $sty = remote_arg(args, stringify!($sarg))?; )*
                            let out = crate::$sname(app.state::<crate::AppState>(), $($sarg),*).await?;
                            serde_json::to_value(out).map_err(err)
                        })
                    }));
                )*
            )*
            $(
                $(
                    add(stringify!($pname), Arc::new(move |_app: tauri::AppHandle, v: serde_json::Value| -> BridgeFut {
                        Box::pin(async move {
                            let args = remote_args(&v)?;
                            let _ = args;
                            $( let $parg: $pty = remote_arg(args, stringify!($parg))?; )*
                            let out = crate::$pname($($parg),*).await?;
                            serde_json::to_value(out).map_err(err)
                        })
                    }));
                )*
            )*
            $(
                $(
                    add(stringify!($yname), Arc::new(move |app: tauri::AppHandle, v: serde_json::Value| -> BridgeFut {
                        Box::pin(async move {
                            let _ = remote_args(&v);
                            crate::$yname(app.state::<crate::AppState>()).into_out()
                        })
                    }));
                )*
            )*
            // Dev-only commands are compiled out of a release build (#[cfg(debug_assertions)]
            // in lib.rs), so their adapters must be too — hence this section exists.
            #[cfg(debug_assertions)]
            $(
                $(
                    add(stringify!($dname), Arc::new(move |_app: tauri::AppHandle, v: serde_json::Value| -> BridgeFut {
                        Box::pin(async move {
                            let args = remote_args(&v)?;
                            $( let $darg: $dty = remote_arg(args, stringify!($darg))?; )*
                            let out = crate::$dname($($darg),*).await?;
                            serde_json::to_value(out).map_err(err)
                        })
                    }));
                )*
            )*
            m
        }
    };
}

bridge! {
    #app: [
        list_sources(),
        add_source(path: String, shallow: Option<bool>),
        remove_source(path: String, purge: Option<bool>),
        promote_peek(path: String),
        open_path(path: String),
        set_source_depth(path: String, shallow: bool),
        autodetect_faces(path: String),
        build_thumbs(path: String),
        analyze_all(path: String),
        analyze_faces(path: String),
        analyze_resume(path: String, faces: bool, semantic: bool),
        locate_photos(path: String),
        set_photo_location(path: String, hashes: Vec<String>, lat: f64, lon: f64),
        set_photo_datetime(path: String, hashes: Vec<String>, datetime: String),
        semantic_index(path: String),
        clusters(path: String, distance: f32),
        models_fetch(),
        edit_photos(path: String, hashes: Vec<String>, edit: blinkview_core::edit::Edit),
        strip_metadata(path: String, hashes: Vec<String>, keep_original: Option<bool>),
        duplicate_review(path: String),
        plan_op(path: String, op: String, param: Option<f32>),
        apply_op(path: String, op: String, param: Option<f32>),
    ]
    #state: [
        survey_folder(path: String),
        peek_folder(path: String),
        peek_photos(path: String),
        end_peek(path: String),
        create_folder(path: String, parent: String, name: String),
        source_data(path: String),
        rescan(path: String),
        photos(path: String, folder: Option<String>, person: Option<String>),
        pending_work(path: String),
        photo_places(path: String),
        semantic_status(path: String),
        semantic_search(path: String, query: String, limit: Option<usize>),
        restore_dismissed(path: String),
        name_clusters(path: String, distance: f32, assignments: std::collections::BTreeMap<usize, String>),
        people_overview(path: String, distance: f32),
        dismiss_cluster(path: String, distance: f32, cluster: usize),
        merge_people(path: String, from: String, into: String),
        name_cluster(path: String, distance: f32, cluster: usize, name: String),
        cluster_photos(path: String, distance: f32, cluster: usize),
        set_rating(path: String, hashes: Vec<String>, rating: u8),
        set_label(path: String, hashes: Vec<String>, label: Option<String>),
        set_album(path: String, hashes: Vec<String>, album: String, member: bool),
        plan_album_migration(path: String),
        apply_album_migration(path: String),
        plan_move(path: String, hashes: Vec<String>, dest: String),
        apply_move(path: String, hashes: Vec<String>, dest: String),
        list_searches(path: String),
        save_search(path: String, name: String, query: String),
        folder_view(path: String, folder: String),
        set_folder_view(path: String, folder: String, sort: String, order: Vec<String>),
        list_albums(path: String),
        photo_detail(path: String, hash: String),
        edit_photo(path: String, hash: String, edit: blinkview_core::edit::Edit),
        delete_photos(path: String, hashes: Vec<String>, dest: Option<String>),
        rename_photo(path: String, hash: String, name: String),
        forget_person(path: String, person: String),
        untag_person(path: String, person: String, hashes: Vec<String>),
        restore_photos(path: String, hashes: Vec<String>),
        empty_trash(path: String),
        apply_duplicate_review(path: String, rejections: Vec<crate::DuplicateRejection>),
        plan_rename(path: String, format: String, hashes: Option<Vec<String>>),
        apply_rename(path: String, format: String, hashes: Option<Vec<String>>),
        history(path: String),
        undo(path: String, id: Option<String>),
    ]
    #plain: [
        place_search(query: String),
        models_status(),
        check_for_updates(),
        open_update(url: String),
    ]
    #dev: [
        bench_payload(n: usize),
    ]
    #sync: [
        cancel_survey(),
        take_open_paths(),
    ]
}

static REGISTRY: LazyLock<HashMap<&'static str, BridgeHandler>> = LazyLock::new(build_registry);

// ---------------------------------------------------------------- pairing secrets

/// 128 bits of fresh randomness, hex — the pairing token.
fn new_token() -> CmdResult<String> {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf).map_err(err)?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// Compare in time independent of where the first differing byte sits. Token length
/// is public (always 32 hex characters), so only content needs the treatment.
fn ct_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// `Cookie:` header value of the bridge session, if present and well-formed.
pub fn cookie_token(headers: &axum::http::HeaderMap) -> Option<String> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|kv| {
        let kv = kv.trim();
        kv.strip_prefix("bvr=").map(str::to_string)
    })
}

// ---------------------------------------------------------------- server state

/// Everything the router and the window need to know about a running bridge.
pub struct RemoteShared {
    pub token: String,
    /// The LAN URL the QR encodes.
    pub url: String,
    /// The QR itself, as an SVG string.
    pub qr: String,
    /// Set when ten wrong pair attempts have been seen; pairing stays dead until the
    /// bridge is toggled off and on again.
    pub disabled: AtomicBool,
    /// Wrong pair attempts so far.
    pub failed_pairs: AtomicU32,
    /// User agents of the live WebSocket connections.
    pub clients: Mutex<Vec<String>>,
    off: tokio::sync::watch::Sender<bool>,
    join: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

impl RemoteShared {
    /// Shut the listener down and forget the token.
    pub fn stop(&self) {
        let _ = self.off.send(true);
        if let Some(j) = self.join.lock().unwrap_or_else(|p| p.into_inner()).take() {
            j.abort();
        }
    }
}

#[derive(Serialize, Clone)]
pub struct RemoteInfo {
    pub enabled: bool,
    /// Set when pairing locked itself out; only re-toggling clears it.
    pub disabled: bool,
    pub url: String,
    /// SVG markup for the pairing QR.
    pub qr: String,
    pub clients: Vec<String>,
}

fn status_of(s: &RemoteShared) -> RemoteInfo {
    RemoteInfo {
        enabled: true,
        disabled: s.disabled.load(Ordering::Relaxed),
        url: s.url.clone(),
        qr: s.qr.clone(),
        clients: s.clients.lock().unwrap_or_else(|p| p.into_inner()).clone(),
    }
}

/// State every handler sees.
#[derive(Clone)]
pub(crate) struct BridgeState {
    app: tauri::AppHandle,
    shared: Arc<RemoteShared>,
}

// ---------------------------------------------------------------- the router

/// The primary LAN address seen from this machine, or loopback when there is none.
///
/// A UDP *connect* to an unroutable address asks the routing table where it would send
/// without sending anything; the local address of that route is the interface a phone
/// on the same network would reach. When there is no route at all the URL falls back
/// to loopback — useless to a phone, but honest about it.
fn lan_ip() -> String {
    std::net::UdpSocket::bind(("0.0.0.0", 0))
        .and_then(|s| {
            s.connect(("10.255.255.255", 1))?;
            s.local_addr()
        })
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".into())
}

/// The pairing QR as a standalone SVG.
fn qr_svg(text: &str) -> String {
    let Ok(code) = qrcodegen::QrCode::encode_text(text, qrcodegen::QrCodeEcc::Medium) else {
        return String::new();
    };
    let n = code.size();
    let quiet = 4;
    let dim = n + quiet * 2;
    let mut path = String::new();
    for y in 0..n {
        for x in 0..n {
            if code.get_module(x, y) {
                path.push_str(&format!("M{} {}h1v1h-1z", x + quiet, y + quiet));
            }
        }
    }
    // r##…## because the SVG attributes themselves contain the `"#` sequence.
    let head = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {dim} {dim}" shape-rendering="crispEdges">"##
    );
    let bg = r##"<rect width="100%" height="100%" fill="#ffffff"/>"##;
    let fg = format!(r##"<path d="{path}" fill="#000000"/>"##);
    format!("{head}{bg}{fg}</svg>")
}

const INDEX_HTML: &str = include_str!("../../dist/index.html");
const REMOTE_JS: &str = include_str!("remote.js");

macro_rules! dist_file {
    ($name:literal, $mime:literal) => {
        ($name, ($mime, include_bytes!(concat!("../../dist/", $name)).as_slice()))
    };
}

/// The embedded frontend. `tauri.conf.json` embeds the same files into the window;
/// this is the same dist, embedded a second time so the phone's browser can load it.
static DIST: LazyLock<HashMap<&'static str, (&'static str, &'static [u8])>> = LazyLock::new(|| {
    HashMap::from([
        dist_file!("app.js", "text/javascript; charset=utf-8"),
        dist_file!("app.css", "text/css; charset=utf-8"),
        dist_file!("logo.png", "image/png"),
        dist_file!("world110.json", "application/json"),
        dist_file!("world50.json", "application/json"),
    ])
});

/// The served index, with the shim loaded ahead of app.js so `window.__TAURI__`
/// exists before the frontend dereferences it.
fn index_page() -> std::borrow::Cow<'static, str> {
    const MARKER: &str = r#"<script src="app.js""#;
    const SHIM: &str = r#"<script src="remote.js"></script>"#;
    if INDEX_HTML.contains(MARKER) {
        std::borrow::Cow::Owned(INDEX_HTML.replacen(MARKER, &format!("{SHIM}{MARKER}"), 1))
    } else {
        // The marker moved: inject at the end of <head> rather than serve a shimless
        // page that cannot reach the bridge.
        std::borrow::Cow::Owned(INDEX_HTML.replacen("</head>", &format!("{SHIM}</head>"), 1))
    }
}

use axum::extract::{Request, State};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

fn text_response(status: StatusCode, body: &str) -> Response {
    (status, body.to_owned()).into_response()
}

fn forbid(msg: &str) -> Response {
    text_response(StatusCode::FORBIDDEN, msg)
}

/// Routes behind the pairing cookie. Built separately from `run` so the live server
/// and any future harness share one definition.
pub(crate) fn router(bs: BridgeState) -> Router {
    Router::new()
        .route("/p/{token}", get(pair))
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/remote.js", get(remote_js))
        .route("/ws", get(ws_upgrade))
        .route("/photo/{*rest}", get(photo))
        .fallback(static_file)
        .with_state(bs)
}

fn authed(headers: &HeaderMap, token: &str) -> bool {
    cookie_token(headers).is_some_and(|t| ct_eq(&t, token))
}

/// `GET /p/<token>` — the route the QR encodes. Correct token: plant the session
/// cookie and enter the app. Wrong token: count the attempt, and lock pairing out
/// after ten.
async fn pair(
    State(bs): State<BridgeState>,
    axum::extract::Path(token): axum::extract::Path<String>,
) -> Response {
    if bs.shared.disabled.load(Ordering::Relaxed) {
        return text_response(
            StatusCode::LOCKED,
            "Remote pairing is locked. Toggle remote access off and on in the desktop app.",
        );
    }
    if !ct_eq(&token, &bs.shared.token) {
        let n = bs.shared.failed_pairs.fetch_add(1, Ordering::Relaxed) + 1;
        if n >= 10 {
            bs.shared.disabled.store(true, Ordering::Relaxed);
        }
        return forbid("Wrong pairing token.");
    }
    let cookie = format!("bvr={}; HttpOnly; Path=/; SameSite=Lax", bs.shared.token);
    let mut res = StatusCode::SEE_OTHER.into_response();
    res.headers_mut().insert(header::SET_COOKIE, cookie.parse().expect("cookie header"));
    res.headers_mut().insert(header::LOCATION, "/".parse().expect("location header"));
    res
}

async fn index(State(bs): State<BridgeState>, headers: HeaderMap) -> Response {
    if !authed(&headers, &bs.shared.token) {
        return forbid("Not paired.");
    }
    axum::response::Html(index_page().into_owned()).into_response()
}

async fn remote_js(State(bs): State<BridgeState>, headers: HeaderMap) -> Response {
    if !authed(&headers, &bs.shared.token) {
        return forbid("Not paired.");
    }
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], REMOTE_JS).into_response()
}

async fn static_file(State(bs): State<BridgeState>, headers: HeaderMap, req: Request) -> Response {
    if !authed(&headers, &bs.shared.token) {
        return forbid("Not paired.");
    }
    let path = req.uri().path().trim_start_matches('/');
    let Some((mime, bytes)) = DIST.get(path) else {
        if path == "favicon.ico" {
            let (_, png) = DIST.get("logo.png").expect("logo.png is embedded");
            return ([(header::CONTENT_TYPE, "image/png")], *png).into_response();
        }
        return text_response(StatusCode::NOT_FOUND, "not found");
    };
    ([(header::CONTENT_TYPE, *mime)], *bytes).into_response()
}

/// `GET /photo/<path>?t=|preview=|full=<hash>` — the scheme handler over HTTP.
///
/// The request is re-expressed as the `photo://` URI `serve_photo` already speaks, so
/// the boundary (`media_scope`) and the LRU are literally the same code the window
/// uses, and `Range` flows through for video seeking.
async fn photo(State(bs): State<BridgeState>, headers: HeaderMap, req: Request) -> Response {
    if !authed(&headers, &bs.shared.token) {
        return forbid("Not paired.");
    }
    // The path stays percent-encoded exactly as the browser sent it: `serve_photo`
    // owns the decoding (and the boundary check on what it decodes to), so this
    // route is a pure change of transport.
    let rest = req.uri().path().strip_prefix("/photo").unwrap_or_default();
    let mut uri = format!("photo://localhost{rest}");
    if let Some(q) = req.uri().query() {
        uri.push('?');
        uri.push_str(q);
    }
    let mut builder = http::Request::builder().uri(uri);
    if let Some(range) = headers.get(header::RANGE) {
        builder = builder.header(header::RANGE, range.clone());
    }
    let Ok(hreq) = builder.body(Vec::new()) else {
        return text_response(StatusCode::BAD_REQUEST, "bad request");
    };
    let app = bs.app.clone();
    // Decoding a photograph is blocking work; keep it off the async workers exactly
    // as the scheme handler's pool does.
    let resp = tokio::task::spawn_blocking(move || crate::serve_photo(&app, hreq)).await;
    match resp {
        Ok(res) => {
            let (parts, body) = res.into_parts();
            Response::from_parts(parts, axum::body::Body::from(body))
        }
        Err(_) => text_response(StatusCode::INTERNAL_SERVER_ERROR, "photo task failed"),
    }
}

async fn ws_upgrade(
    State(bs): State<BridgeState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !authed(&headers, &bs.shared.token) {
        return forbid("Not paired.");
    }
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown device")
        .to_owned();
    ws.on_upgrade(move |sock| connected(bs, sock, ua))
}

/// One paired connection: dispatch requests through the registry, push events.
async fn connected(bs: BridgeState, sock: WebSocket, ua: String) {
    use futures_util::{SinkExt, StreamExt};

    let (mut sink, mut stream) = sock.split();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Message>(64);

    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if sink.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Every funnelled event lands here, serialised as a frame and already.
    // Subscribed before the connection is announced, so the device that just
    // arrived sees its own presence in the client list like everyone else.
    let mut evrx = EVENTS.subscribe();
    let ev_tx = out_tx.clone();
    let forwarder = tokio::spawn(async move {
        loop {
            match evrx.recv().await {
                Ok(frame) => {
                    if ev_tx.send(Message::text(frame)).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    // Browsing can outpace a slow phone; drop old frames rather than
                    // kill the channel. Progress bars are the only lossy events.
                    let _ = n;
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    {
        let mut clients = bs.shared.clients.lock().unwrap_or_else(|p| p.into_inner());
        clients.push(ua.clone());
    }
    emit_all(&bs.app, "remote-clients", &status_of(&bs.shared).clients);

    while let Some(Ok(msg)) = stream.next().await {
        let Message::Text(txt) = msg else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else { continue };
        let Some(id) = v.get("id").and_then(|x| x.as_u64()) else { continue };
        let Some(cmd) = v.get("cmd").and_then(|x| x.as_str()).map(str::to_owned) else {
            let _ = out_tx
                .send(Message::text(
                    json!({ "id": id, "ok": false, "err": "missing cmd" }).to_string(),
                ))
                .await;
            continue;
        };
        let Some(handler) = REGISTRY.get(cmd.as_str()).cloned() else {
            let _ = out_tx
                .send(Message::text(
                    json!({ "id": id, "ok": false, "err": format!("unknown command {cmd}") })
                        .to_string(),
                ))
                .await;
            continue;
        };
        let app = bs.app.clone();
        let tx = out_tx.clone();
        let args = v.get("args").cloned().unwrap_or(serde_json::Value::Null);
        tokio::spawn(async move {
            let out = handler(app, args).await;
            let frame = match out {
                Ok(result) => json!({ "id": id, "ok": true, "result": result }),
                Err(e) => json!({ "id": id, "ok": false, "err": e }),
            };
            let _ = tx.send(Message::text(frame.to_string())).await;
        });
    }

    forwarder.abort();
    drop(out_tx);
    let _ = writer.await;
    {
        let mut clients = bs.shared.clients.lock().unwrap_or_else(|p| p.into_inner());
        clients.retain(|c| c != &ua);
    }
    emit_all(&bs.app, "remote-clients", &status_of(&bs.shared).clients);
}

// ---------------------------------------------------------------- lifecycle

/// A port on every interface; the phone reaches it through the LAN address in the QR.
async fn listen() -> Result<(tokio::net::TcpListener, u16), String> {
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", 0)).await.map_err(err)?;
    let port = listener.local_addr().map_err(err)?.port();
    Ok((listener, port))
}

/// Turn the bridge on. A second call while it runs reports the running one.
///
/// `BLINKVIEW_REMOTE_START=1` in the environment turns it on at launch — for
/// scripted checks, and for anyone who wants the bridge from the first window.
/// The env var is read here, at the moment of starting, so it is an explicit act
/// of whoever launched the process, never a persisted setting.
#[tauri::command]
pub async fn remote_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
) -> CmdResult<RemoteInfo> {
    {
        let guard = state.remote.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(s) = guard.as_ref() {
            return Ok(status_of(s));
        }
    }
    let token = new_token()?;
    let (listener, port) = listen().await?;
    let url = format!("http://{}:{port}/p/{token}", lan_ip());
    let qr = qr_svg(&url);
    let (off, mut off_rx) = tokio::sync::watch::channel(false);
    let shared = Arc::new(RemoteShared {
        token,
        url: url.clone(),
        qr,
        disabled: AtomicBool::new(false),
        failed_pairs: AtomicU32::new(0),
        clients: Mutex::new(Vec::new()),
        off,
        join: Mutex::new(None),
    });
    let router = router(BridgeState { app: app.clone(), shared: shared.clone() });
    let join = tauri::async_runtime::spawn(async move {
        let shutdown = async move { let _ = off_rx.wait_for(|v| *v).await; };
        let _ = axum::serve(listener, router).with_graceful_shutdown(shutdown).await;
    });
    *shared.join.lock().unwrap_or_else(|p| p.into_inner()) = Some(join);
    *state.remote.lock().unwrap_or_else(|p| p.into_inner()) = Some(shared.clone());
    eprintln!("[blinkview] remote bridge listening: {url}");
    Ok(status_of(&shared))
}

/// Called from `run()`'s setup when `BLINKVIEW_REMOTE_START=1`.
pub fn autostart_if_asked(app: &tauri::AppHandle) {
    if std::env::var_os("BLINKVIEW_REMOTE_START").is_none_or(|v| v != "1") {
        return;
    }
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = handle.state::<crate::AppState>();
        if let Err(e) = remote_start(handle.clone(), state).await {
            eprintln!("[blinkview] remote bridge failed to start: {e}");
        }
    });
}

/// Turn the bridge off: stop listening, drop the token, drop the connections.
#[tauri::command]
pub async fn remote_stop(state: tauri::State<'_, crate::AppState>) -> CmdResult<()> {
    if let Some(s) = state.remote.lock().unwrap_or_else(|p| p.into_inner()).take() {
        s.stop();
    }
    Ok(())
}

/// What the toggle dialog shows.
#[tauri::command]
pub async fn remote_status(state: tauri::State<'_, crate::AppState>) -> CmdResult<RemoteInfo> {
    let guard = state.remote.lock().unwrap_or_else(|p| p.into_inner());
    Ok(match guard.as_ref() {
        Some(s) => status_of(s),
        None => RemoteInfo {
            enabled: false,
            disabled: false,
            url: String::new(),
            qr: String::new(),
            clients: Vec::new(),
        },
    })
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    /// Command names the window's frontend actually invokes, scanned from the shipped
    /// `app.js` — the contract under test, not a copy of it.
    fn frontend_commands() -> Vec<String> {
        let src = include_str!("../../dist/app.js");
        let mut out = Vec::new();
        let mut rest = src;
        while let Some(i) = rest.find("invoke(\"") {
            rest = &rest[i + 8..];
            if let Some(end) = rest.find('"') {
                out.push(rest[..end].to_owned());
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Command names registered with the window, scanned from `generate_handler![..]`
    /// in `lib.rs`. Idents arrive as bare names or `module::name`; `#[cfg(...)]`
    /// attributes inside the list are stripped, not parsed.
    fn handler_commands() -> Vec<String> {
        let src = include_str!("lib.rs");
        let marker = "generate_handler![";
        let start = src.find(marker).expect("generate_handler list exists");
        let mut list = src[start + marker.len()..].to_owned();
        // The list can carry `#[cfg(...)]` attributes, whose own `]` would otherwise
        // be mistaken for the end of the command list. Strip them first.
        while let Some(i) = list.find("#[") {
            let Some(close) = list[i..].find(']') else { break };
            list.replace_range(i..i + close + 1, "");
        }
        let end = list.find(']').expect("generate_handler list closes");
        // Entries arrive as bare idents or `module::name` paths; take the name itself.
        let mut names: Vec<String> = list[..end]
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|t| !t.is_empty())
            .filter_map(|t| t.rsplit("::").next())
            .filter(|t| t.len() > 2 && t.chars().next().is_some_and(|c| c.is_ascii_lowercase()))
            .map(str::to_owned)
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// The parity rule, enforced: everything the window invokes must be dispatchable
    /// over the bridge, except exactly the native-service list; and every bridge entry
    /// must name a real window command (a typo in the registry has nowhere to hide).
    #[test]
    fn the_bridge_reaches_every_command_the_frontend_invokes() {
        let exempt: std::collections::HashSet<&str> = NATIVE_ONLY
            .iter()
            .chain(WINDOW_ONLY.iter())
            .copied()
            .collect();
        let handlers: std::collections::HashSet<String> =
            handler_commands().into_iter().collect();

        for cmd in frontend_commands() {
            assert!(
                REGISTRY.contains_key(cmd.as_str()) || exempt.contains(cmd.as_str()),
                "app.js invokes {cmd:?}, which the bridge cannot dispatch and which is \
                 on neither the native-only nor the window-only exemption list"
            );
        }
        for name in handler_commands() {
            assert!(
                REGISTRY.contains_key(name.as_str()) || exempt.contains(name.as_str()),
                "{name:?} is registered with the window but missing from the bridge \
                 registry and both exemption lists"
            );
        }
        for name in REGISTRY.keys() {
            assert!(
                handlers.contains(*name),
                "registry entry {name:?} names no command in generate_handler — a typo \
                 would dispatch into nothing"
            );
        }
    }

    #[test]
    fn equal_tokens_match_and_any_difference_does_not() {
        let t = "0123456789abcdef0123456789abcdef";
        assert!(ct_eq(t, t));
        assert!(!ct_eq(t, "1123456789abcdef0123456789abcdef"));
        assert!(!ct_eq(t, "0123456789abcdef0123456789abcdeF"));
        assert!(!ct_eq(t, "short"));
        assert!(ct_eq("", ""));
    }

    #[test]
    fn tokens_have_128_bits_and_are_never_repeated() {
        let a = new_token().unwrap();
        let b = new_token().unwrap();
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn cookies_are_read_only_from_the_bridge_session() {
        let mut h = HeaderMap::new();
        assert_eq!(cookie_token(&h), None);
        h.insert(header::COOKIE, "other=1; bvr=abc".parse().unwrap());
        assert_eq!(cookie_token(&h).as_deref(), Some("abc"));
        h.insert(header::COOKIE, "bvrx=1".parse().unwrap());
        assert_eq!(cookie_token(&h), None);
    }

    /// The phone's page must load the shim before app.js, or `window.__TAURI__` is
    /// dereferenced by the frontend before it exists.
    #[test]
    fn the_served_index_loads_the_shim_ahead_of_app_js() {
        let page = index_page();
        let shim = page.find("remote.js").expect("shim injected");
        let app = page.find("app.js").expect("frontend present");
        assert!(shim < app, "remote.js must load before app.js");
    }

    #[test]
    fn the_qr_is_a_plausible_svg() {
        let svg = qr_svg("http://192.168.1.10:49300/p/abcd");
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("path d=\"M"));
        assert!(svg.ends_with("</svg>"));
    }

    /// Argument extraction mirrors Tauri: present values deserialise, absent ones
    /// surface as `None` for `Option` parameters, and a wrong shape is an error that
    /// names the argument.
    #[test]
    fn argument_extraction_matches_tauri_semantics() {
        let v = serde_json::json!({"path": "/Photos", "rating": 5});
        let args = remote_args(&v).unwrap();
        let path: String = remote_arg(args, "path").unwrap();
        let rating: u8 = remote_arg(args, "rating").unwrap();
        let label: Option<String> = remote_arg(args, "label").unwrap();
        assert_eq!((path.as_str(), rating, label), ("/Photos", 5, None));

        let none = remote_args(&serde_json::Value::Null).unwrap();
        assert!(none.is_none());

        let bad: Result<String, _> = remote_arg(args, "rating");
        assert!(bad.unwrap_err().contains("rating"));
    }
}
