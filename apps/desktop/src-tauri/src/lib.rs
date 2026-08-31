//! blinkview desktop — a shell over `blinkview-core`.
//!
//! The CLI and this app are peers over one engine: every command here calls the same
//! functions `blinkview` does, so the two can never disagree about what a library
//! contains or what an operation will do.
//!
//! A *source* is a folder the user has added. Each one is an independent library with
//! its own disposable `.blinkview/`, which is what lets sources be added and removed
//! freely without any global database. The list of sources is the only app-level state,
//! and losing it costs nothing but re-adding folders.

use blinkview_core::{
    analyze, dedupe,
    faces::{assign, fetch as model_fetch, file as faces_file, people::People, pipeline, review},
    journal::Journal,
    plan::folder_of,
    rename, scan, scenery, semantic, thumbs,
    userdata::{FolderView, PhotoMeta, UserData, UserDataSet},
    Library,
};
mod remote;
mod watch;

use chrono::{Datelike, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::Manager;

#[derive(Clone, Serialize)]
struct ProgressEvent<'a> {
    op: &'a str,
    done: usize,
    total: usize,
    /// Which library this is about. Without it the window cannot tell whether the
    /// bar it is drawing belongs to the folder being looked at or another one
    /// working in the background, and two operations at once fight over one banner.
    source: &'a str,
}

/// A progress sink that forwards to the webview as a `progress` event.
fn emitter<'a>(
    app: &'a tauri::AppHandle,
    op: &'a str,
    source: &'a str,
) -> impl Fn(usize, usize) + Sync + 'a {
    move |done, total| {
        remote::emit_all(
            app,
            "progress",
            &ProgressEvent {
                op,
                done,
                total,
                source,
            },
        );
    }
}

type R<T> = Result<T, String>;
fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Threads that decode and resize images for the `photo://` scheme.
///
/// Bounded deliberately. Every in-flight request holds a decoded full-size frame —
/// 36 MB for a 12 MP photograph — so an unbounded pool answering a fast scroll would
/// have hundreds of those alive at once.
static IMAGE_POOL: std::sync::LazyLock<rayon::ThreadPool> = std::sync::LazyLock::new(|| {
    let n = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    rayon::ThreadPoolBuilder::new()
        .num_threads(n.clamp(2, 6))
        .thread_name(|i| format!("blinkview-img-{i}"))
        .build()
        .expect("image pool")
});

/// Threads that render video poster frames on demand.
///
/// Two, deliberately: an ffmpeg extracting one frame holds up to ~140 MB of demux and
/// decode buffers for a 1080p stream (measured on the shipped binary), and a fast
/// scroll over a fresh import used to fire dozens of these at once. Separate from
/// [`IMAGE_POOL`] so a burst of poster renders can never occupy — and starve — the
/// threads that decode photographs.
static VIDEO_POOL: std::sync::LazyLock<rayon::ThreadPool> = std::sync::LazyLock::new(|| {
    rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .thread_name(|i| format!("blinkview-video-{i}"))
        .build()
        .expect("video pool")
});

/// The version users and releases see, from `tauri.conf.json`. The workspace Cargo
/// version is an internal `0.0.1`, so using it made every release look newer.
static APP_VERSION: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let config = serde_json::from_str::<serde_json::Value>(include_str!("../tauri.conf.json"))
        .unwrap_or(serde_json::Value::Null);
    config
        .get("version")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("0.0.0")
        .to_owned()
});

#[derive(Default)]
pub struct AppState {
    /// One lock *per library*, not one lock over all of them.
    ///
    /// A single mutex meant a long operation on one library — building thumbnails for
    /// a phone backup — blocked every command for every other library, so switching
    /// source while thumbnails were generating simply hung until it finished.
    libs: Mutex<HashMap<String, Arc<Mutex<Library>>>>,
    sources: Mutex<Vec<SourceEntry>>,
    /// Markerless, session-only libraries used to look at one folder without adding
    /// it. Kept separate from `libs` so no save or watcher path can persist one.
    peeks: Mutex<HashMap<String, Arc<Mutex<Library>>>>,
    /// File-open events can arrive before the webview has installed its listener.
    /// They wait here until the frontend explicitly takes them.
    pending_open: Mutex<Vec<String>>,
    /// A newer survey, or an explicit cancellation, stops the directory walk already
    /// in flight. The walk checks between entries and never opens a file.
    survey_generation: AtomicU64,
    /// One filesystem watcher per open library, so photographs dropped into a folder
    /// in Finder appear without the window being touched.
    watchers: watch::Watchers,
    /// Kept so background work — the first scan of a library, above all — can report
    /// progress without being handed a handle through every call.
    app: Mutex<Option<tauri::AppHandle>>,
    /// One guard per library root, held while it is being opened.
    ///
    /// Opening scans, and scanning writes. Two threads reaching the same unopened
    /// library would each build their own connection and scan concurrently — including
    /// the pass that deletes rows for files it did not see — which corrupted the index
    /// on a real library. The registry lock cannot do this job, because holding it
    /// across a scan blocks every other library.
    opening: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Libraries whose background work should stop. Removing a folder should not leave
    /// its analysis burning CPU for a folder nobody can see any more.
    cancelled: Mutex<std::collections::HashSet<String>>,
    /// Held open for the life of the window. Loading the text tower costs ~270 ms
    /// against ~15 ms to embed a phrase, so a fresh load per keystroke would dominate
    /// the search. Built on first use, not at startup — a library nobody searches
    /// should not pay for it.
    text_encoder: Mutex<Option<semantic::TextEncoder>>,
    /// The remote bridge (ADR-0021) while it runs: its listener, token and clients.
    /// `None` means nothing is listening — the off state, and the default.
    remote: Mutex<Option<std::sync::Arc<remote::RemoteShared>>>,
}

// ---------------------------------------------------------------- sources

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
enum SourceEntry {
    /// Written before source depth existed. It remains recursive and keeps the exact
    /// no-exclusion behaviour that version had until the user edits it.
    Legacy(String),
    Full {
        path: String,
        #[serde(default)]
        shallow: bool,
    },
}

impl SourceEntry {
    fn path(&self) -> &str {
        match self {
            Self::Legacy(path) | Self::Full { path, .. } => path,
        }
    }

    fn shallow(&self) -> bool {
        matches!(self, Self::Full { shallow: true, .. })
    }

    fn skips_default_dirs(&self) -> bool {
        matches!(self, Self::Full { .. })
    }

    fn persisted(path: String, shallow: bool) -> Self {
        Self::Full { path, shallow }
    }
}

#[derive(Serialize, Deserialize, Default)]
struct SourcesFile {
    sources: Vec<SourceEntry>,
}

/// The bundle identifier before the rename (ADR-0017). It names the directory the
/// source list lives in, so it has to survive the rename or every existing install
/// opens on an empty sidebar, looking as though it forgot every library.
const LEGACY_IDENTIFIER: &str = "dev.notdefined.openfoto";

fn sources_path(app: &tauri::AppHandle) -> std::path::PathBuf {
    use tauri::Manager;
    let dir = app
        .path()
        .app_config_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("sources.json");
    adopt_legacy_sources(&dir, &path);
    path
}

/// Copy, not move: a source list is small, and leaving the old one in place keeps the
/// previous install working if someone goes back to it.
fn adopt_legacy_sources(dir: &std::path::Path, path: &std::path::Path) {
    if path.exists() {
        return;
    }
    let Some(legacy) = dir
        .parent()
        .map(|p| p.join(LEGACY_IDENTIFIER).join("sources.json"))
    else {
        return;
    };
    if legacy.is_file() && std::fs::copy(&legacy, path).is_ok() {
        eprintln!(
            "[blinkview] carried the source list over from {}",
            legacy.display()
        );
    }
}

fn load_source_entries(app: &tauri::AppHandle) -> Vec<SourceEntry> {
    std::fs::read(sources_path(app))
        .ok()
        .and_then(|d| serde_json::from_slice::<SourcesFile>(&d).ok())
        .map(|f| f.sources)
        .unwrap_or_default()
}

fn save_sources(app: &tauri::AppHandle, list: &[SourceEntry]) {
    let _ = std::fs::write(
        sources_path(app),
        serde_json::to_vec_pretty(&SourcesFile {
            sources: list.to_vec(),
        })
        .unwrap_or_default(),
    );
}

#[derive(Serialize)]
pub struct SourceInfo {
    path: String,
    name: String,
    photos: usize,
    videos: usize,
    /// Subfolders, deepest-path-first, with their photo counts.
    folders: Vec<FolderInfo>,
    people: Vec<PersonInfo>,
    faces_analysed: usize,
    thumbs_ready: usize,
    missing: bool,
    /// True while the library has not been opened yet, so the row can appear before
    /// its first scan finishes instead of after.
    #[serde(default)]
    indexing: bool,
    /// False means recursive, preserving the behaviour sources had before this field.
    #[serde(default)]
    shallow: bool,
}

#[derive(Serialize)]
pub struct FolderInfo {
    path: String,
    name: String,
    depth: usize,
    /// Photographs in this folder *and every folder beneath it*. A parent showing only
    /// its own loose files is useless once nesting is the organisational model.
    count: usize,
    /// Photographs sitting directly in this folder, so the tree can tell a container
    /// apart from a folder that also holds photographs of its own.
    own: usize,
    /// Whether any folder nests inside this one, so the tree knows to draw a twisty
    /// without searching for children.
    has_children: bool,
}

/// Every ancestor of a folder path, nearest first, ending with the library root ("").
///
/// `Trip/Greece Day3` yields `Trip` then ``. Used for rolling counts up the tree and
/// for resolving the metadata cascade (ADR-0010).
pub fn ancestors(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = path;
    while let Some((parent, _)) = cur.rsplit_once('/') {
        out.push(parent.to_string());
        cur = parent;
    }
    if !path.is_empty() {
        out.push(String::new());
    }
    out
}

#[derive(Serialize)]
pub struct PersonInfo {
    name: String,
    references: usize,
    photos: usize,
    cover: Option<String>,
}

/// One photograph, as the window sees it.
///
/// Every field here is multiplied by the size of the library — at 200,000 photographs
/// this struct *is* the cost of switching source, since the bridge and `JSON.parse`
/// charge by the byte. So anything derivable is derived on the other side and anything
/// absent is omitted rather than sent as a default. `name`, `folder` and `ext` are all
/// inside `path` and are split out on arrival rather than sent three more times.
#[derive(Serialize, Clone)]
pub struct PhotoInfo {
    kind: String,
    #[serde(skip_serializing_if = "is_zero_u8")]
    rating: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    albums: Vec<String>,
    bytes: u64,
    hash: String,
    /// Relative to the library root. The window prepends the source it asked about,
    /// rather than being told the same prefix a hundred thousand times.
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    taken_at: Option<i64>,
    #[serde(skip_serializing_if = "is_zero_usize")]
    faces: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    people: Vec<String>,
    #[serde(skip_serializing_if = "is_zero_u32")]
    width: u32,
    #[serde(skip_serializing_if = "is_zero_u32")]
    height: u32,
}

fn is_zero_u8(n: &u8) -> bool {
    *n == 0
}
fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}
fn is_zero_usize(n: &usize) -> bool {
    *n == 0
}

/// Begin watching a library, so changes made in Finder reach the window.
///
/// The rescan runs on the watcher's own thread and emits `library-changed`; the
/// frontend decides whether to reload, since a rescan of a library nobody is looking at
/// should not disturb the view.
fn start_watching(app: &tauri::AppHandle, state: &AppState, root: &str) {
    let (app, root_owned) = (app.clone(), root.to_string());
    let res = state.watchers.watch(root, move || {
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        let changed = with(&state, &root_owned, |lib| {
            let st = scan::scan(lib, false)?;
            // Folders may have appeared or vanished, so the cascade is no longer known
            // good — a metadata file could have arrived with them.
            lib.invalidate_user_data();
            Ok(st.hashed + st.moved + st.removed)
        });
        match changed {
            Ok(0) => {}
            Ok(n) => {
                remote::emit_all(&app, "library-changed", &(root_owned.clone(), n));
            }
            Err(e) => eprintln!("[blinkview] rescan after a change failed: {e}"),
        }
    });
    if let Err(e) = res {
        // Not fatal: without a watcher the library is merely as current as its last
        // open, which is how it behaved before.
        eprintln!("[blinkview] could not watch {root}: {e}");
    }
}

fn open_lib(state: &AppState, root: &str) -> R<()> {
    // Look, then let go. The first scan of a large library takes minutes, and holding
    // the registry lock across it froze every command for every *other* library too —
    // adding a folder made the whole window unusable until it finished.
    {
        let libs = state.libs.lock().map_err(err)?;
        if libs.contains_key(root) {
            return Ok(());
        }
    }

    // Only one thread opens a given library. Two scanning the same folder at once
    // write over each other, and the delete pass at the end of each removes rows the
    // other has just inserted.
    let gate = {
        let mut opening = state.opening.lock().map_err(err)?;
        opening.entry(root.to_string()).or_default().clone()
    };
    let _held = gate.lock().map_err(err)?;

    // Another thread may have finished opening it while this one waited.
    {
        let libs = state.libs.lock().map_err(err)?;
        if libs.contains_key(root) {
            return Ok(());
        }
    }

    let (shallow, skip_default_dirs) = state
        .sources
        .lock()
        .ok()
        .and_then(|sources| {
            sources
                .iter()
                .find(|source| same_source(source.path(), root))
                .map(|source| (source.shallow(), source.skips_default_dirs()))
        })
        .unwrap_or((false, false));
    let mut lib = Library::open_configured(root, shallow, skip_default_dirs).map_err(err)?;
    // Reconcile with the filesystem the moment a library is opened, rather than
    // waiting to be asked (ADR-0011). Photographs added or reorganised in Finder are
    // picked up before anything is drawn, and the common case is cheap because `scan`
    // skips hashing whenever size and mtime already match. A failure here is not
    // fatal: an unreadable folder should still open, just stale.
    //
    // The first scan of a large library is not cheap, so it reports progress against
    // this source rather than leaving the window with a spinner and no idea.
    let handle = state.app.lock().ok().and_then(|a| a.clone());
    let scanned = match &handle {
        Some(app) => scan::scan_with_progress(&mut lib, false, &emitter(app, "scan", root)),
        None => scan::scan(&mut lib, false),
    };
    if let Err(e) = scanned {
        eprintln!("[blinkview] scan on open failed for {root}: {e}");
    }
    if let Some(app) = &handle {
        remote::emit_all(app, "source-ready", &root.to_string());
    }

    let mut libs = state.libs.lock().map_err(err)?;
    libs.entry(root.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(lib)));
    Ok(())
}

/// Run `f` against a library, without waiting for its first scan to finish.
///
/// A library being indexed for the first time is not in the registry yet, and opening
/// it would block behind the scan. But the scan commits rows as it goes and the index
/// is in WAL mode, so a second connection can read what has landed so far. That is what
/// lets the grid fill in while a folder indexes instead of showing the previous
/// source's photographs, or nothing at all.
///
/// Falls back to the blocking path when there is no index yet — the very first moments
/// of a brand new library, when there is nothing to read anyway.
fn with_readable<T>(
    state: &AppState,
    root: &str,
    f: impl FnOnce(&mut Library) -> anyhow::Result<T>,
) -> R<T> {
    if let Some(peek) = peek_handle(state, root)? {
        let mut guard = peek.lock().map_err(err)?;
        return f(&mut guard).map_err(err);
    }
    let open = {
        let libs = state.libs.lock().map_err(err)?;
        libs.get(root).cloned()
    };
    if let Some(lib) = open {
        // Free? Use it. Busy? Do not queue behind it. A library being analysed holds
        // its lock for hours, and waiting was why re-adding a folder left the window
        // with no library to show while faces were detected.
        if let Ok(mut guard) = lib.try_lock() {
            return f(&mut guard).map_err(err);
        }
        if let Ok(mut ro) = Library::open_readable(root) {
            return f(&mut ro).map_err(err);
        }
        // No index to read from, so waiting is the only option left.
        let mut guard = lib.lock().map_err(err)?;
        return f(&mut guard).map_err(err);
    }
    match Library::open_readable(root) {
        Ok(mut lib) => f(&mut lib).map_err(err),
        // No index on disk yet; take the slow path, which creates one.
        Err(_) => with(state, root, f),
    }
}

/// Run `f` against an open library.
///
/// The guard is confined to this function so it is never held across an await
/// point, which is what lets the command wrappers be `async` and therefore run
/// off the UI thread. Heavy work (thumbnails, face detection) would otherwise
/// freeze the window.
fn with<T>(
    state: &AppState,
    root: &str,
    f: impl FnOnce(&mut Library) -> anyhow::Result<T>,
) -> R<T> {
    if peek_handle(state, root)?.is_some() {
        return Err(format!(
            "{} is open as a read-only peek. Keep this folder before changing photographs.",
            std::path::Path::new(root)
                .file_name()
                .map(|name| name.to_string_lossy())
                .unwrap_or_else(|| root.into())
        ));
    }
    open_lib(state, root)?;
    // Take the registry lock only long enough to find the library, then release it, so
    // work on one library never holds up another.
    let lib = {
        let libs = state.libs.lock().map_err(err)?;
        libs.get(root).ok_or("library not open")?.clone()
    };
    let mut guard = lib.lock().map_err(err)?;
    f(&mut guard).map_err(err)
}

fn peek_handle(state: &AppState, root: &str) -> R<Option<Arc<Mutex<Library>>>> {
    let requested = std::path::Path::new(root)
        .canonicalize()
        .unwrap_or_else(|_| std::path::PathBuf::from(root));
    let peeks = state.peeks.lock().map_err(err)?;
    Ok(peeks.iter().find_map(|(path, peek)| {
        let stored = std::path::Path::new(path)
            .canonicalize()
            .unwrap_or_else(|_| std::path::PathBuf::from(path));
        (stored == requested).then(|| peek.clone())
    }))
}

/// True when `path` sits in `folder` or anywhere beneath it.
///
/// Compared segment-wise so `Trip2/x.jpg` is not treated as living in `Trip`.
pub fn in_folder(path: &str, folder: &str) -> bool {
    if folder.is_empty() {
        return true;
    }
    path.strip_prefix(folder)
        .and_then(|r| r.strip_prefix('/'))
        .is_some()
}

