//! openfoto desktop — a shell over `openfoto-core`.
//!
//! The CLI and this app are peers over one engine: every command here calls the same
//! functions `openfoto` does, so the two can never disagree about what a library
//! contains or what an operation will do.
//!
//! A *source* is a folder the user has added. Each one is an independent library with
//! its own disposable `.openfoto/`, which is what lets sources be added and removed
//! freely without any global database. The list of sources is the only app-level state,
//! and losing it costs nothing but re-adding folders.

use openfoto_core::{
    dedupe,
    userdata::{PhotoMeta, UserDataSet},
    faces::{assign, fetch as model_fetch, file as faces_file, people::People, pipeline, review},
    journal::Journal,
    plan::folder_of, rename, scan, scenery, semantic, thumbs, Library,
};
use serde::{Deserialize, Serialize};
use tauri::Emitter;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;

#[derive(Clone, Serialize)]
struct ProgressEvent<'a> {
    op: &'a str,
    done: usize,
    total: usize,
}

/// A progress sink that forwards to the webview as a `progress` event.
fn emitter<'a>(app: &'a tauri::AppHandle, op: &'a str) -> impl Fn(usize, usize) + Sync + 'a {
    move |done, total| {
        let _ = app.emit("progress", ProgressEvent { op, done, total });
    }
}

type R<T> = Result<T, String>;
fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[derive(Default)]
pub struct AppState {
    libs: Mutex<HashMap<String, Library>>,
    sources: Mutex<Vec<String>>,
    /// Held open for the life of the window. Loading the text tower costs ~270 ms
    /// against ~15 ms to embed a phrase, so a fresh load per keystroke would dominate
    /// the search. Built on first use, not at startup — a library nobody searches
    /// should not pay for it.
    text_encoder: Mutex<Option<semantic::TextEncoder>>,
}

// ---------------------------------------------------------------- sources

#[derive(Serialize, Deserialize, Default)]
struct SourcesFile {
    sources: Vec<String>,
}

fn sources_path(app: &tauri::AppHandle) -> std::path::PathBuf {
    use tauri::Manager;
    let dir = app.path().app_config_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let _ = std::fs::create_dir_all(&dir);
    dir.join("sources.json")
}

fn load_sources(app: &tauri::AppHandle) -> Vec<String> {
    std::fs::read(sources_path(app))
        .ok()
        .and_then(|d| serde_json::from_slice::<SourcesFile>(&d).ok())
        .map(|f| f.sources)
        .unwrap_or_default()
}