fn describe(lib: &mut Library) -> anyhow::Result<SourceInfo> {
    let rows = lib.index.all()?;
    let (mut photos, mut videos) = (0, 0);
    // Two tallies per folder: what sits directly in it, and what sits anywhere beneath.
    // Ancestors are walked so a folder holding only subfolders still gets a row.
    let mut own: BTreeMap<String, usize> = BTreeMap::new();
    let mut total: BTreeMap<String, usize> = BTreeMap::new();
    let mut has_children: BTreeMap<String, bool> = BTreeMap::new();
    for r in &rows {
        // A photograph in Trash is deleted: the grid hides it and the Trash row counts
        // it. Counting it in the library total as well is what made deleting look like
        // it had done nothing — the number beside the folder never moved, because the
        // photograph had merely gone from one counted place to another.
        let trashed = in_folder(&r.path, TRASH);
        if !trashed {
            if r.kind == "photo" {
                photos += 1
            } else {
                videos += 1
            }
        }
        let d = r
            .path
            .rsplit_once('/')
            .map(|(d, _)| d.to_string())
            .unwrap_or_default();
        *own.entry(d.clone()).or_default() += 1;
        *total.entry(d.clone()).or_default() += 1;
        for a in ancestors(&d) {
            // Trash rolls up into its own row, never into the library above it.
            if trashed && a.is_empty() {
                continue;
            }
            *total.entry(a.clone()).or_default() += 1;
            has_children.insert(a, true);
        }
    }
    // A folder with nothing in it yet is invisible to a tree derived from the index —
    // and a folder you just made but cannot see is indistinguishable from one that
    // failed to be made. Directory entries are cheap next to the row work below.
    let empties: BTreeSet<String> = walk_dirs(lib.root())
        .into_iter()
        .filter(|d| !total.contains_key(d))
        .collect();
    for d in &empties {
        total.insert(d.clone(), 0);
        for a in ancestors(d) {
            has_children.insert(a, true);
        }
    }
    let folders = total;
    // Progress is about photographs still in the library. Counting analysed trashed
    // ones against a total that excludes them would offer "Look for people" a
    // negative number.
    let analysed = rows
        .iter()
        .filter(|r| !in_folder(&r.path, TRASH))
        .filter(|r| lib.faces_done(&r.hash).unwrap_or(false))
        .count();
    // One directory read, not one `stat` per photograph. At 200k photos the old form
    // was 200k syscalls every time the sidebar refreshed.
    let ready = std::fs::read_dir(lib.vault().join("thumbs"))
        .map(|d| {
            d.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "jpg"))
                .count()
        })
        .unwrap_or(0)
        .min(photos);

    let people_file = lib.people()?;
    let opt = assign::Options::default();
    let mut claimed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for f in lib.all_faces()? {
        if let Some(e) = f.embedding.as_ref() {
            if let Some(n) = assign::assign(e, &people_file, &opt).person() {
                claimed
                    .entry(n.to_string())
                    .or_default()
                    .insert(f.hash.clone());
            }
        }
    }
    let people = people_file
        .people
        .iter()
        .map(|p| {
            let hashes = claimed.get(&p.name);
            PersonInfo {
                name: p.name.clone(),
                references: p.references.len(),
                photos: hashes.map(|s| s.len()).unwrap_or(0),
                cover: hashes
                    .and_then(|s| s.iter().next())
                    .map(|h| thumbs::thumb_path(lib, h).display().to_string()),
            }
        })
        .collect();

    let root = lib.root().display().to_string();
    Ok(SourceInfo {
        name: lib
            .root()
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| root.clone()),
        path: root,
        photos,
        videos,
        folders: folders
            .into_iter()
            .filter(|(path, n)| *n > 0 || empties.contains(path))
            .map(|(path, count)| FolderInfo {
                depth: if path.is_empty() {
                    0
                } else {
                    path.matches('/').count() + 1
                },
                name: if path.is_empty() {
                    "All photos".into()
                } else {
                    path.rsplit('/').next().unwrap_or(&path).to_string()
                },
                own: own.get(&path).copied().unwrap_or(0),
                has_children: has_children.get(&path).copied().unwrap_or(false),
                count,
                path,
            })
            .collect(),
        people,
        faces_analysed: analysed,
        thumbs_ready: ready,
        missing: false,
        indexing: false,
        shallow: lib.is_shallow(),
    })
}

#[tauri::command]
async fn list_sources(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> R<Vec<SourceInfo>> {
    let list = load_source_entries(&app);
    *state.sources.lock().map_err(err)? = list.clone();
    let mut out = Vec::new();
    for source in list {
        let root = source.path().to_string();
        if !std::path::Path::new(&root).is_dir() {
            out.push(SourceInfo {
                name: root.rsplit('/').next().unwrap_or(&root).to_string(),
                path: root,
                photos: 0,
                videos: 0,
                folders: vec![],
                people: vec![],
                faces_analysed: 0,
                thumbs_ready: 0,
                missing: true,
                indexing: false,
                shallow: source.shallow(),
            });
            continue;
        }
        // Readable, not blocking: listing the sidebar must not wait on a library that
        // is still being indexed, or every folder disappears until that one finishes.
        match with_readable(&state, &root, describe) {
            Ok(info) => {
                // Counts were read, so this folder is known even if the library is not
                // open for writing yet. "Indexing" means there is nothing to read —
                // not merely that we have not opened it.
                start_watching(&app, &state, &root);
                out.push(info);
            }
            Err(_) => {
                // No index yet at all — show the folder rather than dropping it.
                out.push(SourceInfo {
                    name: std::path::Path::new(&root)
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| root.clone()),
                    path: root.clone(),
                    photos: 0,
                    videos: 0,
                    folders: vec![],
                    people: vec![],
                    faces_analysed: 0,
                    thumbs_ready: 0,
                    missing: false,
                    indexing: true,
                    shallow: source.shallow(),
                });
            }
        }
    }
    Ok(out)
}

/// Why `candidate` may not become a source, or `None` when it may.
///
/// Every source is an independent library with its own `.blinkview/`, so a folder that
/// overlaps an existing source would be indexed twice, analysed twice, and removing
/// either copy could delete `blinkview.json` metadata the other still reads. Both
/// directions are refused, not warned about; the way out is one step (remove the
/// nested source first). Paths are canonicalized before comparing — symlinks and case
/// differences resolve to the same folder, and two sources reaching one vault is the
/// one outcome that could corrupt an index, since the open gate is keyed by the path
/// string. A folder that does not exist cannot be canonicalized and falls back to its
/// literal path; refusing it here is not needed because `list_sources` already shows
/// it as missing.
fn source_conflict(candidate: &std::path::Path, sources: &[String]) -> Option<String> {
    let canon = candidate
        .canonicalize()
        .unwrap_or_else(|_| candidate.to_path_buf());
    let name = |p: &std::path::Path| {
        p.file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| p.display().to_string())
    };
    for s in sources {
        let existing = std::path::Path::new(s);
        let other = existing
            .canonicalize()
            .unwrap_or_else(|_| existing.to_path_buf());
        if other == canon {
            return Some(format!("{} is already in your library.", name(&canon)));
        }
        // `starts_with` compares components, so `/Pics2` is not inside `/Pics`.
        if canon.starts_with(&other) {
            return Some(format!(
                "{} is inside your source {} — those photographs are already in the library.",
                name(&canon),
                name(&other)
            ));
        }
        if other.starts_with(&canon) {
            return Some(format!(
                "{} already contains your source {}. Remove {} from the library first if you want {} on its own.",
                name(&canon),
                name(&other),
                name(&other),
                name(&canon)
            ));
        }
    }
    None
}

#[tauri::command]
/// Register a folder and return at once.
///
/// Deliberately does not open the library: the first scan of a phone backup takes
/// minutes, and making the window wait for it before the folder even appears is what
/// made adding one feel like a hang. The row shows up immediately and fills in as
/// `list_sources` opens it in the background.
async fn add_source(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
    shallow: Option<bool>,
) -> R<SourceInfo> {
    register_source(&app, &state, path, shallow.unwrap_or(false))
}

fn register_source(
    app: &tauri::AppHandle,
    state: &AppState,
    path: String,
    shallow: bool,
) -> R<SourceInfo> {
    if !std::path::Path::new(&path).is_dir() {
        return Err(format!("not a directory: {path}"));
    }
    // A folder removed earlier is still marked cancelled; adding it back must undo
    // that, or nothing will ever analyse it again.
    if let Ok(mut c) = state.cancelled.lock() {
        c.remove(&path);
    }
    let list = load_source_entries(app);
    let paths: Vec<String> = list
        .iter()
        .map(|source| source.path().to_string())
        .collect();
    // Overlaps are refused here, at the only door a folder can enter through.
    if let Some(why) = source_conflict(std::path::Path::new(&path), &paths) {
        return Err(why);
    }
    let mut list = list;
    list.push(SourceEntry::persisted(path.clone(), shallow));
    save_sources(app, &list);
    *state.sources.lock().map_err(err)? = list;
    let name = std::path::Path::new(&path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());
    Ok(SourceInfo {
        name,
        path,
        photos: 0,
        videos: 0,
        folders: vec![],
        people: vec![],
        faces_analysed: 0,
        thumbs_ready: 0,
        missing: false,
        indexing: true,
        shallow,
    })
}

#[derive(Serialize)]
pub struct SurveyInfo {
    here: usize,
    below: Option<usize>,
    subfolders: usize,
    excluded: Vec<String>,
}

#[tauri::command]
async fn survey_folder(state: tauri::State<'_, AppState>, path: String) -> R<SurveyInfo> {
    let ticket = state.survey_generation.fetch_add(1, Ordering::Relaxed) + 1;
    let surveyed = scan::survey_folder_cancellable(&path, &|| {
        state.survey_generation.load(Ordering::Relaxed) != ticket
    })
    .map_err(err)?;
    Ok(SurveyInfo {
        here: surveyed.here,
        below: surveyed.below,
        subfolders: surveyed.subfolders,
        excluded: surveyed.excluded,
    })
}

#[tauri::command]
fn cancel_survey(state: tauri::State<'_, AppState>) {
    state.survey_generation.fetch_add(1, Ordering::Relaxed);
}

#[derive(Clone, Serialize)]
pub struct PeekInfo {
    path: String,
    name: String,
    photos: usize,
    videos: usize,
    subfolders: usize,
}

fn direct_subfolder_count(root: &std::path::Path) -> usize {
    std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .count()
        })
        .unwrap_or(0)
}

fn describe_peek(lib: &Library) -> anyhow::Result<PeekInfo> {
    let rows = lib.index.all()?;
    let photos = rows.iter().filter(|row| row.kind == "photo").count();
    let videos = rows.iter().filter(|row| row.kind == "video").count();
    let path = lib.root().display().to_string();
    Ok(PeekInfo {
        name: lib
            .root()
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone()),
        subfolders: direct_subfolder_count(lib.root()),
        path,
        photos,
        videos,
    })
}

fn begin_peek(state: &AppState, path: &str) -> R<PeekInfo> {
    let canonical = std::path::Path::new(path).canonicalize().map_err(err)?;
    if !canonical.is_dir() {
        return Err(format!("not a directory: {}", canonical.display()));
    }
    let key = canonical.display().to_string();
    if let Some(existing) = state.peeks.lock().map_err(err)?.get(&key).cloned() {
        let lib = existing.lock().map_err(err)?;
        return describe_peek(&lib).map_err(err);
    }

    let mut lib = Library::peek(&canonical).map_err(err)?;
    scan::scan_shallow(&mut lib, false).map_err(err)?;
    let info = describe_peek(&lib).map_err(err)?;
    state
        .peeks
        .lock()
        .map_err(err)?
        .insert(key, Arc::new(Mutex::new(lib)));
    Ok(info)
}

#[tauri::command]
async fn peek_folder(state: tauri::State<'_, AppState>, path: String) -> R<PeekInfo> {
    begin_peek(&state, &path)
}

#[tauri::command]
async fn peek_photos(state: tauri::State<'_, AppState>, path: String) -> R<Vec<PhotoInfo>> {
    let peek = peek_handle(&state, &path)?.ok_or_else(|| format!("{path} is not being peeked"))?;
    let lib = peek.lock().map_err(err)?;
    let mut out: Vec<PhotoInfo> = lib
        .index
        .all()
        .map_err(err)?
        .into_iter()
        .map(|row| PhotoInfo {
            kind: row.kind,
            rating: 0,
            label: None,
            albums: Vec::new(),
            bytes: row.size.max(0) as u64,
            hash: row.hash,
            path: row.path,
            taken_at: row.taken_at,
            faces: 0,
            people: Vec::new(),
            width: 0,
            height: 0,
        })
        .collect();
    out.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));
    Ok(out)
}

fn end_peek_for(state: &AppState, path: &str) -> R<()> {
    let canonical = std::path::Path::new(path)
        .canonicalize()
        .unwrap_or_else(|_| std::path::PathBuf::from(path));
    let key = canonical.display().to_string();
    let peek = state
        .peeks
        .lock()
        .map_err(err)?
        .remove(&key)
        .ok_or_else(|| format!("{key} is not being peeked"))?;
    match Arc::try_unwrap(peek) {
        Ok(lock) => lock.into_inner().map_err(err)?.end_peek().map_err(err),
        Err(peek) => {
            state.peeks.lock().map_err(err)?.insert(key, peek);
            Err("the peek is still serving an image; try closing it again".into())
        }
    }
}

#[tauri::command]
async fn end_peek(state: tauri::State<'_, AppState>, path: String) -> R<()> {
    end_peek_for(&state, &path)
}

#[tauri::command]
async fn promote_peek(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> R<SourceInfo> {
    let canonical = std::path::Path::new(&path).canonicalize().map_err(err)?;
    let root = canonical.display().to_string();
    end_peek_for(&state, &root)?;
    register_source(&app, &state, root.clone(), false)?;
    open_lib(&state, &root)?;
    start_watching(&app, &state, &root);
    with_readable(&state, &root, describe)
}

#[derive(Serialize)]
pub struct OpenTarget {
    mode: String,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    peek: Option<PeekInfo>,
}

/// Which added source already covers `canonical`, if any.
///
/// Returns the source's *stored* path — the string the registry, the open gate and
/// the sidebar all key on — with the opened file's path relative to the source root,
/// or the folder when a directory inside the source was opened. Canonicalising only
/// for the comparison is what keeps a symlinked source a single library: returning
/// the resolved path would open the same vault again under a second key.
fn owning_source(
    canonical: &std::path::Path,
    entries: &[SourceEntry],
) -> Option<(String, Option<String>, Option<String>)> {
    for source in entries {
        let root = std::path::Path::new(source.path())
            .canonicalize()
            .unwrap_or_else(|_| std::path::PathBuf::from(source.path()));
        if !canonical.starts_with(&root) {
            continue;
        }
        let rel = canonical
            .strip_prefix(&root)
            .ok()?
            .to_string_lossy()
            .replace('\\', "/");
        if canonical.is_file() {
            return Some((source.path().to_string(), Some(rel), None));
        }
        // A directory: the source root itself, or a folder inside it.
        return Some((
            source.path().to_string(),
            None,
            (!rel.is_empty()).then_some(rel),
        ));
    }
    None
}

#[tauri::command]
async fn open_path(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> R<OpenTarget> {
    let canonical = std::path::Path::new(&path).canonicalize().map_err(err)?;
    if canonical.is_file() && scan::kind_of(&canonical).is_none() {
        return Err(format!("Blinkview cannot view {}", canonical.display()));
    }
    if let Some((stored, file, folder)) = owning_source(&canonical, &load_source_entries(&app)) {
        return Ok(OpenTarget {
            mode: "source".into(),
            path: stored,
            file,
            folder,
            peek: None,
        });
    }
    if canonical.is_file() {
        let root = canonical
            .parent()
            .ok_or("the file has no containing folder")?;
        let info = begin_peek(&state, &root.display().to_string())?;
        return Ok(OpenTarget {
            mode: "peek".into(),
            path: info.path.clone(),
            file: canonical
                .file_name()
                .map(|name| name.to_string_lossy().to_string()),
            folder: None,
            peek: Some(info),
        });
    }
    if canonical.is_dir() {
        let info = begin_peek(&state, &canonical.display().to_string())?;
        return Ok(OpenTarget {
            mode: "peek".into(),
            path: info.path.clone(),
            file: None,
            folder: None,
            peek: Some(info),
        });
    }
    Err(format!("not a file or folder: {}", canonical.display()))
}

#[tauri::command]
fn take_open_paths(state: tauri::State<'_, AppState>) -> R<Vec<String>> {
    let mut pending = state.pending_open.lock().map_err(err)?;
    Ok(std::mem::take(&mut *pending))
}

/// Change whether a source includes subfolders, then reconcile without rehashing rows
/// that remain in scope. Rows that leave a shallow source are derived index state;
/// ratings and labels beside their photographs are untouched.
#[tauri::command]
async fn set_source_depth(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
    shallow: bool,
) -> R<SourceInfo> {
    let mut entries = load_source_entries(&app);
    let Some(index) = entries
        .iter()
        .position(|source| same_source(source.path(), &path))
    else {
        return Err(format!("{path} is not an added folder"));
    };
    let stored = entries[index].path().to_string();
    entries[index] = SourceEntry::persisted(stored, shallow);
    save_sources(&app, &entries);
    *state.sources.lock().map_err(err)? = entries;

    open_lib(&state, &path)?;
    let lib = {
        let libs = state.libs.lock().map_err(err)?;
        libs.get(&path).cloned().ok_or("library not open")?
    };
    let mut lib = lib.lock().map_err(err)?;
    lib.configure_scan(shallow, true);
    let sink = emitter(&app, "scan", &path);
    scan::scan_with_progress(&mut lib, false, &sink).map_err(err)?;
    describe(&mut lib).map_err(err)
}

/// Detect faces for a source that has not been analysed yet.
///
/// Called after a folder is added: finding people is the point of the app, and making
/// the user discover a menu item first is a poor introduction. It is skipped when the
/// models are absent so adding a folder never fails for want of a download.
#[tauri::command]
async fn autodetect_faces(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> R<String> {
    if !model_fetch::specs().iter().all(model_fetch::is_present) {
        return Ok("models not installed".into());
    }
    let sink = emitter(&app, "faces", &path);
    let stop = cancel_flag(&state, &path);
    with(&state, &path, |lib| {
        let st = analyze::run_cancellable(
            lib,
            analyze::Stages {
                thumbs: true,
                faces: true,
                semantic: false,
            },
            &sink,
            &stop,
        )?;
        Ok(format!("{} faces found in {} photos", st.faces, st.decoded))
    })
}

/// What blinkview would leave behind, or take away, when a folder is removed.
///
/// Shown before asking, because "delete blinkview's data" covers two very different
/// things: a cache that costs a rescan, and ratings and names that no machine can
/// reproduce (ADR-0007).
#[derive(Serialize, Default)]
pub struct SourceData {
    /// Bytes under `.blinkview/` — thumbnails, index, journal. Rebuildable.
    cache_bytes: u64,
    /// How many `blinkview.json` files, across the folder tree.
    metadata_files: usize,
    /// Photographs carrying a rating, a label or an album. Not reproducible.
    described: usize,
    /// Named people. Not reproducible.
    people: usize,
    saved_searches: usize,
}

fn dir_bytes(p: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(p) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_bytes(&e.path()),
            Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

#[tauri::command]
async fn source_data(state: tauri::State<'_, AppState>, path: String) -> R<SourceData> {
    let root = std::path::PathBuf::from(&path);
    let mut d = SourceData {
        cache_bytes: dir_bytes(&blinkview_core::cache::vault_for(&root)),
        ..Default::default()
    };
    // The metadata is read through the library so the cascade is counted the same way
    // it is everywhere else — but readably. This runs before the "Remove?" dialog is
    // shown, and on the blocking helper it queued behind a running analysis: the button
    // did nothing for the length of the pass and then the dialog appeared all at once,
    // long after it had been clicked.
    let _ = with_readable(&state, &path, |lib| {
        let set = lib.user_data()?;
        d.saved_searches = set.searches().len();
        Ok(())
    });
    for e in walk_metadata(&root) {
        d.metadata_files += 1;
        if let Ok(bytes) = std::fs::read(&e) {
            if let Ok(u) = serde_json::from_slice::<blinkview_core::userdata::UserData>(&bytes) {
                d.described += u.photos.len();
            }
        }
    }
    d.people = People::named_in(&root);
    Ok(d)
}

/// Every folder at or below `root`, relative to it, skipping the cache and hidden
/// folders. `.blinkview` is excluded by the leading dot, like every other hidden name.
fn walk_dirs(root: &std::path::Path) -> Vec<String> {
    fn rec(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if !p.is_dir() || e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if let Ok(rel) = p.strip_prefix(root) {
                out.push(rel.to_string_lossy().to_string());
            }
            rec(root, &p, out);
        }
    }
    let mut out = Vec::new();
    rec(root, root, &mut out);
    out
}

/// Every `blinkview.json` at or below `root`, skipping the cache and hidden folders.
fn walk_metadata(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let f = root.join(blinkview_core::userdata::FILE);
    if f.is_file() {
        out.push(f);
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() && !e.file_name().to_string_lossy().starts_with('.') {
            out.extend(walk_metadata(&p));
        }
    }
    out
}

/// Whether two paths name the same folder.
///
/// The list is stored as it was added, but the app shows the resolved path, so a
/// folder added through a symlink — `/tmp`, or a volume alias — came back under a name
/// that no longer matched its own entry, and Remove quietly removed nothing.
fn same_source(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let resolve =
        |p: &str| std::fs::canonicalize(p).unwrap_or_else(|_| std::path::PathBuf::from(p));
    resolve(a) == resolve(b)
}

/// Remove a folder from blinkview. The photographs are never touched.
///
/// `purge` additionally deletes what blinkview wrote into the folder. That is offered
/// but never the default: the cache costs only a rescan, while the ratings and names
/// go for good.
#[tauri::command]
async fn remove_source(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
    purge: Option<bool>,
) -> R<String> {
    let list: Vec<SourceEntry> = load_source_entries(&app)
        .into_iter()
        .filter(|source| !same_source(source.path(), &path))
        .collect();
    save_sources(&app, &list);
    *state.sources.lock().map_err(err)? = list;
    state.libs.lock().map_err(err)?.remove(&path);
    // Stop watching, or a removed source keeps rescanning a library nothing displays.
    state.watchers.unwatch(&path);
    // Stop any analysis still running on it.
    if let Ok(mut c) = state.cancelled.lock() {
        c.insert(path.clone());
    }

    if purge != Some(true) {
        return Ok("Removed from blinkview. Nothing on disk was changed.".into());
    }

    let root = std::path::PathBuf::from(&path);
    let mut removed = 0usize;
    // The cache left with the photographs (ADR-0019); the marker is blinkview's too,
    // so it goes as well — there is nothing left for it to name.
    if std::fs::remove_dir_all(blinkview_core::cache::vault_for(&root)).is_ok() {
        blinkview_core::cache::forget(&root);
        removed += 1;
    }
    if std::fs::remove_file(root.join(blinkview_core::cache::MARKER)).is_ok() {
        removed += 1;
    }
    for f in walk_metadata(&root) {
        if std::fs::remove_file(&f).is_ok() {
            removed += 1;
        }
    }
    if std::fs::remove_file(blinkview_core::faces::people::People::path(&root)).is_ok() {
        removed += 1;
    }
    Ok(format!(
        "Removed, and deleted {removed} blinkview file(s). Your photographs are untouched."
    ))
}

/// Make a folder inside the library.
///
/// Folders are the only grouping blinkview has (ADR-0009), so making one before there
/// is anything to put in it is not a convenience — it is how you say where things are
/// going to go. Validated through `fsops`, because the reference drive is exFAT and
/// macOS will happily create a name that volume cannot carry.
#[tauri::command]
async fn create_folder(
    state: tauri::State<'_, AppState>,
    path: String,
    parent: String,
    name: String,
) -> R<String> {
    with(&state, &path, |lib| {
        let name = name.trim();
        blinkview_core::fsops::validate_filename(name)?;
        let parent = parent.trim().trim_matches('/');
        if !lib.abs(parent).is_dir() {
            anyhow::bail!(
                "{} is not a folder in this library",
                if parent.is_empty() {
                    "the library root"
                } else {
                    parent
                }
            );
        }
        let rel = if parent.is_empty() {
            name.to_string()
        } else {
            format!("{parent}/{name}")
        };
        let dir = lib.abs(&rel);
        // Adopting an existing folder silently would make "new folder" a lie, and the
        // one it adopted might be full.
        if dir.exists() {
            anyhow::bail!("{name} already exists here");
        }
        std::fs::create_dir(&dir)?;
        Ok(rel)
    })
}

#[tauri::command]
async fn rescan(state: tauri::State<'_, AppState>, path: String) -> R<SourceInfo> {
    // `open_lib` scans on the way in, so this does not scan again.
    with(&state, &path, describe)
}

// ---------------------------------------------------------------- photos

#[tauri::command]
async fn photos(
    state: tauri::State<'_, AppState>,
    path: String,
    folder: Option<String>,
    person: Option<String>,
) -> R<Vec<PhotoInfo>> {
    with_readable(&state, &path, |lib| {
        let people_file = lib.people()?;
        let user = lib.user_data()?.clone();
        let opt = assign::Options::default();
        let mut who: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut nfaces: BTreeMap<String, usize> = BTreeMap::new();
        for f in lib.all_faces()? {
            *nfaces.entry(f.hash.clone()).or_default() += 1;
            if let Some(e) = f.embedding.as_ref() {
                if let Some(n) = assign::assign(e, &people_file, &opt).person() {
                    if people_file.is_excluded(n, &f.hash) {
                        continue;
                    }
                    let v = who.entry(f.hash.clone()).or_default();
                    if !v.iter().any(|x| x == n) {
                        v.push(n.to_string());
                    }
                }
            }
        }
        let mut out: Vec<PhotoInfo> = lib
            .index
            .all()?
            .into_iter()
            .filter(|r| {
                // Prefix, not exact parent: selecting `Trip` must show `Trip/Greece Day3`
                // too. An empty folder is the library root and matches everything.
                folder.as_ref().is_none_or(|f| in_folder(&r.path, f))
            })
            .filter(|r| {
                person
                    .as_ref()
                    .is_none_or(|p| who.get(&r.hash).is_some_and(|v| v.iter().any(|x| x == p)))
            })
            .map(|r| {
                let sig = lib.index.get_signature(&r.hash).ok().flatten();
                let meta = user.get(&r.hash, folder_of(&r.path));
                PhotoInfo {
                    kind: r.kind.clone(),
                    rating: meta.rating,
                    label: meta.label.clone(),
                    albums: meta.albums.clone(),
                    bytes: r.size.max(0) as u64,
                    taken_at: r.taken_at,
                    faces: nfaces.get(&r.hash).copied().unwrap_or(0),
                    people: who.get(&r.hash).cloned().unwrap_or_default(),
                    width: sig.as_ref().map(|s| s.width).unwrap_or(0),
                    height: sig.as_ref().map(|s| s.height).unwrap_or(0),
                    hash: r.hash.clone(),
                    path: r.path.clone(),
                }
            })
            .collect();
        // Newest first, which is what a photo library defaults to.
        let name = |p: &PhotoInfo| p.path.rsplit('/').next().unwrap_or(&p.path).to_string();
        out.sort_by(|a, b| b.taken_at.cmp(&a.taken_at).then(name(a).cmp(&name(b))));
        Ok(out)
    })
}

#[tauri::command]
async fn build_thumbs(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> R<usize> {
    let sink = emitter(&app, "thumbs", &path);
    let stop = cancel_flag(&state, &path);
    with(&state, &path, |lib| {
        let st = analyze::run_cancellable(lib, analyze::Stages::only_thumbs(), &sink, &stop)?;
        Ok(st.thumbs)
    })
}

/// A predicate the analysis loop consults to know whether to stop.
fn cancel_flag<'a>(state: &'a AppState, path: &str) -> impl Fn() -> bool + Sync + 'a {
    let path = path.to_string();
    move || {
        state
            .cancelled
            .lock()
            .map(|c| c.contains(&path))
            .unwrap_or(false)
    }
}

/// Everything a photograph needs, from one decode (ADR-0013).
///
/// Three separate passes each decoded the same photograph; together they cost 263 ms
/// per photograph against 87 ms for this one.
#[tauri::command]
async fn analyze_all(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> R<String> {
    let sink = emitter(&app, "analyze", &path);
    let stop = cancel_flag(&state, &path);
    with(&state, &path, |lib| {
        let st = analyze::run_cancellable(lib, analyze::Stages::default(), &sink, &stop)?;
        let mut parts = Vec::new();
        if st.thumbs > 0 {
            parts.push(format!("{} thumbnails", st.thumbs));
        }
        if st.faces > 0 {
            parts.push(format!("{} faces", st.faces));
        }
        if st.embedded > 0 {
            parts.push(format!("{} understood", st.embedded));
        }
        Ok(if parts.is_empty() {
            "Everything was already analysed.".to_string()
        } else {
            let mut msg = parts.join(" · ");
            if !st.errors.is_empty() {
                msg.push_str(&format!(" · {} could not be read", st.errors.len()));
            }
            msg
        })
    })
}

#[tauri::command]
async fn analyze_faces(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> R<String> {
    let sink = emitter(&app, "faces", &path);
    let stop = cancel_flag(&state, &path);
    with(&state, &path, |lib| {
        // Thumbnails ride along: the photograph is being decoded anyway.
        let st = analyze::run_cancellable(
            lib,
            analyze::Stages {
                thumbs: true,
                faces: true,
                semantic: false,
            },
            &sink,
            &stop,
        )?;
        Ok(format!(
            "{} photos analysed · {} faces found",
            st.decoded, st.faces
        ))
    })
}

/// What analysis is unfinished for a library.
///
/// Quitting mid-pass is normal — these runs take hours on a large library — and every
/// stage commits per photograph, so the work already done survives. This is what lets
/// the window pick it up again on the next launch instead of waiting to be asked.
#[derive(Serialize, Default)]
pub struct Pending {
    photos: usize,
    thumbs_missing: usize,
    faces_missing: usize,
    clip_missing: usize,
    /// Files blinkview cannot decode. Reported so the count is explained rather than
    /// silently outstanding for ever.
    unreadable: usize,
    /// Whether each stage was ever begun. Resuming is for work already started; a
    /// library nobody has asked to analyse should not start doing it on its own.
    faces_started: bool,
    clip_started: bool,
}

/// Resume the stages a previous session left unfinished.
#[tauri::command]
async fn analyze_resume(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
    faces: bool,
    semantic: bool,
) -> R<String> {
    let sink = emitter(&app, "analyze", &path);
    let stop = cancel_flag(&state, &path);
    with(&state, &path, |lib| {
        let st = analyze::run_cancellable(
            lib,
            analyze::Stages {
                thumbs: true,
                faces,
                semantic,
            },
            &sink,
            &stop,
        )?;
        Ok(format!("{} photos finished", st.decoded))
    })
}

#[tauri::command]
async fn pending_work(state: tauri::State<'_, AppState>, path: String) -> R<Pending> {
    with_readable(&state, &path, |lib| {
        let rows: Vec<_> = lib
            .index
            .all()?
            .into_iter()
            .filter(|r| r.kind == "photo")
            .collect();
        let mut p = Pending {
            photos: rows.len(),
            ..Default::default()
        };
        for r in &rows {
            if lib.index.is_unreadable(&r.hash)? {
                p.unreadable += 1;
                continue;
            }
            if !thumbs::thumb_path(lib, &r.hash).exists() {
                p.thumbs_missing += 1;
            }
            if lib.faces_done(&r.hash)? {
                p.faces_started = true;
            } else {
                p.faces_missing += 1;
            }
            if lib.index.get_clip(&r.hash)?.is_some() {
                p.clip_started = true;
            } else {
                p.clip_missing += 1;
            }
        }
        Ok(p)
    })
}

// ---------------------------------------------------------------- places

/// One photograph on the map.
#[derive(Serialize)]
pub struct PhotoPlace {
    hash: String,
    path: String,
    lat: f64,
    lon: f64,
    /// "Fira, South Aegean, Greece", or absent when the nearest known place is too far
    /// away to be worth naming.
    #[serde(skip_serializing_if = "Option::is_none")]
    place: Option<String>,
}

/// Fill in coordinates for photographs nobody has looked at yet.
///
/// Cheap and incremental: reading GPS is a header parse, and the answer — including
/// "none" — is cached against the content hash, so opening the map a second time does
/// no work.
#[tauri::command]
async fn locate_photos(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> R<blinkview_core::geo::Located> {
    let sink = emitter(&app, "locate", &path);
    with(&state, &path, |lib| blinkview_core::geo::locate(lib, &sink))
}

/// Every photograph that knows where it was taken.
#[tauri::command]
async fn photo_places(state: tauri::State<'_, AppState>, path: String) -> R<Vec<PhotoPlace>> {
    with_readable(&state, &path, |lib| {
        let by_hash: BTreeMap<String, String> = lib
            .index
            .all()?
            .into_iter()
            .map(|r| (r.hash, r.path))
            .collect();
        Ok(lib
            .index
            .located()?
            .into_iter()
            .filter_map(|(hash, lat, lon)| {
                let path = by_hash.get(&hash)?.clone();
                Some(PhotoPlace {
                    place: blinkview_core::geo::nearest(lat, lon).map(|p| p.label()),
                    hash,
                    path,
                    lat,
                    lon,
                })
            })
            .collect())
    })
}

/// Places matching a typed name, for a photograph that has no coordinates of its own.
#[tauri::command]
async fn place_search(query: String) -> R<Vec<blinkview_core::geo::Place>> {
    Ok(blinkview_core::geo::search(&query, 8))
}

/// Write a location into the photographs themselves.
///
/// Straight into the original file, as asked — but each rewrite is read back before it
/// replaces anything (see `geo::write_gps`), and the content hash changes, so ratings
/// and labels are carried across (ADR-0015).
#[tauri::command]
async fn set_photo_location(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
    hashes: Vec<String>,
    lat: f64,
    lon: f64,
) -> R<String> {
    let sink = emitter(&app, "locate", &path);
    with(&state, &path, |lib| {
        let by_hash: BTreeMap<String, String> = lib
            .index
            .all()?
            .into_iter()
            .map(|r| (r.hash, r.path))
            .collect();
        let counter = blinkview_core::progress::Counter::new(hashes.len(), &sink);
        let (mut done, mut refused, mut carried) = (0usize, Vec::new(), Vec::new());
        for h in &hashes {
            counter.tick();
            let Some(rel) = by_hash.get(h) else { continue };
            match blinkview_core::geo::write_gps(&lib.abs(rel), lat, lon) {
                Ok(()) => {
                    let new = scan::hash_file(&lib.abs(rel))?;
                    lib.index.set_gps(&new, Some((lat, lon)))?;
                    carried.push((folder_of(rel).to_string(), h.clone(), new));
                    done += 1;
                }
                Err(e) => refused.push(e.to_string()),
            }
        }
        carry_metadata(lib, &carried)?;
        scan::scan(lib, false)?;
        let where_to = blinkview_core::geo::nearest(lat, lon)
            .map(|p| p.label())
            .unwrap_or_else(|| format!("{lat:.4}, {lon:.4}"));
        Ok(match refused.first() {
            None => format!("{done} placed in {where_to}"),
            Some(why) => format!(
                "{done} placed in {where_to} · {} left alone — {why}",
                refused.len()
            ),
        })
    })
}

fn parse_capture_datetime(value: &str) -> anyhow::Result<NaiveDateTime> {
    let value = value.trim();
    let parsed = ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M:%S"]
        .into_iter()
        .find_map(|format| NaiveDateTime::parse_from_str(value, format).ok())
        .ok_or_else(|| anyhow::anyhow!("choose a complete date and time"))?;
    if !(1900..=9999).contains(&parsed.year()) {
        anyhow::bail!("EXIF dates must be between 1900 and 9999");
    }
    Ok(parsed)
}

/// Correct the camera wall-clock value in the photographs themselves.
///
/// One value is deliberately shared by the whole selection: this is the predictable
/// multi-select operation for a scanner batch or a camera whose clock was unset.
#[tauri::command]
async fn set_photo_datetime(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
    hashes: Vec<String>,
    datetime: String,
) -> R<String> {
    let wanted = parse_capture_datetime(&datetime).map_err(err)?;
    let sink = emitter(&app, "date", &path);
    with(&state, &path, |lib| {
        let by_hash: BTreeMap<String, String> = lib
            .index
            .all()?
            .into_iter()
            .map(|r| (r.hash, r.path))
            .collect();
        let counter = blinkview_core::progress::Counter::new(hashes.len(), &sink);
        let (mut done, mut refused, mut carried) = (0usize, Vec::new(), Vec::new());
        for hash in &hashes {
            counter.tick();
            let Some(rel) = by_hash.get(hash) else {
                continue;
            };
            let cached_gps = lib.index.get_gps(hash)?;
            match blinkview_core::geo::write_datetime(&lib.abs(rel), wanted) {
                Ok(()) => {
                    let new = scan::hash_file(&lib.abs(rel))?;
                    if let Some(gps) = cached_gps {
                        lib.index.set_gps(&new, gps)?;
                    }
                    let _ = std::fs::remove_file(thumbs::thumb_path(lib, hash));
                    carried.push((folder_of(rel).to_string(), hash.clone(), new));
                    done += 1;
                }
                Err(e) => refused.push(e.to_string()),
            }
        }
        carry_metadata(lib, &carried)?;
        scan::scan(lib, false)?;
        let stamped = wanted.format("%-d %b %Y at %H:%M");
        Ok(match refused.first() {
            None => format!("{done} set to {stamped}"),
            Some(why) => format!(
                "{done} set to {stamped} · {} left alone — {why}",
                refused.len()
            ),
        })
    })
}

// ---------------------------------------------------------------- semantic

#[derive(Serialize)]
pub struct SemanticStatus {
    /// False when the models are not downloaded; the UI offers to fetch them.
    available: bool,
    /// Photos with an embedding, against photos that could have one.
    embedded: usize,
    total: usize,
}

#[tauri::command]
async fn semantic_status(state: tauri::State<'_, AppState>, path: String) -> R<SemanticStatus> {
    let available = semantic::TextEncoder::available();
    with(&state, &path, |lib| {
        let total = lib
            .index
            .all()?
            .into_iter()
            .filter(|r| r.kind == "photo")
            .count();
        Ok(SemanticStatus {
            available,
            embedded: lib.index.count_clip()?,
            total,
        })
    })
}

/// Embed every photo that has none yet. Resumable: interrupting loses only the photo
/// in flight, so a large library can be indexed across several sittings.
#[tauri::command]
async fn semantic_index(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> R<String> {
    let sink = emitter(&app, "semantic", &path);
    let stop = cancel_flag(&state, &path);
    with(&state, &path, |lib| {
        let st = analyze::run_cancellable(
            lib,
            analyze::Stages {
                thumbs: true,
                faces: false,
                semantic: true,
            },
            &sink,
            &stop,
        )?;
        Ok(match (st.embedded, st.errors.len()) {
            (0, 0) => "Everything was already understood.".to_string(),
            (n, 0) => format!("{n} photos understood."),
            (n, e) => format!("{n} photos understood · {e} could not be read."),
        })
    })
}

#[derive(Serialize)]
pub struct SemanticHit {
    hash: String,
    score: f32,
}

/// Rank photographs against a phrase. An empty result is a real answer: below the
/// threshold the model is guessing, and a confident wrong photo is worse than none.
#[tauri::command]
async fn semantic_search(
    state: tauri::State<'_, AppState>,
    path: String,
    query: String,
    limit: Option<usize>,
) -> R<Vec<SemanticHit>> {
    let query = query.trim().to_string();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    if !semantic::TextEncoder::available() {
        return Ok(Vec::new());
    }
    let mut guard = state.text_encoder.lock().map_err(err)?;
    if guard.is_none() {
        *guard = Some(semantic::TextEncoder::load().map_err(err)?);
    }
    let enc = guard.as_mut().expect("just loaded");
    with(&state, &path, |lib| {
        let hits = semantic::search_with(
            lib,
            enc,
            &query,
            semantic::DEFAULT_THRESHOLD,
            limit.unwrap_or(500),
        )?;
        Ok(hits
            .into_iter()
            .map(|h| SemanticHit {
                hash: h.hash,
                score: h.score,
            })
            .collect())
    })
}

// ---------------------------------------------------------------- people

#[derive(Serialize)]
pub struct ClusterView {
    id: usize,
    photo_count: usize,
    face_count: usize,
    suggestion: Option<String>,
    similarity: Option<f32>,
    crops: Vec<String>,
    centroid: Vec<f32>,
}

#[tauri::command]
async fn clusters(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
    distance: f32,
) -> R<Vec<ClusterView>> {
    let sink = emitter(&app, "clusters", &path);
    with(&state, &path, |lib| {
        let people = lib.people()?;
        let p = review::build_with_progress(
            lib,
            &people,
            &assign::Options::default(),
            distance,
            &sink,
        )?;
        Ok(p.clusters
            .into_iter()
            .map(|c| ClusterView {
                id: c.id,
                photo_count: c.photo_count,
                face_count: c.face_count,
                suggestion: c.suggestion,
                similarity: c.similarity,
                crops: c.faces.into_iter().map(|f| f.crop).collect(),
                centroid: c.centroid,
            })
            .collect())
    })
}

#[tauri::command]
async fn name_clusters(
    state: tauri::State<'_, AppState>,
    path: String,
    distance: f32,
    assignments: BTreeMap<usize, String>,
) -> R<usize> {
    with(&state, &path, |lib| {
        let mut people = lib.people()?;
        let groups =
            pipeline::cluster_unassigned(lib, &people, &assign::Options::default(), distance)?;
        let mut learned = 0;
        for (id, name) in &assignments {
            if let Some(g) = groups.get(*id) {
                let refs: Vec<Vec<f32>> = g.iter().filter_map(|f| f.embedding.clone()).collect();
                learned += refs.len();
                people.add_references(name, refs);
            }
        }
        lib.save_people(&people)?;
        Ok(learned)
    })
}

// ---------------------------------------------------------------- people overview

/// Everyone the library knows about, and how many faces have been set aside.
#[derive(Serialize)]
pub struct PeopleView {
    entries: Vec<PersonEntry>,
    /// Faces dismissed as not worth naming. Shown so the sidebar can offer them back —
    /// a correction nobody can find is not a correction.
    dismissed: usize,
}

#[derive(Serialize)]
pub struct PersonEntry {
    /// `None` for a group nobody has named yet.
    name: Option<String>,
    /// Cluster id, only meaningful for unnamed groups.
    cluster: Option<usize>,
    photos: usize,
    cover: Option<String>,
    suggestion: Option<String>,
}

/// Everyone the library knows about — named people *and* groups still waiting for a
/// name.
///
/// Unnamed groups were previously reachable only through a modal, so running face
/// detection appeared to do nothing at all. Surfacing them beside named people is what
/// makes detection's result visible.
#[tauri::command]
async fn people_overview(
    state: tauri::State<'_, AppState>,
    path: String,
    distance: f32,
) -> R<PeopleView> {
    with(&state, &path, |lib| {
        let people = lib.people()?;
        let opt = assign::Options::default();
        let root = lib.root().to_path_buf();

        let mut claimed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut cover: BTreeMap<String, (String, i64)> = BTreeMap::new();
        for f in lib.all_faces()? {
            let Some(e) = f.embedding.as_ref() else {
                continue;
            };
            if let Some(n) = assign::assign(e, &people, &opt).person() {
                if people.is_excluded(n, &f.hash) {
                    continue;
                }
                claimed
                    .entry(n.to_string())
                    .or_default()
                    .insert(f.hash.clone());
                cover
                    .entry(n.to_string())
                    .or_insert((f.hash.clone(), f.idx));
            }
        }

        let mut out: Vec<PersonEntry> = people
            .people
            .iter()
            .map(|p| PersonEntry {
                photos: claimed.get(&p.name).map(|s| s.len()).unwrap_or(0),
                cover: cover
                    .get(&p.name)
                    .map(|(h, i)| pipeline::face_crop_path(&root, h, *i).display().to_string()),
                name: Some(p.name.clone()),
                cluster: None,
                suggestion: None,
            })
            .collect();
        out.sort_by(|a, b| b.photos.cmp(&a.photos));

        // Groups nobody has claimed yet, largest first.
        let groups = pipeline::cluster_unassigned(lib, &people, &opt, distance)?;
        // Singletons are included. A person photographed once is still a person, and
        // hiding them meant a small library reported "No faces found" while nine faces
        // sat in the index. Ordering by size and capping the list in the UI keeps the
        // long tail of passers-by from dominating.
        let mut unnamed: Vec<PersonEntry> = groups
            .iter()
            .enumerate()
            .map(|(id, g)| {
                let best = g.iter().max_by(|a, b| {
                    a.score
                        .partial_cmp(&b.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                PersonEntry {
                    name: None,
                    cluster: Some(id),
                    photos: g.iter().map(|f| &f.hash).collect::<BTreeSet<_>>().len(),
                    cover: best.map(|f| {
                        pipeline::face_crop_path(&root, &f.hash, f.idx)
                            .display()
                            .to_string()
                    }),
                    suggestion: best.and_then(|f| {
                        f.embedding.as_ref().and_then(|e| {
                            assign::score_all(e, &people)
                                .first()
                                .and_then(|(n, s)| (*s >= 0.45).then(|| n.clone()))
                        })
                    }),
                }
            })
            .collect();
        unnamed.sort_by(|a, b| b.photos.cmp(&a.photos));
        out.extend(unnamed);
        Ok(PeopleView {
            entries: out,
            dismissed: people.dismissed_count(),
        })
    })
}

/// Set a group of faces aside as not worth naming.
///
/// Recorded against the faces rather than the group, because a group's id is a position
/// in a list recomputed on every pass. The photographs are untouched and the faces stay
/// in the index — this only takes them out of review.
#[tauri::command]
async fn dismiss_cluster(
    state: tauri::State<'_, AppState>,
    path: String,
    distance: f32,
    cluster: usize,
) -> R<String> {
    with(&state, &path, |lib| {
        let mut people = lib.people()?;
        let groups =
            pipeline::cluster_unassigned(lib, &people, &assign::Options::default(), distance)?;
        let g = groups
            .get(cluster)
            .ok_or_else(|| anyhow::anyhow!("no such group"))?;
        let faces: Vec<(String, i64)> = g.iter().map(|f| (f.hash.clone(), f.idx)).collect();
        let photos = g.iter().map(|f| &f.hash).collect::<BTreeSet<_>>().len();
        let n = people.dismiss(&faces);
        lib.save_people(&people)?;
        Ok(format!(
            "Set aside {n} face{} from {photos} photograph{}",
            if n == 1 { "" } else { "s" },
            if photos == 1 { "" } else { "s" }
        ))
    })
}

/// Offer every dismissed face for naming again.
#[tauri::command]
async fn restore_dismissed(state: tauri::State<'_, AppState>, path: String) -> R<String> {
    with(&state, &path, |lib| {
        let mut people = lib.people()?;
        let n = people.restore_dismissed();
        lib.save_people(&people)?;
        Ok(match n {
            0 => "Nothing was set aside".to_string(),
            n => format!("{n} face{} back for naming", if n == 1 { "" } else { "s" }),
        })
    })
}

/// Fold one person into another: the same person, named twice.
///
/// Unlike forgetting one of them, this keeps both sets of reference faces, so the
/// correction makes recognition better rather than worse.
#[tauri::command]
async fn merge_people(
    state: tauri::State<'_, AppState>,
    path: String,
    from: String,
    into: String,
) -> R<String> {
    with(&state, &path, |lib| {
        let mut people = lib.people()?;
        let moved = people.merge(&from, &into)?;
        lib.save_people(&people)?;
        Ok(format!(
            "{from} is now {into} · {moved} more reference faces for {into}"
        ))
    })
}

/// Name one unnamed group, teaching that identity its faces.
#[tauri::command]
async fn name_cluster(
    state: tauri::State<'_, AppState>,
    path: String,
    distance: f32,
    cluster: usize,
    name: String,
) -> R<usize> {
    with(&state, &path, |lib| {
        let mut people = lib.people()?;
        let groups =
            pipeline::cluster_unassigned(lib, &people, &assign::Options::default(), distance)?;
        let g = groups
            .get(cluster)
            .ok_or_else(|| anyhow::anyhow!("no such group"))?;
        let refs: Vec<Vec<f32>> = g.iter().filter_map(|f| f.embedding.clone()).collect();
        let n = refs.len();
        people.add_references(name.trim(), refs);
        lib.save_people(&people)?;
        Ok(n)
    })
}

/// Photo hashes belonging to an unnamed group, so the grid can show them.
#[tauri::command]
async fn cluster_photos(
    state: tauri::State<'_, AppState>,
    path: String,
    distance: f32,
    cluster: usize,
) -> R<Vec<String>> {
    with(&state, &path, |lib| {
        let people = lib.people()?;
        let groups =
            pipeline::cluster_unassigned(lib, &people, &assign::Options::default(), distance)?;
        Ok(groups
            .get(cluster)
            .map(|g| {
                g.iter()
                    .map(|f| f.hash.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default())
    })
}

// ---------------------------------------------------------------- models

#[derive(Serialize)]
pub struct ModelStatus {
    name: String,
    present: bool,
    megabytes: f64,
}

#[tauri::command]
async fn models_status() -> R<Vec<ModelStatus>> {
    Ok(model_fetch::specs()
        .into_iter()
        .map(|s| ModelStatus {
            name: s.name.to_string(),
            present: model_fetch::is_present(&s),
            megabytes: (s.bytes as f64) / 1_048_576.0,
        })
        .collect())
}

/// Download any missing models. Face detection cannot run without them, so the app
/// offers this rather than leaving the user to read a README.
#[tauri::command]
async fn models_fetch(app: tauri::AppHandle, _state: tauri::State<'_, AppState>) -> R<String> {
    let sink = |name: &str, done: usize, total: usize| {
        remote::emit_all(
            &app,
            "progress",
            &ProgressEvent {
                op: "models",
                done,
                total,
                source: "",
            },
        );
        let _ = name;
    };
    let got = model_fetch::fetch_missing(&sink).map_err(err)?;
    Ok(if got.is_empty() {
        "All models already installed".into()
    } else {
        format!("Installed {}", got.join(" and "))
    })
}

// ---------------------------------------------------------------- ratings and labels

/// Apply an edit to each photograph, writing into the folder that holds it.
///
/// The folder comes from the index rather than the caller, so the frontend never has to
/// know where a photograph lives for its rating to land in the right file (ADR-0010).
fn edit_each(
    lib: &mut Library,
    hashes: &[String],
    mut f: impl FnMut(&mut blinkview_core::userdata::UserData, &str),
) -> anyhow::Result<()> {
    let folders: BTreeMap<String, String> = lib
        .index
        .all()?
        .into_iter()
        .map(|r| (r.hash, folder_of(&r.path).to_string()))
        .collect();
    let mut set = UserDataSet::load(lib.root())?;
    for h in hashes {
        let folder = folders.get(h).cloned().unwrap_or_default();
        set.edit(h, &folder, |u| f(u, h));
    }
    set.save(lib.root())?;
    lib.invalidate_user_data();
    Ok(())
}

#[tauri::command]
async fn set_rating(
    state: tauri::State<'_, AppState>,
    path: String,
    hashes: Vec<String>,
    rating: u8,
) -> R<()> {
    with(&state, &path, |lib| {
        edit_each(lib, &hashes, |u, h| u.set_rating(h, rating))
    })
}

#[tauri::command]
async fn set_label(
    state: tauri::State<'_, AppState>,
    path: String,
    hashes: Vec<String>,
    label: Option<String>,
) -> R<()> {
    with(&state, &path, |lib| {
        edit_each(lib, &hashes, |u, h| u.set_label(h, label.clone()))
    })
}

#[tauri::command]
async fn set_album(
    state: tauri::State<'_, AppState>,
    path: String,
    hashes: Vec<String>,
    album: String,
    member: bool,
) -> R<()> {
    with(&state, &path, |lib| {
        // Albums are on the way out (ADR-0009); this keeps existing ones editable
        // until the migration to folders ships.
        let album = album.trim().to_string();
        edit_each(lib, &hashes, |u, h| u.set_album(h, &album, member))
    })
}

/// What migrating albums to folders would do (ADR-0009), without doing it.
#[derive(Serialize)]
pub struct MigrationView {
    moves: usize,
    folders: Vec<(String, usize)>,
    renamed: Vec<(String, String)>,
    skipped: Vec<(String, String)>,
}

#[tauri::command]
async fn plan_album_migration(state: tauri::State<'_, AppState>, path: String) -> R<MigrationView> {
    with(&state, &path, |lib| {
        let m = blinkview_core::albums::plan(lib)?;
        Ok(MigrationView {
            moves: m.plan.len(),
            folders: m.folders.into_iter().collect(),
            renamed: m.renamed,
            skipped: m.plan.skipped,
        })
    })
}

#[tauri::command]
async fn apply_album_migration(state: tauri::State<'_, AppState>, path: String) -> R<String> {
    with(&state, &path, |lib| {
        let m = blinkview_core::albums::plan(lib)?;
        if m.plan.is_empty() {
            return Ok("Nothing to move.".into());
        }
        for dest in m.folders.keys() {
            std::fs::create_dir_all(lib.abs(dest))?;
        }
        let n = m.plan.len();
        let j = m.plan.apply(lib)?;
        // The albums have become folders; leaving the labels behind would show the
        // same grouping twice.
        let mut set = UserDataSet::load(lib.root())?;
        set.clear_albums();
        set.save(lib.root())?;
        lib.invalidate_user_data();
        Ok(format!("{n} photos moved into folders · undo id {}", j.id))
    })
}

/// How long does the bridge take to carry N photographs?
///
/// Debug builds only. Projecting a 200,000-photograph library from a 2,433-photograph
/// measurement has been wrong twice; this answers it at the real size instead.
#[cfg(debug_assertions)]
#[tauri::command]
async fn bench_payload(n: usize) -> R<Vec<PhotoInfo>> {
    Ok((0..n)
        .map(|i| PhotoInfo {
            kind: "photo".into(),
            rating: 0,
            label: None,
            albums: vec![],
            bytes: 3_500_000,
            hash: format!("{:064x}", i as u128 * 0x9E3779B97F4A7C15),
            path: format!("DCIM/Camera/20230501_{:06}.jpg", i),
            taken_at: Some(1_700_000_000 + i as i64),
            faces: 0,
            people: vec![],
            width: 4032,
            height: 3024,
        })
        .collect())
}

// ---------------------------------------------------------------- commands

/// A planned move, for the command layer's preview (ADR-0012).
///
/// Nothing reaches the disk without being listed here first: there is no path from a
/// typed sentence to a file move that skips the preview.
#[derive(Serialize)]
pub struct MoveView {
    dest: String,
    moves: Vec<(String, String)>,
    skipped: Vec<(String, String)>,
}

#[tauri::command]
async fn plan_move(
    state: tauri::State<'_, AppState>,
    path: String,
    hashes: Vec<String>,
    dest: String,
) -> R<MoveView> {
    with(&state, &path, |lib| {
        let p = blinkview_core::plan::move_into(lib, &hashes, &dest)?;
        Ok(MoveView {
            dest: dest.trim().trim_matches('/').to_string(),
            moves: p
                .ops
                .iter()
                .map(|o| (o.from().to_string(), o.to().to_string()))
                .collect(),
            skipped: p.skipped,
        })
    })
}

#[tauri::command]
async fn apply_move(
    state: tauri::State<'_, AppState>,
    path: String,
    hashes: Vec<String>,
    dest: String,
) -> R<String> {
    with(&state, &path, |lib| {
        let p = blinkview_core::plan::move_into(lib, &hashes, &dest)?;
        if p.is_empty() {
            return Ok("Nothing to move.".into());
        }
        let n = p.len();
        std::fs::create_dir_all(lib.abs(dest.trim().trim_matches('/')))?;
        let j = p.apply(lib)?;
        Ok(format!("{n} moved to {} · undo id {}", dest.trim(), j.id))
    })
}

// ---------------------------------------------------------------- saved searches

#[tauri::command]
async fn list_searches(
    state: tauri::State<'_, AppState>,
    path: String,
) -> R<Vec<blinkview_core::userdata::SavedSearch>> {
    with(&state, &path, |lib| {
        Ok(UserDataSet::load(lib.root())?.searches().to_vec())
    })
}

#[tauri::command]
async fn save_search(
    state: tauri::State<'_, AppState>,
    path: String,
    name: String,
    query: String,
) -> R<()> {
    with(&state, &path, |lib| {
        let mut set = UserDataSet::load(lib.root())?;
        set.set_search(name.trim(), &query);
        set.save(lib.root())?;
        lib.invalidate_user_data();
        Ok(())
    })
}

/// The directory holding a folder's own `blinkview.json`.
fn folder_dir(root: &std::path::Path, folder: &str) -> std::path::PathBuf {
    if folder.is_empty() {
        root.to_path_buf()
    } else {
        root.join(folder)
    }
}

/// How a folder is arranged.
///
/// Reads that folder's own file rather than the cascade: an arrangement is about the
/// folder, not about the photographs in it, so a subfolder nobody arranged must not
/// inherit one. One file read, deliberately — `UserDataSet::load` walks the whole tree
/// (100 ms on a phone backup) and this runs every time a folder is selected.
#[tauri::command]
async fn folder_view(
    state: tauri::State<'_, AppState>,
    path: String,
    folder: String,
) -> R<FolderView> {
    with_readable(&state, &path, |lib| {
        Ok(UserData::load(&folder_dir(lib.root(), &folder))?
            .view
            .unwrap_or_default())
    })
}

/// Record how a folder is arranged. An empty sort with no order clears it.
#[tauri::command]
async fn set_folder_view(
    state: tauri::State<'_, AppState>,
    path: String,
    folder: String,
    sort: String,
    order: Vec<String>,
) -> R<()> {
    with(&state, &path, |lib| {
        let dir = folder_dir(lib.root(), &folder);
        if !dir.is_dir() {
            anyhow::bail!("no such folder: {folder}");
        }
        let mut u = UserData::load(&dir)?;
        let v = FolderView { sort, order };
        u.view = (!v.is_empty()).then_some(v);
        // An empty file is litter in a folder people browse in Finder.
        if u.photos.is_empty() && u.searches.is_empty() && u.view.is_none() {
            let _ = std::fs::remove_file(UserData::path(&dir));
        } else {
            u.save(&dir)?;
        }
        lib.invalidate_user_data();
        Ok(())
    })
}

#[tauri::command]
async fn list_albums(state: tauri::State<'_, AppState>, path: String) -> R<Vec<(String, usize)>> {
    with(&state, &path, |lib| {
        Ok(UserDataSet::load(lib.root())?
            .albums()
            .into_iter()
            .collect())
    })
}

/// Everything worth showing about one photo, for the info panel.
#[derive(Serialize)]
pub struct PhotoDetail {
    path: String,
    bytes: u64,
    width: u32,
    height: u32,
    taken_at: Option<i64>,
    taken_from: Option<String>,
    kind: String,
    faces: usize,
    people: Vec<String>,
    meta: PhotoMeta,
    hash: String,
    /// What the file says about how it was taken. Absent fields are genuinely absent —
    /// a screenshot has none of them, and anything through a messaging app has been
    /// stripped already.
    exif: blinkview_core::metadata::Exif,
    /// Whether blinkview could remove that record without re-encoding the pixels.
    strippable: bool,
}

#[tauri::command]
async fn photo_detail(
    state: tauri::State<'_, AppState>,
    path: String,
    hash: String,
) -> R<PhotoDetail> {
    with_readable(&state, &path, |lib| {
        let row = lib
            .index
            .all()?
            .into_iter()
            .find(|r| r.hash == hash)
            .ok_or_else(|| anyhow::anyhow!("photo not found"))?;
        let sig = lib.index.get_signature(&hash)?;
        let mut people = Vec::new();
        let mut faces = 0;
        let meta = if lib.is_peek() {
            // A peek must not even *read through* the user-data loaders: adopting a
            // legacy metadata filename is a migration and therefore a write. Peeks
            // have no ratings or face analysis by design.
            PhotoMeta::default()
        } else {
            let people_file = lib.people()?;
            let opt = assign::Options::default();
            for f in lib.all_faces()? {
                if f.hash != hash {
                    continue;
                }
                faces += 1;
                if let Some(e) = f.embedding.as_ref() {
                    if let Some(n) = assign::assign(e, &people_file, &opt).person() {
                        if !people_file.is_excluded(n, &hash) && !people.iter().any(|x| x == n) {
                            people.push(n.to_string());
                        }
                    }
                }
            }
            lib.user_data()?.get(&hash, folder_of(&row.path))
        };
        Ok(PhotoDetail {
            path: row.path.clone(),
            bytes: row.size.max(0) as u64,
            width: sig.as_ref().map(|s| s.width).unwrap_or(0),
            height: sig.as_ref().map(|s| s.height).unwrap_or(0),
            taken_at: row.taken_at,
            taken_from: row.taken_src.clone(),
            kind: row.kind.clone(),
            faces,
            people,
            meta,
            exif: blinkview_core::metadata::read(&lib.abs(&row.path)),
            strippable: blinkview_core::metadata::strippable(&lib.abs(&row.path)),
            hash,
        })
    })
}

// ---------------------------------------------------------------- photo editing

/// Rotate and/or crop one photo.
///
/// `keep_original` defaults to true and moves the untouched file to `Originals/`,
/// mirroring how deleting moves a photo to `Trash/`. See blinkview_core::edit for why
/// the original is not kept in the (disposable) vault.
#[tauri::command]
async fn edit_photo(
    state: tauri::State<'_, AppState>,
    path: String,
    hash: String,
    edit: blinkview_core::edit::Edit,
) -> R<String> {
    with(&state, &path, |lib| {
        let row = lib
            .index
            .all()?
            .into_iter()
            .find(|r| r.hash == hash)
            .ok_or_else(|| anyhow::anyhow!("photo not found"))?;
        let out = blinkview_core::edit::apply(lib, &row.path, &edit)?;
        // The file changed, so its hash did: re-scan to re-identify it, and drop the
        // stale thumbnail and face data keyed to the old content.
        let _ = std::fs::remove_file(thumbs::thumb_path(lib, &hash));
        // Ratings and labels are keyed by that hash too (ADR-0007), so they have to be
        // carried across or editing a five-star photograph silently unrates it.
        carry_metadata(
            lib,
            &[(
                folder_of(&row.path).to_string(),
                hash.clone(),
                out.hash.clone(),
            )],
        )?;
        scan::scan(lib, false)?;
        Ok(match out.original {
            Some(o) => format!(
                "Saved {}x{} · original kept in {}",
                out.width, out.height, o
            ),
            None => format!("Saved {}x{} · original not kept", out.width, out.height),
        })
    })
}

/// Move ratings and labels onto the hashes rewritten files now have.
///
/// One load and one save for the whole batch: `UserDataSet::load` walks the tree, which
/// was measured at 100 ms on a phone backup and is not something to do per photograph.
fn carry_metadata(lib: &mut Library, moves: &[(String, String, String)]) -> anyhow::Result<()> {
    if moves.iter().all(|(_, from, to)| from == to) {
        return Ok(());
    }
    let mut set = UserDataSet::load(lib.root())?;
    let mut touched = false;
    for (folder, from, to) in moves {
        touched |= set.rekey(folder, from, to);
    }
    if touched {
        set.save(lib.root())?;
        lib.invalidate_user_data();
    }
    Ok(())
}

/// Apply one edit to many photographs.
///
/// `keep_original` matters more here than it does for a single photograph, because a
/// batch multiplies a mistake: forty files changed at once are forty to put back. It
/// defaults to true exactly as the single-photograph path does.
///
/// One rescan at the end rather than one per file: every edit changes the content
/// hash, and re-identifying the library forty times over would cost more than the
/// edits.
#[tauri::command]
async fn edit_photos(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
    hashes: Vec<String>,
    edit: blinkview_core::edit::Edit,
) -> R<String> {
    let sink = emitter(&app, "edit", &path);
    with(&state, &path, |lib| {
        let by_hash: BTreeMap<String, String> = lib
            .index
            .all()?
            .into_iter()
            .map(|r| (r.hash, r.path))
            .collect();
        let counter = blinkview_core::progress::Counter::new(hashes.len(), &sink);
        let (mut done, mut failed, mut carried) = (0usize, Vec::new(), Vec::new());
        for h in &hashes {
            counter.tick();
            let Some(rel) = by_hash.get(h) else { continue };
            match blinkview_core::edit::apply(lib, rel, &edit) {
                Ok(out) => {
                    // The content changed, so the thumbnail keyed to the old bytes is
                    // now a picture of something that no longer exists.
                    let _ = std::fs::remove_file(thumbs::thumb_path(lib, h));
                    carried.push((folder_of(rel).to_string(), h.clone(), out.hash));
                    done += 1;
                }
                // One unreadable file must not abandon the other thirty-nine.
                Err(e) => failed.push(format!("{rel}: {e}")),
            }
        }
        carry_metadata(lib, &carried)?;
        scan::scan(lib, false)?;
        Ok(match failed.len() {
            0 => format!("{done} changed"),
            k => format!("{done} changed · {k} could not be read"),
        })
    })
}

/// Remove what photographs say about how they were taken.
///
/// Keeps the original by default (ADR-0015): `taken_at` comes from EXIF first
/// (ADR-0003), so a stripped photograph falls back to its filename or its mtime for a
/// date, and that is not something to do to someone's library without a way back.
#[tauri::command]
async fn strip_metadata(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
    hashes: Vec<String>,
    keep_original: Option<bool>,
) -> R<String> {
    let keep = keep_original != Some(false);
    let sink = emitter(&app, "strip", &path);
    with(&state, &path, |lib| {
        let by_hash: BTreeMap<String, String> = lib
            .index
            .all()?
            .into_iter()
            .map(|r| (r.hash, r.path))
            .collect();
        let counter = blinkview_core::progress::Counter::new(hashes.len(), &sink);
        let (mut done, mut refused, mut carried) = (0usize, Vec::new(), Vec::new());
        for h in &hashes {
            counter.tick();
            let Some(rel) = by_hash.get(h) else { continue };
            match blinkview_core::metadata::strip_file(lib, rel, keep) {
                Ok(out) => {
                    let _ = std::fs::remove_file(thumbs::thumb_path(lib, h));
                    carried.push((folder_of(rel).to_string(), h.clone(), out.hash));
                    done += 1;
                }
                Err(e) => refused.push(e.to_string()),
            }
        }
        carry_metadata(lib, &carried)?;
        scan::scan(lib, false)?;
        let kept = if keep && done > 0 {
            " · originals kept in Originals/"
        } else {
            ""
        };
        Ok(match refused.first() {
            None => format!("{done} stripped{kept}"),
            Some(why) => format!(
                "{done} stripped{kept} · {} left alone — {why}",
                refused.len()
            ),
        })
    })
}

// ---------------------------------------------------------------- editing

pub const TRASH: &str = "Trash";

/// Move photos to the library's `Trash/` folder.
///
/// Deliberately not an unlink. Deleting is the one action a photo app can take that a
/// user cannot undo by hand, so it goes through the same journalled Move as everything
/// else and stays recoverable — both from `Trash/` in Finder and via undo.
#[tauri::command]
async fn delete_photos(
    state: tauri::State<'_, AppState>,
    path: String,
    hashes: Vec<String>,
    dest: Option<String>,
) -> R<String> {
    // Somewhere else of your choosing, or the library Trash. Either way it is the same
    // journalled Move, so ⌘Z reverses it identically — and either way it stays inside
    // the library, because leaving it is the separate, explicit "Empty…" step.
    let dest = dest
        .map(|d| d.trim().trim_matches('/').to_string())
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| TRASH.to_string());
    with(&state, &path, |lib| {
        std::fs::create_dir_all(lib.abs(&dest))?;
        let want: BTreeSet<String> = hashes.into_iter().collect();
        let mut plan = blinkview_core::Plan::new("delete");
        for r in lib.index.all()? {
            if !want.contains(&r.hash) || in_folder(&r.path, &dest) {
                continue;
            }
            let name = r.path.rsplit('/').next().unwrap_or(&r.path);
            plan.ops.push(blinkview_core::Op::Move {
                hash: r.hash.clone(),
                from: r.path.clone(),
                to: format!("{dest}/{name}"),
            });
        }
        if plan.is_empty() {
            return Ok("Nothing to delete".into());
        }
        let n = plan.len();
        let j = plan.apply(lib)?;
        let where_to = if dest == TRASH {
            "Trash".to_string()
        } else {
            dest
        };
        Ok(format!("Moved {n} to {where_to} · undo id {}", j.id))
    })
}

/// Rename one photo, keeping it in place.
#[tauri::command]
async fn rename_photo(
    state: tauri::State<'_, AppState>,
    path: String,
    hash: String,
    name: String,
) -> R<String> {
    with(&state, &path, |lib| {
        let row = lib
            .index
            .all()?
            .into_iter()
            .find(|r| r.hash == hash)
            .ok_or_else(|| anyhow::anyhow!("photo not found"))?;
        let ext = row.path.rsplit('.').next().unwrap_or("jpg").to_string();
        let mut new_name = name.trim().to_string();
        if !new_name
            .to_lowercase()
            .ends_with(&format!(".{}", ext.to_lowercase()))
        {
            new_name = format!("{new_name}.{ext}");
        }
        blinkview_core::fsops::validate_filename(&new_name)?;
        let dir = row
            .path
            .rsplit_once('/')
            .map(|(d, _)| format!("{d}/"))
            .unwrap_or_default();
        let to = format!("{dir}{new_name}");
        if to == row.path {
            return Ok("Name unchanged".into());
        }
        let mut plan = blinkview_core::Plan::new("rename-one");
        plan.ops.push(blinkview_core::Op::Rename {
            hash,
            from: row.path,
            to: to.clone(),
        });
        plan.apply(lib)?;
        Ok(format!("Renamed to {new_name}"))
    })
}

/// Remove a person from photos, and move them out of that person's folder.
/// Forget a person entirely: their name and the references behind it.
///
/// The photographs are untouched — this removes only the claim that they are this
/// person. Needed because a name can end up matching nothing (every photo untagged, or
/// a mistaken second spelling) and a name that matches nothing is not information.
#[tauri::command]
async fn forget_person(
    state: tauri::State<'_, AppState>,
    path: String,
    person: String,
) -> R<String> {
    with(&state, &path, |lib| {
        let mut people = lib.people()?;
        if !people.remove(&person) {
            return Ok(format!("{person} was not a known person"));
        }
        lib.save_people(&people)?;
        Ok(format!("Forgot {person}"))
    })
}

#[tauri::command]
async fn untag_person(
    state: tauri::State<'_, AppState>,
    path: String,
    person: String,
    hashes: Vec<String>,
) -> R<String> {
    with(&state, &path, |lib| {
        let mut people = lib.people()?;
        people.exclude(&person, &hashes);
        lib.save_people(&people)?;

        // Anything sitting in that person's folder goes back to the library root.
        let want: BTreeSet<String> = hashes.iter().cloned().collect();
        let mut plan = blinkview_core::Plan::new("untag");
        for r in lib.index.all()? {
            if !want.contains(&r.hash) {
                continue;
            }
            if r.path.starts_with(&format!("{person}/")) {
                let name = r.path.rsplit('/').next().unwrap_or(&r.path).to_string();
                plan.ops.push(blinkview_core::Op::Move {
                    hash: r.hash.clone(),
                    from: r.path.clone(),
                    to: name,
                });
            }
        }
        let moved = plan.len();
        if !plan.is_empty() {
            plan.apply(lib)?;
        }

        // If nothing is left for this person, forget them. A name matching zero
        // photographs is not information, and the sidebar would otherwise keep offering
        // it forever.
        let opt = assign::Options::default();
        let still_has = lib.all_faces()?.into_iter().any(|f| {
            f.embedding.as_ref().is_some_and(|e| {
                assign::assign(e, &people, &opt).person() == Some(person.as_str())
                    && !people.is_excluded(&person, &f.hash)
            })
        });
        let forgotten = if still_has {
            false
        } else {
            let removed = people.remove(&person);
            if removed {
                lib.save_people(&people)?;
            }
            removed
        };

        Ok(format!(
            "Removed {} from {} photo{}{}{}",
            person,
            want.len(),
            if want.len() == 1 { "" } else { "s" },
            if moved > 0 {
                format!(", {moved} moved back to root")
            } else {
                String::new()
            },
            if forgotten {
                format!(" — no photos left, so {person} was forgotten")
            } else {
                String::new()
            }
        ))
    })
}

/// Restore photos from the library Trash back to the root.
#[tauri::command]
async fn restore_photos(
    state: tauri::State<'_, AppState>,
    path: String,
    hashes: Vec<String>,
) -> R<String> {
    with(&state, &path, |lib| {
        let want: BTreeSet<String> = hashes.into_iter().collect();
        let mut plan = blinkview_core::Plan::new("restore");
        for r in lib.index.all()? {
            if !want.contains(&r.hash) || !r.path.starts_with(&format!("{TRASH}/")) {
                continue;
            }
            let name = r.path.rsplit('/').next().unwrap_or(&r.path).to_string();
            plan.ops.push(blinkview_core::Op::Move {
                hash: r.hash.clone(),
                from: r.path.clone(),
                to: name,
            });
        }
        if plan.is_empty() {
            return Ok("Nothing to restore".into());
        }
        let n = plan.len();
        plan.apply(lib)?;
        Ok(format!("Restored {n}"))
    })
}

/// Move one file into the system Trash, crossing filesystems when it must.
///
/// `rename` is a metadata hop and cannot leave the volume, and `~/.Trash` sits on the
/// boot volume while the library can be anywhere — a phone backup on an external drive
/// failed against it with EXDEV for every file, and "Empty Trash" reported moving
/// nothing. Copying the bytes across and then removing the original ends in the same
/// place; if the remove fails the file exists in both, is not counted as moved, and
/// the next attempt takes the clobber-avoiding name.
fn move_to_system_trash(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::rename(from, to)
        .or_else(|_| std::fs::copy(from, to).and_then(|_| std::fs::remove_file(from)))
}

/// Hand the library Trash over to the system Trash.
///
/// This is the one place blinkview stops being reversible by itself, so it hands off
/// rather than unlinking: the files land in the macOS Trash where Finder can still
/// recover them. The library journal cannot undo this, which is why it is a separate,
/// explicit action rather than part of delete.
#[tauri::command]
async fn empty_trash(state: tauri::State<'_, AppState>, path: String) -> R<String> {
    with(&state, &path, |lib| {
        let dir = lib.abs(TRASH);
        if !dir.is_dir() {
            return Ok("Trash is already empty".into());
        }
        let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("no HOME"))?;
        let sys = std::path::PathBuf::from(home).join(".Trash");
        std::fs::create_dir_all(&sys)?;
        let mut moved = 0;
        for e in std::fs::read_dir(&dir)? {
            let p = e?.path();
            if !p.is_file() {
                continue;
            }
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            // Never clobber something already in the system Trash.
            let mut dest = sys.join(&name);
            let mut n = 2;
            while dest.exists() {
                let (stem, ext) = name.rsplit_once('.').unwrap_or((name.as_str(), ""));
                dest = sys.join(format!("{stem} {n}.{ext}"));
                n += 1;
            }
            if move_to_system_trash(&p, &dest).is_ok() {
                if let Some(rel) = lib.rel(&p) {
                    lib.index.remove_path(&rel)?;
                }
                moved += 1;
            }
        }
        Ok(format!("Moved {moved} to the system Trash"))
    })
}

// ---------------------------------------------------------------- operations

fn selected_files(lib: &Library, hashes: &[String]) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let wanted: BTreeSet<&str> = hashes.iter().map(String::as_str).collect();
    if wanted.is_empty() {
        anyhow::bail!("select at least one photo or video");
    }
    let root = std::fs::canonicalize(lib.root())?;
    let mut files = Vec::new();
    for row in lib.index.all()? {
        if !wanted.contains(row.hash.as_str()) {
            continue;
        }
        let file = std::fs::canonicalize(lib.abs(&row.path))?;
        if !file.starts_with(&root) || !file.is_file() {
            anyhow::bail!("{} is no longer a file in this library", row.path);
        }
        files.push(file);
    }
    files.sort();
    files.dedup();
    if files.is_empty() {
        anyhow::bail!("the selection is no longer in this library");
    }
    Ok(files)
}

#[cfg(target_os = "macos")]
fn show_native_share(
    window: &tauri::WebviewWindow,
    files: Vec<std::path::PathBuf>,
) -> anyhow::Result<()> {
    use objc2::AnyThread;
    use objc2_app_kit::{NSSharingServicePicker, NSView};
    use objc2_foundation::{NSArray, NSRectEdge, NSString, NSURL};

    let view = window.ns_view()? as usize;
    window.run_on_main_thread(move || unsafe {
        let view = &*(view as *mut NSView);
        let urls: Vec<objc2::rc::Retained<objc2::runtime::AnyObject>> = files
            .iter()
            .map(|path| NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy())))
            .map(Into::into)
            .collect::<Vec<_>>();
        let items = NSArray::from_retained_slice(&urls);
        // SAFETY: every item is an NSURL, which implements NSPasteboardWriting and is
        // explicitly accepted by NSSharingServicePicker.
        let picker = NSSharingServicePicker::initWithItems(NSSharingServicePicker::alloc(), &items);
        picker.showRelativeToRect_ofView_preferredEdge(view.bounds(), view, NSRectEdge::MinY);
    })?;
    Ok(())
}

#[tauri::command]
async fn share_photos(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    path: String,
    hashes: Vec<String>,
) -> R<()> {
    let files = with_readable(&state, &path, |lib| selected_files(lib, &hashes))?;
    #[cfg(target_os = "macos")]
    {
        show_native_share(&window, files).map_err(err)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, files);
        Err("native sharing is currently available on macOS".into())
    }
}

#[tauri::command]
async fn start_file_drag(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
    hashes: Vec<String>,
) -> R<()> {
    let files = with_readable(&state, &path, |lib| selected_files(lib, &hashes))?;
    #[cfg(target_os = "macos")]
    {
        let (tx, rx) = std::sync::mpsc::channel();
        let app = window.app_handle().clone();
        app.run_on_main_thread(move || {
            let result = drag::start_drag(
                &window,
                drag::DragItem::Files(files),
                drag::Image::Raw(include_bytes!("../icons/128x128.png").to_vec()),
                |_result, _position| {},
                drag::Options::default(),
            )
            .map_err(|e| e.to_string());
            let _ = tx.send(result);
        })
        .map_err(err)?;
        rx.recv().map_err(err)?.map_err(err)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, files);
        Err("outbound file drag is currently available on macOS".into())
    }
}

#[derive(Clone, Serialize)]
pub struct UpdateInfo {
    current: String,
    latest: String,
    available: bool,
    url: String,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    draft: bool,
    prerelease: bool,
}

fn release_url_is_safe(url: &str) -> bool {
    url.strip_prefix("https://github.com/notdefined-inc/blinkview/releases/")
        .and_then(|tail| tail.strip_prefix("tag/"))
        .is_some_and(|tag| {
            !tag.is_empty()
                && tag.len() <= 80
                && tag
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_'))
        })
}

/// `v0.1.0` and `0.1.0` name the same release; GitHub tags carry the `v`.
fn parse_release_tag(tag: &str) -> R<semver::Version> {
    semver::Version::parse(tag.trim().trim_start_matches('v'))
        .map_err(|_| "GitHub returned an invalid release version".to_string())
}

#[tauri::command]
async fn check_for_updates() -> R<UpdateInfo> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(5)))
        .build()
        .new_agent();
    let mut response = agent
        .get("https://api.github.com/repos/notdefined-inc/blinkview/releases/latest")
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", format!("Blinkview/{}", *APP_VERSION))
        .call()
        .map_err(err)?;
    let release: GithubRelease =
        serde_json::from_reader(response.body_mut().as_reader()).map_err(err)?;
    if release.draft || release.prerelease || !release_url_is_safe(&release.html_url) {
        return Err("GitHub returned a release Blinkview will not offer".into());
    }
    let current = semver::Version::parse(&APP_VERSION).map_err(err)?;
    let latest = parse_release_tag(&release.tag_name)?;
    Ok(UpdateInfo {
        available: latest > current,
        current: current.to_string(),
        latest: latest.to_string(),
        url: release.html_url,
    })
}

#[tauri::command]
async fn open_update(url: String) -> R<()> {
    if !release_url_is_safe(&url) {
        return Err("refusing to open a non-Blinkview release URL".into());
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/open")
            .arg(&url)
            .spawn()
            .map_err(err)?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = url;
        Err("opening the release page is currently available on macOS".into())
    }
}

#[derive(Serialize)]
pub struct PlanView {
    label: String,
    moves: Vec<(String, String)>,
    skipped: Vec<(String, String)>,
}

#[derive(Serialize)]
pub struct DuplicateReviewItem {
    hash: String,
    path: String,
    bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    taken_at: Option<i64>,
    width: u32,
    height: u32,
    /// Relative within this burst. It deliberately describes visible detail, not taste.
    quality: u8,
    sharpness: f64,
    recommended: bool,
    #[serde(skip_serializing_if = "is_zero_u8")]
    rating: u8,
}

#[derive(Serialize)]
pub struct DuplicateReviewGroup {
    id: String,
    batch_id: String,
    batch_title: String,
    batch_detail: String,
    reclaimable: u64,
    items: Vec<DuplicateReviewItem>,
}

#[derive(Serialize)]
pub struct DuplicateReview {
    groups: Vec<DuplicateReviewGroup>,
    reclaimable: u64,
}

fn duplicate_batch(taken_at: Option<i64>, place: Option<String>) -> (String, String, String) {
    let Some(at) = taken_at.and_then(|v| chrono::DateTime::<Utc>::from_timestamp(v, 0)) else {
        return (
            "undated".into(),
            "Undated".into(),
            "Capture time missing".into(),
        );
    };
    let detail = at.format("%A, %-d %B %Y").to_string();
    match place {
        Some(place) => {
            // A place and ISO week is a conservative local trip boundary: it avoids
            // merging every photograph ever taken at home while keeping a multi-day
            // visit together.
            let week = at.iso_week();
            (
                format!("trip:{}-{:02}:{place}", week.year(), week.week()),
                format!("Trip · {place}"),
                detail,
            )
        }
        None => (
            format!("day:{}", at.format("%Y-%m-%d")),
            at.format("%-d %B").to_string(),
            detail,
        ),
    }
}

/// The evidence behind duplicate detection, shaped for a human decision rather than
/// flattened into a move plan. No operation here mutates a photograph.
#[tauri::command]
async fn duplicate_review(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> R<DuplicateReview> {
    let sink = emitter(&app, "duplicates", &path);
    with(&state, &path, |lib| {
        // Both passes are incremental. GPS is a cheap header read and lets the review
        // offer trip-sized batches; signatures remain the on-device Vision evidence.
        blinkview_core::geo::locate(lib, &sink)?;
        dedupe::ensure_signatures_with_progress(lib, &sink)?;
        let user = lib.user_data()?.clone();
        let groups = dedupe::find_groups(lib, &dedupe::Options::default())?;
        let mut out = Vec::with_capacity(groups.len());

        for group in groups {
            let keep_hash = group.keep.hash.clone();
            let keep_path = group.keep.path.clone();
            let place = lib
                .index
                .get_gps(&keep_hash)?
                .flatten()
                .and_then(|(lat, lon)| blinkview_core::geo::nearest(lat, lon))
                .map(|p| p.label());
            let (batch_id, batch_title, batch_detail) = duplicate_batch(group.keep.taken_at, place);
            let reclaimable = group.duplicates.iter().map(|r| r.size.max(0) as u64).sum();
            let mut rows = Vec::with_capacity(group.duplicates.len() + 1);
            rows.push(group.keep);
            rows.extend(group.duplicates);

            let signatures: Vec<_> = rows
                .iter()
                .map(|r| lib.index.get_signature(&r.hash))
                .collect::<anyhow::Result<Vec<_>>>()?;
            let max_sharpness = signatures
                .iter()
                .flatten()
                .map(|s| s.sharpness)
                .fold(0.0_f64, f64::max);
            let items = rows
                .into_iter()
                .zip(signatures)
                .map(|(row, sig)| {
                    let sharpness = sig.as_ref().map(|s| s.sharpness).unwrap_or(0.0);
                    let quality = if max_sharpness <= f64::EPSILON {
                        100
                    } else {
                        ((sharpness / max_sharpness).sqrt() * 100.0)
                            .round()
                            .clamp(0.0, 100.0) as u8
                    };
                    let meta = user.get(&row.hash, folder_of(&row.path));
                    DuplicateReviewItem {
                        // Exact byte-for-byte duplicates intentionally share a hash;
                        // the path distinguishes the physical copy being kept within
                        // this one reviewed plan, while the hash still verifies it.
                        recommended: row.hash == keep_hash && row.path == keep_path,
                        width: sig.as_ref().map(|s| s.width).unwrap_or(0),
                        height: sig.as_ref().map(|s| s.height).unwrap_or(0),
                        sharpness,
                        quality,
                        rating: meta.rating,
                        bytes: row.size.max(0) as u64,
                        taken_at: row.taken_at,
                        hash: row.hash,
                        path: row.path,
                    }
                })
                .collect();
            out.push(DuplicateReviewGroup {
                id: format!("{keep_hash}|{keep_path}"),
                batch_id,
                batch_title,
                batch_detail,
                reclaimable,
                items,
            });
        }
        let reclaimable = out.iter().map(|g| g.reclaimable).sum();
        Ok(DuplicateReview {
            groups: out,
            reclaimable,
        })
    })
}

#[derive(Deserialize)]
struct DuplicateRejection {
    hash: String,
    path: String,
}

/// Apply the exact physical copies a duplicate review rejected.
///
/// A hash alone cannot express this decision because exact duplicates share one by
/// definition. The relative path is accepted only when the index still maps it to the
/// reviewed hash; a Finder move between review and apply makes the plan stale and is
/// refused rather than guessed.
#[tauri::command]
async fn apply_duplicate_review(
    state: tauri::State<'_, AppState>,
    path: String,
    rejections: Vec<DuplicateRejection>,
) -> R<String> {
    with(&state, &path, |lib| {
        if rejections.is_empty() {
            return Ok("Nothing to move".into());
        }
        let rows: BTreeMap<String, blinkview_core::index::FileRow> = lib
            .index
            .all()?
            .into_iter()
            .map(|row| (row.path.clone(), row))
            .collect();
        let mut plan = blinkview_core::Plan::new("duplicate review");
        for rejected in rejections {
            let row = rows.get(&rejected.path).ok_or_else(|| {
                anyhow::anyhow!(
                    "{} moved since it was reviewed; run the review again",
                    rejected.path
                )
            })?;
            if row.hash != rejected.hash {
                anyhow::bail!(
                    "{} changed since it was reviewed; run the review again",
                    rejected.path
                );
            }
            if in_folder(&row.path, TRASH) {
                continue;
            }
            let name = row.path.rsplit('/').next().unwrap_or(&row.path);
            plan.ops.push(blinkview_core::Op::Move {
                hash: row.hash.clone(),
                from: row.path.clone(),
                to: format!("{TRASH}/{name}"),
            });
        }
        if plan.is_empty() {
            return Ok("Nothing to move".into());
        }
        std::fs::create_dir_all(lib.abs(TRASH))?;
        let count = plan.len();
        let journal = plan.apply(lib)?;
        Ok(format!("Moved {count} to Trash · undo id {}", journal.id))
    })
}

fn build_plan(
    lib: &mut Library,
    op: &str,
    param: Option<f32>,
    mkdirs: bool,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> anyhow::Result<blinkview_core::Plan> {
    Ok(match op {
        "dedupe" => {
            dedupe::ensure_signatures_with_progress(lib, progress)?;
            let mut o = dedupe::Options::default();
            if let Some(v) = param {
                o.rmse = v
            }
            if mkdirs {
                std::fs::create_dir_all(lib.abs(&o.dest))?;
            }
            dedupe::plan(lib, &o)?
        }
        "scenery" => {
            let mut o = scenery::Options::default();
            if let Some(v) = param {
                o.max_face = v
            }
            if mkdirs {
                std::fs::create_dir_all(lib.abs(&o.dest))?;
            }
            scenery::plan(lib, &o)?
        }
        "rename" => rename::plan(lib, rename::DEFAULT_FORMAT)?,
        "file" => {
            let people = lib.people()?;
            if mkdirs {
                for n in people.people.iter().map(|p| &p.name) {
                    std::fs::create_dir_all(lib.abs(n))?;
                }
            }
            faces_file::plan(lib, &people, &assign::Options::default())?.plan
        }
        other => anyhow::bail!("unknown operation {other}"),
    })
}

#[tauri::command]
async fn plan_op(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
    op: String,
    param: Option<f32>,
) -> R<PlanView> {
    let sink = emitter(&app, "plan", &path);
    with(&state, &path, |lib| {
        let p = build_plan(lib, &op, param, false, &sink)?;
        Ok(PlanView {
            label: op.clone(),
            moves: p
                .ops
                .iter()
                .map(|o| (o.from().to_string(), o.to().to_string()))
                .collect(),
            skipped: p.skipped.clone(),
        })
    })
}

#[tauri::command]
async fn apply_op(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
    op: String,
    param: Option<f32>,
) -> R<String> {
    let sink = emitter(&app, "apply", &path);
    with(&state, &path, |lib| {
        let p = build_plan(lib, &op, param, true, &sink)?;
        if p.is_empty() {
            return Ok("Nothing to do".into());
        }
        let n = p.len();
        let j = p.apply(lib)?;
        Ok(format!("{n} changes applied · undo id {}", j.id))
    })
}

/// Preview a rename, over a pattern and a scope.
///
/// Both halves matter. The pattern is the user's, so it is validated before it is
/// rendered — an unknown specifier otherwise panics inside chrono. The scope is a list
/// of hashes, or everything when absent; uniqueness is still checked library-wide, so
/// renaming twelve selected files can never collide with a name another folder holds.
#[tauri::command]
async fn plan_rename(
    state: tauri::State<'_, AppState>,
    path: String,
    format: String,
    hashes: Option<Vec<String>>,
) -> R<PlanView> {
    with(&state, &path, |lib| {
        let p = rename::plan_scoped(lib, &format, hashes.as_deref())?;
        Ok(PlanView {
            label: "rename".into(),
            moves: p
                .ops
                .iter()
                .map(|o| (o.from().to_string(), o.to().to_string()))
                .collect(),
            skipped: p.skipped.clone(),
        })
    })
}

#[tauri::command]
async fn apply_rename(
    state: tauri::State<'_, AppState>,
    path: String,
    format: String,
    hashes: Option<Vec<String>>,
) -> R<String> {
    with(&state, &path, |lib| {
        let p = rename::plan_scoped(lib, &format, hashes.as_deref())?;
        if p.is_empty() {
            return Ok("Nothing to rename.".into());
        }
        let n = p.len();
        let skipped = p.skipped.len();
        let j = p.apply(lib)?;
        Ok(match skipped {
            0 => format!("{n} renamed · undo id {}", j.id),
            k => format!("{n} renamed · {k} left alone · undo id {}", j.id),
        })
    })
}

#[tauri::command]
async fn history(state: tauri::State<'_, AppState>, path: String) -> R<Vec<(String, usize)>> {
    with(&state, &path, |lib| {
        let mut out = Vec::new();
        for id in Journal::list(lib)? {
            let j = Journal::load(lib, &id)?;
            out.push((j.id, j.ops.len()));
        }
        out.reverse();
        Ok(out)
    })
}

#[tauri::command]
async fn undo(state: tauri::State<'_, AppState>, path: String, id: Option<String>) -> R<String> {
    with(&state, &path, |lib| {
        let ids = Journal::list(lib)?;
        let target = id
            .or_else(|| ids.last().cloned())
            .ok_or_else(|| anyhow::anyhow!("nothing to undo"))?;
        let j = Journal::load(lib, &target)?;
        let n = j.undo(lib)?;
        Ok(format!("Reversed {n} changes"))
    })
}

/// Serve photos and thumbnails over a `photo://` scheme.
///
/// Tauri's built-in `asset:` protocol is gated by a glob scope that would have to be
/// effectively unrestricted here, since a source can be any folder the user picks.
/// Registering our own scheme makes the boundary explicit and auditable: a request is
/// served only if it resolves inside a folder the user has actually added.
/// Point `blinkview-core` at the ffmpeg bundled beside this executable.
///
/// Also called, over HTTP, by the remote bridge (ADR-0021) — same function, so the
/// boundary cannot drift between the window and a paired device.
///
/// Tauri's `externalBin` places `ffmpeg-<target-triple>` next to the binary — inside
/// `Contents/MacOS` on macOS — and strips the triple at install time, so at runtime it
/// is simply `ffmpeg`. Core cannot ask Tauri where that is without depending on Tauri,
/// and it is also used by the CLI, which has no bundle; an environment variable is the
/// whole of the coupling (ADR-0014).
///
/// Nothing here is fatal. A build without the sidecar falls back to PATH exactly as
/// before, which is what a `cargo run` during development does.
fn export_bundled_ffmpeg(app: &tauri::App) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(dir) = exe.parent() else { return };
    let name = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    let beside = dir.join(name);
    let candidate = if beside.is_file() {
        beside
    } else {
        // Development: `cargo run` leaves the sidecar where the build script put it.
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("binaries");
        let triple = src.join(format!("{name}-{}", std::env::consts::ARCH));
        let plain = src.join(name);
        if plain.is_file() {
            plain
        } else if triple.is_file() {
            triple
        } else {
            return;
        }
    };
    let _ = app;
    unsafe { std::env::set_var(blinkview_core::thumbs::FFMPEG_ENV, &candidate) };
}

/// Bytes budget for the thumbnail cache.
///
/// ~2,000 thumbnails at the measured ~33 KB average, so an afternoon of browsing
/// fits; a photograph-sized entry (~400 KB previews at worst) still leaves hundreds
/// of slots. On top of a webview that was measured holding gigabytes, 64 MB is noise.
const THUMB_CACHE_BUDGET: usize = 64 * 1024 * 1024;

/// A byte-budgeted LRU over small derived files — thumbnails, previews.
///
/// WKWebView cannot be relied on to cache custom-scheme responses, and grid cells are
/// destroyed offscreen and rebuilt on re-entry, so every scroll-back re-requested each
/// thumbnail and paid the full handler round trip: the flicker, and the slow scroll.
/// Thumbnails are content-addressed and never change, so there is nothing to
/// invalidate; an entry leaves only when the budget needs the room.
struct ThumbCache {
    map: std::collections::HashMap<std::path::PathBuf, (Vec<u8>, u64)>,
    bytes: usize,
    clock: u64,
    budget: usize,
}

impl ThumbCache {
    fn new(budget: usize) -> Self {
        Self {
            map: std::collections::HashMap::new(),
            bytes: 0,
            clock: 0,
            budget,
        }
    }

    /// The bytes for `path`, if cached. Reading counts as a use: the entry stays.
    fn get(&mut self, path: &std::path::Path) -> Option<&[u8]> {
        self.map.get_mut(path).map(|(bytes, stamp)| {
            self.clock += 1;
            *stamp = self.clock;
            bytes.as_slice()
        })
    }

    /// Cache `bytes`, evicting the least recently used entries to stay in budget.
    /// An entry larger than the whole budget is refused: it would evict everything
    /// and still not fit.
    fn put(&mut self, path: &std::path::Path, bytes: Vec<u8>) {
        let len = bytes.len();
        if len > self.budget {
            return;
        }
        self.clock += 1;
        if let Some((old, _)) = self.map.insert(path.to_path_buf(), (bytes, self.clock)) {
            self.bytes -= old.len();
        }
        self.bytes += len;
        while self.bytes > self.budget {
            // O(n) over a few hundred entries, a handful of times per minute of
            // browsing — cheaper than a dependency for a true O(1) list.
            let victim = self
                .map
                .iter()
                .min_by_key(|(_, (_, stamp))| *stamp)
                .map(|(path, _)| path.clone());
            let Some(victim) = victim else { break };
            if let Some((old, _)) = self.map.remove(&victim) {
                self.bytes -= old.len();
            }
        }
    }
}

static THUMB_CACHE: std::sync::LazyLock<std::sync::Mutex<ThumbCache>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(ThumbCache::new(THUMB_CACHE_BUDGET)));

/// Serve a small derived file through [`THUMB_CACHE`].
///
/// A cache hit never touches the filesystem; a miss reads once and remembers. The
/// eviction victim has the oldest use, so what a viewer keeps scrolling back to is
/// exactly what stays fast.
fn serve_cached(path: &std::path::Path) -> Option<http::Response<Vec<u8>>> {
    let mut cache = THUMB_CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(bytes) = cache.get(path) {
        return Some(ok_response(bytes.to_vec(), path));
    }
    let bytes = std::fs::read(path).ok()?;
    cache.put(path, bytes.clone());
    Some(ok_response(bytes, path))
}

/// The value of one `?key=` parameter of a request URI, percent-decoding left to the
/// path handling (hashes are hex, so they never arrive encoded).
fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    query.and_then(|q| {
        q.split('&')
            .find_map(|kv| kv.strip_prefix(key).map(str::to_string))
    })
}

#[derive(Clone)]
struct MediaScope {
    vault: std::path::PathBuf,
}

/// Resolve the narrow filesystem grant behind `photo://`.
///
/// Added sources grant their tree, as before. A peek grants only files whose direct
/// parent is the peeked folder; `starts_with` here would expose every subfolder even
/// though a peek promises not to recurse.
fn media_scope(app: &tauri::AppHandle, canon: &std::path::Path) -> Option<MediaScope> {
    for source in load_source_entries(app) {
        let Ok(root) = std::path::Path::new(source.path()).canonicalize() else {
            continue;
        };
        if canon.starts_with(&root) {
            return Some(MediaScope {
                vault: blinkview_core::cache::vault_for(&root),
            });
        }
    }

    let state = app.try_state::<AppState>()?;
    let peeks = state.peeks.lock().ok()?;
    for peek in peeks.values() {
        let lib = peek.lock().ok()?;
        if peek_grants(lib.root(), canon) {
            return Some(MediaScope {
                vault: lib.vault().to_path_buf(),
            });
        }
    }
    None
}

fn peek_grants(root: &std::path::Path, candidate: &std::path::Path) -> bool {
    candidate.parent() == Some(root)
}

/// Whether answering this request means spawning ffmpeg — a video's poster that does
/// not exist yet.
///
/// Routing happens before any pool is chosen so the slow case never occupies a
/// photograph-decode thread. Hashes are content-addressed, so "no source has this
/// poster" is a genuine miss; the render itself still happens inside `serve_photo`,
/// which re-checks existence and would simply serve the file if we guessed wrong.
fn needs_video_render(app: &tauri::AppHandle, request: &http::Request<Vec<u8>>) -> bool {
    let Ok(decoded) = percent_decode(request.uri().path()) else {
        return false;
    };
    let Ok(path) = std::path::Path::new(&decoded).canonicalize() else {
        return false;
    };
    let hash = query_param(request.uri().query(), "t=");
    let thumb = hash.as_deref().and_then(|hash| {
        media_scope(app, &path).map(|scope| thumbs::thumb_path_in(&scope.vault, hash))
    });
    video_thumb_miss(&path, hash.as_deref(), thumb.as_deref())
}

fn video_thumb_miss(
    path: &std::path::Path,
    hash: Option<&str>,
    thumb: Option<&std::path::Path>,
) -> bool {
    let is_video = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "mp4" | "mov" | "m4v"));
    if !is_video {
        return false;
    }
    hash.is_some() && thumb.is_some_and(|path| !path.exists())
}