fn save_sources(app: &tauri::AppHandle, list: &[String]) {
    let _ = std::fs::write(
        sources_path(app),
        serde_json::to_vec_pretty(&SourcesFile { sources: list.to_vec() }).unwrap_or_default(),
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

#[derive(Serialize, Clone)]
pub struct PhotoInfo {
    kind: String,
    rating: u8,
    label: Option<String>,
    albums: Vec<String>,
    /// Uppercase extension, for filtering by type.
    ext: String,
    bytes: u64,
    hash: String,
    path: String,
    name: String,
    folder: String,
    thumb: String,
    taken_at: Option<i64>,
    faces: usize,
    people: Vec<String>,
    width: u32,
    height: u32,
}

fn open_lib(state: &AppState, root: &str) -> R<()> {
    let mut libs = state.libs.lock().map_err(err)?;
    if !libs.contains_key(root) {
        let mut lib = Library::open(root).map_err(err)?;
        // Reconcile with the filesystem the moment a library is opened, rather than
        // waiting to be asked (ADR-0011). Photographs added or reorganised in Finder
        // are picked up before anything is drawn, and the common case is cheap because
        // `scan` skips hashing whenever size and mtime already match. A failure here is
        // not fatal: an unreadable folder should still open, just stale.
        if let Err(e) = scan::scan(&mut lib, false) {
            eprintln!("[openfoto] scan on open failed for {root}: {e}");
        }
        libs.insert(root.to_string(), lib);
    }
    Ok(())
}

/// Run `f` against an open library.
///
/// The guard is confined to this function so it is never held across an await
/// point, which is what lets the command wrappers be `async` and therefore run
/// off the UI thread. Heavy work (thumbnails, face detection) would otherwise
/// freeze the window.
fn with<T>(state: &AppState, root: &str, f: impl FnOnce(&mut Library) -> anyhow::Result<T>) -> R<T> {
    open_lib(state, root)?;
    let mut libs = state.libs.lock().map_err(err)?;
    let lib = libs.get_mut(root).ok_or("library not open")?;
    f(lib).map_err(err)
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
        if r.kind == "photo" { photos += 1 } else { videos += 1 }
        let d = r.path.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
        *own.entry(d.clone()).or_default() += 1;
        *total.entry(d.clone()).or_default() += 1;
        for a in ancestors(&d) {
            *total.entry(a.clone()).or_default() += 1;
            has_children.insert(a, true);
        }
    }
    let folders = total;
    let analysed = rows.iter().filter(|r| lib.faces_done(&r.hash).unwrap_or(false)).count();
    let ready = rows
        .iter()
        .filter(|r| r.kind == "photo" && thumbs::thumb_path(lib, &r.hash).exists())
        .count();

    let people_file = People::load(lib.root())?;
    let opt = assign::Options::default();
    let mut claimed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for f in lib.all_faces()? {
        if let Some(e) = f.embedding.as_ref() {
            if let Some(n) = assign::assign(e, &people_file, &opt).person() {
                claimed.entry(n.to_string()).or_default().insert(f.hash.clone());
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
            .filter(|(_, n)| *n > 0)
            .map(|(path, count)| FolderInfo {
                depth: if path.is_empty() { 0 } else { path.matches('/').count() + 1 },
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
    })
}

#[tauri::command]
async fn list_sources(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> R<Vec<SourceInfo>> {
    let list = load_sources(&app);
    *state.sources.lock().map_err(err)? = list.clone();
    let mut out = Vec::new();
    for root in list {
        if !std::path::Path::new(&root).is_dir() {
            out.push(SourceInfo {
                name: root.rsplit('/').next().unwrap_or(&root).to_string(),
                path: root,
                photos: 0, videos: 0, folders: vec![], people: vec![],
                faces_analysed: 0, thumbs_ready: 0, missing: true,
            });
            continue;
        }
        match with(&state, &root, describe) {
            Ok(info) => out.push(info),
            Err(_) => continue,
        }
    }
    Ok(out)
}

#[tauri::command]
async fn add_source(app: tauri::AppHandle, state: tauri::State<'_, AppState>, path: String) -> R<SourceInfo> {
    let mut list = load_sources(&app);
    if !list.contains(&path) {
        list.push(path.clone());
        save_sources(&app, &list);
    }
    with(&state, &path, |lib| {
        scan::scan(lib, false)?;
        describe(lib)
    })
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
    let sink = emitter(&app, "faces");
    with(&state, &path, |lib| {
        let st = pipeline::analyze_with_progress(lib, pipeline::DEFAULT_SCORE, &sink)?;
        Ok(format!("{} faces found in {} photos", st.faces, st.photos))
    })
}

#[tauri::command]
fn remove_source(app: tauri::AppHandle, state: tauri::State<'_, AppState>, path: String) -> R<()> {
    let list: Vec<String> = load_sources(&app).into_iter().filter(|p| p != &path).collect();
    save_sources(&app, &list);
    state.libs.lock().map_err(err)?.remove(&path);
    Ok(())
}

#[tauri::command]
async fn rescan(state: tauri::State<'_, AppState>, path: String) -> R<SourceInfo> {
    with(&state, &path, |lib| {
        scan::scan(lib, false)?;
        describe(lib)
    })
}

// ---------------------------------------------------------------- photos

#[tauri::command]
async fn photos(
    state: tauri::State<'_, AppState>,
    path: String,
    folder: Option<String>,
    person: Option<String>,
) -> R<Vec<PhotoInfo>> {
    with(&state, &path, |lib| {
        let people_file = People::load(lib.root())?;
        let user = UserDataSet::load(lib.root())?;
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
                person.as_ref().is_none_or(|p| {
                    who.get(&r.hash).is_some_and(|v| v.iter().any(|x| x == p))
                })
            })
            .map(|r| {
                let sig = lib.index.get_signature(&r.hash).ok().flatten();
                let meta = user.get(&r.hash, folder_of(&r.path));
                PhotoInfo {
                    kind: r.kind.clone(),
                    rating: meta.rating,
                    label: meta.label.clone(),
                    albums: meta.albums.clone(),
                    ext: r.path.rsplit('.').next().unwrap_or("").to_uppercase(),
                    bytes: r.size.max(0) as u64,
                    name: r.path.rsplit('/').next().unwrap_or(&r.path).to_string(),
                    folder: r.path.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default(),
                    thumb: thumbs::thumb_path(lib, &r.hash).display().to_string(),
                    taken_at: r.taken_at,
                    faces: nfaces.get(&r.hash).copied().unwrap_or(0),
                    people: who.get(&r.hash).cloned().unwrap_or_default(),
                    width: sig.as_ref().map(|s| s.width).unwrap_or(0),
                    height: sig.as_ref().map(|s| s.height).unwrap_or(0),
                    hash: r.hash.clone(),
                    path: lib.abs(&r.path).display().to_string(),
                }
            })
            .collect();
        // Newest first, which is what a photo library defaults to.
        out.sort_by(|a, b| b.taken_at.cmp(&a.taken_at).then(a.name.cmp(&b.name)));
        Ok(out)
    })
}

#[tauri::command]
async fn build_thumbs(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> R<usize> {
    let sink = emitter(&app, "thumbs");
    with(&state, &path, |lib| thumbs::build_with_progress(lib, &sink))
}

#[tauri::command]
async fn analyze_faces(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> R<String> {
    let sink = emitter(&app, "faces");
    with(&state, &path, |lib| {
        let st = pipeline::analyze_with_progress(lib, pipeline::DEFAULT_SCORE, &sink)?;
        Ok(format!("{} photos analysed · {} faces found", st.photos, st.faces))
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
        let total = lib.index.all()?.into_iter().filter(|r| r.kind == "photo").count();
        Ok(SemanticStatus { available, embedded: lib.index.count_clip()?, total })
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
    let sink = emitter(&app, "semantic");
    with(&state, &path, |lib| {
        let st = semantic::analyze(lib, &sink)?;
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
        Ok(hits.into_iter().map(|h| SemanticHit { hash: h.hash, score: h.score }).collect())
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
    let sink = emitter(&app, "clusters");
    with(&state, &path, |lib| {
        let people = People::load(lib.root())?;
        let p = review::build_with_progress(lib, &people, &assign::Options::default(), distance, &sink)?;
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
        let mut people = People::load(lib.root())?;
        let groups = pipeline::cluster_unassigned(lib, &people, &assign::Options::default(), distance)?;
        let mut learned = 0;
        for (id, name) in &assignments {
            if let Some(g) = groups.get(*id) {
                let refs: Vec<Vec<f32>> = g.iter().filter_map(|f| f.embedding.clone()).collect();
                learned += refs.len();
                people.add_references(name, refs);
            }
        }
        people.save(lib.root())?;
        Ok(learned)
    })
}

// ---------------------------------------------------------------- people overview

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
) -> R<Vec<PersonEntry>> {
    with(&state, &path, |lib| {
        let people = People::load(lib.root())?;
        let opt = assign::Options::default();
        let root = lib.root().to_path_buf();

        let mut claimed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut cover: BTreeMap<String, (String, i64)> = BTreeMap::new();
        for f in lib.all_faces()? {
            let Some(e) = f.embedding.as_ref() else { continue };
            if let Some(n) = assign::assign(e, &people, &opt).person() {
                if people.is_excluded(n, &f.hash) {
                    continue;
                }
                claimed.entry(n.to_string()).or_default().insert(f.hash.clone());
                cover.entry(n.to_string()).or_insert((f.hash.clone(), f.idx));
            }
        }

        let mut out: Vec<PersonEntry> = people
            .people
            .iter()
            .map(|p| PersonEntry {
                photos: claimed.get(&p.name).map(|s| s.len()).unwrap_or(0),
                cover: cover.get(&p.name).map(|(h, i)| {
                    pipeline::face_crop_path(&root, h, *i).display().to_string()
                }),
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
                let best = g
                    .iter()
                    .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal));
                PersonEntry {
                    name: None,
                    cluster: Some(id),
                    photos: g.iter().map(|f| &f.hash).collect::<BTreeSet<_>>().len(),
                    cover: best.map(|f| {
                        pipeline::face_crop_path(&root, &f.hash, f.idx).display().to_string()
                    }),
                    suggestion: best.and_then(|f| {
                        f.embedding.as_ref().and_then(|e| {
                            assign::score_all(e, &people).first().and_then(|(n, s)| {
                                (*s >= 0.45).then(|| n.clone())
                            })
                        })
                    }),
                }
            })
            .collect();
        unnamed.sort_by(|a, b| b.photos.cmp(&a.photos));
        out.extend(unnamed);
        Ok(out)
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
        let mut people = People::load(lib.root())?;
        let groups = pipeline::cluster_unassigned(lib, &people, &assign::Options::default(), distance)?;
        let g = groups.get(cluster).ok_or_else(|| anyhow::anyhow!("no such group"))?;
        let refs: Vec<Vec<f32>> = g.iter().filter_map(|f| f.embedding.clone()).collect();
        let n = refs.len();
        people.add_references(name.trim(), refs);
        people.save(lib.root())?;
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
        let people = People::load(lib.root())?;
        let groups = pipeline::cluster_unassigned(lib, &people, &assign::Options::default(), distance)?;
        Ok(groups
            .get(cluster)
            .map(|g| g.iter().map(|f| f.hash.clone()).collect::<BTreeSet<_>>().into_iter().collect())
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
async fn models_fetch(app: tauri::AppHandle) -> R<String> {
    let sink = |name: &str, done: usize, total: usize| {
        let _ = app.emit("progress", ProgressEvent { op: "models", done, total });
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
    mut f: impl FnMut(&mut openfoto_core::userdata::UserData, &str),
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
    set.save(lib.root())
}

#[tauri::command]
async fn set_rating(state: tauri::State<'_, AppState>, path: String, hashes: Vec<String>, rating: u8) -> R<()> {
    with(&state, &path, |lib| {
        edit_each(lib, &hashes, |u, h| u.set_rating(h, rating))
    })
}

#[tauri::command]
async fn set_label(state: tauri::State<'_, AppState>, path: String, hashes: Vec<String>, label: Option<String>) -> R<()> {
    with(&state, &path, |lib| {
        edit_each(lib, &hashes, |u, h| u.set_label(h, label.clone()))
    })
}

#[tauri::command]
async fn set_album(state: tauri::State<'_, AppState>, path: String, hashes: Vec<String>, album: String, member: bool) -> R<()> {
    with(&state, &path, |lib| {
        // Albums are on the way out (ADR-0009); this keeps existing ones editable
        // until the migration to folders ships.
        let album = album.trim().to_string();
        edit_each(lib, &hashes, |u, h| u.set_album(h, &album, member))
    })
}

#[tauri::command]
async fn list_albums(state: tauri::State<'_, AppState>, path: String) -> R<Vec<(String, usize)>> {
    with(&state, &path, |lib| {
        Ok(UserDataSet::load(lib.root())?.albums().into_iter().collect())
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
}

#[tauri::command]
async fn photo_detail(state: tauri::State<'_, AppState>, path: String, hash: String) -> R<PhotoDetail> {
    with(&state, &path, |lib| {
        let row = lib
            .index
            .all()?
            .into_iter()
            .find(|r| r.hash == hash)
            .ok_or_else(|| anyhow::anyhow!("photo not found"))?;
        let sig = lib.index.get_signature(&hash)?;
        let people_file = People::load(lib.root())?;
        let opt = assign::Options::default();
        let mut people = Vec::new();
        let mut faces = 0;
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
            meta: UserDataSet::load(lib.root())?.get(&hash, folder_of(&row.path)),
            hash,
        })
    })
}

// ---------------------------------------------------------------- photo editing

/// Rotate and/or crop one photo.
///
/// `keep_original` defaults to true and moves the untouched file to `Originals/`,
/// mirroring how deleting moves a photo to `Trash/`. See openfoto_core::edit for why
/// the original is not kept in the (disposable) vault.
#[tauri::command]
async fn edit_photo(
    state: tauri::State<'_, AppState>,
    path: String,
    hash: String,
    edit: openfoto_core::edit::Edit,
) -> R<String> {
    with(&state, &path, |lib| {
        let row = lib
            .index
            .all()?
            .into_iter()
            .find(|r| r.hash == hash)
            .ok_or_else(|| anyhow::anyhow!("photo not found"))?;
        let out = openfoto_core::edit::apply(lib, &row.path, &edit)?;
        // The file changed, so its hash did: re-scan to re-identify it, and drop the
        // stale thumbnail and face data keyed to the old content.
        let _ = std::fs::remove_file(thumbs::thumb_path(lib, &hash));
        scan::scan(lib, false)?;
        Ok(match out.original {
            Some(o) => format!("Saved {}x{} · original kept in {}", out.width, out.height, o),
            None => format!("Saved {}x{} · original not kept", out.width, out.height),
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
async fn delete_photos(state: tauri::State<'_, AppState>, path: String, hashes: Vec<String>) -> R<String> {
    with(&state, &path, |lib| {
        std::fs::create_dir_all(lib.abs(TRASH))?;
        let want: BTreeSet<String> = hashes.into_iter().collect();
        let mut plan = openfoto_core::Plan::new("delete");
        for r in lib.index.all()? {
            if !want.contains(&r.hash) || r.path.starts_with(&format!("{TRASH}/")) {
                continue;
            }
            let name = r.path.rsplit('/').next().unwrap_or(&r.path);
            plan.ops.push(openfoto_core::Op::Move {
                hash: r.hash.clone(),
                from: r.path.clone(),
                to: format!("{TRASH}/{name}"),
            });
        }
        if plan.is_empty() {
            return Ok("Nothing to delete".into());
        }
        let n = plan.len();
        let j = plan.apply(lib)?;
        Ok(format!("Moved {n} to Trash · undo id {}", j.id))
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
        if !new_name.to_lowercase().ends_with(&format!(".{}", ext.to_lowercase())) {
            new_name = format!("{new_name}.{ext}");
        }
        openfoto_core::fsops::validate_filename(&new_name)?;
        let dir = row.path.rsplit_once('/').map(|(d, _)| format!("{d}/")).unwrap_or_default();
        let to = format!("{dir}{new_name}");
        if to == row.path {
            return Ok("Name unchanged".into());
        }
        let mut plan = openfoto_core::Plan::new("rename-one");
        plan.ops.push(openfoto_core::Op::Rename { hash, from: row.path, to: to.clone() });
        plan.apply(lib)?;
        Ok(format!("Renamed to {new_name}"))
    })
}

/// Remove a person from photos, and move them out of that person's folder.
#[tauri::command]
async fn untag_person(
    state: tauri::State<'_, AppState>,
    path: String,
    person: String,
    hashes: Vec<String>,
) -> R<String> {
    with(&state, &path, |lib| {
        let mut people = People::load(lib.root())?;
        people.exclude(&person, &hashes);
        people.save(lib.root())?;

        // Anything sitting in that person's folder goes back to the library root.
        let want: BTreeSet<String> = hashes.iter().cloned().collect();
        let mut plan = openfoto_core::Plan::new("untag");
        for r in lib.index.all()? {
            if !want.contains(&r.hash) {
                continue;
            }
            if r.path.starts_with(&format!("{person}/")) {
                let name = r.path.rsplit('/').next().unwrap_or(&r.path).to_string();
                plan.ops.push(openfoto_core::Op::Move {
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
        Ok(format!(
            "Removed {} from {} photo{}{}",
            person,
            want.len(),
            if want.len() == 1 { "" } else { "s" },
            if moved > 0 { format!(", {moved} moved back to root") } else { String::new() }
        ))
    })
}

/// Restore photos from the library Trash back to the root.
#[tauri::command]
async fn restore_photos(state: tauri::State<'_, AppState>, path: String, hashes: Vec<String>) -> R<String> {
    with(&state, &path, |lib| {
        let want: BTreeSet<String> = hashes.into_iter().collect();
        let mut plan = openfoto_core::Plan::new("restore");
        for r in lib.index.all()? {
            if !want.contains(&r.hash) || !r.path.starts_with(&format!("{TRASH}/")) {
                continue;
            }
            let name = r.path.rsplit('/').next().unwrap_or(&r.path).to_string();
            plan.ops.push(openfoto_core::Op::Move { hash: r.hash.clone(), from: r.path.clone(), to: name });
        }
        if plan.is_empty() {
            return Ok("Nothing to restore".into());
        }
        let n = plan.len();
        plan.apply(lib)?;
        Ok(format!("Restored {n}"))
    })
}

/// Hand the library Trash over to the system Trash.
///
/// This is the one place openfoto stops being reversible by itself, so it hands off
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
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            // Never clobber something already in the system Trash.
            let mut dest = sys.join(&name);
            let mut n = 2;
            while dest.exists() {
                let (stem, ext) = name.rsplit_once('.').unwrap_or((name.as_str(), ""));
                dest = sys.join(format!("{stem} {n}.{ext}"));
                n += 1;
            }
            if std::fs::rename(&p, &dest).is_ok() {
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

#[derive(Serialize)]
pub struct PlanView {
    label: String,
    moves: Vec<(String, String)>,
    skipped: Vec<(String, String)>,
}

fn build_plan(
    lib: &mut Library,
    op: &str,
    param: Option<f32>,
    mkdirs: bool,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> anyhow::Result<openfoto_core::Plan> {
    Ok(match op {
        "dedupe" => {
            dedupe::ensure_signatures_with_progress(lib, progress)?;
            let mut o = dedupe::Options::default();
            if let Some(v) = param { o.rmse = v }
            if mkdirs { std::fs::create_dir_all(lib.abs(&o.dest))?; }
            dedupe::plan(lib, &o)?
        }
        "scenery" => {
            let mut o = scenery::Options::default();
            if let Some(v) = param { o.max_face = v }
            if mkdirs { std::fs::create_dir_all(lib.abs(&o.dest))?; }
            scenery::plan(lib, &o)?
        }
        "rename" => rename::plan(lib, rename::DEFAULT_FORMAT)?,
        "file" => {
            let people = People::load(lib.root())?;
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
    let sink = emitter(&app, "plan");
    with(&state, &path, |lib| {
        let p = build_plan(lib, &op, param, false, &sink)?;
        Ok(PlanView {
            label: op.clone(),
            moves: p.ops.iter().map(|o| (o.from().to_string(), o.to().to_string())).collect(),
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
    let sink = emitter(&app, "apply");
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
        let target = id.or_else(|| ids.last().cloned())
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
fn serve_photo(app: &tauri::AppHandle, request: http::Request<Vec<u8>>) -> http::Response<Vec<u8>> {
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
    let Ok(decoded) = percent_decode(request.uri().path()) else { return deny(400) };
    let path = std::path::PathBuf::from(&decoded);
    // `?t=<hash>` asks for the thumbnail of this photo. Serving it from cache when
    // present and rendering it on demand when not is what makes the grid usable
    // immediately on a large library: the virtualised viewport only ever requests the
    // few dozen images actually on screen, so thumbnails are produced in view order
    // instead of by a pre-pass that has to finish before anything is visible.
    let param = |k: &str| {
        request.uri().query().and_then(|q| {
            q.split('&').find_map(|kv| kv.strip_prefix(k).map(|v| v.to_string()))
        })
    };
    let thumb_hash = param("t=");
    // `?full=<hash>` asks for the full-size image. HEIC is the reason this exists:
    // WKWebView cannot decode it (verified), so it is transcoded once and cached
    // rather than converted on every view.
    let full_hash = param("full=");

    // Boundary: the file must live inside a source the user added.
    let sources = load_sources(app);
    let Ok(canon) = path.canonicalize() else { return deny(404) };
    let allowed = sources.iter().any(|s| {
        std::path::Path::new(s)
            .canonicalize()
            .map(|root| canon.starts_with(root))
            .unwrap_or(false)
    });
    if !allowed {
        return deny(403);
    }
    let source_root = |canon: &std::path::Path| {
        sources
            .iter()
            .map(std::path::PathBuf::from)
            .find(|r| r.canonicalize().map(|c| canon.starts_with(c)).unwrap_or(false))
    };

    // Full-size request for a format the webview cannot decode: serve a cached JPEG.
    if thumb_hash.is_none() && openfoto_core::imageio::needs_conversion(&canon) {
        if let (Some(hash), Some(root)) = (full_hash, source_root(&canon)) {
            let derived = root
                .join(openfoto_core::library::VAULT_DIR)
                .join("derived")
                .join(format!("{hash}.jpg"));
            if !derived.exists()
                && openfoto_core::imageio::convert_to_jpeg(&canon, &derived).is_err()
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
            match source_root(&canon) {
                None => canon.clone(),
                Some(root) => {
                    let t = openfoto_core::thumbs::thumb_path_at(&root, hash);
                    if !t.exists() {
                        let is_video = canon
                            .extension()
                            .and_then(|e| e.to_str())
                            .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "mp4" | "mov" | "m4v"));
                        if openfoto_core::thumbs::render_to(&canon, &t, is_video).is_err() {
                            // Fall back to the original rather than showing nothing.
                            return match std::fs::read(&canon) {
                                Ok(b) => ok_response(b, &canon),
                                Err(_) => deny(404),
                            };
                        }
                    }
                    t
                }
            }
        }
    };

    match std::fs::read(&serve) {
        Ok(bytes) => ok_response(bytes, &serve),
        Err(_) => deny(404),
    }
}

fn ok_response(bytes: Vec<u8>, path: &std::path::Path) -> http::Response<Vec<u8>> {
    let mime = match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
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
        .body(bytes)
        .unwrap()
}

fn percent_decode(s: &str) -> Result<String, ()> {
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .register_uri_scheme_protocol("photo", |ctx, request| serve_photo(ctx.app_handle(), request));

    // UI verification bridge. Behind the `ui-bridge` feature and additionally gated on
    // a debug build, so a release binary never exposes a WebSocket server.
    #[cfg(all(feature = "ui-bridge", debug_assertions))]
    {
        builder = builder.plugin(tauri_plugin_mcp_bridge::init());
    }

    builder
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            list_sources, add_source, remove_source, rescan,
            photos, build_thumbs, analyze_faces,
            clusters, name_clusters,
            plan_op, apply_op, history, undo,
            delete_photos, rename_photo, untag_person, restore_photos, empty_trash,
            models_status, models_fetch,
            people_overview, name_cluster, cluster_photos, autodetect_faces,
            edit_photo, set_rating, set_label, set_album, list_albums, photo_detail,
            semantic_status, semantic_index, semantic_search
        ])
        .run(tauri::generate_context!())
        .expect("error while running openfoto");
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(ancestors("Trip/Greece Day3"), vec!["Trip".to_string(), String::new()]);
        assert_eq!(ancestors("Trip"), vec![String::new()]);
        // The root has no ancestors, and must not report itself as one or counts
        // would be doubled at the top of the tree.
        assert!(ancestors("").is_empty());
    }
}