pub(crate) fn serve_photo(
    app: &tauri::AppHandle,
    request: http::Request<Vec<u8>>,
) -> http::Response<Vec<u8>> {
    let deny = |code: u16| {
        http::Response::builder()
            .status(code)
            .header("Access-Control-Allow-Origin", "*")
            .body(Vec::new())
            .unwrap()
    };
    // The URI path *is* the absolute filesystem path — "photo://localhost/Users/..."
    // yields "/Users/...". Stripping the leading slash would make it relative and
    // nothing would ever resolve.
    let Ok(decoded) = percent_decode(request.uri().path()) else {
        return deny(400);
    };
    let path = std::path::PathBuf::from(&decoded);
    // `?t=<hash>` asks for the thumbnail of this photo. Serving it from cache when
    // present and rendering it on demand when not is what makes the grid usable
    // immediately on a large library: the virtualised viewport only ever requests the
    // few dozen images actually on screen, so thumbnails are produced in view order
    // instead of by a pre-pass that has to finish before anything is visible.
    let param = |k: &str| query_param(request.uri().query(), k);
    let thumb_hash = param("t=");
    // `?full=<hash>` asks for the full-size image. HEIC is the reason this exists:
    // WKWebView cannot decode it (verified), so it is transcoded once and cached
    // rather than converted on every view.
    let full_hash = param("full=");
    // `?preview=<hash>` asks for the lightbox preview — a derived 2000 px JPEG.
    let preview_hash = param("preview=");

    let Ok(canon) = path.canonicalize() else {
        return deny(404);
    };
    let Some(scope) = media_scope(app, &canon) else {
        return deny(403);
    };

    // The lightbox preview: a derived 2000 px JPEG, rendered on first request. This is
    // what makes the stepper quick — a step used to decode the full original every
    // time, and a 12–48 MP decode per keypress is a wait, not a step.
    if let Some(hash) = preview_hash.as_deref() {
        let p = blinkview_core::thumbs::preview_path_in(&scope.vault, hash);
        if !p.exists() {
            match blinkview_core::thumbs::render_preview(&canon, &p) {
                Ok(true) => {}
                // The source is small enough to be its own preview.
                Ok(false) => {
                    return match std::fs::read(&canon) {
                        Ok(b) => ok_response(b, &canon),
                        Err(_) => deny(404),
                    };
                }
                Err(_) => return deny(500),
            }
        }
        return match serve_cached(&p) {
            Some(res) => res,
            None => deny(404),
        };
    }

    // Full-size request for a format the webview cannot decode: serve a cached JPEG.
    if thumb_hash.is_none() && blinkview_core::imageio::needs_conversion(&canon) {
        if let Some(hash) = full_hash {
            let derived = scope.vault.join("derived").join(format!("{hash}.jpg"));
            if !derived.exists()
                && blinkview_core::imageio::convert_to_jpeg(&canon, &derived).is_err()
            {
                return deny(500);
            }
            return match std::fs::read(&derived) {
                Ok(b) => ok_response(b, &derived),
                Err(_) => deny(404),
            };
        }
    }

    // Resolve to a thumbnail path, rendering it if this is the first request.
    let serve = match &thumb_hash {
        None => canon.clone(),
        Some(hash) => {
            {
                let t = blinkview_core::thumbs::thumb_path_in(&scope.vault, hash);
                if !t.exists() {
                    let is_video = canon.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                        matches!(e.to_ascii_lowercase().as_str(), "mp4" | "mov" | "m4v")
                    });
                    if blinkview_core::thumbs::render_to(&canon, &t, is_video).is_err() {
                        // Falling back to the original is only sane for a still.
                        // A video's "thumbnail" would be the whole clip — read
                        // into memory in one piece, handed to an <img>, and held
                        // there by the webview. On a phone backup with 507 clips
                        // averaging 32 MB that is 15.7 GB of video in the render
                        // process, which macOS answers by killing it: the window
                        // goes black, reloads, asks for the same thumbnails and
                        // does it again. A video with no poster frame is the
                        // documented degradation, so serve nothing and let the
                        // cell keep its play badge.
                        if is_video {
                            return deny(404);
                        }
                        return match std::fs::read(&canon) {
                            Ok(b) => ok_response(b, &canon),
                            Err(_) => deny(404),
                        };
                    }
                }
                t
            }
        }
    };

    // A thumbnail comes from an <img>, which streams nothing: there is no Range
    // header to honour, and the bytes are worth keeping in RAM (see ThumbCache).
    if thumb_hash.is_some() {
        return match serve_cached(&serve) {
            Some(res) => res,
            None => deny(404),
        };
    }

    // Videos are streamed, not swallowed. A range request reads only the slice asked
    // for; without this a 500MB clip had to be read into memory and handed over whole
    // before playback could begin, and every seek did it again.
    let range = request
        .headers()
        .get("Range")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    match serve_file(&serve, range.as_deref()) {
        Some(res) => res,
        None => deny(404),
    }
}

/// The most a single response will carry.
///
/// A player asking for "bytes=0-" means "whatever you have"; answering with an entire
/// film would put it back in memory, which is the thing being fixed.
const RANGE_CHUNK: u64 = 4 * 1024 * 1024;

/// Serve a file, honouring a `Range` header.
fn serve_file(path: &std::path::Path, range: Option<&str>) -> Option<http::Response<Vec<u8>>> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();

    let Some((start, end)) = range.and_then(|r| parse_range(r, len)) else {
        // No range asked for. Small files go whole; anything large still advertises
        // that it can be seeked, so the player will come back with ranges.
        let mut buf = Vec::with_capacity(len as usize);
        file.read_to_end(&mut buf).ok()?;
        let mut res = ok_response(buf, path);
        res.headers_mut()
            .insert("Accept-Ranges", "bytes".parse().ok()?);
        return Some(res);
    };

    let end = end
        .min(start.saturating_add(RANGE_CHUNK - 1))
        .min(len.saturating_sub(1));
    let n = end.saturating_sub(start) + 1;
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = vec![0u8; n as usize];
    file.read_exact(&mut buf).ok()?;

    let mut res = ok_response(buf, path);
    *res.status_mut() = http::StatusCode::PARTIAL_CONTENT;
    let h = res.headers_mut();
    h.insert("Accept-Ranges", "bytes".parse().ok()?);
    h.insert(
        "Content-Range",
        format!("bytes {start}-{end}/{len}").parse().ok()?,
    );
    // A partial response must not be cached as if it were the whole file.
    h.insert("Cache-Control", "no-store".parse().ok()?);
    Some(res)
}

/// `bytes=START-END`, either end optional. Suffix ranges (`bytes=-500`) included.
fn parse_range(header: &str, len: u64) -> Option<(u64, u64)> {
    let spec = header.trim().strip_prefix("bytes=")?;
    // Multiple ranges are legal but no player we serve asks for them; one is enough.
    let (a, b) = spec.split_once('-')?;
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        if n == 0 || len == 0 {
            return None;
        }
        return Some((len.saturating_sub(n), len - 1));
    }
    let start: u64 = a.parse().ok()?;
    if start >= len {
        return None;
    }
    let end = if b.is_empty() {
        len - 1
    } else {
        b.parse::<u64>().ok()?.min(len - 1)
    };
    (start <= end).then_some((start, end))
}

fn ok_response(bytes: Vec<u8>, path: &std::path::Path) -> http::Response<Vec<u8>> {
    let mime = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("mp4") => "video/mp4",
        Some("mov") => "video/quicktime",
        _ => "image/jpeg",
    };
    http::Response::builder()
        .status(200)
        .header("Content-Type", mime)
        .header("Cache-Control", "max-age=31536000")
        .header("Access-Control-Allow-Origin", "*")
        // Without this the range headers are invisible to anything using fetch, which
        // makes a streaming problem impossible to diagnose from the page.
        .header(
            "Access-Control-Expose-Headers",
            "Content-Range, Accept-Ranges, Content-Length",
        )
        .body(bytes)
        .unwrap()
}

pub(crate) fn percent_decode(s: &str) -> Result<String, ()> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).map_err(|_| ())?;
            out.push(u8::from_str_radix(hex, 16).map_err(|_| ())?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| ())
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
fn queue_open_urls(app: &tauri::AppHandle, urls: Vec<tauri::Url>) {
    let paths: Vec<String> = urls
        .into_iter()
        .filter_map(|url| url.to_file_path().ok())
        .filter(|path| path.is_dir() || (path.is_file() && scan::kind_of(path).is_some()))
        .map(|path| path.display().to_string())
        .collect();
    if paths.is_empty() {
        return;
    }
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(mut pending) = state.pending_open.lock() {
            pending.extend(paths.iter().cloned());
        }
    }
    for path in paths {
        remote::emit_all(app, "open-path", &path);
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // Asynchronous, not the synchronous variant: `serve_photo` decodes and resizes
        // a full-size photograph when a thumbnail is not cached yet, and on the
        // synchronous protocol that work happens on the UI thread — which is why
        // scrolling crawled while thumbnails were being produced and was fine once
        // they existed. Served on a pool instead, the grid scrolls at full speed while
        // the work happens behind it.
        .setup(|app| {
            export_bundled_ffmpeg(app);
            // Kept so a background scan can report progress against its own source.
            if let Some(state) = app.try_state::<AppState>() {
                if let Ok(mut slot) = state.app.lock() {
                    *slot = Some(app.handle().clone());
                }
            }
            remote::autostart_if_asked(app.handle());
            Ok(())
        })
        .register_asynchronous_uri_scheme_protocol("photo", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            // A video poster that does not exist yet means an ffmpeg spawn — measured
            // at up to ~140 MB and a third of a second apiece. Those go to their own
            // two threads so a burst of them can never occupy the photograph decoders.
            if needs_video_render(&app, &request) {
                VIDEO_POOL.spawn(move || responder.respond(serve_photo(&app, request)));
            } else {
                IMAGE_POOL.spawn(move || responder.respond(serve_photo(&app, request)));
            }
        });

    // UI verification bridge. Behind the `ui-bridge` feature and additionally gated on
    // a debug build, so a release binary never exposes a WebSocket server.
    #[cfg(all(feature = "ui-bridge", debug_assertions))]
    {
        builder = builder.plugin(tauri_plugin_mcp_bridge::init());
    }

    builder
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            #[cfg(debug_assertions)]
            bench_payload,
            list_sources,
            add_source,
            remove_source,
            rescan,
            create_folder,
            set_source_depth,
            survey_folder,
            cancel_survey,
            peek_folder,
            peek_photos,
            end_peek,
            promote_peek,
            open_path,
            take_open_paths,
            photos,
            build_thumbs,
            analyze_faces,
            clusters,
            name_clusters,
            plan_op,
            apply_op,
            plan_rename,
            apply_rename,
            history,
            undo,
            delete_photos,
            rename_photo,
            untag_person,
            restore_photos,
            empty_trash,
            models_status,
            models_fetch,
            people_overview,
            name_cluster,
            cluster_photos,
            autodetect_faces,
            dismiss_cluster,
            restore_dismissed,
            merge_people,
            edit_photo,
            edit_photos,
            strip_metadata,
            set_rating,
            set_label,
            set_album,
            list_albums,
            photo_detail,
            semantic_status,
            semantic_index,
            semantic_search,
            locate_photos,
            photo_places,
            place_search,
            set_photo_location,
            set_photo_datetime,
            plan_album_migration,
            apply_album_migration,
            list_searches,
            save_search,
            folder_view,
            set_folder_view,
            plan_move,
            apply_move,
            duplicate_review,
            apply_duplicate_review,
            share_photos,
            start_file_drag,
            check_for_updates,
            open_update,
            forget_person,
            analyze_all,
            source_data,
            pending_work,
            analyze_resume,
            remote::remote_start,
            remote::remote_stop,
            remote::remote_status
        ])
        .build(tauri::generate_context!())
        .expect("error while building blinkview")
        .run(|app, event| {
            #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
            if let tauri::RunEvent::Opened { urls } = event {
                queue_open_urls(app, urls);
            }
            #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
            let _ = (app, event);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cache beside the fixture, so a unit test never writes to the machine's.
    fn cache_for(dir: &std::path::Path) -> std::path::PathBuf {
        dir.parent().unwrap().join(format!(
            "{}-cache",
            dir.file_name().unwrap().to_string_lossy()
        ))
    }

    #[test]
    fn legacy_source_entries_stay_recursive_until_edited() {
        let file: SourcesFile = serde_json::from_str(r#"{"sources":["/Photos"]}"#).unwrap();
        assert_eq!(file.sources, vec![SourceEntry::Legacy("/Photos".into())]);
        assert!(!file.sources[0].shallow());
        assert!(!file.sources[0].skips_default_dirs());

        let current: SourcesFile =
            serde_json::from_str(r#"{"sources":[{"path":"/Desktop","shallow":true}]}"#).unwrap();
        assert!(current.sources[0].shallow());
        assert!(current.sources[0].skips_default_dirs());
        assert_eq!(
            serde_json::to_value(&current).unwrap(),
            serde_json::json!({"sources":[{"path":"/Desktop","shallow":true}]})
        );
    }

    #[test]
    fn a_peek_grants_its_direct_files_but_never_its_subtree() {
        let root = std::path::Path::new("/Desktop/Trip");
        assert!(peek_grants(
            root,
            std::path::Path::new("/Desktop/Trip/a.jpg")
        ));
        assert!(!peek_grants(
            root,
            std::path::Path::new("/Desktop/Trip/Private/a.jpg")
        ));
        assert!(!peek_grants(root, std::path::Path::new("/Desktop/a.jpg")));
    }

    /// Opening a photograph that already lives in an added source must route to that
    /// library, keyed by the *stored* path, positioned on the file.
    #[test]
    fn open_routing_prefers_the_owning_source() {
        let dir = std::env::temp_dir().join(format!("blinkview-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let nested = dir.join("Photos").join("Trip");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("a.jpg"), b"one").unwrap();
        let entries = vec![SourceEntry::persisted(
            dir.join("Photos").display().to_string(),
            false,
        )];

        let file = nested.join("a.jpg").canonicalize().unwrap();
        let (stored, file_rel, folder) = owning_source(&file, &entries).unwrap();
        assert_eq!(
            stored,
            entries[0].path(),
            "the stored key, not a resolved copy"
        );
        assert_eq!(file_rel.as_deref(), Some("Trip/a.jpg"));
        assert_eq!(folder, None);

        // The source root itself, and a folder inside it, open the library too.
        let root = dir.join("Photos").canonicalize().unwrap();
        assert_eq!(
            owning_source(&root, &entries),
            Some((entries[0].path().to_string(), None, None))
        );
        let sub = nested.canonicalize().unwrap();
        assert_eq!(
            owning_source(&sub, &entries),
            Some((entries[0].path().to_string(), None, Some("Trip".into())))
        );

        // Unrelated paths own nothing, so they become peeks.
        let outside = dir.join("Elsewhere");
        std::fs::create_dir_all(&outside).unwrap();
        assert_eq!(
            owning_source(&outside.canonicalize().unwrap(), &entries),
            None
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The symlink case the stored-key rule exists for: opening through the resolved
    /// path must still answer with the path the library was added under.
    #[test]
    fn open_routing_through_a_symlink_keeps_the_stored_key() {
        let dir = std::env::temp_dir().join(format!("blinkview-open-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let real = dir.join("real");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("a.jpg"), b"one").unwrap();
        let link = dir.join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        #[cfg(not(unix))]
        return;
        let entries = vec![SourceEntry::persisted(link.display().to_string(), false)];
        let through_real = real.join("a.jpg").canonicalize().unwrap();
        let (stored, file_rel, _) = owning_source(&through_real, &entries).unwrap();
        assert_eq!(stored, link.display().to_string());
        assert_eq!(file_rel.as_deref(), Some("a.jpg"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ranges_are_parsed_the_way_a_player_sends_them() {
        // The common opening move: "send me what you have from the start".
        assert_eq!(parse_range("bytes=0-", 1000), Some((0, 999)));
        assert_eq!(parse_range("bytes=100-199", 1000), Some((100, 199)));
        // A seek near the end.
        assert_eq!(parse_range("bytes=900-", 1000), Some((900, 999)));
        // Suffix range: the last 500 bytes, used to read an MP4 trailer atom.
        assert_eq!(parse_range("bytes=-500", 1000), Some((500, 999)));
        // Past the end must be refused, not clamped into a bogus slice.
        assert_eq!(parse_range("bytes=1000-", 1000), None);
        assert_eq!(parse_range("bytes=5000-6000", 1000), None);
        // An end beyond the file is clamped, which is legal and expected.
        assert_eq!(parse_range("bytes=990-5000", 1000), Some((990, 999)));
        // Nonsense is refused rather than guessed at.
        assert_eq!(parse_range("chunks=0-10", 1000), None);
        assert_eq!(parse_range("bytes=abc", 1000), None);
        assert_eq!(parse_range("bytes=50-10", 1000), None);
    }

    #[test]
    fn a_folder_contains_everything_beneath_it() {
        assert!(in_folder("Trip/a.jpg", "Trip"));
        assert!(in_folder("Trip/Greece Day3/a.jpg", "Trip"));
        assert!(in_folder("Trip/Greece Day3/a.jpg", "Trip/Greece Day3"));
        // The library root holds everything.
        assert!(in_folder("a.jpg", ""));
        assert!(in_folder("Trip/a.jpg", ""));
    }

    #[test]
    fn a_folder_does_not_contain_its_name_prefixed_siblings() {
        // The bug this guards: a plain starts_with would put Trip2 inside Trip.
        assert!(!in_folder("Trip2/a.jpg", "Trip"));
        assert!(!in_folder("Tripoli/a.jpg", "Trip"));
        // A photograph in the parent is not in the child.
        assert!(!in_folder("Trip/a.jpg", "Trip/Greece"));
        // The folder itself is not a photograph in it.
        assert!(!in_folder("Trip", "Trip"));
    }

    #[test]
    fn ancestors_walk_to_the_root() {
        assert_eq!(
            ancestors("Trip/Greece Day3"),
            vec!["Trip".to_string(), String::new()]
        );
        assert_eq!(ancestors("Trip"), vec![String::new()]);
        // The root has no ancestors, and must not report itself as one or counts
        // would be doubled at the top of the tree.
        assert!(ancestors("").is_empty());
    }

    fn cache(budget: usize) -> ThumbCache {
        ThumbCache::new(budget)
    }

    #[test]
    fn a_cached_entry_serves_without_the_filesystem() {
        let mut c = cache(1000);
        c.put(std::path::Path::new("/t/a.jpg"), vec![1, 2, 3]);
        assert_eq!(
            c.get(std::path::Path::new("/t/a.jpg")),
            Some(&[1, 2, 3][..])
        );
        assert_eq!(c.bytes, 3);
    }

    #[test]
    fn the_least_recently_used_entry_is_evicted_first() {
        let mut c = cache(10);
        let p = |n: &str| std::path::Path::new(n).to_path_buf();
        c.put(&p("/a"), vec![0; 4]);
        c.put(&p("/b"), vec![0; 4]);
        c.put(&p("/c"), vec![0; 4]); // 12 bytes > 10: one must go
                                     // The oldest stamp is /a, which was never re-read.
        assert!(c.get(&p("/a")).is_none(), "/a should have been evicted");
        assert!(c.get(&p("/b")).is_some());
        assert!(c.get(&p("/c")).is_some());
        assert!(c.bytes <= 10, "{} bytes in a 10-byte cache", c.bytes);
    }

    #[test]
    fn a_read_refreshes_recency_so_scrolled_back_rows_stay() {
        let mut c = cache(10);
        let p = |n: &str| std::path::Path::new(n).to_path_buf();
        c.put(&p("/a"), vec![0; 4]);
        c.put(&p("/b"), vec![0; 4]);
        // The user scrolls back to /a: it is the newest use now.
        assert!(c.get(&p("/a")).is_some());
        c.put(&p("/c"), vec![0; 4]); // 12 > 10: evict, but not /a
        assert!(c.get(&p("/a")).is_some(), "just-read /a must survive");
        assert!(c.get(&p("/b")).is_none());
    }

    #[test]
    fn an_entry_bigger_than_the_budget_is_refused() {
        let mut c = cache(10);
        c.put(std::path::Path::new("/big"), vec![0; 100]);
        assert!(c.get(std::path::Path::new("/big")).is_none());
        assert_eq!(c.bytes, 0);
    }

    #[test]
    fn reinserting_replaces_without_leaking_budget() {
        let mut c = cache(100);
        let p = std::path::Path::new("/a");
        c.put(p, vec![0; 60]);
        c.put(p, vec![0; 60]); // replace, not accumulate
        assert_eq!(c.bytes, 60, "the old bytes must be released");
    }

    #[test]
    fn a_source_cannot_be_added_twice_or_nested() {
        let dir = std::env::temp_dir().join(format!("blinkview-conflict-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let parent = dir.join("Backup");
        let child = parent.join("day1");
        std::fs::create_dir_all(&child).unwrap();
        let sources = vec![parent.display().to_string()];

        // Exact re-add: an answer, not a silent no-op.
        let dup = source_conflict(&parent, &sources).unwrap();
        assert!(dup.contains("already in your library"), "{dup}");
        // A subfolder of a source would become a second library over the same photos.
        let inside = source_conflict(&child, &sources).unwrap();
        assert!(inside.contains("inside your source"), "{inside}");
        // …and the other direction: adding the parent over an existing source.
        let around = source_conflict(&parent, &[child.display().to_string()]).unwrap();
        assert!(around.contains("already contains your source"), "{around}");

        // Unrelated folders are fine, and a name-prefixed sibling is not "inside":
        // /Pics2 does not live in /Pics.
        let other = dir.join("Elsewhere");
        let sibling = dir.join("Backup2");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        assert_eq!(source_conflict(&other, &sources), None);
        assert_eq!(source_conflict(&sibling, &sources), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The dangerous case the string compare could not see: two paths, one folder.
    /// Two libraries over one vault would scan the same SQLite concurrently, because
    /// the open gate is keyed by the path string.
    #[test]
    fn the_same_folder_through_a_symlink_is_still_a_duplicate() {
        let dir =
            std::env::temp_dir().join(format!("blinkview-conflict-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let real = dir.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = dir.join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        #[cfg(not(unix))]
        std::fs::copy(&real, &link).unwrap();
        let msg = source_conflict(&link, &[real.display().to_string()]).unwrap();
        assert!(msg.contains("already in your library"), "{msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The number beside a folder is what you can still see in it.
    ///
    /// Deleting moves a photograph into `Trash/`, where it stays indexed — so a total
    /// taken over every row does not move when you delete, and the sidebar then
    /// disagrees with a grid that hides trashed photographs.
    #[test]
    fn trashed_photographs_are_counted_by_the_trash_row_only() {
        let dir = std::env::temp_dir().join(format!("blinkview-counts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Day1")).unwrap();
        std::fs::create_dir_all(dir.join(TRASH)).unwrap();
        // Indexing is by extension, so the bytes only have to differ: each file needs
        // its own content hash to become its own row.
        for (rel, bytes) in [
            ("a.jpg", b"one".as_slice()),
            ("Day1/b.jpg", b"two".as_slice()),
            ("Trash/c.jpg", b"three".as_slice()),
            ("Trash/d.mp4", b"four".as_slice()),
        ] {
            std::fs::write(dir.join(rel), bytes).unwrap();
        }
        let mut lib = Library::open_in(&dir, cache_for(&dir)).unwrap();
        scan::scan(&mut lib, false).unwrap();
        let info = describe(&mut lib).unwrap();

        assert_eq!(
            info.photos, 2,
            "a trashed photograph is not in the library total"
        );
        assert_eq!(info.videos, 0, "a trashed clip is not in the library total");
        let count = |p: &str| info.folders.iter().find(|f| f.path == p).map(|f| f.count);
        assert_eq!(
            count(TRASH),
            Some(2),
            "the Trash row counts what is in the Trash"
        );
        assert_eq!(
            count(""),
            Some(2),
            "the library root does not roll up its Trash"
        );
        assert_eq!(count("Day1"), Some(1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn emptying_the_trash_moves_the_file_and_removes_the_original() {
        let dir = std::env::temp_dir().join(format!("blinkview-sys-trash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let from = dir.join("a.jpg");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&from, b"photo").unwrap();
        // A differently-named parent stands in for ~/.Trash; the same-volume path is
        // the rename, and the cross-volume fallback needs a second filesystem, which
        // is verified against a real external drive instead of here.
        let dest = dir.join("system").join("a.jpg");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        move_to_system_trash(&from, &dest).unwrap();
        assert!(
            !from.exists(),
            "the original must not linger in the library Trash"
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"photo");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_video_owed_a_poster_is_routed_to_ffmpeg() {
        let dir = std::env::temp_dir().join(format!("blinkview-route-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("clip.mp4");
        let still = dir.join("shot.jpg");
        let t = blinkview_core::thumbs::thumb_path_at(&dir, "abc");

        // A video with no poster anywhere is the slow case.
        assert!(video_thumb_miss(&clip, Some("abc"), Some(&t)));
        // Write its poster under the source: no longer a miss.
        std::fs::create_dir_all(t.parent().unwrap()).unwrap();
        std::fs::write(&t, b"poster").unwrap();
        assert!(!video_thumb_miss(&clip, Some("abc"), Some(&t)));
        // Stills never route to ffmpeg; neither does a video without a thumb request.
        assert!(!video_thumb_miss(&still, Some("abc"), Some(&t)));
        assert!(!video_thumb_miss(&clip, None, Some(&t)));
        // The routing helper resolves through the machine's cache root; take back what
        // the test just put there, or a unit test litters the real thing.
        blinkview_core::cache::forget(&dir);
        let _ = std::fs::remove_dir_all(t.ancestors().nth(2).unwrap());
        let _ = std::fs::remove_file(dir.join(blinkview_core::cache::MARKER));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn capture_time_input_is_strict_but_accepts_picker_precision() {
        let with_seconds = parse_capture_datetime("2026-08-19T14:03:27").unwrap();
        assert_eq!(
            with_seconds.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2026-08-19 14:03:27"
        );
        let minute = parse_capture_datetime("2026-08-19T14:03").unwrap();
        assert_eq!(minute.format("%S").to_string(), "00");
        assert!(parse_capture_datetime("next Tuesday").is_err());
        assert!(parse_capture_datetime("1899-12-31T23:59").is_err());
    }

    #[test]
    fn duplicate_batches_are_days_or_conservative_trip_weeks() {
        let at = chrono::DateTime::parse_from_rfc3339("2026-08-19T14:03:27Z")
            .unwrap()
            .timestamp();
        let (id, title, detail) = duplicate_batch(Some(at), None);
        assert_eq!(id, "day:2026-08-19");
        assert_eq!(title, "19 August");
        assert!(detail.contains("2026"));

        let (id, title, _) = duplicate_batch(Some(at), Some("Kyoto, Japan".into()));
        assert!(id.starts_with("trip:2026-34:"), "{id}");
        assert_eq!(title, "Trip · Kyoto, Japan");
    }

    #[test]
    fn only_this_repositories_release_tags_can_be_opened() {
        assert!(release_url_is_safe(
            "https://github.com/notdefined-inc/blinkview/releases/tag/v0.1.0"
        ));
        assert!(!release_url_is_safe(
            "https://example.com/releases/tag/v0.1.0"
        ));
        assert!(!release_url_is_safe(
            "https://github.com/notdefined-inc/blinkview/releases/../../other"
        ));
        assert!(!release_url_is_safe(
            "https://github.com/notdefined-inc/blinkview/releases/tag/v0.1.0?download=1"
        ));
    }

    /// A folder added through a symlink is shown resolved, so removing it by the name
    /// the UI knows has to still find the entry the list holds.
    #[test]
    fn a_source_is_removed_under_either_name() {
        let dir = std::env::temp_dir().join(format!("blinkview-rm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let stored = dir.to_string_lossy().to_string();
        let shown = dir.canonicalize().unwrap().to_string_lossy().to_string();
        assert!(same_source(&stored, &shown));
        assert!(same_source(&stored, &stored));
        assert!(!same_source(&stored, "/nowhere/else"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Switching library must reload an open map, not only blank it. The first attempt
    /// cleared the points and left the map reading "Loading locations…" for good.
    #[test]
    fn source_switch_reloads_an_open_map() {
        let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../dist/app.js"))
            .unwrap();
        let body = src
            .split_once("async function selectSource(")
            .expect("selectSource")
            .1
            .split_once("\nasync function loadMapData(")
            .expect("loadMapData follows selectSource")
            .0;
        assert!(
            body.contains("MAP.points = [];"),
            "selectSource must drop the previous library's points"
        );
        assert!(
            body.contains("if (!$(\"#mapview\").hidden) loadMapData();"),
            "selectSource must reload the map for the new library"
        );
        // A slow response from a library the user has already left must be discarded;
        // comparing `S.source` alone cannot catch a switch away and back again.
        assert!(src.contains("if (MAP.request !== request || S.source !== source) return;"));
    }

    /// The installed app compares itself against GitHub using the version in
    /// `tauri.conf.json`, not the workspace `Cargo.toml`. Comparing against the
    /// workspace version made the latest release look newer than itself.
    #[test]
    fn update_check_uses_the_release_version_not_the_workspace_version() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(&*APP_VERSION, conf["version"].as_str().unwrap());
        assert_ne!(
            &*APP_VERSION,
            env!("CARGO_PKG_VERSION"),
            "these have drifted together — the regression this guards is no longer visible"
        );
        // The bug in the user's hands: 0.1.0 installed, v0.1.0 published, banner shown.
        let current = semver::Version::parse(&APP_VERSION).unwrap();
        let published = format!("v{}", *APP_VERSION);
        assert!(
            parse_release_tag(&published).unwrap() <= current,
            "an app must not offer itself an update"
        );
        assert!(parse_release_tag("v0.2.0").unwrap() > current);
        assert!(parse_release_tag("nightly").is_err());
    }
}
