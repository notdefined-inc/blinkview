/* Blinkview desktop.
   The engine lives in Rust; this file is presentation and interaction only. */
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

/* Photos are served by our own `photo://` scheme (see serve_photo in lib.rs), which
   only serves files inside folders the user has added as a source. */
/* Photographs arrive with a path relative to their library, since repeating the same
   root a hundred thousand times is most of what a source switch costs. Absolute paths
   still work: face crops and thumbnails inside the cache are handed over whole. */
/* Split what the backend did not repeat.
   `name`, `folder` and `ext` all live inside `path`, and sending them again cost about
   a third of the payload — which at 200,000 photographs is the difference the bridge
   charges for. Doing it here is a string split per photograph; sending it was megabytes. */
function hydrate(list) {
  const pairs = new Map();
  for (const p of list) {
    const cut = p.path.lastIndexOf("/");
    p.name = cut < 0 ? p.path : p.path.slice(cut + 1);
    p.folder = cut < 0 ? "" : p.path.slice(0, cut);
    const dot = p.name.lastIndexOf(".");
    p.ext = dot < 0 ? "" : p.name.slice(dot + 1).toUpperCase();
    p.rating ||= 0;
    p.faces ||= 0;
    p.albums ||= [];
    p.people ||= [];
    const stem = (dot < 0 ? p.name : p.name.slice(0, dot)).toLocaleLowerCase();
    const key = `${p.folder.toLocaleLowerCase()}\u0000${stem}`;
    if (!pairs.has(key)) pairs.set(key, { photos: [], videos: [] });
    pairs.get(key)[p.kind === "video" ? "videos" : "photos"].push(p);
  }
  // Apple's on-disk Live Photo shape is a still and MOV beside each other with the
  // same stem. Pairing is derived every load, so moving either file in Finder updates
  // the presentation without leaving metadata behind.
  for (const pair of pairs.values()) {
    const still = pair.photos[0];
    const motion = pair.videos.find(p => p.ext === "MOV");
    if (!still || !motion) continue;
    still.liveVideo = motion;
    motion.liveStill = still;
  }
  return list;
}

const photoUrl = p => {
  const abs = p.startsWith("/") ? p : `${S.source}/${p}`;
  return "photo://localhost" + encodeURIComponent(abs).replace(/%2F/g, "/");
};
const dialog = window.__TAURI__.dialog;

const S = {
  sources: [],
  source: null,          // active source path
  peek: null,            // markerless, session-only folder being viewed read-only
  folder: null,          // active subfolder, null = whole source
  person: null,
  cluster: null,           // an unnamed group being viewed
  clusterHashes: null,
  people: [],              // named + unnamed, from people_overview
  dismissed: 0,            // faces set aside as not worth naming
  peopleCollapsed: true,
  albums: [],            // legacy, only for the migration prompt
  searches: [],
  expanded: null,
  sort: "newest",
  order: [],               // custom arrangement of the selected folder, by hash
  elsewhere: 0,            // query matches hidden by the current folder or person
  group: "date",           // "date" or "folder" — how the grid sections itself
  photos: [],
  view: [],              // filtered/sorted photos currently on screen
  lbIndex: -1,
  lbList: [],
  lbScope: "view",         // "view" = everything on screen, "folder" = just this one
  busy: {},                // per-source progress: { [path]: { op, done, total } }
  resetScroll: false,      // set by navigation, never by background refreshes
  loading: false,          // a library's photographs are on their way
  sel: new Set(),          // selected photo hashes
  lastIndex: -1,           // anchor for shift-range selection
  zoom: 1, panX: 0, panY: 0,
  edit: null,              // pending, unsaved edit on the open photo
  cropping: false,
  crop: null,              // {x,y,w,h} fractions of the displayed image
  cropAR: null,            // locked aspect ratio, or null for free
  keepOriginal: true,      // safe editing, remembered per session
  // Semantic search is the one filter that cannot run in the browser: it needs the
  // text encoder. Results arrive after the grid has already drawn, so they are held
  // here and folded in on the next pass rather than blocking the typed query.
  semantic: null,          // { query, scores: Map<hash, score>, state }
  semanticReady: null,     // { available, embedded, total }
};

const TRASH = "Trash";

/* Camera RAW. Blinkview reads the preview the camera embedded and never writes one
   back (ADR-0018), so the controls that rewrite a file are turned off for these rather
   than left to fail on the way to disk. Kept in step with `raw::RAW_EXT`. */
const RAW_EXT = ["cr2", "cr3", "dng", "nef", "arw", "raf"];
const isRaw = path => RAW_EXT.includes(String(path).split(".").pop().toLowerCase());

/* A folder contains everything beneath it (ADR-0009). Compared segment-wise, so
   `Trip2` is not read as living inside `Trip` — the same rule as `in_folder` in the
   backend, and the two must agree or the grid and the counts disagree. */
function inFolder(path, folder) {
  if (!folder) return true;
  return path === folder || path.startsWith(folder + "/");
}

const $ = s => document.querySelector(s);
const el = (t, a = {}, ...kids) => {
  const n = document.createElement(t);
  for (const [k, v] of Object.entries(a)) {
    if (v === null || v === undefined || v === false) continue;
    if (k === "class") n.className = v;
    else if (k.startsWith("on")) n.addEventListener(k.slice(2), v);
    else n.setAttribute(k, v === true ? "" : v);
  }
  for (const c of kids.flat()) if (c !== null && c !== undefined) n.append(c.nodeType ? c : String(c));
  return n;
};

/* ---------------- toasts and progress ---------------- */
function toast(msg, kind = "info", sticky = false) {
  const t = el("div", { class: "toast", "data-kind": kind, role: "status" },
    kind === "busy" ? el("span", { class: "sp" }) : null, msg);
  $("#toasts").append(t);
  if (!sticky) setTimeout(() => t.remove(), kind === "error" ? 8000 : 3500);
  return t;
}

/* The current busy toast, so backend progress events can find it. Long work would
   otherwise be indistinguishable from a hang — the reason this exists at all. */
let liveToast = null;
let pendingUpdate = null;

async function checkUpdates(manual = false) {
  try {
    const info = await invoke("check_for_updates");
    if (!info.available) {
      if (manual) toast(`Blinkview ${info.current} is up to date`, "ok");
      return;
    }
    pendingUpdate = info;
    $("#update-title").textContent = `Blinkview ${info.latest} is ready`;
    $("#update-note").textContent = `You have ${info.current} · GitHub release`;
    $("#update-banner").hidden = false;
    document.body.classList.add("has-update");
  } catch (e) {
    if (manual) toast(`Could not check for updates — ${e}`, "error");
  }
}

function dismissUpdate() {
  $("#update-banner").hidden = true;
  document.body.classList.remove("has-update");
}

/* `source` names the library this operation is about, so background work on another
   folder cannot repaint this banner. */
async function busy(msg, fn, source = null) {
  const label = el("span", {}, msg);
  const pct = el("span", { class: "pct num" });
  const fill = el("span");
  const bar = el("div", { class: "tbar", hidden: true }, fill);
  const t = el("div", { class: "toast", "data-kind": "busy", role: "status" },
    el("div", { class: "trow" }, el("span", { class: "sp" }), label, pct), bar);
  // A banner belongs to the library it is about. Work on one folder followed the user
  // to every other one, claiming the window while they were somewhere else; the source
  // row shows it instead.
  if (source) {
    t.dataset.src = source;
    t.hidden = source !== S.source;
  }
  $("#toasts").append(t);
  const prev = liveToast;
  liveToast = { label, pct, bar, fill, msg, source };
  try { return await fn(); }
  catch (e) { toast(String(e), "error"); throw e; }
  finally {
    // Restore whatever banner was underneath rather than clearing the only one.
    liveToast = prev;
    t.remove();
    if (source) { delete S.busy[source]; paintSourceProgress(source); }
  }
}

const OP_LABEL = {
  faces: "Detecting faces", thumbs: "Building thumbnails", scan: "Indexing folder",
  analyze: "Analysing photos",
  clusters: "Grouping faces", plan: "Analysing photos", apply: "Analysing photos",
  models: "Downloading face models", semantic: "Understanding photos",
};

/* Progress belongs to a library, not to the window.
   Work on one folder used to drive whatever banner happened to be open, so two jobs at
   once fought over it and a background scan narrated itself over the thing you were
   actually doing. Each source now carries its own state, shown on its own row, and the
   banner only speaks for the job it was opened for. */
listen("progress", ({ payload }) => {
  const { op, done, total, source } = payload;
  if (!total) return;

  if (source) {
    S.busy[source] = { op, done, total };
    paintSourceProgress(source);
    // While the library being looked at is still indexing, keep pulling in what has
    // landed so far. The index commits as it walks and reads do not wait on it, so the
    // grid fills in rather than sitting empty until the whole folder is done.
    if (op === "scan" && source === S.source) scheduleIndexingRefresh();
  }
  // The banner tracks one operation: the one it was opened for.
  if (!liveToast) return;
  if (liveToast.source && source && liveToast.source !== source) return;
  liveToast.label.textContent = OP_LABEL[op] || liveToast.msg;
  liveToast.pct.textContent = `${done} / ${total}`;
  liveToast.bar.hidden = false;
  liveToast.fill.style.width = Math.round((done / total) * 100) + "%";
});

/* Refilling on every progress tick would re-query hundreds of times a second, so it is
   throttled to something a person can actually perceive. */
let indexingRefresh = 0;
function scheduleIndexingRefresh() {
  const now = performance.now();
  if (now - indexingRefresh < 900) return;
  indexingRefresh = now;
  loadPhotos().catch(() => {});
}

/* Short labels for the sidebar, where there is room for two words and no more. */
const OP_SHORT = {
  scan: "indexing", faces: "faces", thumbs: "thumbnails",
  semantic: "reading", analyze: "analysing", clusters: "grouping",
};

/** Show only the banners belonging to the library on screen. */
function syncToastScope() {
  for (const t of document.querySelectorAll("#toasts .toast[data-src]")) {
    t.hidden = t.dataset.src !== S.source;
  }
}

/** Draw a source's own progress on its own row, without rebuilding the sidebar. */
function paintSourceProgress(source) {
  const row = document.querySelector(`#sources [data-src="${CSS.escape(source)}"]`);
  if (!row) return;
  const b = S.busy[source];
  let bar = row.querySelector(".srcbar");
  const count = row.querySelector(".n");

  if (!b) {
    bar?.remove();
    row.classList.remove("working");
    if (count && count.dataset.was !== undefined) {
      count.textContent = count.dataset.was;
      delete count.dataset.was;
    }
    return;
  }
  // A rescan of an unchanged library takes a third of a second — showing a bar for it
  // is a flicker that reads as "indexing again" when nothing was reindexed.
  const started = (b.since ||= performance.now());
  if (b.op === "scan" && performance.now() - started < 400) return;

  row.classList.add("working");
  // Say which job it is. A bar alone left the user guessing whether a folder was being
  // indexed or having its faces detected.
  if (count) {
    if (count.dataset.was === undefined) count.dataset.was = count.textContent;
    const pct = b.total ? Math.round((b.done / b.total) * 100) : 0;
    count.textContent = `${OP_SHORT[b.op] || b.op} ${pct}%`;
  }
  if (!bar) {
    bar = el("span", { class: "srcbar" }, el("span", { class: "srcfill" }));
    row.append(bar);
  }
  bar.querySelector(".srcfill").style.width =
    Math.round((b.done / Math.max(b.total, 1)) * 100) + "%";
  bar.title = `${OP_LABEL[b.op] || b.op} — ${b.done.toLocaleString()} of ${b.total.toLocaleString()}`;
}

/* ---------------- date helpers ---------------- */
const DAY = ts => {
  const d = new Date(ts * 1000);
  return d.toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long", year: "numeric" });
};
const TIME = ts => new Date(ts * 1000).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });

/* ---------------- sidebar ---------------- */
function renderSidebar() {
  const box = $("#sources");
  box.replaceChildren(...S.sources.map(s => {
    const active = s.path === S.source && !S.folder && !S.person;
    return el("div", {
      class: "row srcrow" + (s.missing ? " missing" : ""), "aria-current": String(active),
      title: s.path, "data-src": s.path,
      oncontextmenu: e => showSourceCtx(e, s),
    },
      el("button", { class: "grow srcopen", onclick: () => { if (!s.missing) selectSource(s.path); } },
        el("span", { class: "dotmark" }),
        el("span", { class: "grow" }, s.missing ? `${s.name} (missing)` : s.name)),
      el("span", { class: "n num" },
        s.missing ? "" : (s.indexing || S.busy[s.path] ? "indexing\u2026" : String(s.photos))),
      s.shallow ? el("span", { class: "depthmark", title: "This folder only" }, "direct") : null,
      el("button", { class: "mini sact", title: `Remove ${s.name} from Blinkview`,
        onclick: e => { e.stopPropagation(); removeSource(s.path); } }, "\u2715"));
  }));

  const src = S.sources.find(s => s.path === S.source);
  const pb = $("#people-block"), fb = $("#folders-block");
  if (!src) {
    pb.hidden = true; fb.hidden = true;
    $("#searches-block").hidden = true;
    $("#albums-block").hidden = true;
    $("#trash-block").hidden = true;
    return;
  }

  pb.hidden = false;
  const collapsed = S.peopleCollapsed;
  const named = S.people.filter(p => p.name);
  const unnamed = S.people.filter(p => !p.name);
  const face = p => p.cover
    ? el("img", { class: "avatar", src: photoUrl(p.cover), alt: "", loading: "lazy" })
    : el("span", { class: "avatar blank" });

  // A name matching nothing cannot be browsed to anything, so it is not listed as if
  // it could be. It stays removable through the person it duplicates, or by untagging.
  const rows = named.filter(p => p.photos > 0).map(p => el("div", {
    class: "row prow", "aria-current": String(S.person === p.name),
  },
    el("button", { class: "grow prun", onclick: () => selectPerson(p.name) },
      face(p), el("span", { class: "grow" }, p.name)),
    el("span", { class: "n num" }, String(p.photos)),
    // Only offered when there is somebody to merge into. A control that can only
    // fail is worse than no control.
    named.length > 1
      ? el("button", { class: "mini pact", title: `${p.name} is the same person as\u2026`,
          onclick: () => mergePersonPrompt(p.name) }, "\u21C4")
      : null,
    el("button", { class: "mini pact", title: `Forget ${p.name}`,
      onclick: () => forgetPerson(p.name) }, "\u2715")));

  const empties = named.filter(p => p.photos === 0);
  if (empties.length) {
    rows.push(el("button", { class: "row faint", title: empties.map(p => p.name).join(", "),
      onclick: () => forgetEmptyPeople(empties.map(p => p.name)) },
      el("span", { class: "grow" },
        `${empties.length} name${empties.length === 1 ? "" : "s"} matching no photos`),
      el("span", { class: "n num" }, "\u2715")));
  }

  // Set-aside faces sit with the other summary row rather than at the bottom: a
  // correction nobody can find again is not really a correction.
  if (S.dismissed > 0) {
    rows.push(el("button", { class: "row faint",
      title: "Offer these faces for naming again",
      onclick: restoreDismissed },
      el("span", { class: "grow" },
        `${S.dismissed} face${S.dismissed === 1 ? "" : "s"} set aside`),
      el("span", { class: "n num" }, "\u21A9")));
  }

  // Unnamed groups are shown too. Detection finding 243 faces and the sidebar still
  // reading "None named yet" is what made face detection look broken.
  for (const u of unnamed.slice(0, 12)) {
    rows.push(el("div", {
      class: "row prow unnamed", "aria-current": String(S.cluster === u.cluster),
    },
      el("button", { class: "grow prun",
        title: u.suggestion ? `Looks like ${u.suggestion}` : "Unnamed person",
        onclick: () => selectCluster(u.cluster) },
        face(u),
        el("span", { class: "grow" }, u.suggestion ? `${u.suggestion}?` : "Who is this?")),
      el("span", { class: "n num" }, String(u.photos)),
      // Not everyone in a photograph is someone to name. The ✕ means the same thing
      // it does on a named row: take this out of the list.
      el("button", { class: "mini pact", title: "Not someone to name \u2014 set these faces aside",
        onclick: () => dismissCluster(u.cluster) }, "\u2715")));
  }
  if (!rows.length) {
    const unscanned = src.faces_analysed < src.photos;
    if (unscanned) {
      // Saying "not scanned yet" without offering the scan leaves the user to find a
      // menu item they have no reason to know exists.
      const left = src.photos - src.faces_analysed;
      rows.push(el("button", {
        class: "row scanrow",
        title: `Look for people in ${left.toLocaleString()} photograph${left === 1 ? "" : "s"}`,
        onclick: () => analyze(),
      },
        el("span", { class: "grow" }, "Look for people"),
        el("span", { class: "n num" }, left.toLocaleString())));
    } else {
      rows.push(el("div", { class: "row", style: "color:var(--ink-faint)" }, "No faces found"));
    }
  }
  if (unnamed.length > 12) {
    rows.push(el("div", { class: "row", style: "color:var(--text-faint)" },
      `+${unnamed.length - 12} more groups`));
  }
  $("#people").replaceChildren(...(collapsed ? rows.slice(0, 3) : rows));
  const toggle = $("#people-toggle");
  toggle.hidden = rows.length <= 3;
  toggle.textContent = collapsed ? `Show all ${rows.length}` : "Show less";
  toggle.onclick = () => { S.peopleCollapsed = !S.peopleCollapsed; renderSidebar(); };

  // Trash is always listed, even when empty. Somewhere deleted photographs go is a
  // thing people look for before they delete anything, and a row that appears only
  // once you have used it cannot answer the question you had beforehand.
  const trash = src.folders.find(f => f.path === TRASH);
  const count = trash ? trash.count : 0;
  const tb = $("#trash-block");
  tb.hidden = false;
  const rows2 = [
    el("button", {
      class: "row", "aria-current": String(S.folder === TRASH),
      title: count ? `${count} photo${count === 1 ? "" : "s"} you can still restore`
                   : "Deleted photographs wait here until you empty it",
      onclick: () => count && selectFolder(TRASH),
    },
      el("span", { class: "tico" }, "\u{1F5D1}"),
      el("span", { class: "grow" }, "Trash"),
      el("span", { class: "n num" }, count ? String(count) : "empty")),
  ];
  if (count) {
    rows2.push(el("button", {
      class: "row", title: "Hand these to the system Trash — Blinkview can no longer undo it",
      onclick: emptyTrash,
    }, el("span", { class: "grow", style: "color:var(--ink-faint)" }, "Empty\u2026")));
  }
  $("#trash").replaceChildren(...rows2);

  const folders = src.folders.filter(f =>
    f.path !== "" && f.path !== TRASH && !f.path.startsWith(TRASH + "/"));
  fb.hidden = folders.length === 0;
  renderFolderTree(folders);
}

/* ---------------- folder tree ----------------
   Folders are the only way photographs are grouped (ADR-0009), so the tree is the
   main way around the library. Collapsed by default and expanded along the path to
   wherever you are, because a photo library is wide — many sibling day folders —
   and showing every level at once is noise, not information. */

/** Expanded folder paths. Remembered per library, since it is a property of that
    library's shape rather than a global preference. */
function expandedSet() {
  S.expanded ||= {};
  return (S.expanded[S.source] ||= new Set([""]));
}

/** Expansion is a view preference, not library data — it belongs in the browser,
    not in a file that travels with the photographs. */
function saveFolderState() {
  try {
    const out = {};
    for (const [src, set] of Object.entries(S.expanded || {})) out[src] = [...set];
    localStorage.setItem("blinkview.expanded", JSON.stringify(out));
  } catch { /* private window, or storage disabled — the tree still works */ }
}

function loadFolderState() {
  try {
    const raw = JSON.parse(localStorage.getItem("blinkview.expanded") || "{}");
    S.expanded = Object.fromEntries(Object.entries(raw).map(([k, v]) => [k, new Set(v)]));
  } catch { S.expanded = {}; }
}

function isExpanded(path) {
  return expandedSet().has(path);
}

function toggleFolder(path) {
  const ex = expandedSet();
  if (ex.has(path)) ex.delete(path); else ex.add(path);
  saveFolderState();
  renderSidebar();
}

/** A folder is visible when every ancestor above it is expanded. */
function folderVisible(path) {
  const parts = path.split("/");
  for (let i = 1; i < parts.length; i++) {
    if (!isExpanded(parts.slice(0, i).join("/"))) return false;
  }
  return true;
}

function renderFolderTree(folders) {
  // Expand the path to the current folder, so selecting one always reveals it.
  if (S.folder) {
    const parts = S.folder.split("/");
    for (let i = 1; i < parts.length; i++) expandedSet().add(parts.slice(0, i).join("/"));
  }
  const rows = folders.filter(f => folderVisible(f.path)).map(f => {
    const open = isExpanded(f.path);
    const twisty = f.has_children
      ? el("button", {
          class: "twisty" + (open ? " open" : ""), tabindex: "-1",
          "aria-label": open ? `Collapse ${f.name}` : `Expand ${f.name}`,
          onclick: e => { e.stopPropagation(); toggleFolder(f.path); }
        }, "\u203A")
      : el("span", { class: "twisty blank" });
    return el("button", {
      class: `row folderrow indent-${Math.min(f.depth - 1, 4)}`,
      "aria-current": String(S.folder === f.path),
      title: f.own && f.own !== f.count ? `${f.count} in total, ${f.own} directly here` : "",
      onclick: () => selectFolder(f.path)
    },
      twisty,
      el("span", { class: "grow" }, f.name),
      el("span", { class: "n num" }, String(f.count)));
  });
  $("#folders").replaceChildren(...rows);
}

/* ---------------- justified grid ----------------
   Real photo apps do not square-crop everything; rows are filled to a target height
   and scaled to the container so aspect ratios survive. */
function justify(photos, containerWidth, target = 200, gap = 3) {
  const rows = [];
  let row = [], ratioSum = 0;
  for (const p of photos) {
    const r = (p.width && p.height) ? p.width / p.height : 1.5;
    row.push({ p, r }); ratioSum += r;
    const needed = ratioSum * target + gap * (row.length - 1);
    if (needed >= containerWidth) {
      const h = (containerWidth - gap * (row.length - 1)) / ratioSum;
      rows.push({ items: row, h });
      row = []; ratioSum = 0;
    }
  }
  if (row.length) rows.push({ items: row, h: Math.min(target, (containerWidth - gap * (row.length - 1)) / ratioSum) });
  return rows;
}

const io = new IntersectionObserver(entries => {
  for (const e of entries) {
    if (!e.isIntersecting) continue;
    const img = e.target;
    if (img.dataset.src) { img.src = img.dataset.src; delete img.dataset.src; }
    io.unobserve(img);
  }
}, { rootMargin: "600px 0px" });

/* ---------------- virtualised grid ----------------
   Layout is computed for every photo (arithmetic only, cheap at any size) but DOM is
   created solely for the rows near the viewport. Without this a 20k-photo library
   builds 20k cells up front and the window stops responding. */

let LAYOUT = { blocks: [], height: 0, width: 0 };
const ROW_H = 200, GAP = 3, HEAD_H = 46, OVERSCAN = 900;

/* Which section a photograph falls under when grouping by folder.
   Sections are the immediate children of wherever you are, not full paths: standing in
   `Trip`, the useful headings are `Greece Day1` and `Swiss Day1`, not the same prefix
   repeated on every row. Photographs sitting loose in the selected folder get their own
   section so they are not silently absent. */
function sectionFor(p) {
  const base = S.folder || "";
  const rel = base ? p.folder.slice(base.length).replace(/^\//, "") : p.folder;
  if (!rel) return base ? base.split("/").pop() : "Loose photos";
  return rel.split("/")[0];
}

function computeLayout(width) {
  const blocks = [];
  let y = 0;
  // A custom arrangement *is* the order. Grouping it by day or folder would re-sort
  // what someone placed by hand, so it draws as one run with no headings.
  if (S.sort === "custom" || S.peek) {
    for (const r of justify(S.view, width, ROW_H, GAP)) {
      blocks.push({ kind: "row", y, h: r.h, items: r.items });
      y += r.h + GAP;
    }
    LAYOUT = { blocks, height: y, width };
    return;
  }
  const groups = new Map();
  for (const p of S.view) {
    const key = S.group === "folder" ? sectionFor(p) : (p.taken_at ? DAY(p.taken_at) : "Undated");
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(p);
  }
  // Grouped by folder, sections read alphabetically: a folder ordering that follows
  // capture date puts `Swiss Day1` above `Greece Day1` and looks arbitrary.
  const ordered = S.group === "folder"
    ? [...groups.entries()].sort((a, b) => a[0].localeCompare(b[0]))
    : groups.entries();
  for (const [day, items] of ordered) {
    blocks.push({ kind: "head", y, h: HEAD_H, day, n: items.length,
                  month: items[0]?.taken_at ? monthKey(items[0].taken_at) : "",
                  hashes: items.map(p => p.hash) });
    y += HEAD_H;
    for (const r of justify(items, width, ROW_H, GAP)) {
      blocks.push({ kind: "row", y, h: r.h, items: r.items });
      y += r.h + GAP;
    }
    y += 18; // breathing room between days
  }
  LAYOUT = { blocks, height: y, width };
}

function armLivePhoto(cell, p, event) {
  if (!p.liveVideo || event.button > 0 || matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  cell._liveHolding = true;
  clearTimeout(cell._liveTimer);
  cell._liveTimer = setTimeout(() => {
    if (!cell._liveHolding) return;
    cell._liveConsumed = true;
    const video = el("video", {
      class: "live-motion", src: photoUrl(p.liveVideo.path), muted: true,
      loop: true, playsinline: true, preload: "auto", "aria-hidden": "true",
    });
    video.muted = true;
    video.addEventListener("canplay", () => {
      if (!cell._liveHolding || !video.isConnected) return;
      cell.classList.add("live-playing");
      video.play().catch(() => {});
    }, { once: true });
    cell.prepend(video);
  }, 250);
}

function releaseLivePhoto(cell) {
  cell._liveHolding = false;
  clearTimeout(cell._liveTimer);
  cell.classList.remove("live-playing");
  const video = cell.querySelector(".live-motion");
  if (video) { video.pause(); video.removeAttribute("src"); video.load(); video.remove(); }
}

function cellFor(p, w, h) {
  const img = el("img", { alt: p.name, loading: "lazy", decoding: "async" });
  // Ask for the *original* with ?t=<hash>: the handler serves the cached thumbnail
  // or renders it now. The request fires only when the observer says the cell is
  // actually approaching the viewport — a fast slider flick used to fire requests
  // for a thousand pixels of rows on either side, cells the user never saw, and all
  // of them queued behind the decodes that were wanted.
  img.dataset.src = photoUrl(p.path) + "?t=" + p.hash;
  img.addEventListener("load", () => {
    img.classList.add("on");
    img.closest(".cell")?.classList.add("loaded");   // ends the shimmer
  }, { once: true });
  io.observe(img);
  // Arranging by hand only makes sense over a whole folder. A search result or a
  // person is a slice of several folders, and there is nowhere honest to record the
  // order of a slice (ADR-0009: a folder owns its photographs).
  const canArrange = canArrangeHere();
  const cell = el("div", {
    class: "cell" + (S.sel.has(p.hash) ? " sel" : ""),
    style: `width:${Math.max(40, w)}px;height:${h}px`,
    title: p.name,
    "data-hash": p.hash,
    draggable: "true",
    ondragstart: e => {
      releaseLivePhoto(e.currentTarget);
      if (!canArrange) {
        e.dataTransfer.effectAllowed = "copy";
        e.dataTransfer.setData("text/plain", p.name);
        const hashes = S.sel.has(p.hash) ? [...S.sel] : [p.hash];
        invoke("start_file_drag", { path: S.source, hashes }).catch(error => toast(String(error), "error"));
        return;
      }
      dragHash = p.hash;
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", p.hash);
      e.currentTarget.classList.add("dragging");
    },
    ondragend: canArrange ? (e => {
      dragHash = null;
      e.currentTarget.classList.remove("dragging");
      clearDropMarks();
    }) : null,
    ondragover: canArrange ? (e => {
      if (!dragHash || dragHash === p.hash) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = "move";
      const r = e.currentTarget.getBoundingClientRect();
      const after = e.clientX > r.left + r.width / 2;
      clearDropMarks();
      e.currentTarget.classList.add(after ? "dropafter" : "dropbefore");
    }) : null,
    ondragleave: canArrange ? (e => e.currentTarget.classList.remove("dropbefore", "dropafter")) : null,
    ondrop: canArrange ? (e => {
      e.preventDefault();
      const after = e.currentTarget.classList.contains("dropafter");
      clearDropMarks();
      if (dragHash && dragHash !== p.hash) reorderTo(dragHash, p.hash, after);
      dragHash = null;
    }) : null,
    onclick: e => {
      if (e.currentTarget._liveConsumed) { e.currentTarget._liveConsumed = false; return; }
      if (e.metaKey || e.ctrlKey) { toggleSel(p); return; }
      if (e.shiftKey) { rangeSel(p); return; }
      if (S.sel.size) { toggleSel(p); return; }
      openLightbox(p);
    },
    oncontextmenu: e => {
      e.preventDefault();
      if (!S.sel.has(p.hash)) { S.sel.clear(); toggleSel(p); }
      showCtx(e.clientX, e.clientY);
    },
    onpointerdown: p.liveVideo ? (e => armLivePhoto(e.currentTarget, p, e)) : null,
    onpointerup: p.liveVideo ? (e => releaseLivePhoto(e.currentTarget)) : null,
    onpointercancel: p.liveVideo ? (e => releaseLivePhoto(e.currentTarget)) : null,
    onpointerleave: p.liveVideo ? (e => releaseLivePhoto(e.currentTarget)) : null,
  }, img,
    el("button", {
      class: "pick", "aria-label": `Select ${p.name}`, tabindex: "-1",
      onclick: e => { e.stopPropagation(); toggleSel(p); }
    }, "\u2713"),
    p.kind === "video" ? el("span", { class: "play" }, "\u25B6") : null,
    p.liveVideo ? el("span", { class: "live-pill" }, "LIVE") : null,
    p.people.length ? el("span", { class: "badge" }, p.people.join(", ")) : null);
  return cell;
}

function paintViewport() {
  const main = $("#main"), stage = $("#stage");
  if (!LAYOUT.blocks.length) return;
  const top = main.scrollTop - OVERSCAN;
  const bottom = main.scrollTop + main.clientHeight + OVERSCAN;

  const wanted = new Set();
  const frag = document.createDocumentFragment();
  LAYOUT.blocks.forEach((b, i) => {
    if (b.y + b.h < top || b.y > bottom) return;
    wanted.add(i);
    if (stage.querySelector(`[data-b="${i}"]`)) return;
    const node = b.kind === "head"
      ? el("div", { class: "dayhead", "data-b": i, style: `top:${b.y}px`,
          title: `Select these ${b.n}`,
          onclick: () => selectGroup(b.hashes) },
          el("b", {}, b.day), el("span", { class: "num" }, String(b.n)),
          el("span", { class: "headpick" }, "\u2713"))
      : el("div", { class: "jrow", "data-b": i, style: `top:${b.y}px;height:${b.h}px` },
          b.items.map(({ p, r }) => cellFor(p, r * b.h, b.h)));
    frag.append(node);
  });
  stage.append(frag);
  for (const n of [...stage.children]) {
    const i = Number(n.dataset.b);
    if (!wanted.has(i)) {
      // Detached cells must leave the observer too: an observed-then-removed image
      // is kept alive by it, and a fast scroller would accumulate hundreds.
      n.querySelectorAll("img[data-src]").forEach(im => io.unobserve(im));
      n.remove();
    }
  }
}

function renderGrid() {
  const stage = $("#stage");
  if (!S.source) return renderWelcome();
  if (!S.view.length) {
    stage.className = "";
    stage.style.height = "";
    if (S.loading && !S.photos.length) {
      stage.replaceChildren(el("div", { class: "welcome" },
        el("div", { class: "art pulse" }, el("span", {}, "\u25F4")),
        el("h2", {}, "Opening\u2026")));
      return;
    }
    const scanning = S.busy[S.source]?.op === "scan";
    if (scanning && !S.photos.length) {
      const b = S.busy[S.source];
      const pct = b.total ? Math.round((b.done / b.total) * 100) : 0;
      stage.replaceChildren(el("div", { class: "welcome" },
        el("div", { class: "art pulse" }, el("span", {}, "\u25F4")),
        el("h2", {}, "Indexing this folder"),
        el("p", {}, b.total
          ? `${b.done.toLocaleString()} of ${b.total.toLocaleString()} files so far. Photographs appear as they are found — you can keep using the other folders.`
          : "Reading the folder. Photographs appear as they are found."),
        el("div", { class: "idxbar" }, el("span", { style: `width:${pct}%` }))));
      return;
    }
    // A semantic query in flight has not answered yet, so "no photos match" would be
    // a wrong answer shown before the right one arrives.
    const looking = S.semantic && S.semantic.state === "busy";
    stage.replaceChildren(el("div", { class: "welcome" },
      el("div", { class: looking ? "art pulse" : "art" }, el("span", {}, looking ? "✨" : "◇")),
      el("h2", {}, looking ? "Looking\u2026" : "Nothing here yet"),
      el("p", {}, looking
        ? `Reading what your photos show, for \u201C${S.semantic.query}\u201D.`
        : S.photos.length
          ? "No photos match this filter."
          : "This folder has no photos Blinkview can read, or it has not finished indexing."),
      !looking && S.photos.length
        ? el("button", { class: "btn ghost", onclick: () => selectSource(S.source) }, "Show all photos")
        : null));
    return;
  }
  stage.className = "virt";
  computeLayout(stage.clientWidth || $("#main").clientWidth - 48 || 1000);
  stage.style.height = LAYOUT.height + "px";
  stage.replaceChildren();
  // Only a deliberate move resets the scroll. Background work — a scan finding more
  // photographs, a watcher noticing a change — refreshes the grid underneath someone
  // who is reading it, and yanking them to the top for that is maddening.
  if (S.resetScroll) {
    $("#main").scrollTop = 0;
    S.resetScroll = false;
  }
  paintViewport();
}

function renderWelcome() {
  // The mark is defined once in the titlebar; clone it so gradient ids stay unique.
  const mark = document.querySelector(".logomark")?.cloneNode(true);
  $("#stage").replaceChildren(el("div", { class: "welcome" },
    el("div", { class: "art" }, mark || "◎"),
    el("h2", {}, "Your folders, your photos"),
    el("p", {}, "Blinkview reads folders you already have. Nothing is copied into a database, and nothing moves unless you ask. Add a folder to begin."),
    el("button", { class: "btn", onclick: addSource }, "Add a folder")));
}

/* ---------------- selection ---------------- */
function toggleSel(p) {
  S.sel.has(p.hash) ? S.sel.delete(p.hash) : S.sel.add(p.hash);
  S.lastIndex = S.view.findIndex(x => x.hash === p.hash);
  paintSel();
}
function rangeSel(p) {
  const i = S.view.findIndex(x => x.hash === p.hash);
  const a = S.lastIndex < 0 ? i : S.lastIndex;
  for (let k = Math.min(a, i); k <= Math.max(a, i); k++) S.sel.add(S.view[k].hash);
  paintSel();
}
function clearSel() { S.sel.clear(); paintSel(); }

/** Select or deselect a whole day or folder section by its heading. */
function selectGroup(hashes) {
  const all = hashes.every(h => S.sel.has(h));
  for (const h of hashes) all ? S.sel.delete(h) : S.sel.add(h);
  // Anchor further shift-selection at the end of the group just handled.
  S.lastIndex = S.view.findIndex(x => x.hash === hashes[hashes.length - 1]);
  paintSel();
}

/* Shift+arrows extend the selection from the last touched photo, the way a file
   manager does. Plain arrows move the anchor without selecting, so you can walk to a
   starting point and then extend. */
function moveSel(delta, extend) {
  if (!S.view.length) return;
  const from = S.lastIndex < 0 ? 0 : S.lastIndex;
  // Up/down should cross the row the cursor is actually in. Rows vary — a day with two
  // photographs is a two-wide row — so taking the width of the first row moves by the
  // wrong amount everywhere else.
  const here = S.view[from];
  const row = LAYOUT.blocks.find(b => b.kind === "row" && b.items.some(x => x.p.hash === here.hash));
  const per = row?.items.length || 1;
  const step = Math.abs(delta) === 2 ? per * (delta / 2) : delta;
  const to = Math.max(0, Math.min(S.view.length - 1, from + step));
  if (extend) {
    const [a, b] = to > from ? [from, to] : [to, from];
    for (let k = a; k <= b; k++) S.sel.add(S.view[k].hash);
  }
  S.lastIndex = to;
  paintSel();
  // Keep the moving edge on screen.
  const cell = document.querySelector(`.cell[data-hash="${S.view[to].hash}"]`);
  if (cell) cell.scrollIntoView({ block: "nearest" });
  else {
    const blk = LAYOUT.blocks.find(b => b.kind === "row" && b.items.some(x => x.p.hash === S.view[to].hash));
    if (blk) $("#main").scrollTop = Math.max(0, blk.y - 120);
  }
}
function paintSel() {
  // Only mounted cells exist; the rest pick up their state when they scroll in.
  for (const c of document.querySelectorAll(".cell"))
    c.classList.toggle("sel", S.sel.has(c.dataset.hash));
  const bar = $("#selbar");
  bar.hidden = S.sel.size === 0;
  $("#selcount").textContent = `${S.sel.size} selected`;
  $("#sel-untag").hidden = !S.person;
  if (S.person) $("#sel-untag").textContent = `Not ${S.person}`;
  const inTrash = S.folder === TRASH;
  $("#sel-restore").hidden = !inTrash;
  $("#sel-delete").hidden = inTrash;
  const reason = S.peek ? `“${S.peek.name}” is a read-only peek. Keep the folder to make changes.` : "";
  for (const id of ["#sel-untag", "#sel-date", "#sel-restore", "#sel-delete"]) {
    const button = $(id);
    button.disabled = !!S.peek;
    button.title = reason;
  }
}

function dateInputValue(photo) {
  if (!photo?.taken_at) return "";
  return new Date(photo.taken_at * 1000).toISOString().slice(0, 16);
}

async function setDateTimePrompt(hashes = [...S.sel]) {
  if (!hashes.length) return;
  const first = S.photos.find(p => hashes.includes(p.hash));
  const chosen = await new Promise(resolve => {
    let d;
    const input = el("input", {
      class: "nameinput", type: "datetime-local", step: "1",
      value: dateInputValue(first), "aria-label": "Correct capture date and time",
      onkeydown: e => { e.stopPropagation(); if (e.key === "Enter" && e.currentTarget.value) d.done(e.currentTarget.value); },
    });
    d = dialogFrame("Set Date & Time", [
      el("p", { class: "asktext" },
        hashes.length === 1
          ? "This writes the corrected capture time into the JPEG itself, so it survives a rescan."
          : `The same date and time will be written into all ${hashes.length} selected files. Unsupported files stay untouched.`),
      input,
      el("div", { class: "askrow" },
        el("button", { class: "btn ghost", onclick: () => d.done(null) }, "Cancel"),
        el("button", { class: "btn", onclick: () => d.done(input.value) }, "Write to file")),
    ]);
    d.attach(resolve);
    document.addEventListener("keydown", d.onKey, true);
    document.body.append(d.box);
    setTimeout(() => input.focus(), 40);
  });
  if (!chosen) return;
  const msg = await busy(`Writing capture time into ${hashes.length}…`,
    () => invoke("set_photo_datetime", { path: S.source, hashes, datetime: chosen }), S.source);
  toast(msg, "ok");
  clearSel();
  await refreshSources();
  await loadPhotos();
}

async function shareHashes(hashes) {
  if (!hashes?.length) return;
  try { await invoke("share_photos", { path: S.source, hashes }); }
  catch (e) { toast(String(e), "error"); }
}

/* ---------------- context menu ---------------- */
function showCtx(x, y) {
  const menu = $("#ctx");
  const n = S.sel.size;
  const one = n === 1 ? S.view.find(p => S.sel.has(p.hash)) : null;
  const item = (label, key, fn, cls = "") =>
    el("button", { class: cls, role: "menuitem", onclick: () => { hideCtx(); fn(); } },
      el("span", {}, label), key ? el("span", { class: "k" }, key) : null);

  const items = [];
  if (one) items.push(item("Open", "↩", () => openLightbox(one)));
  if (S.peek) {
    items.push(item(`Share ${n}…`, "", () => shareHashes([...S.sel])));
    items.push(el("hr"));
    items.push(el("button", {
      class: "readonly", role: "menuitem", disabled: true,
      title: `“${S.peek.name}” is a read-only peek. Keep the folder to make changes.`,
    }, "Read-only peek · keep to make changes"));
    menu.replaceChildren(...items);
    placeContextMenu(menu, x, y);
    return;
  }
  if (one) items.push(item("Rename…", "", () => renamePhoto(one)));
  if (S.person) items.push(item(`Not ${S.person}`, "", untagSelected));
  items.push(el("hr"));
  items.push(item(`Move ${n} to\u2026`, "", moveSelectedPrompt));
  items.push(item(`Colour ${n}\u2026`, "", colourSelectedPrompt));
  items.push(item(`Set Date & Time for ${n}\u2026`, "", () => setDateTimePrompt([...S.sel])));
  items.push(item(`Where was this?\u2026`, "", placePrompt));
  items.push(item(`Share ${n}\u2026`, "", () => shareHashes([...S.sel])));
  items.push(item(`Strip metadata from ${n}\u2026`, "", stripSelectedPrompt));
  items.push(el("hr"));
  if (S.folder === TRASH) items.push(item(`Restore ${n}`, "", restoreSelected));
  else {
    items.push(item(`Move ${n} to Trash`, "⌫", deleteSelected, "danger"));
    // The keystroke stays Trash — a key should not ask a question — so choosing
    // somewhere else is its own item.
    items.push(item(`Delete ${n} to\u2026`, "", () => deleteSelectedTo(), "danger"));
  }
  menu.replaceChildren(...items);

  placeContextMenu(menu, x, y);
}

function placeContextMenu(menu, x, y) {
  menu.hidden = false;
  const r = menu.getBoundingClientRect();
  menu.style.left = Math.min(x, innerWidth - r.width - 8) + "px";
  menu.style.top = Math.min(y, innerHeight - r.height - 8) + "px";
}
function hideCtx() { $("#ctx").hidden = true; }

function showSourceCtx(event, source) {
  event.preventDefault();
  event.stopPropagation();
  const menu = $("#ctx");
  const item = (label, fn, cls = "") => el("button", {
    class: cls, role: "menuitem", onclick: () => { hideCtx(); fn(); },
  }, el("span", {}, label));
  menu.replaceChildren(
    item(source.shallow ? "Include subfolders" : "This folder only",
      () => changeSourceDepth(source, !source.shallow)),
    el("hr"),
    item(`Remove ${source.name} from Blinkview`, () => removeSource(source.path), "danger"));
  placeContextMenu(menu, event.clientX, event.clientY);
}

async function changeSourceDepth(source, shallow) {
  const action = shallow ? "show only files directly in it" : "include its subfolders";
  const ok = await confirmDialog(
    `Change “${source.name}”?`,
    `Blinkview will ${action}. Ratings and labels stay with their photographs.`,
    shallow ? "This folder only" : "Include subfolders");
  if (!ok) return;
  try {
    const info = await busy("Updating folder depth…",
      () => invoke("set_source_depth", { path: source.path, shallow }), source.path);
    S.sources = S.sources.map(s => s.path === source.path ? info : s);
    renderSidebar();
    if (S.source === source.path) await loadPhotos();
  } catch { /* busy already reports the backend's reason */ }
}

/* ---------------- editing ---------------- */
/** Apply one colour preset to everything selected.
    The preview is the preset's own name and what it does — there is no per-photograph
    preview, because forty previews is not a preview. The originals are kept, which is
    what makes the batch safe to try. */
async function colourSelectedPrompt() {
  // A preset writes a new JPEG over each file, which a camera RAW never accepts
  // (ADR-0018). Drop them here and say so, rather than reporting failures afterwards.
  const raws = S.photos.filter(p => S.sel.has(p.hash) && isRaw(p.path));
  const hashes = [...S.sel].filter(h => !raws.some(r => r.hash === h));
  if (!hashes.length) {
    return toast(raws.length
      ? "Camera RAW files are shown from the preview inside them and never rewritten"
      : "Nothing selected");
  }
  const chosen = await new Promise(resolve => {
    let d;
    d = dialogFrame(`Colour ${hashes.length} photograph${hashes.length === 1 ? "" : "s"}`, [
      el("p", { class: "asktext" },
        "Each original is kept in Originals/, so this can be undone by hand if it is wrong."
        + (raws.length ? ` ${raws.length} camera RAW ${raws.length === 1 ? "file is" : "files are"} left alone.` : "")),
      el("div", { class: "movepicks" }, PRESETS.map(([name, vals]) =>
        el("button", { class: "sugg-pill", onclick: () => d.done(vals) }, name))),
      el("div", { class: "askrow" },
        el("button", { class: "btn ghost", onclick: () => d.done(null) }, "Cancel")),
    ]);
    d.attach(resolve);
    document.addEventListener("keydown", d.onKey, true);
    document.body.append(d.box);
  });
  if (!chosen) return;
  const msg = await busy(`Colouring ${hashes.length}\u2026`, () => invoke("edit_photos", {
    path: S.source, hashes,
    edit: {
      rotate: "none", straighten: 0, flip_h: false, flip_v: false, crop: null,
      keep_original: true,
      adjust: {
        brightness: chosen.brightness / 100,
        contrast: chosen.contrast / 100,
        saturation: chosen.saturation / 100,
      },
    },
  }), S.source);
  toast(msg, "ok");
  clearSel(); await refreshSources(); await loadPhotos();
}

/** Remove what the selected photographs say about how they were taken. */
async function stripSelectedPrompt() {
  const hashes = [...S.sel];
  if (!hashes.length) return;
  const ok = await confirmDialog(
    `Strip metadata from ${hashes.length} photograph${hashes.length === 1 ? "" : "s"}?`,
    "Camera, lens, exposure and any location are removed. The pixels are untouched — " +
    "nothing is re-encoded — and each original is kept in Originals/, because the date " +
    "Blinkview sorts by comes from that metadata. Videos and HEIC are left alone.",
    "Strip");
  if (!ok) return;
  const msg = await busy(`Stripping ${hashes.length}\u2026`,
    () => invoke("strip_metadata", { path: S.source, hashes }), S.source);
  toast(msg, "ok");
  clearSel(); await refreshSources(); await loadPhotos();
}

/** Delete somewhere other than Trash. Still a journalled move inside the library, so
    ⌘Z reverses it exactly as the Trash route does. */
async function deleteSelectedTo() {
  const hashes = [...S.sel];
  if (!hashes.length) return;
  const dest = await pickFolderPrompt(`Delete ${hashes.length} to`, "Deleted-2026");
  if (!dest) return;
  const msg = await busy(`Moving ${hashes.length} to ${dest}…`,
    () => invoke("delete_photos", { path: S.source, hashes, dest }));
  toast(msg + " — press ⌘Z to undo", "ok");
  clearSel(); await refreshSources(); await reload();
}

async function deleteSelected() {
  const hashes = [...S.sel];
  if (!hashes.length) return;
  const msg = await busy(`Moving ${hashes.length} to Trash…`,
    () => invoke("delete_photos", { path: S.source, hashes }));
  toast(msg + " — press ⌘Z to undo", "ok");
  // Counts live in the sidebar: the folder that just got lighter and the Trash that
  // got its first photograph. The plan already updated the index, so this is a
  // re-read, not a rescan — the same cost as clicking the source.
  clearSel(); await refreshSources(); await reload();
}
/* ---------------- saved searches ----------------
   What albums were used for across folders (ADR-0009). Only the query is stored, so a
   saved search stays current as photographs are added — an album would need
   remembering. */

async function refreshSearches() {
  const t = beginLoad("searches");
  if (!t.source) { S.searches = []; return; }
  let searches;
  try { searches = await invoke("list_searches", { path: t.source }); } catch { searches = []; }
  if (!stillCurrent(t)) return;
  S.searches = searches;
  renderSearches();
}

function renderSearches() {
  const block = $("#searches-block"), list = $("#searches");
  if (!block) return;
  block.hidden = !S.searches.length;
  if (!S.searches.length) return;
  list.replaceChildren(...S.searches.map(sv => {
    const row = el("div", { class: "row srow", "aria-current": String($("#search").value.trim() === sv.query) },
      el("button", { class: "grow srun", title: sv.query,
        onclick: () => { $("#search").value = sv.query; applyFilter(); renderFilters(); renderSearches(); syncClear(); } },
        sv.name),
      // Editing and removing are visible actions rather than a right-click nobody
      // discovers; they appear on hover so the list stays quiet.
      el("button", { class: "mini sact", title: "Edit this search",
        onclick: () => editSearchPrompt(sv) }, "\u270E"),
      el("button", { class: "mini sact", title: "Delete this search",
        onclick: () => deleteSearch(sv) }, "\u2715"));
    return row;
  }));
}

async function deleteSearch(sv) {
  const ok = await confirmDialog("Forget this search?",
    `\u201C${sv.name}\u201D will be removed. The photographs are untouched.`, "Forget");
  if (!ok) return;
  await invoke("save_search", { path: S.source, name: sv.name, query: "" });
  await refreshSearches();
  toast(`Forgot \u201C${sv.name}\u201D`, "ok");
}

/** Edit a saved search's name and its query. Renaming replaces the old entry. */
function editSearchPrompt(sv) {
  document.querySelector(".namebar")?.remove();
  const name = el("input", { class: "nameinput", type: "text", value: sv.name,
    "aria-label": "Search name" });
  const query = el("input", { class: "nameinput grow", type: "text", value: sv.query,
    "aria-label": "Search query" });
  const save = async () => {
    const n = name.value.trim(), q = query.value.trim();
    if (!n || !q) { toast("A name and a query are both needed", "info"); return; }
    if (n !== sv.name) await invoke("save_search", { path: S.source, name: sv.name, query: "" });
    await invoke("save_search", { path: S.source, name: n, query: q });
    bar.remove();
    await refreshSearches();
    toast(`Saved \u201C${n}\u201D`, "ok");
  };
  const bar = el("div", { class: "namebar" },
    el("span", {}, "Saved search"), name, query,
    el("button", { class: "btn sm", onclick: save }, "Save"),
    el("button", { class: "mini", onclick: () => bar.remove(), title: "Cancel" }, "\u2715"));
  for (const i of [name, query]) {
    i.onkeydown = e => {
      if (e.key === "Escape") bar.remove();
      if (e.key === "Enter") save();
    };
  }
  $("#main").prepend(bar);
  setTimeout(() => name.focus(), 60);
}

/** Keep the current query under a name. */
function saveSearchPrompt() {
  const q = $("#search").value.trim();
  if (!q) { toast("Type a search first, then save it", "info"); return; }
  document.querySelector(".namebar")?.remove();
  const bar = el("div", { class: "namebar" },
    el("span", { class: "grow" }, `Save \u201C${q}\u201D as`),
    el("input", {
      class: "nameinput", type: "text", placeholder: "Name this search",
      "aria-label": "Search name",
      onkeydown: async e => {
        if (e.key === "Escape") { bar.remove(); return; }
        if (e.key !== "Enter") return;
        const v = e.target.value.trim();
        if (!v) return;
        bar.remove();
        await invoke("save_search", { path: S.source, name: v, query: q });
        await refreshSearches();
        toast(`Saved \u201C${v}\u201D`, "ok");
      }
    }),
    el("button", { class: "mini", onclick: () => bar.remove(), title: "Cancel" }, "\u2715"));
  $("#main").prepend(bar);
  setTimeout(() => bar.querySelector("input")?.focus(), 60);
}

/* Move a selection into a folder. Existing folders first — typing a name every time is
   how a library grows three spellings of the same trip. */
function moveSelectedPrompt() {
  const hashes = [...S.sel];
  if (!hashes.length) return;
  document.querySelector(".namebar")?.remove();

  const go = async dest => {
    document.querySelector(".namebar")?.remove();
    const view = await invoke("plan_move", { path: S.source, hashes, dest });
    if (!view.moves.length) {
      toast(view.skipped.length ? `Nothing moved: ${view.skipped[0][1]}` : "Already there", "info");
      return;
    }
    const extra = view.skipped.length ? `  ${view.skipped.length} left alone (name already taken).` : "";
    const ok = await confirmDialog("Move these photos?",
      `${view.moves.length} will move into \u201C${dest}\u201D.${extra}`, "Move");
    if (!ok) return;
    const msg = await busy("Moving\u2026", () => invoke("apply_move", { path: S.source, hashes, dest }));
    toast(msg + " \u2014 \u2318Z to undo", "ok");
    clearSel();
    await refreshSources(); await loadPhotos();
  };

  const folders = (S.sources.find(x => x.path === S.source)?.folders || [])
    .filter(f => f.path && f.path !== TRASH && f.path !== S.folder);
  const bar = el("div", { class: "namebar" },
    el("span", { class: "grow" }, `Move ${hashes.length} photo${hashes.length === 1 ? "" : "s"} to`),
    el("div", { class: "movepicks" }, folders.slice(0, 8).map(f =>
      el("button", { class: "sugg-pill", title: f.path, onclick: () => go(f.path) }, f.name))),
    el("input", {
      class: "nameinput", type: "text", placeholder: "or a new folder name",
      "aria-label": "Destination folder",
      onkeydown: e => {
        if (e.key === "Escape") { bar.remove(); return; }
        if (e.key !== "Enter") return;
        const v = e.target.value.trim();
        if (v) go(v);
      }
    }),
    el("button", { class: "mini", onclick: () => bar.remove(), title: "Cancel" }, "\u2715"));
  $("#main").prepend(bar);
  setTimeout(() => bar.querySelector("input")?.focus(), 60);
}

async function untagSelected() {
  const hashes = [...S.sel];
  if (!hashes.length || !S.person) return;
  const person = S.person;
  const msg = await busy(`Removing ${person}…`,
    () => invoke("untag_person", { path: S.source, person, hashes }));
  toast(msg, "ok");
  clearSel(); await refreshSources(); await reload();
}
async function renamePhoto(p) {
  const name = await promptDialog("Rename photo", p.name);
  if (!name || name === p.name) return;
  const msg = await busy("Renaming…",
    () => invoke("rename_photo", { path: S.source, hash: p.hash, name }));
  toast(msg, "ok"); await reload();
}
async function reload() { await loadPhotos(); }

async function restoreSelected() {
  const hashes = [...S.sel];
  if (!hashes.length) return;
  const msg = await busy("Restoring…", () => invoke("restore_photos", { path: S.source, hashes }));
  toast(msg, "ok");
  clearSel(); await refreshSources(); await reload();
}
async function emptyTrash() {
  const ok = await confirmDialog("Empty the Trash",
    "Move everything in Trash to the macOS Trash? Blinkview can no longer undo this — Finder can still recover the files.",
    "Empty Trash", true);
  if (!ok) return;
  const msg = await busy("Emptying Trash…", () => invoke("empty_trash", { path: S.source }));
  toast(msg, "ok");
  if (S.folder === TRASH) S.folder = null;
  await refreshSources(); await reload();
}

/* ---------------- lightbox ---------------- */
/* Two scopes, because both are wanted at different moments.
   By default the viewer steps through everything on screen, which is what every photo
   app does and what makes a rolled-up folder browse the way it looks. But standing in
   `Trip` and reaching a picture from `Greece Day3`, it is reasonable to want just that
   day — so the scope is a control rather than a rule, and it says which one it is. */
function setLbScope(mode) {
  const cur = S.lbList[S.lbIndex];
  if (!cur) return;
  S.lbScope = mode;
  // A folder always means the folder *and* everything beneath it — the same rule the
  // sidebar and the grid use (ADR-0009). Narrowing to a parent must not hide its
  // children.
  const list = mode === "folder"
    ? S.view.filter(q => inFolder(q.folder, cur.folder))
    : S.view.slice();
  if (!list.length) return;
  S.lbList = list;
  S.lbIndex = Math.max(0, list.findIndex(q => q.hash === cur.hash));
  paintLightbox();
}

function renderScope(p) {
  const el2 = $("#lb-folder");
  const narrowed = S.lbScope === "folder";
  const where = S.person
    ? `\u{1F464} ${S.person}`
    : (narrowed ? p.folder || "root" : S.folder || "everything in view");
  const count = `${S.lbIndex + 1} of ${S.lbList.length}`;
  // Offer the other scope only when it would actually be a different set.
  const otherSize = narrowed
    ? S.view.length
    : S.view.filter(q => inFolder(q.folder, p.folder)).length;
  const canSwitch = !S.person && otherSize !== S.lbList.length && otherSize > 0;

  el2.replaceChildren(document.createTextNode(`${where} \u00B7 ${count}`));
  if (!canSwitch) return;
  const b = el("button", {
    class: "lb-scope",
    title: narrowed
      ? "Step through everything on screen"
      : `Step through ${p.folder || "the root folder"} and anything inside it`,
  }, narrowed ? `\u2194 all ${S.view.length}` : `\u25A3 this folder (${otherSize})`);
  b.onclick = () => setLbScope(narrowed ? "view" : "folder");
  el2.append(b);
}

function openLightbox(photo) {
  // Step through exactly what is on screen, in the order it is shown.
  //
  // This used to fall back to the photograph's own folder when nothing was filtered,
  // which was the Picasa rule and stopped fitting once the grid began rolling up
  // subfolders: clicking a clip in a mixed, date-sorted grid walked `WhatsApp Video`
  // — thirty-seven videos and no photographs — so the arrow keys appeared to skip
  // every picture. It also ignored the sort, since that list came from S.photos in
  // backend order rather than S.view in the order displayed.
  //
  // The folder context that rule existed for is still there: the filmstrip shows the
  // neighbours either side of wherever you are.
  S.lbScope = "view";
  const list = S.view.slice();
  const at = list.findIndex(p => p.hash === photo.hash);
  openViewer(list, at >= 0 ? at : 0);
}

/* Any ordered set of photos can drive the viewer — the grid's view, a folder, or an
   Ask answer. */
function openViewer(list, index) {
  S.lbList = list;
  S.lbIndex = index;
  $("#lightbox").hidden = false;
  paintLightbox();
}
/* Stepping through a folder must not wait on a full-size decode.
   The thumbnail is already cached, so it goes up immediately and the 2000 px preview
   (rendered once per photograph, then a ~400 KB JPEG) replaces it when it arrives —
   if the viewer is still on that photograph. Holding an arrow key then costs a cached
   thumbnail per frame instead of a twelve-megapixel original. */
let lbLoadSeq = 0;

function showFull(p) {
  const img = $("#lb-img");
  const seq = ++lbLoadSeq;
  const full = photoUrl(p.path) + "?preview=" + p.hash;
  const thumb = photoUrl(p.path) + "?t=" + p.hash;

  img.classList.add("provisional");
  img.src = thumb;

  const hi = new Image();
  hi.decoding = "async";
  hi.onload = () => {
    if (seq !== lbLoadSeq) return;   // moved on; this photograph is no longer showing
    img.src = full;
    img.classList.remove("provisional");
  };
  hi.onerror = () => {
    if (seq !== lbLoadSeq) return;
    img.src = full;                  // let the <img> report the failure itself
    img.classList.remove("provisional");
  };
  hi.src = full;
}

/** Warm the neighbours, so the next arrow press is already decoded. */
function preloadAround(i) {
  for (const d of [1, -1, 2, -2]) {
    const q = S.lbList[(i + d + S.lbList.length) % S.lbList.length];
    if (!q || q.kind === "video") continue;
    const im = new Image();
    im.decoding = "async";
    im.src = photoUrl(q.path) + "?preview=" + q.hash;
  }
}

function mediaClock(seconds) {
  if (!Number.isFinite(seconds)) return "0:00";
  const whole = Math.max(0, Math.floor(seconds));
  const mins = Math.floor(whole / 60);
  return `${mins}:${String(whole % 60).padStart(2, "0")}`;
}

function mountVideoControls(video, stage) {
  const play = el("button", { "aria-label": "Pause" }, "Ⅱ");
  const seek = el("input", { class: "video-seek", type: "range", min: "0", max: "1000", value: "0", "aria-label": "Video position" });
  const time = el("span", { class: "video-time num" }, "0:00 / 0:00");
  const mute = el("button", { "aria-label": "Mute" }, "◖");
  const volume = el("input", { type: "range", min: "0", max: "1", step: ".05", value: "1", "aria-label": "Volume", style: "width:70px" });
  const full = el("button", { "aria-label": "Full screen" }, "⛶");
  const controls = el("div", { class: "video-controls", role: "group", "aria-label": "Video playback" },
    play, seek, time, mute, volume, full);
  const sync = () => {
    play.textContent = video.paused ? "▶" : "Ⅱ";
    play.setAttribute("aria-label", video.paused ? "Play" : "Pause");
    seek.value = video.duration ? String(Math.round(video.currentTime / video.duration * 1000)) : "0";
    time.textContent = `${mediaClock(video.currentTime)} / ${mediaClock(video.duration)}`;
    mute.textContent = video.muted || video.volume === 0 ? "×" : "◖";
    mute.setAttribute("aria-label", video.muted ? "Unmute" : "Mute");
  };
  play.onclick = () => video.paused ? video.play().catch(() => {}) : video.pause();
  video.onclick = play.onclick;
  seek.oninput = () => { if (video.duration) video.currentTime = Number(seek.value) / 1000 * video.duration; };
  mute.onclick = () => { video.muted = !video.muted; sync(); };
  volume.oninput = () => { video.volume = Number(volume.value); video.muted = false; sync(); };
  full.onclick = () => video.requestFullscreen?.();
  for (const event of ["loadedmetadata", "timeupdate", "play", "pause", "volumechange", "ended"])
    video.addEventListener(event, sync);
  stage.append(controls);
  sync();
}

function paintLightbox() {
  const p = S.lbList[S.lbIndex];
  if (!p) return;
  const stage = document.querySelector(".lb-stage");
  const isVideo = p.kind === "video";
  stage.querySelector("video")?.remove();
  stage.querySelector(".video-controls")?.remove();
  const img = $("#lb-img");
  img.hidden = isVideo;
  if (isVideo) {
    lbLoadSeq++;                     // cancel any full-size load still in flight
    const v = el("video", {
      id: "lb-video", src: photoUrl(p.path), autoplay: true,
      preload: "auto", playsinline: true,
    });
    stage.append(v);
    mountVideoControls(v, stage);
  } else {
    showFull(p);
  }
  resetZoom();
  $("#lb-name").textContent = p.name;
  $("#lb-meta").textContent = [
    p.taken_at ? `${DAY(p.taken_at)} · ${TIME(p.taken_at)}` : "Undated",
    p.width ? `${p.width}×${p.height}` : null,
    p.people.length ? p.people.join(", ") : (p.faces ? `${p.faces} face${p.faces > 1 ? "s" : ""}` : null),
  ].filter(Boolean).join("   ·   ");
  renderScope(p);
  // Rotate, flip, crop and adjust all end in a JPEG written over the source.
  const raw = isRaw(p.path);
  const readOnly = !!S.peek;
  for (const id of ["#lb-rot-l", "#lb-rot-r", "#lb-flip", "#lb-crop", "#lb-adjust"]) {
    const b = $(id);
    if (!b) continue;
    b.dataset.title ||= b.title;          // remembered before the reason replaces it
    b.disabled = raw || readOnly;
    b.title = readOnly
      ? `“${S.peek.name}” is a read-only peek. Keep this folder before editing.`
      : raw
        ? "A camera RAW is shown from the preview inside it, and never rewritten"
        : b.dataset.title;
  }
  for (const id of ["#lb-rename", "#lb-delete"]) {
    const b = $(id);
    b.dataset.title ||= b.title;
    b.disabled = readOnly;
    b.title = readOnly
      ? `“${S.peek.name}” is a read-only peek. Keep this folder before changing files.`
      : b.dataset.title;
  }

  // Render a window around the current photo rather than the whole folder. A few
  // hundred thumbnails in one flex row is both slow and, on WKWebView, enough to
  // destabilise the layout of the sibling image.
  const strip = $("#lb-strip");
  const WIN = 30;
  const from = Math.max(0, S.lbIndex - WIN);
  const to = Math.min(S.lbList.length, S.lbIndex + WIN + 1);
  // Rebuilding sixty-one <img> elements on every arrow press is most of the cost of
  // stepping. The window only has to change when the cursor nears its edge; the rest
  // of the time moving the highlight is enough.
  const fresh = strip.dataset.from !== String(from) || strip.dataset.to !== String(to);
  if (fresh) {
    strip.dataset.from = String(from);
    strip.dataset.to = String(to);
    strip.replaceChildren(...S.lbList.slice(from, to).map((q, k) => {
      const i = from + k;
      return el("img", {
        src: photoUrl(q.path) + "?t=" + q.hash, alt: q.name, loading: "lazy", decoding: "async",
        "data-i": String(i),
        onclick: () => { S.lbIndex = i; paintLightbox(); }
      });
    }));
  }
  for (const n of strip.children) {
    n.setAttribute("aria-current", String(Number(n.dataset.i) === S.lbIndex));
  }
  strip.querySelector('[aria-current="true"]')?.scrollIntoView({ inline: "center", block: "nearest" });
  preloadAround(S.lbIndex);
  paintStars();
  if (!$("#infopanel").hidden) { $("#infopanel").hidden = true; toggleInfo(); }
}
function closeLightbox() {
  $("#lightbox").hidden = true;
  $("#lb-img").src = "";
  document.querySelector(".lb-stage")?.querySelector("video")?.remove();
  document.querySelector(".lb-stage")?.querySelector(".video-controls")?.remove();
  resetZoom();
  S.edit = null;
  if (S.cropping) endCrop(false);
  $("#adjustbar").hidden = true;
  $("#infopanel").hidden = true;
  $("#straighten").value = 0;
  $("#straighten-val").textContent = "0\u00B0";
  for (const k of ["brightness", "contrast", "saturation"]) {
    $(`#adj-${k}`).value = 0;
    $(`#adj-${k}-val`).textContent = "0";
  }
  $("#lb-img").style.filter = "";
  applyEditPreview();
}

/* ---------------- zoom and pan ----------------
   Zoom is applied as a transform on the image rather than by resizing it, so panning
   costs nothing and the browser keeps the decoded bitmap. Panning is clamped to the
   image's own edges so it can never be dragged off screen and lost. */
const MAX_ZOOM = 8;

function resetZoom() {
  S.zoom = 1; S.panX = 0; S.panY = 0;
  applyZoom();
}

function clampPan() {
  const img = $("#lb-img");
  const stage = document.querySelector(".lb-stage");
  if (!img || !stage) return;
  // Overflow at the current zoom, halved because the image is centred.
  const ox = Math.max(0, (img.clientWidth * S.zoom - stage.clientWidth) / 2);
  const oy = Math.max(0, (img.clientHeight * S.zoom - stage.clientHeight) / 2);
  S.panX = Math.max(-ox, Math.min(ox, S.panX));
  S.panY = Math.max(-oy, Math.min(oy, S.panY));
}

function applyZoom() {
  const img = $("#lb-img");
  if (!img) return;
  clampPan();
  img.style.transform = `translate(${S.panX}px, ${S.panY}px) scale(${S.zoom})`;
  img.style.cursor = S.zoom > 1 ? "grab" : "";
  const lb = $("#lightbox");
  lb.dataset.zoomed = S.zoom > 1 ? "1" : "0";
  $("#lb-zoom").textContent = S.zoom > 1 ? `${Math.round(S.zoom * 100)}%` : "";
}

/* Zoom about the pointer, so the point under the cursor stays put. */
function zoomAt(factor, clientX, clientY) {
  const img = $("#lb-img");
  if (!img) return;
  const before = S.zoom;
  S.zoom = Math.max(1, Math.min(MAX_ZOOM, S.zoom * factor));
  if (S.zoom === before) return;
  const r = img.getBoundingClientRect();
  const cx = clientX - (r.left + r.width / 2);
  const cy = clientY - (r.top + r.height / 2);
  const ratio = S.zoom / before;
  S.panX = S.panX - cx * (ratio - 1);
  S.panY = S.panY - cy * (ratio - 1);
  if (S.zoom === 1) { S.panX = 0; S.panY = 0; }
  applyZoom();
}
function step(d) {
  if (S.lbIndex < 0) return;
  S.lbIndex = (S.lbIndex + d + S.lbList.length) % S.lbList.length;
  paintLightbox();
}

/* ---------------- data ---------------- */
async function refreshSources() {
  S.sources = await invoke("list_sources");
  renderSidebar();
}

async function refreshPeople() {
  const t = beginLoad("people");
  if (!t.source) return;
  let people;
  try {
    people = await invoke("people_overview", { path: t.source, distance: 0.55 });
  } catch { people = { entries: [], dismissed: 0 }; }
  if (!stillCurrent(t)) return;
  S.people = people.entries || [];
  S.dismissed = people.dismissed || 0;
  await refreshAlbums();
  await refreshSearches();
  await refreshSources();
}
async function refreshAlbums() {
  const t = beginLoad("albums");
  if (!t.source) { S.albums = []; return; }
  let albums;
  try { albums = await invoke("list_albums", { path: t.source }); } catch { albums = []; }
  if (!stillCurrent(t)) return;
  S.albums = albums;
  renderAlbums();
}

/** Albums were removed (ADR-0009). A library that still has them is offered the
    migration to folders rather than having the grouping quietly stripped. */
function renderAlbums() {
  const block = $("#albums-block"), list = $("#albums");
  if (!block) return;
  block.hidden = !S.albums.length;
  if (!S.albums.length) return;
  const total = S.albums.reduce((n, a) => n + a[1], 0);
  list.replaceChildren(
    el("div", { class: "migrate-note" },
      `${S.albums.length} album${S.albums.length === 1 ? "" : "s"} \u00B7 ${total} photos`),
    el("button", { class: "row migrate", onclick: migrateAlbums },
      el("span", { class: "grow" }, "Turn into folders\u2026")));
}

/** Show exactly what the migration would do before doing any of it. */
async function migrateAlbums() {
  const m = await invoke("plan_album_migration", { path: S.source });
  if (!m.moves) { toast("Nothing to move", "info"); return; }
  const lines = [
    `${m.moves} photo${m.moves === 1 ? "" : "s"} will move into ` +
      `${m.folders.length} folder${m.folders.length === 1 ? "" : "s"}: ` +
      m.folders.map(([n, c]) => `${n} (${c})`).join(", "),
  ];
  if (m.renamed.length) {
    lines.push("Renamed for the filesystem: " +
      m.renamed.map(([a, b]) => `\u201C${a}\u201D \u2192 \u201C${b}\u201D`).join(", "));
  }
  if (m.skipped.length) {
    // A photo in two albums can only live in one folder — say so rather than guess.
    lines.push(`${m.skipped.length} left where they are (a file lives in one folder): ` +
      m.skipped.slice(0, 3).map(([f]) => f).join(", ") +
      (m.skipped.length > 3 ? "\u2026" : ""));
  }
  // Undo reverses the moves but not the cleared labels, so say so rather than let
  // someone discover it by undoing.
  lines.push("Undo puts the photographs back, but the album names are not restored.");
  const ok = await confirmDialog("Turn albums into folders?", lines.join("  \u00B7  "), "Move them");
  if (!ok) return;
  const msg = await busy("Moving\u2026", () => invoke("apply_album_migration", { path: S.source }));
  toast(msg + " \u2014 press \u2318Z to undo", "ok");
  await refreshSources(); await refreshAlbums(); await loadPhotos();
}

/* ---------------- load ordering ----------------
   Async loads race, and whichever `invoke` returns last wins. That is how clicking one
   library could leave another's photographs on screen: the filesystem watcher fires a
   reload in the background while a source switch is in flight, and the slower result
   overwrites the newer one.

   Every load records which library it was started for and its position in that queue.
   A result that is no longer current is discarded instead of painted, so what is shown
   always belongs to the library named in the breadcrumb. */

const loadSeq = {};

/* Keyed by kind *and* source. A single counter per kind was wrong in a way that showed
   up as an empty grid: a background load for the library you just left would bump the
   counter, discarding the load for the library you just opened — and then discard
   itself too, because its own source no longer matched. Nothing ever set the
   photographs, so "Nothing here yet" stayed up for good. */
function beginLoad(kind) {
  const key = `${kind}\u0000${S.source || ""}`;
  loadSeq[key] = (loadSeq[key] || 0) + 1;
  return { key, seq: loadSeq[key], source: S.source };
}

/** False once a newer load of the same kind *for the same source* started. */
function stillCurrent(t) {
  return t.seq === loadSeq[t.key] && t.source === S.source;
}

async function loadPhotos() {
  const t = beginLoad("photos");
  if (!t.source) return;
  S.loading = true;
  // Ask for the library this load is *for*, not whatever S.source happens to be by
  // the time the request is built.
  const photos = hydrate(await invoke(S.peek ? "peek_photos" : "photos",
    S.peek ? { path: t.source } : { path: t.source, folder: null, person: null }));
  if (!stillCurrent(t)) return;
  S.loading = false;
  S.photos = photos;
  applyFilter();
}
/* ---------------- search ----------------
   A date is the most natural way to look for a photo, so the query is parsed for date
   parts first and any combination is allowed: a year, a month, a day, or any mix of
   them ("august", "2026", "aug 2026", "23 august", "23 aug 2026", "2026-08-23").
   Whatever is left over is matched as text against filename, folder and people. */

const MONTHS = ["january","february","march","april","may","june",
                "july","august","september","october","november","december"];
const LABEL_NAMES = ["red","orange","yellow","green","blue","purple","grey"];

function parseQuery(q, people = [], albums = []) {
  const want = {
    year: null, month: null, day: null,
    person: null, album: null, kind: null, ext: null,
    minRating: null, label: null, fav: false,
  };
  const text = [];
  const colours = [];
  const names = people.filter(Boolean).map(n => n.toLowerCase());
  const albumNames = albums.map(a => a.toLowerCase());

  // Names are consumed as whole phrases before anything is tokenised. "Greece 2026" is
  // one album, not a year and a stray word, and "Anna Maria" is one person. Longest
  // first, so a longer name wins over a shorter one it contains.
  let rest = q.toLowerCase();
  const phrases = [...albumNames.map(n => ["album", n]), ...names.map(n => ["person", n])]
    .filter(([, n]) => n.includes(" "))
    .sort((a, b) => b[1].length - a[1].length);
  for (const [field, name] of phrases) {
    const re = new RegExp(`(^|\\s)${name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(?=\\s|$)`);
    if (re.test(rest) && want[field] === null) {
      want[field] = name;
      rest = rest.replace(re, " ");
    }
  }

  const tokens = rest.split(/[\s,]+/).filter(Boolean);

  for (const raw of tokens) {
    // Explicit field:value always wins, for people who want precision.
    const [field, ...rest] = raw.split(":");
    const val = rest.join(":");
    if (val) {
      if (field === "person" || field === "who") { want.person = val; continue; }
      if (field === "album") { want.album = val; continue; }
      if (field === "type") { want.kind = val === "video" ? "video" : "photo"; continue; }
      if (field === "ext") { want.ext = val.toUpperCase(); continue; }
      if (field === "label" || field === "colour" || field === "color") { want.label = val; continue; }
      if (field === "rating" || field === "stars") { want.minRating = parseInt(val, 10) || 0; continue; }
    }

    const iso = raw.match(/^(\d{4})[-/.](\d{1,2})(?:[-/.](\d{1,2}))?$/);
    if (iso) {
      want.year = +iso[1]; want.month = +iso[2];
      if (iso[3]) want.day = +iso[3];
      continue;
    }
    const dmy = raw.match(/^(\d{1,2})[-/.](\d{1,2})[-/.](\d{4})$/);
    if (dmy) { want.day = +dmy[1]; want.month = +dmy[2]; want.year = +dmy[3]; continue; }

    const m = MONTHS.findIndex(n => n.startsWith(raw) && raw.length >= 3);
    if (m >= 0) { want.month = m + 1; continue; }
    if (/^\d{4}$/.test(raw)) { want.year = +raw; continue; }
    const d = raw.match(/^(\d{1,2})(?:st|nd|rd|th)?$/);
    if (d && +d[1] >= 1 && +d[1] <= 31) { want.day = +d[1]; continue; }

    // Bare words that happen to be a person, an album, a colour or a type are
    // recognised as such, so "sam august 2026" needs no syntax at all.
    if (names.includes(raw)) { want.person = raw; continue; }
    if (albumNames.includes(raw)) { want.album = raw; continue; }
    // Colours stay in place rather than being claimed. "red" alone means the red
    // label; in "a red sailing boat" it describes the boat, and swallowing it would
    // search for a boat of no particular colour. Kept in order: word order matters
    // to the model that reads the phrase.
    if (LABEL_NAMES.includes(raw)) { colours.push(text.length); text.push(raw); continue; }
    if (raw === "video" || raw === "videos") { want.kind = "video"; continue; }
    if (raw === "photo" || raw === "photos") { want.kind = "photo"; continue; }
    if (raw === "fav" || raw === "favourite" || raw === "favorite") { want.fav = true; continue; }
    // The "+" may sit either side: the filter panel emits "4star", but the chip it
    // draws reads "★★★★+", so someone typing what they see writes "4stars+".
    const stars = raw.match(/^(\d)\+?(?:star|stars)\+?$/);
    if (stars) { want.minRating = +stars[1]; continue; }

    text.push(raw);
  }
  // Only when the leftover words are *nothing but* colours is this a label filter.
  if (colours.length === text.length && text.length) {
    want.label = text[0];
    text.length = 0;
  }
  const hasFilter = Object.entries(want).some(([k, v]) =>
    k === "fav" ? v : v !== null);
  return { want, text, hasFilter };
}

/** The chips shown under the search field, so the query is legible at a glance. */
function queryChips({ want, text }) {
  const out = [];
  const date = [];
  if (want.day !== null) date.push(String(want.day));
  if (want.month !== null) date.push(MONTHS[want.month - 1].replace(/^./, c => c.toUpperCase()));
  if (want.year !== null) date.push(String(want.year));
  if (date.length) out.push({ kind: "date", text: date.join(" ") });
  if (want.person) out.push({ kind: "person", text: want.person });
  if (want.album) out.push({ kind: "album", text: want.album });
  if (want.kind) out.push({ kind: "type", text: want.kind === "video" ? "Videos" : "Photos" });
  if (want.ext) out.push({ kind: "type", text: want.ext });
  if (want.label) out.push({ kind: "label", text: want.label });
  if (want.minRating) out.push({ kind: "rating", text: "★".repeat(want.minRating) + "+" });
  if (want.fav) out.push({ kind: "rating", text: "★★★★★" });
  if (text.length) {
    const phrase = text.join(" ");
    const sem = S.semantic && S.semantic.query === phrase ? S.semantic : null;
    // Availability is checked before any result state: "nothing recognised" would be
    // a lie when the reason is that the models are not installed.
    if (S.semanticReady && !S.semanticReady.available) {
      out.push({ kind: "sem empty", text: "\u2728 needs the search models" });
      for (const t of text) out.push({ kind: "text", text: t });
    } else if (S.semanticReady && S.semanticReady.embedded === 0) {
      out.push({ kind: "sem action", text: "\u2728 Understand these photos", act: understand });
      for (const t of text) out.push({ kind: "text", text: t });
    } else if (sem && sem.state === "busy") {
      out.push({ kind: "sem busy", text: `\u2728 ${phrase}\u2026` });
    } else if (sem && sem.state === "ok") {
      out.push({ kind: "sem", text: `\u2728 ${phrase} \u00B7 ${sem.scores.size}` });
    } else if (sem && sem.state === "none") {
      // An empty result is a real answer, not a failure — say so rather than
      // leaving a chip that looks like it is still working.
      out.push({ kind: "sem empty", text: `\u2728 ${phrase} \u00B7 nothing recognised` });
    } else {
      for (const t of text) out.push({ kind: "text", text: t });
    }
  }
  return out;
}

/** Embed every photo so scenes become searchable. Resumable, and safe to re-run. */
async function understand() {
  const msg = await busy("Understanding photos… this runs once per photo",
    () => invoke("semantic_index", { path: S.source }), S.source);
  toast(msg, "ok");
  await refreshSemanticStatus();
  S.semantic = null;
  applyFilter();
}

/* ---------------- semantic ----------------
   Everything else about a photo is known from its name, its folder or its metadata.
   What it *shows* is not, so this is the one filter that has to go to the backend.
   The literal query is applied first and the grid drawn from it; the semantic answer
   folds in when it arrives, so typing never waits on a model. */

let semTimer = null;
let semSeq = 0;

function scheduleSemantic(phrase) {
  clearTimeout(semTimer);
  if (!phrase) { S.semantic = null; return; }
  if (S.semantic && S.semantic.query === phrase && S.semantic.state !== "busy") return;
  // Not a dead end: someone who fetches the models mid-session should not have to
  // restart, so a typed phrase re-checks rather than trusting a stale "no".
  if (S.semanticReady && !S.semanticReady.available) { semTimer = setTimeout(recheck, 220); return; }
  if (S.semanticReady && S.semanticReady.embedded === 0) return;

  S.semantic = { query: phrase, scores: new Map(), state: "busy" };
  // Long enough that a phrase typed at speed costs one search, short enough to feel
  // like the results simply appear.
  semTimer = setTimeout(() => runSemantic(phrase, ++semSeq), 220);
}

async function recheck() {
  const was = S.semanticReady && S.semanticReady.available;
  await refreshSemanticStatus();
  if ((S.semanticReady && S.semanticReady.available) !== was) applyFilter();
}

async function runSemantic(phrase, seq) {
  try {
    const hits = await invoke("semantic_search", { path: S.source, query: phrase });
    if (seq !== semSeq) return;              // a newer query overtook this one
    S.semantic = {
      query: phrase,
      scores: new Map(hits.map(h => [h.hash, h.score])),
      state: hits.length ? "ok" : "none",
    };
    // Nothing came back. That is usually a real answer, but it is also what a missing
    // model looks like from here, so confirm which before reporting it.
    if (!hits.length) await refreshSemanticStatus();
  } catch (e) {
    if (seq !== semSeq) return;
    S.semantic = { query: phrase, scores: new Map(), state: "none" };
    console.warn("semantic search failed:", e);
  }
  applyFilter();
}

/** Cheap enough to call on every source switch, and it decides whether to search at all. */
async function refreshSemanticStatus() {
  const t = beginLoad("semantic");
  if (!t.source) { S.semanticReady = null; return; }
  let ready;
  try {
    ready = await invoke("semantic_status", { path: t.source });
  } catch {
    ready = null;
  }
  if (!stillCurrent(t)) return;
  S.semanticReady = ready;
}

/* Show what the query was understood to mean. Someone typing "sam august 2026" should
   see it become a person and a date, not wonder why nothing matched. */
function showQueryChips(parsed) {
  const bar = $("#qchips");
  if (!parsed || (!parsed.hasFilter && !parsed.text.length)) { bar.hidden = true; return; }
  bar.hidden = false;
  const chips = queryChips(parsed).map(c => {
    if (!c.act) return el("span", { class: `qc qc-${c.kind}` }, c.text);
    const b = el("button", { class: `qc qc-${c.kind}`, type: "button" }, c.text);
    b.onclick = c.act;
    return b;
  });
  // The answer to "I searched and got nothing" is usually "you are standing in a
  // folder". Say how many the query finds outside it, and offer the one click.
  if (S.elsewhere > 0) {
    chips.push(el("button", { class: "qc qc-wide", type: "button",
      title: "Clear the folder and search the whole library",
      onclick: widenSearch },
      `${S.elsewhere} elsewhere \u2014 search all of ${
        S.sources.find(s => s.path === S.source)?.name || "the library"}`));
  }
  // Keeping a query is the replacement for making an album, so the offer belongs
  // where the query itself is shown.
  chips.push(el("button", { class: "qc qc-save", type: "button",
    title: "Keep this search in the sidebar", onclick: saveSearchPrompt }, "\u2606 Save"));
  bar.replaceChildren(...chips);
}

/** Drop whatever is narrowing the view, keeping the query. */
async function widenSearch() {
  S.folder = null; S.person = null;
  S.cluster = null; S.clusterHashes = null;
  S.resetScroll = true;
  renderSidebar();
  await loadFolderView();
  renderFilters();
  applyFilter();
}

function matchesQuery(p, parsed) {
  const { want, text } = parsed;
  if (!matchesStructured(p, want)) return false;
  if (!text.length) return true;
  const hay = [p.name, p.folder, p.people.join(" ")].join(" ").toLowerCase();
  if (text.every(t => hay.includes(t))) return true;
  // Falling through to what the photo *shows*. A union, not a replacement: typing a
  // folder name must keep working, and "church" should still find churches.
  return semanticMatches(p, text);
}

/* The structured half of a query — everything decidable from a photo's metadata.
   Shared by the omnibar and the Ask panel so the two can never disagree. */
function matchesStructured(p, want) {
  if (want.year !== null || want.month !== null || want.day !== null) {
    if (!p.taken_at) return false;
    const d = new Date(p.taken_at * 1000);
    if (want.year !== null && d.getFullYear() !== want.year) return false;
    if (want.month !== null && d.getMonth() + 1 !== want.month) return false;
    if (want.day !== null && d.getDate() !== want.day) return false;
  }
  if (want.person && !p.people.some(n => n.toLowerCase() === want.person)) return false;
  if (want.album && !(p.albums || []).some(a => a.toLowerCase() === want.album)) return false;
  if (want.kind && p.kind !== want.kind) return false;
  if (want.ext && p.ext !== want.ext) return false;
  if (want.label && (p.label || "") !== want.label) return false;
  if (want.minRating && (p.rating || 0) < want.minRating) return false;
  if (want.fav && (p.rating || 0) < 5) return false;
  return true;
}

/** True when the leftover words were understood as a scene and this photo matched. */
function semanticMatches(p, text) {
  const sem = S.semantic;
  return !!sem && sem.query === text.join(" ") && sem.scores.has(p.hash);
}

function applyFilter() {
  document.querySelector(".namebar")?.remove();
  const q = $("#search").value.trim();
  const parsed = q
    ? parseQuery(q, S.people.filter(p => p.name).map(p => p.name), S.albums.map(a => a[0]))
    : null;
  scheduleSemantic(!S.peek && parsed && parsed.text.length ? parsed.text.join(" ") : null);
  S.view = S.photos.filter(p =>
    // Trash is a real folder, but it should not appear in the library view unless
    // the user deliberately opens it.
    !p.liveStill &&
    (S.folder === TRASH || p.folder !== TRASH) &&
    (!S.folder || inFolder(p.folder, S.folder)) &&
    (!S.person || p.people.includes(S.person)) &&
    (!S.clusterHashes || S.clusterHashes.has(p.hash)) &&
    (!parsed || matchesQuery(p, parsed)));
  sortView();
  // Searching inside a folder can hide the very file being looked for: it is in the
  // library, just not here. Rather than answering "nothing", count what the same query
  // finds outside this folder so the chip can offer to widen.
  const narrowed = (S.folder && S.folder !== TRASH) || S.person || S.cluster !== null;
  S.elsewhere = parsed && narrowed
    ? Math.max(0, S.photos.filter(p => p.folder !== TRASH && matchesQuery(p, parsed)).length
                  - S.view.length)
    : 0;
  showQueryChips(parsed);
  const src = S.sources.find(s => s.path === S.source);
  $("#crumb").textContent = S.peek ? "" : [
    src?.name, S.folder,
    S.person ? `\u{1F464} ${S.person}` : null,
    S.cluster !== null ? "\u{1F464} unnamed person" : null,
  ].filter(Boolean).join("  \u203A  ") + `   \u00B7   ${S.view.length} items`;
  // WebKit can leave a newly repopulated sticky element at zero width until it is
  // inserted back into the flow. Re-anchoring it makes the header deterministic.
  const head = document.querySelector(".libhead");
  head.replaceWith(head);
  renderTimelineTools();
  renderGrid();
  paintSel();
  renderFilters();
}

function syncPeekChrome() {
  const peeking = !!S.peek;
  const bar = $("#peekbar");
  bar.hidden = !peeking;
  if (peeking) {
    $("#peek-name").textContent = S.peek.name;
    const items = S.peek.photos + S.peek.videos;
    const folders = S.peek.subfolders;
    $("#peek-note").textContent =
      `${items.toLocaleString()} item${items === 1 ? "" : "s"} directly in this folder` +
      (folders ? ` · ${folders.toLocaleString()} subfolder${folders === 1 ? "" : "s"} not shown` : "");
  }
  const reason = peeking
    ? `“${S.peek.name}” is a read-only peek. Keep this folder to use this feature.`
    : "";
  for (const id of ["#btn-ask", "#btn-map", "#btn-tools", "#btn-newfolder", "#btn-review"]) {
    const button = $(id);
    if (!button) continue;
    button.dataset.normalTitle ||= button.title;
    button.disabled = peeking;
    button.title = reason || button.dataset.normalTitle;
  }
  renderTimelineTools();
  paintSel();
}

function refusePeek(action = "make changes") {
  if (!S.peek) return false;
  toast(`“${S.peek.name}” is a read-only peek. Keep this folder to ${action}.`);
  return true;
}

async function releasePeek() {
  if (!S.peek) return true;
  const current = S.peek;
  if (!$("#lightbox").hidden) closeLightbox();
  try {
    await invoke("end_peek", { path: current.path });
  } catch (e) {
    toast(String(e), "error");
    return false;
  }
  S.peek = null;
  syncPeekChrome();
  return true;
}

async function enterPeek(info, openedFile = null) {
  if (S.peek && S.peek.path !== info.path && !(await releasePeek())) return;
  if (!$("#mapview").hidden) closeMap();
  toggleAsk(false);
  $("#sheet").hidden = true;
  S.peek = info;
  S.source = info.path;
  S.folder = null; S.person = null; S.cluster = null; S.clusterHashes = null;
  S.people = []; S.albums = []; S.searches = [];
  S.semantic = null; S.semanticReady = null;
  S.photos = []; S.view = []; S.sel.clear();
  S.sort = "name"; S.group = "folder"; S.order = [];
  S.resetScroll = true;
  syncPeekChrome();
  renderSidebar();
  syncToastScope();
  renderGrid();
  try {
    await busy(`Opening ${info.name}…`, loadPhotos, info.path);
  } catch {
    await releasePeek();
    return;
  }
  if (openedFile) {
    const wanted = S.view.find(p => p.path === openedFile || p.name === openedFile);
    if (wanted) openLightbox(wanted);
  }
}

async function closePeek() {
  if (!(await releasePeek())) return;
  S.source = null; S.photos = []; S.view = []; S.sel.clear();
  $("#crumb").textContent = "";
  renderSidebar();
  const first = S.sources.find(s => !s.missing);
  if (first) await selectSource(first.path);
  else renderWelcome();
}

async function keepPeek() {
  if (!S.peek) return;
  const current = S.peek;
  if (!$("#lightbox").hidden) closeLightbox();
  try {
    const info = await busy(`Adding ${current.name}…`,
      () => invoke("promote_peek", { path: current.path }), current.path);
    S.peek = null;
    syncPeekChrome();
    await refreshSources();
    toast(`${info.name} is now a source`, "ok");
    await selectSource(info.path);
  } catch { /* busy already reports the reason; the peek remains open */ }
}

async function selectSource(path) {
  if (!(await releasePeek())) return;
  S.source = path; S.folder = null; S.person = null;
  S.cluster = null; S.clusterHashes = null; S.people = [];
  S.semantic = null; S.semanticReady = null;
  syncPeekChrome();
  // Drop the previous library's photographs before the new one's arrive. Leaving them
  // up meant the breadcrumb named one folder while the grid showed another.
  S.photos = []; S.view = []; S.sel.clear();
  S.resetScroll = true;
  // The map is a library view, not a separate library. Leave it open across a source
  // switch, but take the old points away now; a stale country should never survive
  // long enough to be mistaken for the new library.
  if (!$("#mapview").hidden) {
    MAP.request = null;
    MAP.points = [];
    MAP.clusters = [];
    MAP.bucketZoom = null;
    $("#mapcount").textContent = "Loading locations…";
    $("#mapwhere").textContent = "";
    scheduleMap();
  }
  refreshSemanticStatus();
  renderSidebar();
  syncToastScope();
  renderGrid();
  // The root of a library remembers its arrangement too, so read it before the
  // photographs arrive and are sorted.
  await loadFolderView();
  await busy("Loading library…", loadPhotos, path);
  refreshPeople();
  // The map is a view onto the current library, so it reloads with it rather than
  // waiting to be closed and reopened.
  if (!$("#mapview").hidden) loadMapData();
  // Thumbnails are produced on demand by the photo:// handler as cells scroll into
  // view, so nothing blocks the first paint. A background pass backfills the rest so
  // later scrolling is instant, but it is an optimisation, not a prerequisite.
  const src = S.sources.find(s => s.path === path);
  if (src && src.thumbs_ready < src.photos) {
    invoke("build_thumbs", { path })
      .then(() => refreshSources())
      .catch(() => {});
  }
  // Opening a folder is also the moment to finish what a previous session started —
  // `source-ready` only fires when a library is opened for writing, which reading its
  // counts does not do.
  resumeUnfinished(path);
}

/** Fetch the located photographs for the current source. `request` discards a response
    from a source the user has already left: map loads can take longer than a click. */
async function loadMapData() {
  const source = S.source;
  if (!source) return;
  const request = { source };
  MAP.request = request;
  await loadRings(110);
  loadRings(50);                                  // the finer level arrives behind us
  // Reading GPS is a header parse, and the answer is cached against the content hash,
  // so this is only slow the first time.
  const found = await busy("Reading where photographs were taken…",
    () => invoke("locate_photos", { path: source }), source);
  const places = await invoke("photo_places", { path: source });
  if (MAP.request !== request || S.source !== source) return;
  MAP.points = places.map(p => {
    const [wx, wy] = project(p.lon, p.lat);
    return { ...p, wx, wy };
  });
  MAP.bucketZoom = null;
  fitMap();
  clusterPoints();
  const n = MAP.points.length;
  $("#mapcount").textContent = n ? `${n} located` : "None located yet";
  $("#mapwhere").textContent = n
    ? `${MAP.clusters.length} place${MAP.clusters.length === 1 ? "" : "s"}`
    : (found.checked
        ? "No photograph here carries coordinates — select some and choose Where was this?"
        : "");
  scheduleMap();
}
async function selectFolder(f) {
  S.folder = (S.folder === f ? null : f);
  S.person = null;
  S.resetScroll = true;
  renderSidebar();
  // Each folder remembers how it was arranged, so the sort is a property of where you
  // are standing rather than one setting for the window.
  await loadFolderView();
  renderFilters();
  applyFilter();
}
/** Set a group of faces aside, so review stops offering strangers to name.
    The photographs are untouched and the faces stay in the index — this only takes
    them out of review, which is why bringing them back is one click. */
async function dismissCluster(id) {
  const msg = await busy("Setting aside\u2026",
    () => invoke("dismiss_cluster", { path: S.source, distance: 0.55, cluster: id }));
  toast(msg, "ok");
  // Group ids are positions in a list that has just been recomputed, so whatever was
  // selected no longer means what it meant.
  S.cluster = null; S.clusterHashes = null;
  await refreshPeople(); await loadPhotos();
}

async function restoreDismissed() {
  const msg = await busy("Bringing them back\u2026",
    () => invoke("restore_dismissed", { path: S.source }));
  toast(msg, "ok");
  S.cluster = null; S.clusterHashes = null;
  await refreshPeople(); await loadPhotos();
}

/** The same person, named twice. Folds one into the other, keeping both sets of
    reference faces — which is the difference from forgetting one of them. */
async function mergePersonPrompt(name) {
  const others = S.people.filter(p => p.name && p.name !== name);
  if (!others.length) return;
  const into = await new Promise(resolve => {
    let d;
    d = dialogFrame(`${name} is the same person as\u2026`, [
      el("p", { class: "asktext" },
        `${name}'s reference faces are added to whoever you pick, so recognition gets ` +
        "better rather than worse. The photographs are untouched."),
      el("div", { class: "movepicks" }, others.map(p =>
        el("button", { class: "sugg-pill", onclick: () => d.done(p.name) },
          p.cover ? el("img", { class: "fface", src: photoUrl(p.cover), alt: "" }) : null,
          p.name))),
      el("div", { class: "askrow" },
        el("button", { class: "btn ghost", onclick: () => d.done(null) }, "Cancel")),
    ]);
    d.attach(resolve);
    document.addEventListener("keydown", d.onKey, true);
    document.body.append(d.box);
  });
  if (!into) return;
  const msg = await busy(`Merging ${name} into ${into}\u2026`,
    () => invoke("merge_people", { path: S.source, from: name, into }));
  toast(msg, "ok");
  // Looking at the person who just stopped existing would show an empty grid.
  if (S.person === name) S.person = into;
  await refreshPeople(); await loadPhotos();
}

/** Remove a person. The photographs stay; only the claim about who is in them goes. */
async function forgetPerson(name) {
  const ok = await confirmDialog(`Forget ${name}?`,
    "The photographs are untouched — only the name and the faces learned for it are removed.",
    "Forget");
  if (!ok) return;
  const msg = await invoke("forget_person", { path: S.source, person: name });
  toast(msg, "ok");
  if (S.person === name) S.person = null;
  await refreshPeople(); await loadPhotos();
}

async function forgetEmptyPeople(names) {
  const ok = await confirmDialog(
    `Forget ${names.length} unused name${names.length === 1 ? "" : "s"}?`,
    `${names.join(", ")} — none of them match any photograph. Nothing else changes.`,
    "Forget");
  if (!ok) return;
  for (const n of names) await invoke("forget_person", { path: S.source, person: n });
  toast(`Forgot ${names.length}`, "ok");
  await refreshPeople(); await loadPhotos();
}

function selectPerson(p) {
  S.resetScroll = true;
  S.person = (S.person === p ? null : p);
  S.folder = null; S.cluster = null; S.clusterHashes = null;
  renderSidebar(); applyFilter();
}

/* Viewing an unnamed group shows its photos and offers a name inline, so naming
   someone never requires opening a modal and hunting for them. */
async function selectCluster(id) {
  if (S.cluster === id) { S.cluster = null; S.clusterHashes = null; renderSidebar(); applyFilter(); return; }
  S.person = null; S.folder = null; S.cluster = id;
  const hashes = await busy("Finding this person's photos…",
    () => invoke("cluster_photos", { path: S.source, distance: 0.55, cluster: id }));
  S.clusterHashes = new Set(hashes);
  renderSidebar(); applyFilter();
  namePrompt(id);
}

/** Assign a face cluster to a name, new or existing. Existing merges into that person. */
async function nameAs(id, name) {
  const known = S.people.some(p => p.name === name);
  await busy(known ? `Adding to ${name}\u2026` : `Learning ${name}\u2026`,
    () => invoke("name_cluster", { path: S.source, distance: 0.55, cluster: id, name }));
  toast(known ? `Added to ${name}` : `Named ${name}`, "ok");
  S.cluster = null; S.clusterHashes = null;
  document.querySelector(".namebar")?.remove();
  await refreshPeople(); await loadPhotos();
}

function namePrompt(id) {
  const u = S.people.find(p => p.cluster === id);
  const bar = el("div", { class: "namebar" },
    u?.cover ? el("img", { class: "avatar lg", src: photoUrl(u.cover), alt: "" }) : null,
    el("span", { class: "grow" }, "Who is this?"),
    // Existing people first: retyping a name risks a second spelling of someone who
    // is already known, and merging into them is usually what is meant.
    el("div", { class: "movepicks" }, S.people.filter(p => p.name).slice(0, 8).map(p =>
      el("button", { class: "sugg-pill", title: `Add to ${p.name}`, onclick: () => nameAs(id, p.name) },
        p.cover ? el("img", { class: "fface", src: photoUrl(p.cover), alt: "" }) : null,
        p.name))),
    el("input", {
      class: "nameinput", type: "text", placeholder: u?.suggestion || "Or a new name",
      "aria-label": "Person name",
      onkeydown: async e => {
        if (e.key === "Escape") { bar.remove(); return; }
        if (e.key !== "Enter") return;
        const v = e.target.value.trim() || u?.suggestion;
        if (v) await nameAs(id, v);
      }
    }),
    el("button", { class: "btn ghost sm", onclick: () => { S.cluster = null; S.clusterHashes = null; renderSidebar(); applyFilter(); } }, "Skip"));
  $("#stage").before(bar);
  setTimeout(() => bar.querySelector("input")?.focus(), 60);
}

async function addSource() {
  const picked = await dialog.open({ directory: true, multiple: false, title: "Add a photo folder" });
  if (!picked) return;
  const survey = await surveyBeforeAdd(picked);
  if (!survey || survey.error) return;
  const shallow = survey.subfolders ? await sourceDepthPrompt(picked, survey) : false;
  if (shallow === null) return;
  await registerSurveyedSource(picked, shallow);
}

function surveyBeforeAdd(path) {
  return new Promise(resolve => {
    let cancelled = false;
    let d;
    const cancel = () => d.done(null);
    d = dialogFrame(`Counting “${path.split("/").pop() || path}”…`, [
      el("div", { class: "surveybusy" },
        el("span", { class: "sp", "aria-hidden": "true" }),
        el("span", {}, "Looking at folder names and file types — no photographs are opened.")),
      el("div", { class: "askrow" },
        el("button", { class: "btn ghost", onclick: cancel }, "Cancel")),
    ]);
    d.box.querySelector(".sheet-panel").classList.add("survey-panel");
    d.attach(value => {
      if (value === null) {
        cancelled = true;
        invoke("cancel_survey").catch(() => {});
      }
      resolve(value);
    });
    document.addEventListener("keydown", d.onKey, true);
    document.body.append(d.box);
    invoke("survey_folder", { path })
      .then(survey => { if (!cancelled) d.done(survey); })
      .catch(error => {
        if (cancelled) return;
        toast(String(error), "error");
        d.done({ error: true });
      });
  });
}

function sourceDepthPrompt(path, survey) {
  return new Promise(resolve => {
    let d;
    const direct = survey.here.toLocaleString();
    const below = survey.below === null
      ? "More than 200,000 more photographs and videos are below it."
      : `${survey.below.toLocaleString()} more photograph${survey.below === 1 ? " is" : "s are"} below it.`;
    const preferShallow = survey.below === null || survey.below > 50_000 ||
      survey.below > 20 * Math.max(survey.here, 1);
    const only = el("button", {
      class: preferShallow ? "btn" : "btn ghost", onclick: () => d.done(true),
    }, "This folder only");
    const recursive = el("button", {
      class: preferShallow ? "btn ghost" : "btn", onclick: () => d.done(false),
    }, "Include subfolders");
    const excluded = survey.excluded.length
      ? `Skipping ${survey.excluded.map(name => `“${name}”`).join(", ")}.`
      : "No system or cache folders found to skip.";
    d = dialogFrame(`Add “${path.split("/").pop() || path}”?`, [
      el("ul", { class: "surveyfacts" },
        el("li", {}, el("strong", {}, direct),
          ` photograph${survey.here === 1 ? "" : "s"} or video${survey.here === 1 ? "" : "s"} directly in this folder.`),
        el("li", { class: preferShallow ? "surveywarn" : "" }, below),
        el("li", {}, el("strong", {}, survey.subfolders.toLocaleString()),
          ` subfolder${survey.subfolders === 1 ? "" : "s"}. ${excluded}`)),
      el("div", { class: "askrow" },
        el("button", { class: "btn ghost", onclick: () => d.done(null) }, "Cancel"),
        only, recursive),
    ]);
    d.box.querySelector(".sheet-panel").classList.add("survey-panel");
    d.attach(resolve);
    document.addEventListener("keydown", d.onKey, true);
    document.body.append(d.box);
    setTimeout(() => (preferShallow ? only : recursive).focus(), 40);
  });
}

async function registerSurveyedSource(path, shallow) {
  // The folder appears at once and indexes behind itself. Waiting for the first scan
  // of a phone backup before even showing the row is what made this feel like a hang.
  try {
    const info = await invoke("add_source", { path, shallow });
    S.sources = [...S.sources.filter(s => s.path !== info.path), info];
    S.busy[info.path] = { op: "scan", done: 0, total: 0 };
    renderSidebar();
    toast(`${info.name} added`, "ok");
    // Go to what was just added. Reads no longer wait on the scan, so this shows the
    // folder filling in rather than blocking on it.
    await selectSource(info.path);
    refreshSources().then(() => autodetect(info.path));
  } catch (e) {
    // A refusal — already in the library, nested inside a source — is an answer,
    // not a crash; the backend worded it for the user.
    toast(String(e), "error");
  }
}

/* Pick up analysis that a previous session left unfinished.
   These passes take hours on a large library and quitting part-way is normal; every
   stage commits per photograph, so what was done survives. Only stages that were
   *already begun* resume — a folder nobody has asked to analyse must not start burning
   CPU on its own the next time the app opens. */
const resuming = new Set();

async function resumeUnfinished(path) {
  if (resuming.has(path)) return;
  let p;
  try { p = await invoke("pending_work", { path }); } catch { return; }

  const faces = p.faces_started && p.faces_missing > 0;
  const semantic = p.clip_started && p.clip_missing > 0;
  if (!faces && !semantic) return;

  resuming.add(path);
  const left = Math.max(faces ? p.faces_missing : 0, semantic ? p.clip_missing : 0);
  const what = faces && semantic ? "faces and scenes" : faces ? "faces" : "scenes";
  toast(`Resuming ${what} — ${left.toLocaleString()} photo${left === 1 ? "" : "s"} left`, "ok");
  try {
    S.busy[path] = { op: "analyze", done: 0, total: left };
    paintSourceProgress(path);
    await invoke("analyze_resume", { path, faces, semantic });
    if (path === S.source) { await refreshPeople(); await loadPhotos(); }
  } catch (e) {
    console.warn("resume failed:", e);
  } finally {
    resuming.delete(path);
    delete S.busy[path];
    paintSourceProgress(path);
    await refreshSources();
  }
}

/* A library finished its first scan. Refresh the sidebar so its counts appear, and the
   grid too if it is the one being looked at. */
listen("source-ready", async ({ payload }) => {
  delete S.busy[payload];
  await refreshSources();
  if (payload === S.source) await loadPhotos();
  resumeUnfinished(payload);
});

/* Finding people is the point of the app, so a newly added folder is scanned for
   faces without being asked. Silent if the models are not installed. */
async function autodetect(path) {
  try {
    const msg = await busy("Looking for people…", () => invoke("autodetect_faces", { path }), path);
    if (msg.includes("models not installed")) return;
    await refreshPeople();
    const unnamed = S.people.filter(p => !p.name).length;
    if (unnamed) toast(`${unnamed} people found — name them in the sidebar`, "ok");
  } catch { /* reported by busy */ }
}
/* Removing a folder is not deleting photographs, and the dialog has to make that
   obvious — it is the fear the question raises. Deleting what Blinkview wrote is offered
   in the same breath but is never the default, because half of it (ratings, names)
   cannot be reproduced by anything (ADR-0007). */
async function removeSource(path) {
  const name = path.split("/").filter(Boolean).pop() || path;
  let d = {};
  try { d = await invoke("source_data", { path }); } catch { /* show the plain question */ }

  const mb = (d.cache_bytes || 0) / 1048576;
  const lost = [];
  if (d.described) lost.push(`${d.described} rated or labelled`);
  if (d.people) lost.push(`${d.people} named ${d.people === 1 ? "person" : "people"}`);
  if (d.saved_searches) lost.push(`${d.saved_searches} saved search${d.saved_searches === 1 ? "" : "es"}`);

  const purge = el("input", { type: "checkbox", id: "purge-data" });
  const choice = await new Promise(resolve => {
    const dlg = dialogFrame(`Remove ${name}?`, [
      el("p", { class: "asktext" },
        "It stops appearing in Blinkview. ",
        el("b", {}, "Your photographs are not deleted"),
        " \u2014 the folder and everything in it stays exactly where it is."),
      el("label", { class: "purgerow", for: "purge-data" },
        purge,
        el("span", {},
          el("b", {}, "Also delete Blinkview's own files"),
          el("span", { class: "asub" },
            mb >= 0.1 ? ` ${mb.toFixed(mb < 10 ? 1 : 0)} MB of thumbnails and index, which would be rebuilt` : " the cache",
            lost.length
              ? el("span", { class: "purgewarn" }, ` \u00B7 and ${lost.join(", ")}, which cannot be recovered`)
              : null))),
      el("div", { class: "askrow" },
        el("button", { class: "btn ghost", onclick: () => dlg.done(null) }, "Cancel"),
        el("button", { class: "btn", onclick: () => dlg.done(purge.checked) }, "Remove")),
    ]);
    dlg.attach(resolve);
    document.addEventListener("keydown", dlg.onKey, true);
    document.body.append(dlg.box);
  });
  if (choice === null) return;

  const msg = await invoke("remove_source", { path, purge: choice });
  // Its banner and its row are going; so is any progress recorded against it.
  delete S.busy[path];
  for (const t of document.querySelectorAll(`#toasts .toast[data-src="${CSS.escape(path)}"]`)) t.remove();
  toast(msg, choice ? "warn" : "ok");
  if (S.source === path) { S.source = null; S.photos = []; S.view = []; renderWelcome(); }
  await refreshSources();
}

/** Make a folder where the sidebar says you are.
    Folders are the only grouping there is (ADR-0009), so making an empty one is how
    you say where things are going to go, before there is anything to put in it. */
async function newFolderPrompt() {
  if (!S.source) return toast("Add a folder first");
  if (refusePeek("create folders")) return;
  const parent = S.folder && S.folder !== TRASH ? S.folder : "";
  const name = await promptDialog(
    parent ? `New folder inside ${parent.split("/").pop()}` : "New folder", "");
  if (!name || !name.trim()) return;
  try {
    const rel = await invoke("create_folder", { path: S.source, parent, name: name.trim() });
    await refreshSources();
    toast(`${rel} created`, "ok");
    await selectFolder(rel);
  } catch (e) {
    toast(String(e), "error");
  }
}

/* ---------------- organize sheet ---------------- */
const OPS = [
  { id: "scenery", title: "Split out scenery", desc: "Move photos with no close-up person into Scenery." },
  { id: "file",    title: "File by person",    desc: "Move each photo into a folder named for the person in it." },
];

const DUP = {
  data: null, batches: [], batch: 0, group: 0, focus: null,
  keepers: new Map(), reviewed: new Set(),
};

function bytesLabel(bytes) {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let n = Math.max(0, Number(bytes) || 0), i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  return `${n >= 10 || i === 0 ? n.toFixed(0) : n.toFixed(1)} ${units[i]}`;
}

function dupCurrentGroup() { return DUP.batches[DUP.batch]?.groups[DUP.group] || null; }
function dupFocusedItem() {
  const group = dupCurrentGroup();
  return group?.items.find(item => item.path === DUP.focus) || group?.items[0] || null;
}

async function openDuplicateReview() {
  if (!S.source) return;
  $("#sheet").hidden = true;
  const data = await busy("Comparing photographs on this Mac…",
    () => invoke("duplicate_review", { path: S.source }), S.source).catch(() => null);
  if (!data) return;
  if (!data.groups.length) return toast("No confirmed near-duplicates found", "ok");
  const byBatch = new Map();
  for (const group of data.groups) {
    if (!byBatch.has(group.batch_id)) byBatch.set(group.batch_id, {
      id: group.batch_id, title: group.batch_title, detail: group.batch_detail, groups: [],
    });
    byBatch.get(group.batch_id).groups.push(group);
  }
  DUP.data = data;
  DUP.batches = [...byBatch.values()].sort((a, b) => {
    const newest = batch => Math.max(...batch.groups.flatMap(group => group.items.map(item => item.taken_at || 0)));
    return newest(b) - newest(a);
  });
  DUP.batch = 0; DUP.group = 0; DUP.focus = null;
  DUP.keepers = new Map(data.groups.map(group => [group.id, group.items.find(item => item.recommended)?.path || group.items[0].path]));
  DUP.reviewed = new Set();
  $("#dup-review").hidden = false;
  renderDuplicateReview();
}

function closeDuplicateReview() {
  $("#dup-review").hidden = true;
  DUP.data = null; DUP.batches = []; DUP.focus = null;
}

function renderDuplicateReview() {
  const group = dupCurrentGroup();
  if (!group) return closeDuplicateReview();
  const batch = DUP.batches[DUP.batch];
  const keeper = DUP.keepers.get(group.id);
  if (!DUP.focus || !group.items.some(item => item.path === DUP.focus)) {
    DUP.focus = group.items.find(item => item.path !== keeper)?.path || keeper;
  }
  const focus = dupFocusedItem();
  const keepItem = group.items.find(item => item.path === keeper) || group.items[0];
  const compare = focus.path === keepItem.path
    ? group.items.find(item => item.path !== keepItem.path) || keepItem
    : focus;
  const reviewedGroups = DUP.data.groups.filter(g => DUP.reviewed.has(g.id));
  const rejected = reviewedGroups.flatMap(g => g.items.filter(item => item.path !== DUP.keepers.get(g.id)));
  const reclaim = rejected.reduce((sum, item) => sum + item.bytes, 0);

  $("#dup-summary").textContent = `${DUP.data.groups.length} bursts · ${bytesLabel(DUP.data.reclaimable)} possible`;
  $("#dup-context-title").textContent = batch.title;
  $("#dup-context-detail").textContent = `${batch.detail} · ${group.items.length} frames`;
  $("#dup-progress").textContent = `Burst ${DUP.group + 1} of ${batch.groups.length}`;
  $("#dup-quality-note").textContent = `Detail score is relative to this burst. ${keepItem.name || keepItem.path.split("/").pop()} is the current keeper.`;

  $("#dup-batches").replaceChildren(...DUP.batches.map((entry, index) => {
    const done = entry.groups.every(g => DUP.reviewed.has(g.id));
    const bytes = entry.groups.reduce((sum, g) => sum + g.reclaimable, 0);
    return el("button", {
      class: `dup-batch${done ? " reviewed" : ""}`, "aria-current": String(index === DUP.batch),
      onclick: () => { DUP.batch = index; DUP.group = 0; DUP.focus = null; renderDuplicateReview(); },
    }, el("span", { class: "batch-state" }),
      el("span", {}, el("b", {}, entry.title), el("small", {}, `${entry.groups.length} burst${entry.groups.length === 1 ? "" : "s"}`)),
      el("span", { class: "batch-space num" }, bytesLabel(bytes)));
  }));

  const pane = item => el("figure", {
    class: `dup-pane${item.path === keeper ? " keep" : ""}${item.path === DUP.focus ? " selected" : ""}`,
    onclick: () => { DUP.focus = item.path; renderDuplicateReview(); },
  },
    el("img", { src: photoUrl(item.path) + "?preview=" + item.hash, alt: item.path.split("/").pop() }),
    el("span", { class: "keep-mark" }, item.path === keeper ? "KEEP" : "COMPARE"),
    el("figcaption", {}, el("b", {}, item.path.split("/").pop()),
      el("span", {}, item.width && item.height ? `${item.width}×${item.height}` : ""),
      el("span", { class: "pane-spacer" }),
      el("span", { class: "quality-score num" }, `${item.quality}/100`)));
  $("#dup-compare").replaceChildren(pane(keepItem), pane(compare));
  $("#dup-film").replaceChildren(...group.items.map(item => el("button", {
    class: `dup-thumb${item.path === keeper ? " keep" : ""}`,
    "aria-current": String(item.path === DUP.focus), title: item.path,
    onclick: () => { DUP.focus = item.path; renderDuplicateReview(); },
  }, el("img", { src: photoUrl(item.path) + "?t=" + item.hash, alt: "" }),
    el("span", { class: "num" }, item.quality))));

  $("#dup-like").setAttribute("aria-pressed", String((focus.rating || 0) === 5));
  $("#dup-like").textContent = (focus.rating || 0) === 5 ? "★ Liked" : "☆ Like";
  $("#dup-reclaim").textContent = DUP.reviewed.size
    ? `${rejected.length} ready for Trash · ${bytesLabel(reclaim)}`
    : "Decisions stay staged until you apply";
  $("#dup-apply").textContent = DUP.reviewed.size ? `Move reviewed to Trash · ${bytesLabel(reclaim)}` : "Move reviewed to Trash";
  $("#dup-apply").disabled = DUP.reviewed.size === 0;
  const atStart = DUP.batch === 0 && DUP.group === 0;
  const atEnd = DUP.batch === DUP.batches.length - 1 && DUP.group === batch.groups.length - 1;
  const undecided = DUP.data.groups.length - DUP.reviewed.size;
  $("#dup-prev").disabled = atStart;
  // Nothing left to accept: say so rather than offering a button that re-renders the
  // same frame, which is indistinguishable from a dead one.
  $("#dup-next").textContent = undecided === 0
    ? "All bursts reviewed"
    : (atEnd ? "Accept keeper" : "Next →");
  $("#dup-next").disabled = undecided === 0;
  $("#dup-accept-all").disabled = undecided === 0;
  $("#dup-accept-all").textContent = undecided && undecided < DUP.data.groups.length
    ? `Keep the remaining ${undecided}`
    : "Keep all suggestions";
}

function reviewDuplicateKeep() {
  const group = dupCurrentGroup(), focus = dupFocusedItem();
  if (!group || !focus) return;
  DUP.keepers.set(group.id, focus.path);
  DUP.reviewed.add(group.id);
  renderDuplicateReview();
}

/** The first burst still without a decision, in reading order. Reviewing is not
    linear — the day list jumps around — so the end of the last day is not the end of
    the work. */
function firstUndecided() {
  for (let b = 0; b < DUP.batches.length; b++) {
    const groups = DUP.batches[b].groups;
    for (let g = 0; g < groups.length; g++) {
      if (!DUP.reviewed.has(groups[g].id)) return { batch: b, group: g };
    }
  }
  return null;
}

function stepDuplicate(direction) {
  const batch = DUP.batches[DUP.batch];
  if (!batch) return;
  if (direction > 0) DUP.reviewed.add(dupCurrentGroup().id); // accepts the current suggestion
  let group = DUP.group + direction, batchIndex = DUP.batch;
  if (group >= batch.groups.length) {
    if (batchIndex + 1 < DUP.batches.length) { batchIndex += 1; group = 0; }
    else {
      // Off the end of the last day. Anything skipped earlier is still waiting, and
      // standing still on a burst already decided is what made this look broken.
      const next = firstUndecided();
      if (!next) return finishDuplicateReview();
      batchIndex = next.batch; group = next.group;
    }
  }
  if (group < 0) { batchIndex = Math.max(0, batchIndex - 1); group = batchIndex === DUP.batch ? 0 : DUP.batches[batchIndex].groups.length - 1; }
  DUP.batch = batchIndex; DUP.group = group; DUP.focus = null;
  renderDuplicateReview();
}

/** Every burst has a keeper. Nothing has moved yet — the Trash step still confirms. */
function finishDuplicateReview() {
  renderDuplicateReview();
  const rejected = DUP.data.groups.flatMap(g => g.items.filter(i => i.path !== DUP.keepers.get(g.id)));
  const bytes = rejected.reduce((sum, item) => sum + item.bytes, 0);
  toast(`All ${DUP.data.groups.length} bursts reviewed — ${rejected.length} frames, ${bytesLabel(bytes)} ready`, "ok");
  $("#dup-apply").focus();
}

/** Take the suggested keeper everywhere it has not been overruled.
    For someone who trusts the sharpness score and does not want to open 88 bursts. */
function acceptAllSuggestions() {
  if (!DUP.data) return;
  for (const group of DUP.data.groups) DUP.reviewed.add(group.id);
  finishDuplicateReview();
}

async function toggleDuplicateLike() {
  const item = dupFocusedItem();
  if (!item) return;
  item.rating = item.rating === 5 ? 0 : 5;
  await invoke("set_rating", { path: S.source, hashes: [item.hash], rating: item.rating });
  renderDuplicateReview();
}

async function moveDuplicateCurrent() {
  const item = dupFocusedItem();
  if (!item) return;
  const dest = await pickFolderPrompt("Move this photo to", "Keepers");
  if (!dest) return;
  const plan = await invoke("plan_move", { path: S.source, hashes: [item.hash], dest });
  if (!plan.moves.length) return toast("Already there", "info");
  const ok = await confirmDialog("Move this photo?", `It will move into “${dest}”.`, "Move");
  if (!ok) return;
  const msg = await invoke("apply_move", { path: S.source, hashes: [item.hash], dest });
  toast(msg + " — ⌘Z to undo", "ok");
  closeDuplicateReview();
  await refreshSources(); await loadPhotos();
}

async function applyDuplicateReview() {
  const groups = DUP.data.groups.filter(group => DUP.reviewed.has(group.id));
  const rejected = groups.flatMap(group => group.items.filter(item => item.path !== DUP.keepers.get(group.id)));
  if (!rejected.length) return;
  const bytes = rejected.reduce((sum, item) => sum + item.bytes, 0);
  const ok = await confirmDialog("Move reviewed files to Trash?",
    `${rejected.length} file${rejected.length === 1 ? "" : "s"} (${bytesLabel(bytes)}) will move to Blinkview Trash. Nothing is permanently erased, and ⌘Z restores the exact prior folders.`,
    "Move to Trash", true);
  if (!ok) return;
  const msg = await busy("Applying duplicate decisions…",
    () => invoke("apply_duplicate_review", {
      path: S.source, rejections: rejected.map(item => ({ hash: item.hash, path: item.path })),
    }), S.source);
  toast(msg + " — empty Blinkview Trash when you are ready to reclaim the space", "ok");
  closeDuplicateReview();
  await refreshSources(); await loadPhotos();
}

/* The shipped default. Hyphens rather than colons because the reference drive is
   exFAT, where `:` is reserved (fsops::RESERVED). */
const DEFAULT_RENAME = "%I-%M-%S_%p_%d_%b_%Y";

/** What a rename would touch: the selection if there is one, else where you stand. */
function renameScope() {
  const sel = [...S.sel];
  if (sel.length) {
    return { hashes: sel, what: `the ${sel.length} you selected` };
  }
  if (S.folder) {
    const hs = S.photos.filter(p => inFolder(p.folder, S.folder)).map(p => p.hash);
    return { hashes: hs, what: `everything in ${S.folder} (${hs.length})` };
  }
  return { hashes: null, what: `the whole library (${S.photos.length})` };
}

function renameBlock() {
  const scope = renameScope();
  const out = el("div", { class: "planout", hidden: true });
  const pat = el("input", {
    class: "nameinput", type: "text", value: S.renameFormat || DEFAULT_RENAME,
    "aria-label": "Filename pattern", style: "width:100%;margin-top:var(--s2)",
    onkeydown: e => { e.stopPropagation(); if (e.key === "Enter") preview.click(); },
    // A pattern that has changed has not been previewed, and nothing is applied
    // without a preview.
    oninput: e => { S.renameFormat = e.target.value; apply.disabled = true; },
  });
  const apply = el("button", { class: "btn", disabled: true,
    onclick: () => runRenameApply(scope, pat.value, out, apply) }, "Apply");
  const preview = el("button", { class: "btn ghost",
    onclick: () => runRenamePreview(scope, pat.value, out, apply) }, "Preview");
  return el("div", { class: "op" },
    el("div", { class: "txt" },
      el("b", {}, "Rename files"),
      el("span", {}, `Renames ${scope.what}. %Y year · %m month · %d day · %H hour · ` +
        `%M minute · %S second · %b month name · %%n a counter`),
      pat, out),
    preview, apply);
}

async function runRenamePreview(scope, format, out, applyBtn) {
  out.hidden = false;
  try {
    const plan = await busy("Working out the names…",
      () => invoke("plan_rename", { path: S.source, format, hashes: scope.hashes }));
    const lines = plan.moves.slice(0, 5)
      .map(([f, t]) => `${f.split("/").pop()}  \u2192  ${t.split("/").pop()}`);
    const more = plan.moves.length > 5 ? `\n\u2026and ${plan.moves.length - 5} more` : "";
    const skipped = plan.skipped.length
      ? `\n${plan.skipped.length} left alone \u2014 ${plan.skipped[0][1]}` : "";
    out.textContent = plan.moves.length
      ? `${plan.moves.length} to rename:\n` + lines.join("\n") + more + skipped
      : "Nothing to rename." + skipped;
    applyBtn.disabled = plan.moves.length === 0;
  } catch (e) {
    // A pattern chrono cannot read is an answer, not a crash.
    out.textContent = String(e);
    applyBtn.disabled = true;
  }
}

async function runRenameApply(scope, format, out, applyBtn) {
  const msg = await busy("Renaming\u2026",
    () => invoke("apply_rename", { path: S.source, format, hashes: scope.hashes }));
  toast(msg + " \u2014 \u2318Z to undo", "ok");
  applyBtn.disabled = true; out.hidden = true;
  await invoke("rescan", { path: S.source });
  await refreshSources(); await loadPhotos();
}
function openSheet() {
  if (!S.source) return toast("Add a folder first");
  if (refusePeek("organize photographs")) return;
  $("#sheet-title").textContent = "Organize";
  $("#sheet-body").replaceChildren(
    el("div", { class: "op" },
      el("div", { class: "txt" }, el("b", {}, "Review near-duplicates"),
        el("span", {}, "Compare burst shots full-screen, choose each keeper, then safely reclaim space.")),
      el("button", { class: "btn", onclick: openDuplicateReview }, "Review")),
    ...OPS.map(op => {
      const out = el("div", { class: "planout", hidden: true });
      const apply = el("button", { class: "btn", disabled: true, onclick: () => runApply(op, out, apply) }, "Apply");
      return el("div", { class: "op" },
        el("div", { class: "txt" }, el("b", {}, op.title), el("span", {}, op.desc), out),
        el("button", { class: "btn ghost", onclick: () => runPreview(op, out, apply) }, "Preview"),
        apply);
    }),
    renameBlock(),
    el("div", { class: "op", id: "op-faces" },
      el("div", { class: "txt" }, el("b", {}, "Find people"),
        el("span", { id: "faces-note" }, "Detect faces, then name the groups Blinkview finds.")),
      el("button", { class: "btn ghost", onclick: analyze }, "Detect faces"),
      el("button", { class: "btn", onclick: openReview }, "Review people")),
    el("div", { class: "op" },
      el("div", { class: "txt" }, el("b", {}, "Undo"), el("span", {}, "Reverse the most recent change.")),
      el("button", { class: "btn ghost", onclick: doUndo }, "Undo last")),
    el("div", { class: "op" },
      el("div", { class: "txt" }, el("b", {}, "Check for updates…"),
        el("span", {}, "Contacts GitHub for release metadata only. No photo or library data leaves this Mac.")),
      el("button", { class: "btn ghost", onclick: () => checkUpdates(true) }, "Check")));
  $("#sheet").hidden = false;
  checkModels();
}

/* Face work needs two ONNX models that are not shipped with the app. If they are
   missing, say so here and offer to fetch them rather than failing later with a
   file-not-found deep inside detection. */
async function checkModels() {
  const st = await invoke("models_status").catch(() => []);
  const missing = st.filter(m => !m.present);
  const note = $("#faces-note");
  const row = $("#op-faces");
  if (!note || !row) return;
  row.querySelector(".getmodels")?.remove();
  if (!missing.length) return;
  const mb = Math.round(missing.reduce((a, m) => a + m.megabytes, 0));
  note.textContent = `Needs the face models (${mb} MB) — they are not bundled.`;
  row.append(el("button", {
    class: "btn getmodels",
    onclick: async () => {
      const msg = await busy("Downloading face models…", () => invoke("models_fetch"));
      toast(msg, "ok");
      checkModels();
    }
  }, `Download ${mb} MB`));
}
async function runPreview(op, out, applyBtn) {
  const plan = await busy(`Planning ${op.title.toLowerCase()}…`,
    () => invoke("plan_op", { path: S.source, op: op.id, param: null }));
  const lines = plan.moves.slice(0, 40).map(([a, b]) => `${a}  →  ${b}`);
  const extra = plan.moves.length > 40 ? `\n… and ${plan.moves.length - 40} more` : "";
  const skipped = plan.skipped.length
    ? `\n\nLeft alone (${plan.skipped.length}):\n` + plan.skipped.slice(0, 8).map(([p, w]) => `${p} — ${w}`).join("\n")
    : "";
  out.hidden = false;
  out.textContent = plan.moves.length
    ? `${plan.moves.length} changes:\n` + lines.join("\n") + extra + skipped
    : "Nothing to change." + skipped;
  applyBtn.disabled = plan.moves.length === 0;
}
async function runApply(op, out, applyBtn) {
  const msg = await busy(`Applying ${op.title.toLowerCase()}…`,
    () => invoke("apply_op", { path: S.source, op: op.id, param: null }));
  toast(msg, "ok");
  applyBtn.disabled = true; out.hidden = true;
  await invoke("rescan", { path: S.source });
  await refreshSources(); await loadPhotos();
}
async function analyze() {
  const msg = await busy("Detecting faces… this runs once per photo", () => invoke("analyze_faces", { path: S.source }), S.source);
  toast(msg, "ok");
  $("#sheet").hidden = true;
  await refreshPeople(); await loadPhotos();
  const unnamed = S.people.filter(p => !p.name).length;
  if (unnamed) toast(`${unnamed} people to name — see the sidebar`, "ok");
}
async function doUndo() {
  const msg = await busy("Undoing…", () => invoke("undo", { path: S.source, id: null }));
  toast(msg, "ok");
  await invoke("rescan", { path: S.source });
  await refreshSources(); await loadPhotos();
}

/* ---------------- people review ---------------- */
const chosen = new Map();
const cosine = (a, b) => { let s = 0; for (let i = 0; i < a.length; i++) s += a[i] * b[i]; return s; };

async function openReview() {
  const cl = await busy("Grouping faces…", () => invoke("clusters", { path: S.source, distance: 0.55 }), S.source);
  chosen.clear();
  $("#sheet-title").textContent = "Who is this?";
  const known = (S.sources.find(s => s.path === S.source)?.people || []).map(p => p.name);

  const grid = el("div", { class: "pgrid" });
  const save = el("button", { class: "btn", disabled: true, onclick: () => saveNames(cl) }, "Save names");

  const paint = () => {
    for (const c of cl) {
      const card = grid.querySelector(`[data-c="${c.id}"]`);
      if (card) card.dataset.named = chosen.get(c.id) ? "1" : "0";
      const row = card?.querySelector(".chips");
      row?.querySelectorAll(".chip.echo").forEach(n => n.remove());
      if (!row || chosen.get(c.id)) continue;
      const best = new Map();
      for (const [oid, name] of chosen) {
        if (!name || oid === c.id) continue;
        const other = cl.find(x => x.id === oid);
        if (!other) continue;
        const s = cosine(c.centroid, other.centroid);
        if (s >= 0.45 && s > (best.get(name) ?? 0)) best.set(name, s);
      }
      for (const [name, s] of [...best].sort((a, b) => b[1] - a[1]).slice(0, 2)) {
        row.prepend(el("button", {
          class: "chip echo",
          onclick: () => { card.querySelector(".pname").value = name; chosen.set(c.id, name); paint(); }
        }, `Also ${name}? ${s.toFixed(2)}`));
      }
    }
    const n = [...chosen.values()].filter(Boolean).length;
    save.disabled = n === 0;
    save.textContent = n ? `Save ${n} name${n > 1 ? "s" : ""}` : "Save names";
  };

  grid.replaceChildren(...cl.map(c => {
    const name = el("input", {
      class: "pname", type: "text", placeholder: c.suggestion || "Add a name",
      "aria-label": `Name for group ${c.id}`,
      oninput: e => { const v = e.target.value.trim(); v ? chosen.set(c.id, v) : chosen.delete(c.id); paint(); }
    });
    const chips = el("div", { class: "chips" },
      ...[...new Set([c.suggestion, ...known].filter(Boolean))].map(nm =>
        el("button", { class: "chip", onclick: () => { name.value = nm; chosen.set(c.id, nm); paint(); } }, nm)));
    return el("div", { class: "pcard", "data-c": c.id, "data-named": "0" },
      el("div", { class: "hero" },
        c.crops[0] ? el("img", { src: c.crops[0], alt: "" }) : null,
        el("span", { class: "cnt" }, `${c.photo_count} photos`)),
      c.crops.length > 1 ? el("div", { class: "strip" },
        c.crops.slice(1, 5).map(src => el("img", { src, alt: "" }))) : null,
      el("div", { class: "foot" }, name, chips));
  }));

  $("#sheet-body").replaceChildren(
    cl.length ? grid : el("p", { style: "color:var(--text-dim)" },
      "No unnamed faces. Run “Detect faces” first, or everyone here is already named."),
    el("div", { class: "op" }, el("div", { class: "txt" }), save));
  $("#sheet").hidden = false;
  paint();
}
async function saveNames(cl) {
  const assignments = {};
  for (const [k, v] of chosen) if (v) assignments[k] = v;
  const n = await busy("Learning faces…",
    () => invoke("name_clusters", { path: S.source, distance: 0.55, assignments }));
  toast(`Learned ${n} reference faces`, "ok");
  $("#sheet").hidden = true;
  await refreshSources(); await loadPhotos();
}

/* ---------------- drag and drop ----------------
   Tauri reports OS drops on the window itself; the webview never sees a real path in
   a DOM drop event, so the listener is on the Tauri event rather than `ondrop`. */
async function handleOpenPath(path) {
  try {
    const target = await busy(`Opening ${path.split("/").pop() || path}…`,
      () => invoke("open_path", { path }));
    if (target.mode === "peek") {
      await enterPeek(target.peek, target.file || null);
      return;
    }
    await refreshSources();
    await selectSource(target.path);
    if (target.folder) await selectFolder(target.folder);
    if (target.file) {
      const wanted = S.view.find(photo => photo.path === target.file);
      if (wanted) openLightbox(wanted);
      else toast(`${target.file.split("/").pop()} is not in the indexed library`, "error");
    }
  } catch { /* busy already reports the backend's explanation */ }
}

let openPathChain = Promise.resolve();
function drainOpenPaths() {
  openPathChain = openPathChain.then(async () => {
    const paths = await invoke("take_open_paths");
    for (const path of [...new Set(paths)]) await handleOpenPath(path);
  }).catch(error => toast(String(error), "error"));
  return openPathChain;
}

/* A drag that begins inside the app is a pan or a selection, never a folder drop.
   Without this guard, dragging to pan a zoomed photo raised the drop overlay. */
const dropBlocked = () => !$("#lightbox").hidden || !$("#sheet").hidden;

listen("tauri://drag-enter", () => { if (!dropBlocked()) document.body.classList.add("dropping"); });
listen("tauri://drag-leave", () => document.body.classList.remove("dropping"));
listen("tauri://drag-drop", async ({ payload }) => {
  document.body.classList.remove("dropping");
  if (dropBlocked()) return;
  const paths = payload?.paths || [];
  if (!paths.length) return;
  await handleOpenPath(paths[0]);
});

// Native open events can arrive before this script is ready. The backend queues them;
// the event is only a prompt to drain that queue, which also prevents handling one
// path twice when launch delivery and the live event overlap.
listen("open-path", drainOpenPaths);

/* ---------------- editing ----------------
   Edits are previewed with a CSS transform and only written when saved, so nothing
   touches a photo until the user commits. */

function editState() {
  if (!S.edit) {
    S.edit = { rotate: 0, flipH: false, crop: null, straighten: 0,
               brightness: 0, contrast: 0, saturation: 0 };
  }
  return S.edit;
}

/* Zoom needed so a straightened photo shows no blank corner — the mirror of
   edit::inscribed_same_aspect. The preview must zoom by exactly what the save trims,
   or it shows the user something they will not get. */
function straightenZoom(w, h, degrees) {
  if (!w || !h || !degrees) return 1;
  const a = w / h;
  const rad = Math.abs(degrees) * Math.PI / 180;
  const sin = Math.abs(Math.sin(rad)), cos = Math.abs(Math.cos(rad));
  const v = Math.min(w / (2 * (a * cos + sin)), h / (2 * (a * sin + cos)));
  const kw = 2 * a * v;
  return kw > 0 ? w / kw : 1;
}

/* Adjustments preview through a CSS filter, which is the compositor's job and costs
   nothing; the same numbers are applied per-pixel in Rust on save. The mapping has to
   agree with edit::adjust or the preview would lie. */
function filterFor(e) {
  if (!e) return "";
  const b = 1 + (e.brightness || 0) / 100 * 0.8;
  const c = Math.pow(((e.contrast || 0) / 100) + 1, 2);
  const s = 1 + (e.saturation || 0) / 100;
  return `brightness(${b}) contrast(${c}) saturate(${s})`;
}

/* ---------------- crop ----------------
   The rectangle is kept in fractions of the *displayed* image, which is also the space
   the backend crops in (it rotates and flips before cropping, so the two agree). */

/* The crop rectangle must sit over the photo's *layout* box, not its rendered bounding
   box. Once the image is rotated, getBoundingClientRect returns the enclosing box of the
   rotated shape — larger than the visible photo — so crop fractions computed from it
   land in the wrong place. Transforms do not affect layout, and the straightened result
   fills exactly the layout box, so that is the correct frame. */
function imgRect() {
  const img = $("#lb-img");
  const stage = document.querySelector(".lb-stage").getBoundingClientRect();
  return {
    left: stage.left + img.offsetLeft,
    top: stage.top + img.offsetTop,
    width: img.offsetWidth,
    height: img.offsetHeight,
  };
}

function startCrop() {
  if (!S.lbList[S.lbIndex]) return;
  S.cropping = true;
  S.crop = S.edit?.crop ? { ...S.edit.crop } : { x: 0.08, y: 0.08, w: 0.84, h: 0.84 };
  resetZoom();
  $("#cropper").hidden = false;
  $("#cropbar").hidden = false;
  drawCrop();
}

function endCrop(commit) {
  if (commit) {
    const c = S.crop;
    // A crop that covers everything is not a crop.
    editState().crop = (c && (c.w < 0.995 || c.h < 0.995)) ? { ...c } : null;
  }
  S.cropping = false;
  $("#cropper").hidden = true;
  $("#cropbar").hidden = true;
  applyEditPreview();
}

function drawCrop() {
  if (!S.cropping) return;
  const r = imgRect();
  const stage = document.querySelector(".lb-stage").getBoundingClientRect();
  const box = $("#crop-rect");
  box.style.left = (r.left - stage.left + S.crop.x * r.width) + "px";
  box.style.top = (r.top - stage.top + S.crop.y * r.height) + "px";
  box.style.width = (S.crop.w * r.width) + "px";
  box.style.height = (S.crop.h * r.height) + "px";

  // Report the pixel size that will actually be written, not the on-screen size.
  const p = S.lbList[S.lbIndex];
  const quarter = (S.edit?.rotate || 0) % 180 !== 0;
  const srcW = quarter ? p.height : p.width;
  const srcH = quarter ? p.width : p.height;
  if (srcW && srcH) {
    $("#cropdims").textContent =
      `${Math.round(S.crop.w * srcW)} × ${Math.round(S.crop.h * srcH)}`;
  }
}

/** Resize from a handle, honouring a locked aspect ratio and staying inside the image. */
function resizeCrop(handle, fx, fy) {
  const c = S.crop;
  let { x, y, w, h } = c;
  const MIN = 0.04;
  if (handle.includes("w")) { const nx = Math.min(fx, x + w - MIN); w += x - nx; x = nx; }
  if (handle.includes("e")) { w = Math.max(MIN, Math.min(fx - x, 1 - x)); }
  if (handle.includes("n")) { const ny = Math.min(fy, y + h - MIN); h += y - ny; y = ny; }
  if (handle.includes("s")) { h = Math.max(MIN, Math.min(fy - y, 1 - y)); }

  if (S.cropAR) {
    // Keep the ratio in *pixel* space, which is not the same as fraction space.
    const r = imgRect();
    const px = w * r.width, py = h * r.height;
    if (px / py > S.cropAR) w = (py * S.cropAR) / r.width;
    else h = (px / S.cropAR) / r.height;
    if (handle.includes("n")) y = c.y + c.h - h;
    if (handle.includes("w")) x = c.x + c.w - w;
  }
  S.crop = {
    x: Math.max(0, Math.min(x, 1 - MIN)),
    y: Math.max(0, Math.min(y, 1 - MIN)),
    w: Math.max(MIN, Math.min(w, 1 - Math.max(0, x))),
    h: Math.max(MIN, Math.min(h, 1 - Math.max(0, y))),
  };
  drawCrop();
}

function rotateBy(deg) {
  const e = editState();
  e.rotate = (e.rotate + deg + 360) % 360;
  applyEditPreview();
}

function applyEditPreview() {
  const img = $("#lb-img");
  const e = S.edit;
  const dirty = e && (e.rotate !== 0 || e.crop || e.flipH || Math.abs(e.straighten || 0) >= 0.05
    || e.brightness || e.contrast || e.saturation);
  $("#lb-save").hidden = !dirty;
  $("#lb-revert").hidden = !dirty;
  if (!img) return;
  const r = (S.edit?.rotate || 0) + (S.edit?.straighten || 0);
  const flip = S.edit?.flipH ? " scaleX(-1)" : "";
  img.style.filter = filterFor(S.edit);

  // Zoom past the blank corners by the same factor the save trims by, and clip to the
  // photo's own box so the preview frames exactly the saved result.
  const deg = S.edit?.straighten || 0;
  const shown = img.getBoundingClientRect();
  const sZoom = deg
    ? straightenZoom(shown.width || img.naturalWidth, shown.height || img.naturalHeight, deg)
    : 1;
  img.classList.toggle("straightening", !!deg);
  // Rotating a landscape photo into portrait needs the preview scaled to fit.
  const stage = document.querySelector(".lb-stage");
  const quarter = r === 90 || r === 270;
  let fit = 1;
  if (quarter && img.naturalWidth) {
    const availW = stage.clientWidth - 32, availH = stage.clientHeight - 32;
    const shown = img.getBoundingClientRect();
    if (shown.height > 0) fit = Math.min(availW / shown.height, availH / shown.width, 1);
  }
  img.style.transform =
    `translate(${S.panX}px, ${S.panY}px) scale(${S.zoom * fit * sZoom}) rotate(${r}deg)${flip}`;
  if (S.cropping) drawCrop();
}

function discardEdit() {
  S.edit = null;
  applyEditPreview();
  applyZoom();
}

async function saveEdit() {
  const p = S.lbList[S.lbIndex];
  if (!p || !S.edit) return;
  const keep = await askKeepOriginal();
  if (keep === null) return;
  S.keepOriginal = keep;
  const rotate = { 0: "none", 90: "cw90", 180: "cw180", 270: "cw270" }[S.edit.rotate] || "none";
  const msg = await busy("Saving…", () => invoke("edit_photo", {
    path: S.source, hash: p.hash,
    edit: {
      rotate, flip_h: !!S.edit.flipH, flip_v: false,
      straighten: S.edit.straighten || 0,
      adjust: {
        brightness: (S.edit.brightness || 0) / 100,
        contrast: (S.edit.contrast || 0) / 100,
        saturation: (S.edit.saturation || 0) / 100,
      },
      crop: S.edit.crop, keep_original: keep
    }
  }));
  toast(msg, "ok");
  S.edit = null;
  closeLightbox();
  await refreshSources(); await loadPhotos();
}

/* Asked once per session, defaulting to keeping the original. Editing is the only
   thing here that changes a photograph, so the safe answer is the pre-selected one. */
function askKeepOriginal() {
  return new Promise(resolve => {
    const box = el("div", { class: "sheet", id: "ask" },
      el("div", { class: "sheet-panel small", role: "dialog", "aria-modal": "true" },
        el("div", { class: "sheet-head" }, el("h2", {}, "Save changes")),
        el("div", { class: "sheet-body" },
          el("p", { class: "asktext" },
            "Blinkview can keep the untouched original so you can go back to it."),
          el("label", { class: "askopt" },
            el("input", { type: "radio", name: "keep", value: "1", checked: true }),
            el("span", {}, el("b", {}, "Keep the original"),
              el("small", {}, "Moved to the Originals folder. Reversible."))),
          el("label", { class: "askopt" },
            el("input", { type: "radio", name: "keep", value: "0" }),
            el("span", {}, el("b", {}, "Replace it"),
              el("small", {}, "The original is not kept. This cannot be undone."))),
          el("div", { class: "askrow" },
            el("button", { class: "btn ghost", onclick: () => { box.remove(); resolve(null); } }, "Cancel"),
            el("button", {
              class: "btn btn-primary",
              onclick: () => {
                const v = box.querySelector("input[name=keep]:checked").value === "1";
                box.remove(); resolve(v);
              }
            }, "Save")))));
    document.body.append(box);
    if (!S.keepOriginal) box.querySelector('input[value="0"]').checked = true;
  });
}

/* ---------------- filter panel ----------------
   Every filter also exists as a search term, and the panel writes into the search box
   rather than keeping a parallel state. One source of truth means a filter chosen by
   tapping and one typed by hand can never disagree. */

function queryHas(term) {
  return $("#search").value.toLowerCase().split(/\s+/).includes(term.toLowerCase());
}

function toggleTerm(term, group = []) {
  const cur = $("#search").value.split(/\s+/).filter(Boolean);
  const lower = term.toLowerCase();
  const had = cur.some(t => t.toLowerCase() === lower);
  // Drop anything from the same group, so picking a month replaces the last one.
  let next = cur.filter(t => !group.some(g => g.toLowerCase() === t.toLowerCase()));
  if (!had) next.push(term);
  $("#search").value = next.join(" ");
  applyFilter();
  renderFilters();
  syncClear();
}

/* ---------------- search suggestions ----------------
   A face is easier to recognise than a name is to recall, so focusing the field shows
   who is in the library rather than waiting to be asked. It is also how the semantic
   search becomes discoverable: nobody guesses they can type "a church" unless shown. */

const SCENE_IDEAS = ["a night sky", "snowy mountains", "green trees",
                     "food on a plate", "a city street", "the beach"];

function renderSuggest() {
  const box = $("#suggest");
  const typed = $("#search").value.trim();
  if (!S.source || typed) { box.hidden = true; return; }

  const kids = [];
  const named = S.people.filter(p => p.name && p.cover);
  if (named.length) {
    kids.push(el("h4", {}, "People"));
    const grid = el("div", { class: "sgrid" });
    for (const p of named.slice(0, 8)) {
      const b = el("button", { class: "sface", type: "button", title: p.name },
        el("img", { src: photoUrl(p.cover), alt: "" }), el("span", {}, p.name));
      b.onmousedown = e => e.preventDefault();   // keep focus; blur would close this
      b.onclick = () => { pickSuggestion(p.name); };
      grid.append(b);
    }
    kids.push(grid);
  }

  // Only offered once the library has been embedded — suggesting a search that
  // cannot run is worse than not suggesting one.
  if (S.semanticReady && S.semanticReady.available && S.semanticReady.embedded > 0) {
    kids.push(el("h4", {}, "Try a scene"));
    const grid = el("div", { class: "sgrid" });
    for (const q of SCENE_IDEAS) {
      const b = el("button", { class: "sugg-pill scene", type: "button" }, q);
      b.onmousedown = e => e.preventDefault();
      b.onclick = () => pickSuggestion(q);
      grid.append(b);
    }
    kids.push(grid);
  }

  if (!kids.length) { box.hidden = true; return; }
  box.replaceChildren(...kids);
  box.hidden = false;
}

function pickSuggestion(term) {
  $("#search").value = term;
  $("#suggest").hidden = true;
  applyFilter();
  renderFilters();
  syncClear();
}

function renderFilters() {
  if ($("#filters").hidden) return;
  const opt = (label, term, group, extra) =>
    el("button", {
      class: "fopt", "aria-pressed": String(queryHas(term)),
      onclick: () => toggleTerm(term, group)
    }, extra || null, label);

  const names = S.people.filter(p => p.name);
  $("#f-people").replaceChildren(...names.slice(0, 10).map(p =>
    opt(p.name, p.name, names.map(x => x.name),
      p.cover ? el("img", { class: "fface", src: photoUrl(p.cover), alt: "" }) : null)));
  if (!names.length) $("#f-people").replaceChildren(el("span", { class: "meta" }, "No one named yet"));

  const years = [...new Set(S.photos.map(p => p.taken_at && new Date(p.taken_at * 1000).getFullYear())
    .filter(Boolean))].sort((a, b) => b - a);
  const months = MONTHS.map(m => m.slice(0, 3));
  $("#f-when").replaceChildren(
    ...years.slice(0, 6).map(y => opt(String(y), String(y), years.map(String))),
    ...months.map(m => opt(m.replace(/^./, c => c.toUpperCase()), m, months)));

  const ratings = ["1star", "2star", "3star", "4star", "5star"];
  $("#f-rating").replaceChildren(...ratings.map((r, i) =>
    opt("★".repeat(i + 1) + (i < 4 ? "+" : ""), r, ratings)));

  $("#f-label").replaceChildren(...LABEL_NAMES.map(l =>
    opt(l, l, LABEL_NAMES, el("span", { class: "swatch", style: `background:${labelColour(l)}` }))));

  const types = ["photos", "videos"];
  $("#f-type").replaceChildren(...types.map(t => opt(t.replace(/^./, c => c.toUpperCase()), t, types)));

  const groups = [["date", "By date"], ["folder", "By folder"]];
  const gEl = $("#f-group");
  if (gEl) {
    const arranged = S.sort === "custom";
    gEl.replaceChildren(...groups.map(([k, label]) =>
      el("button", {
        class: "fopt", "aria-pressed": String(!arranged && S.group === k),
        // Under a custom arrangement the grid is one run, so a grouping control that
        // still looked live would be a lie.
        disabled: arranged,
        title: arranged ? "A custom arrangement is not grouped" : null,
        onclick: () => { S.group = k; renderGrid(); renderFilters(); }
      }, label)));
  }

  const sorts = [["newest", "Newest"], ["oldest", "Oldest"], ["name", "Name"],
                 ["rating", "Rating"], ["size", "Size"], ["custom", "Custom"]];
  $("#f-sort").replaceChildren(...sorts.map(([k, label]) =>
    el("button", {
      class: "fopt", "aria-pressed": String(S.sort === k),
      title: k === "custom" ? "Drag photographs into the order you want" : null,
      onclick: () => {
        S.sort = k;
        // Choosing Custom with nothing arranged yet starts from what is on screen,
        // so the order does not jump the moment it is switched on.
        if (k === "custom" && !S.order.length) S.order = S.view.map(p => p.hash);
        applyFilter();
        renderFilters();
        saveFolderView();
      }
    }, label)));
}

function labelColour(l) {
  return { red: "#f87171", orange: "#fb923c", yellow: "#fbbf24", green: "#4ade80",
           blue: "#60a5fa", purple: "#c4b5fd", grey: "#9ca3af" }[l] || "#888";
}

function monthKey(ts) {
  const d = new Date(ts * 1000);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}`;
}

function renderTimelineTools() {
  const tools = $("#timeline-tools");
  if (!tools) return;
  tools.hidden = !S.source || !S.view.length || !!S.peek;
  $("#sort-newest").setAttribute("aria-pressed", String(S.sort === "newest"));
  $("#sort-oldest").setAttribute("aria-pressed", String(S.sort === "oldest"));
  const select = $("#month-jump");
  const prior = select.value;
  const keys = [...new Set(S.view.filter(p => p.taken_at).map(p => monthKey(p.taken_at)))];
  select.replaceChildren(el("option", { value: "" }, "Jump to month…"), ...keys.map(key => {
    const [year, month] = key.split("-").map(Number);
    const label = new Date(year, month - 1, 1).toLocaleDateString(undefined, { month: "long", year: "numeric" });
    return el("option", { value: key }, label);
  }));
  if (keys.includes(prior)) select.value = prior;
  select.disabled = !["newest", "oldest"].includes(S.sort);
  select.title = select.disabled ? "Choose Newest or Oldest to jump through the timeline" : "";
}

function setTimelineSort(sort) {
  S.sort = sort;
  S.group = "date";
  S.resetScroll = true;
  applyFilter();
  saveFolderView();
}

function jumpToMonth(key) {
  if (!key) return;
  const block = LAYOUT.blocks.find(b => b.kind === "head" && b.month === key);
  if (!block) return toast("That month is not in this view");
  $("#main").scrollTo({ top: Math.max(0, block.y - 6), behavior: "smooth" });
}

/* ---------------- arranging a folder ----------------
   The arrangement lives in that folder's own `blinkview.json` (ADR-0010), beside the
   ratings of the photographs it holds — a folder saying how it is ordered is the same
   kind of fact as a folder saying what is in it, and it travels with the folder when
   it is copied in Finder. It is read from that folder alone, never inherited: a
   subfolder nobody arranged must not be reordered by its parent. */

let dragHash = null;

/** The folder whose arrangement is on screen. `""` is the library root. */
function folderKey() {
  return S.folder || "";
}

/** Arranging is only offered over a whole folder — not a search result or a person. */
function canArrangeHere() {
  return !S.peek && !S.person && S.cluster === null && !$("#search").value.trim();
}

function clearDropMarks() {
  for (const c of document.querySelectorAll(".cell.dropbefore,.cell.dropafter")) {
    c.classList.remove("dropbefore", "dropafter");
  }
}

async function loadFolderView() {
  S.sort = "newest";
  S.order = [];
  if (!S.source || S.peek) return;
  try {
    const v = await invoke("folder_view", { path: S.source, folder: folderKey() });
    if (v?.sort) S.sort = v.sort;
    S.order = v?.order || [];
  } catch {
    // A folder with no file has no arrangement, which is not an error.
  }
}

function saveFolderView() {
  if (!S.source || S.peek) return;
  invoke("set_folder_view", {
    path: S.source, folder: folderKey(), sort: S.sort, order: S.order,
  }).catch(e => toast(String(e), "error"));
}

/** Move one photograph before or after another, and remember the result. */
function reorderTo(fromHash, toHash, after) {
  // Seed from what is on screen, so the first drag places everything rather than
  // leaving the rest to fall in by date around one pinned photograph.
  const order = S.order.length ? S.order.slice() : S.view.map(p => p.hash);
  const known = new Set(order);
  for (const p of S.view) if (!known.has(p.hash)) order.push(p.hash);

  const from = order.indexOf(fromHash);
  if (from < 0) return;
  const [moved] = order.splice(from, 1);
  const at = order.indexOf(toHash);
  if (at < 0) return;
  order.splice(after ? at + 1 : at, 0, moved);

  S.order = order;
  S.sort = "custom";
  applyFilter();
  renderFilters();
  saveFolderView();
}

function sortView() {
  if (S.sort === "custom") {
    const rank = new Map(S.order.map((h, i) => [h, i]));
    // A photograph added since the arrangement falls in after it, newest first,
    // rather than disappearing from a folder someone arranged.
    S.view.sort((a, b) => {
      const ra = rank.has(a.hash) ? rank.get(a.hash) : Infinity;
      const rb = rank.has(b.hash) ? rank.get(b.hash) : Infinity;
      return ra !== rb ? ra - rb : (b.taken_at || 0) - (a.taken_at || 0);
    });
    return;
  }
  const by = {
    newest: (a, b) => (b.taken_at || 0) - (a.taken_at || 0),
    oldest: (a, b) => (a.taken_at || 0) - (b.taken_at || 0),
    name: (a, b) => a.name.localeCompare(b.name),
    rating: (a, b) => (b.rating || 0) - (a.rating || 0) || (b.taken_at || 0) - (a.taken_at || 0),
    size: (a, b) => (b.bytes || 0) - (a.bytes || 0),
  }[S.sort] || null;
  if (by) S.view.sort(by);
}

/* ---------------- rating, label, info ---------------- */

function paintStars() {
  const p = S.lbList[S.lbIndex];
  const box = $("#lb-stars");
  if (!p) return;
  box.replaceChildren(...[1, 2, 3, 4, 5].map(n =>
    el("button", {
      class: "star", "data-on": (p.rating || 0) >= n ? "1" : "0",
      "aria-label": `${n} star${n > 1 ? "s" : ""}`,
      disabled: !!S.peek,
      title: S.peek ? `“${S.peek.name}” is a read-only peek. Keep this folder before rating.` : null,
      onclick: async () => {
        const next = p.rating === n ? 0 : n;
        await invoke("set_rating", { path: S.source, hashes: [p.hash], rating: next });
        p.rating = next;
        paintStars(); renderGrid();
      }
    }, "\u2605")));
  $("#lb-labels").replaceChildren(...LABEL_NAMES.map(l =>
    el("button", {
      class: "labeldot", style: `background:${labelColour(l)}`,
      "aria-pressed": String(p.label === l), "aria-label": l,
      disabled: !!S.peek,
      title: S.peek ? `“${S.peek.name}” is a read-only peek. Keep this folder before labelling.` : null,
      onclick: async () => {
        const next = p.label === l ? null : l;
        await invoke("set_label", { path: S.source, hashes: [p.hash], label: next });
        p.label = next;
        paintStars(); renderGrid();
      }
    })));
}

async function toggleInfo() {
  const panel = $("#infopanel");
  if (!panel.hidden) { panel.hidden = true; return; }
  const p = S.lbList[S.lbIndex];
  if (!p) return;
  const d = await invoke("photo_detail", { path: S.source, hash: p.hash });
  const mb = (d.bytes / 1048576).toFixed(1);
  const row = (k, v) => v ? el("div", { class: "inforow" }, el("b", {}, k), el("span", {}, String(v))) : null;
  // row() returns null for absent values, and replaceChildren stringifies null into
  // a literal "null" text node — filter the list before handing it over.
  panel.replaceChildren(...[
    el("h3", {}, "Info"),
    row("File", d.path),
    row("Size", `${mb} MB`),
    row("Pixels", d.width ? `${d.width} × ${d.height}` : null),
    row("Taken", d.taken_at ? `${DAY(d.taken_at)} · ${TIME(d.taken_at)}` : "Unknown"),
    row("Date from", d.taken_from),
    row("Kind", d.kind),
    row("Camera", d.exif?.camera),
    row("Lens", d.exif?.lens),
    row("Exposure", [d.exif?.exposure, d.exif?.aperture, d.exif?.focal].filter(Boolean).join(" · ") || null),
    row("ISO", d.exif?.iso),
    // "Does this say where I live" is the question people actually have, so it is
    // answered either way rather than only when there is something to report.
    row("Location", d.exif?.gps || (d.exif?.present ? "not recorded" : null)),
    row("Faces", d.faces || null),
    // Face entries with no name yet come back as nulls; joining them raw prints
    // "null, null" — count the nulls as nobody and keep the row for real names.
    row("People", d.people.filter(Boolean).join(", ") || null),
    row("Rating", d.meta.rating ? "\u2605".repeat(d.meta.rating) : null),
    row("Label", d.meta.label),
    row("Albums", (d.meta.albums || []).join(", ") || null),
    // Offered where the metadata is being read, which is where someone decides they
    // would rather not send it to anybody.
    d.strippable && d.exif?.present
      ? el("button", { class: "btn ghost sm", style: "margin-top:var(--s3)",
          onclick: () => { S.sel.clear(); S.sel.add(d.hash); paintSel(); stripSelectedPrompt(); } },
          "Strip metadata\u2026")
      : null,
  ].filter(Boolean));
  panel.hidden = false;
}

/* ---------------- map ----------------
   Drawn, never fetched. Every other photo app streams raster tiles, which means the
   tile server learns where its users have been on every pan — for a library whose
   whole premise is that nothing leaves the machine (ADR-0001), that is the one leak
   that would undo the premise. So the basemap is Natural Earth outlines bundled at two
   levels of detail, projected Web Mercator onto a canvas. The upside of having no
   tiles to wait for is that it pans at frame rate from the first paint.

   Places come from the bundled GeoNames table through `photo_places`, so the label
   under the cursor is resolved the same way the search box resolves a typed city. */

const MAP = {
  points: [],        // {hash, path, lat, lon, place, wx, wy}
  clusters: [],      // {x, y, n, items, place}
  rings: { 110: null, 50: null },
  zoom: 1.6,
  cx: 0, cy: 20,     // centre, in degrees
  hover: null,
  raf: 0,
  bucketZoom: null,
};

/* Web Mercator, normalised to 0..1. y is clamped short of the poles, where the
   projection runs to infinity. */
function project(lon, lat) {
  const x = (lon + 180) / 360;
  const s = Math.sin(Math.max(-85.05, Math.min(85.05, lat)) * Math.PI / 180);
  const y = 0.5 - Math.log((1 + s) / (1 - s)) / (4 * Math.PI);
  return [x, y];
}
function unproject(x, y) {
  const lon = x * 360 - 180;
  const lat = Math.atan(Math.sinh(Math.PI * (1 - 2 * y))) * 180 / Math.PI;
  return [lon, lat];
}
const mapScale = () => 256 * Math.pow(2, MAP.zoom);

/** World pixels -> canvas pixels, given the current centre. */
function mapView(canvas) {
  const dpr = window.devicePixelRatio || 1;
  const w = canvas.width / dpr, h = canvas.height / dpr;
  const s = mapScale();
  const [ccx, ccy] = project(MAP.cx, MAP.cy);
  return { w, h, s, ox: ccx * s - w / 2, oy: ccy * s - h / 2, dpr };
}

async function loadRings(level) {
  if (MAP.rings[level]) return MAP.rings[level];
  // Bundled beside the app; this is a local file read, not a network request.
  const res = await fetch(`world${level}.json`);
  const raw = await res.json();
  // One Path2D per ring, built once in world space and drawn under a transform, so
  // panning costs a matrix rather than a hundred thousand lineTo calls.
  const paths = raw.map(ring => {
    const p = new Path2D();
    for (let i = 0; i < ring.length; i++) {
      const [x, y] = project(ring[i][0], ring[i][1]);
      i ? p.lineTo(x, y) : p.moveTo(x, y);
    }
    p.closePath();
    return p;
  });
  MAP.rings[level] = paths;
  return paths;
}

/** Group photographs that would land on top of each other, in world space so a pan
    does not re-bucket. Recomputed only when the zoom level changes. */
function clusterPoints() {
  const s = mapScale();
  const cell = 46 / s;                       // ~46px apart on screen
  const buckets = new Map();
  for (const p of MAP.points) {
    const key = `${Math.floor(p.wx / cell)}:${Math.floor(p.wy / cell)}`;
    let b = buckets.get(key);
    if (!b) buckets.set(key, (b = { wx: 0, wy: 0, items: [] }));
    b.wx += p.wx; b.wy += p.wy; b.items.push(p);
  }
  MAP.clusters = [...buckets.values()].map(b => ({
    wx: b.wx / b.items.length,
    wy: b.wy / b.items.length,
    n: b.items.length,
    items: b.items,
    // The label of the most common place in the group reads better than the label of
    // whichever photograph happened to be first.
    place: commonest(b.items.map(i => i.place).filter(Boolean)),
  }));
  MAP.bucketZoom = Math.round(MAP.zoom * 2);
}

function commonest(list) {
  const tally = new Map();
  for (const v of list) tally.set(v, (tally.get(v) || 0) + 1);
  let best = null, n = 0;
  for (const [v, c] of tally) if (c > n) { best = v; n = c; }
  return best;
}

function css(name) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

function drawMap() {
  const canvas = $("#mapcanvas");
  if (!canvas || $("#mapview").hidden) return;
  const ctx = canvas.getContext("2d");
  const rect = canvas.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  if (canvas.width !== Math.round(rect.width * dpr) || canvas.height !== Math.round(rect.height * dpr)) {
    canvas.width = Math.round(rect.width * dpr);
    canvas.height = Math.round(rect.height * dpr);
  }
  const v = mapView(canvas);
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, v.w, v.h);

  // Sea: a flat wash rather than a gradient, so land reads as the figure.
  ctx.fillStyle = css("--map-sea");
  ctx.fillRect(0, 0, v.w, v.h);

  const level = MAP.zoom >= 4 ? 50 : 110;
  const paths = MAP.rings[level] || MAP.rings[110];
  if (paths) {
    ctx.save();
    ctx.translate(-v.ox, -v.oy);
    ctx.scale(v.s, v.s);
    ctx.fillStyle = css("--map-land");
    ctx.strokeStyle = css("--map-coast");
    // The transform scales line width too, so undo it to keep a hairline a hairline.
    ctx.lineWidth = 1 / v.s;
    for (const p of paths) { ctx.fill(p); ctx.stroke(p); }
    ctx.restore();
  }

  if (Math.round(MAP.zoom * 2) !== MAP.bucketZoom) clusterPoints();

  const accent = css("--accent");
  for (const c of MAP.clusters) {
    const x = c.wx * v.s - v.ox, y = c.wy * v.s - v.oy;
    if (x < -60 || y < -60 || x > v.w + 60 || y > v.h + 60) continue;
    const r = Math.min(30, 9 + Math.sqrt(c.n) * 3.2);
    const hot = MAP.hover === c;
    ctx.beginPath();
    ctx.arc(x, y, r + (hot ? 3 : 0), 0, Math.PI * 2);
    ctx.fillStyle = accent;
    ctx.globalAlpha = hot ? 1 : 0.88;
    ctx.fill();
    ctx.globalAlpha = 1;
    ctx.lineWidth = 2;
    ctx.strokeStyle = css("--map-pin-ring");
    ctx.stroke();
    if (c.n > 1) {
      ctx.fillStyle = css("--accent-ink");
      ctx.font = `600 ${r > 16 ? 13 : 11}px ui-sans-serif,-apple-system,"SF Pro Text",sans-serif`;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText(String(c.n), x, y + 0.5);
    }
  }
}

function scheduleMap() {
  if (MAP.raf) return;
  MAP.raf = requestAnimationFrame(() => { MAP.raf = 0; drawMap(); });
}

/** The cluster under a pointer, if any. */
function clusterAt(clientX, clientY) {
  const canvas = $("#mapcanvas");
  const rect = canvas.getBoundingClientRect();
  const v = mapView(canvas);
  const px = clientX - rect.left, py = clientY - rect.top;
  let best = null, bestD = 1e9;
  for (const c of MAP.clusters) {
    const x = c.wx * v.s - v.ox, y = c.wy * v.s - v.oy;
    const r = Math.min(30, 9 + Math.sqrt(c.n) * 3.2) + 4;
    const d = Math.hypot(px - x, py - y);
    if (d <= r && d < bestD) { best = c; bestD = d; }
  }
  return best;
}

/** Frame every located photograph. */
function fitMap() {
  if (!MAP.points.length) { MAP.zoom = 1.4; MAP.cx = 0; MAP.cy = 20; return; }
  let x0 = 1, y0 = 1, x1 = 0, y1 = 0;
  for (const p of MAP.points) {
    x0 = Math.min(x0, p.wx); x1 = Math.max(x1, p.wx);
    y0 = Math.min(y0, p.wy); y1 = Math.max(y1, p.wy);
  }
  const canvas = $("#mapcanvas");
  const rect = canvas.getBoundingClientRect();
  const pad = 120;
  const w = Math.max(1e-6, x1 - x0), h = Math.max(1e-6, y1 - y0);
  const fit = Math.min((rect.width - pad) / (w * 256), (rect.height - pad) / (h * 256));
  MAP.zoom = Math.max(0.6, Math.min(12, Math.log2(Math.max(fit, 1e-6))));
  [MAP.cx, MAP.cy] = unproject((x0 + x1) / 2, (y0 + y1) / 2);
  MAP.bucketZoom = null;
}

async function openMap() {
  if (!S.source) return toast("Add a folder first");
  $("#mapview").hidden = false;
  $("#stage").hidden = true;
  // The map fills #main, so the grid's header would show through its own HUD.
  document.querySelector(".libhead").hidden = true;
  $("#qchips").dataset.wasShown = String(!$("#qchips").hidden);
  $("#qchips").hidden = true;
  $("#filters").dataset.wasShown = String(!$("#filters").hidden);
  $("#filters").hidden = true;
  $("#btn-map").setAttribute("aria-pressed", "true");
  await loadMapData();
}

function closeMap() {
  $("#mapview").hidden = true;
  $("#stage").hidden = false;
  document.querySelector(".libhead").hidden = false;
  $("#qchips").hidden = $("#qchips").dataset.wasShown !== "true";
  $("#filters").hidden = $("#filters").dataset.wasShown !== "true";
  $("#btn-map").setAttribute("aria-pressed", "false");
}

function toggleMap() {
  $("#mapview").hidden ? openMap() : closeMap();
}

/** Show one cluster's photographs in the grid. */
function showCluster(c) {
  const want = new Set(c.items.map(i => i.hash));
  closeMap();
  S.person = null; S.cluster = null;
  S.clusterHashes = want;
  S.folder = null;
  $("#search").value = "";
  syncClear();
  S.resetScroll = true;
  renderSidebar();
  applyFilter();
  toast(`${want.size} from ${c.place || "here"}`, "ok");
}

{
  const canvas = $("#mapcanvas");
  let drag = null;
  canvas.addEventListener("pointerdown", e => {
    drag = { x: e.clientX, y: e.clientY, cx: MAP.cx, cy: MAP.cy, moved: false };
    canvas.setPointerCapture(e.pointerId);
    canvas.classList.add("dragging");
  });
  canvas.addEventListener("pointermove", e => {
    if (drag) {
      const v = mapView(canvas);
      const [sx, sy] = project(drag.cx, drag.cy);
      const nx = sx - (e.clientX - drag.x) / v.s;
      const ny = sy - (e.clientY - drag.y) / v.s;
      [MAP.cx, MAP.cy] = unproject(nx, Math.max(0, Math.min(1, ny)));
      if (Math.hypot(e.clientX - drag.x, e.clientY - drag.y) > 3) drag.moved = true;
      scheduleMap();
      return;
    }
    const c = clusterAt(e.clientX, e.clientY);
    if (c !== MAP.hover) {
      MAP.hover = c;
      canvas.style.cursor = c ? "pointer" : "grab";
      if (c) {
        $("#mapcount").textContent = `${c.n} photograph${c.n === 1 ? "" : "s"}`;
        $("#mapwhere").textContent = c.place || "somewhere unnamed";
      } else {
        $("#mapcount").textContent = `${MAP.points.length} located`;
        $("#mapwhere").textContent = `${MAP.clusters.length} place${MAP.clusters.length === 1 ? "" : "s"}`;
      }
      scheduleMap();
    }
  });
  const endDrag = e => {
    if (drag && !drag.moved) {
      const c = clusterAt(e.clientX, e.clientY);
      if (c) showCluster(c);
    }
    drag = null;
    canvas.classList.remove("dragging");
  };
  canvas.addEventListener("pointerup", endDrag);
  canvas.addEventListener("pointercancel", () => { drag = null; canvas.classList.remove("dragging"); });
  canvas.addEventListener("wheel", e => {
    e.preventDefault();
    // Zoom about the cursor, so the place under it stays under it.
    const rect = canvas.getBoundingClientRect();
    const v = mapView(canvas);
    const wx = (e.clientX - rect.left + v.ox) / v.s;
    const wy = (e.clientY - rect.top + v.oy) / v.s;
    MAP.zoom = Math.max(0.6, Math.min(14, MAP.zoom - e.deltaY * 0.0022));
    const nv = mapView(canvas);
    const nx = wx - (e.clientX - rect.left) / nv.s + (nv.w / 2) / nv.s;
    const ny = wy - (e.clientY - rect.top) / nv.s + (nv.h / 2) / nv.s;
    [MAP.cx, MAP.cy] = unproject(nx, Math.max(0, Math.min(1, ny)));
    if (MAP.zoom >= 4) loadRings(50);
    scheduleMap();
  }, { passive: false });
}

/** Give photographs that carry no coordinates a place, by name. */
async function placePrompt() {
  const hashes = [...S.sel];
  if (!hashes.length) return;
  const chosen = await new Promise(resolve => {
    let d;
    const list = el("div", { class: "movepicks" });
    const input = el("input", {
      class: "nameinput", type: "text", style: "width:100%",
      placeholder: "A town or city — Santorini, Kyoto, Reykjavík",
      "aria-label": "Where was this taken?",
      oninput: async e => {
        const q = e.target.value;
        const hits = q.trim().length >= 2
          ? await invoke("place_search", { query: q }).catch(() => [])
          : [];
        list.replaceChildren(...hits.map(p =>
          el("button", { class: "sugg-pill", title: `${p.lat.toFixed(3)}, ${p.lon.toFixed(3)}`,
            onclick: () => d.done(p) },
            [p.name, p.region, p.country].filter((v, i, a) => v && a.indexOf(v) === i).join(", "))));
      },
      onkeydown: e => { e.stopPropagation(); if (e.key === "Escape") d.done(null); },
    });
    d = dialogFrame(`Where were these ${hashes.length} taken?`, [
      el("p", { class: "asktext" },
        "The place is written into the photographs themselves, so it travels with them. " +
        "Each file is read back afterwards to check, and any that cannot be is left alone."),
      input, list,
      el("div", { class: "askrow" },
        el("button", { class: "btn ghost", onclick: () => d.done(null) }, "Cancel")),
    ]);
    d.attach(resolve);
    document.addEventListener("keydown", d.onKey, true);
    document.body.append(d.box);
    setTimeout(() => input.focus(), 40);
  });
  if (!chosen) return;
  const msg = await busy(`Writing the location into ${hashes.length}…`,
    () => invoke("set_photo_location", {
      path: S.source, hashes, lat: chosen.lat, lon: chosen.lon }), S.source);
  toast(msg, "ok");
  clearSel();
  await refreshSources(); await loadPhotos();
}

/* ---------------- wiring ---------------- */
$("#btn-add").onclick = addSource;
$("#peek-close").onclick = closePeek;
$("#peek-keep").onclick = keepPeek;
$("#btn-newfolder").onclick = newFolderPrompt;
$("#btn-map").onclick = toggleMap;
$("#map-in").onclick = () => { MAP.zoom = Math.min(14, MAP.zoom + 0.8); scheduleMap(); };
$("#map-out").onclick = () => { MAP.zoom = Math.max(0.6, MAP.zoom - 0.8); scheduleMap(); };
$("#map-fit").onclick = () => { fitMap(); scheduleMap(); };
$("#btn-tools").onclick = openSheet;
$("#sort-newest").onclick = () => setTimelineSort("newest");
$("#sort-oldest").onclick = () => setTimelineSort("oldest");
$("#month-jump").onchange = e => jumpToMonth(e.target.value);
$("#update-close").onclick = dismissUpdate;
$("#update-download").onclick = () => pendingUpdate && invoke("open_update", { url: pendingUpdate.url }).catch(e => toast(String(e), "error"));
/* The initial theme is applied by an inline script in index.html (pre-paint);
   this only flips it and remembers the choice. */
$("#btn-theme").onclick = () => {
  const next = document.documentElement.dataset.theme === "light" ? "dark" : "light";
  document.documentElement.dataset.theme = next;
  try { localStorage.setItem("of-theme", next); } catch (e) { /* private mode: fine to forget */ }
};
$("#btn-review").onclick = openReview;
$("#sheet-close").onclick = () => ($("#sheet").hidden = true);
$("#sheet").onclick = e => { if (e.target.id === "sheet") $("#sheet").hidden = true; };
$("#lb-close").onclick = closeLightbox;
$("#lb-share").onclick = () => {
  const p = S.lbList[S.lbIndex];
  if (p) shareHashes([p.hash]);
};
$("#lb-info").onclick = toggleInfo;
$("#btn-filter").onclick = () => {
  const f = $("#filters");
  f.hidden = !f.hidden;
  /* The panel lives at the top of main; opening it while scrolled down showed
     nothing. Bring the top into view instead. */
  if (!f.hidden) $("#main").scrollTo({ top: 0 });
  renderFilters();
};
{
  const stage = document.querySelector(".lb-stage");
  stage.addEventListener("wheel", e => {
    e.preventDefault();
    // Trackpad pinch arrives as ctrlKey+wheel; a plain wheel also zooms here since
    // there is nothing else to scroll.
    zoomAt(Math.exp(-e.deltaY * 0.0025), e.clientX, e.clientY);
  }, { passive: false });
  stage.addEventListener("dblclick", e => {
    S.zoom > 1 ? resetZoom() : zoomAt(3, e.clientX, e.clientY);
  });
  let drag = null;
  stage.addEventListener("dragstart", e => e.preventDefault());
  stage.addEventListener("pointerdown", e => {
    if (S.zoom <= 1) return;
    drag = { x: e.clientX, y: e.clientY, px: S.panX, py: S.panY };
    stage.setPointerCapture(e.pointerId);
    $("#lb-img").style.cursor = "grabbing";
  });
  stage.addEventListener("pointermove", e => {
    if (!drag) return;
    S.panX = drag.px + (e.clientX - drag.x);
    S.panY = drag.py + (e.clientY - drag.y);
    applyZoom();
  });
  const endDrag = () => { drag = null; $("#lb-img").style.cursor = S.zoom > 1 ? "grab" : ""; };
  stage.addEventListener("pointerup", endDrag);
  stage.addEventListener("pointercancel", endDrag);
}
$("#lb-rename").onclick = () => { const p = S.lbList[S.lbIndex]; if (p) renamePhoto(p); };
$("#lb-rot-l").onclick = () => rotateBy(-90);
$("#lb-rot-r").onclick = () => rotateBy(90);
$("#lb-save").onclick = saveEdit;
$("#lb-revert").onclick = discardEdit;
$("#lb-crop").onclick = () => (S.cropping ? endCrop(true) : startCrop());
$("#lb-flip").onclick = () => { const e = editState(); e.flipH = !e.flipH; applyEditPreview(); };
$("#crop-done").onclick = () => endCrop(true);
{
  const st = $("#straighten");
  st.oninput = () => {
    editState().straighten = parseFloat(st.value);
    $("#straighten-val").textContent = `${st.value}\u00B0`;
    applyEditPreview();
  };
}
$("#lb-adjust").onclick = () => {
  const bar = $("#adjustbar");
  bar.hidden = !bar.hidden;
  if (!bar.hidden && S.cropping) endCrop(true);
};
/* The same five the core defines, in the units the sliders use (core works in -1..1,
   the sliders in -100..100). Kept in step by the test in edit.rs that asserts the two
   lists match. */
const PRESETS = [
  ["Mono",  { brightness: 0,  contrast: 12,  saturation: -100 }],
  ["Warm",  { brightness: 4,  contrast: 6,   saturation: 18 }],
  ["Cool",  { brightness: 2,  contrast: 8,   saturation: -12 }],
  ["Punch", { brightness: 0,  contrast: 28,  saturation: 30 }],
  ["Faded", { brightness: 8,  contrast: -18, saturation: -22 }],
];

function applyPreset(vals) {
  const e = editState();
  for (const k of ["brightness", "contrast", "saturation"]) {
    e[k] = vals[k];
    $(`#adj-${k}`).value = String(vals[k]);
    $(`#adj-${k}-val`).textContent = String(vals[k]);
  }
  applyEditPreview();
}

$("#adj-presets").replaceChildren(...PRESETS.map(([name, vals]) =>
  el("button", { class: "fopt sm", title: `Start from ${name}`,
    onclick: () => applyPreset(vals) }, name)));

$("#adj-done").onclick = () => ($("#adjustbar").hidden = true);
$("#adj-reset").onclick = () => {
  const e = editState();
  e.brightness = e.contrast = e.saturation = 0;
  for (const k of ["brightness", "contrast", "saturation"]) {
    $(`#adj-${k}`).value = 0;
    $(`#adj-${k}-val`).textContent = "0";
  }
  applyEditPreview();
};
for (const k of ["brightness", "contrast", "saturation"]) {
  const el2 = $(`#adj-${k}`);
  el2.oninput = () => {
    editState()[k] = parseInt(el2.value, 10);
    $(`#adj-${k}-val`).textContent = el2.value;
    applyEditPreview();
  };
}
$("#crop-cancel").onclick = () => endCrop(false);
for (const b of document.querySelectorAll(".cropbar .chip")) {
  b.onclick = () => {
    for (const o of document.querySelectorAll(".cropbar .chip")) o.setAttribute("aria-pressed", "false");
    b.setAttribute("aria-pressed", "true");
    S.cropAR = b.dataset.ar === "free" ? null : parseFloat(b.dataset.ar);
    if (S.cropAR) resizeCrop("se", S.crop.x + S.crop.w, S.crop.y + S.crop.h);
  };
}
{
  // Drag the rectangle to move it, a handle to resize it.
  const cropper = $("#cropper");
  let drag = null;
  const frac = e => {
    const r = imgRect();
    return { fx: (e.clientX - r.left) / r.width, fy: (e.clientY - r.top) / r.height };
  };
  cropper.addEventListener("pointerdown", e => {
    if (!S.cropping) return;
    const h = e.target.dataset?.h;
    const { fx, fy } = frac(e);
    drag = h ? { handle: h } : { move: true, fx, fy, start: { ...S.crop } };
    cropper.setPointerCapture(e.pointerId);
    e.preventDefault();
  });
  cropper.addEventListener("pointermove", e => {
    if (!drag || !S.cropping) return;
    const { fx, fy } = frac(e);
    if (drag.handle) { resizeCrop(drag.handle, fx, fy); return; }
    const c = drag.start;
    S.crop = {
      ...c,
      x: Math.max(0, Math.min(c.x + (fx - drag.fx), 1 - c.w)),
      y: Math.max(0, Math.min(c.y + (fy - drag.fy), 1 - c.h)),
    };
    drawCrop();
  });
  const stop = () => { drag = null; };
  cropper.addEventListener("pointerup", stop);
  cropper.addEventListener("pointercancel", stop);
}
$("#lb-delete").onclick = async () => {
  const p = S.lbList[S.lbIndex]; if (!p) return;
  S.sel = new Set([p.hash]); closeLightbox(); await deleteSelected();
};
$("#sel-all").onclick = () => { S.view.forEach(p => S.sel.add(p.hash)); paintSel(); };
$("#sel-none").onclick = clearSel;
$("#sel-date").onclick = () => setDateTimePrompt([...S.sel]);
$("#sel-share").onclick = () => shareHashes([...S.sel]);
$("#sel-delete").onclick = deleteSelected;
$("#sel-restore").onclick = restoreSelected;
$("#sel-untag").onclick = untagSelected;
$("#dup-close").onclick = closeDuplicateReview;
$("#dup-keep").onclick = reviewDuplicateKeep;
$("#dup-prev").onclick = () => stepDuplicate(-1);
$("#dup-next").onclick = () => stepDuplicate(1);
$("#dup-like").onclick = toggleDuplicateLike;
$("#dup-share").onclick = () => { const item = dupFocusedItem(); if (item) shareHashes([item.hash]); };
$("#dup-move").onclick = moveDuplicateCurrent;
$("#dup-accept-all").onclick = acceptAllSuggestions;
$("#dup-apply").onclick = applyDuplicateReview;
addEventListener("click", e => { if (!e.target.closest("#ctx")) hideCtx(); });
$("#lb-prev").onclick = () => step(-1);
$("#lb-next").onclick = () => step(1);
$("#search").oninput = () => { S.resetScroll = true; applyFilter(); renderSuggest(); syncClear(); };

/* The native clear affordance on <input type=search> does not survive the restyle, and
   a query with no visible way out is a dead end for anyone who did not think to select
   all and delete. */
function syncClear() {
  const b = $("#search-clear");
  if (b) b.hidden = !$("#search").value;
}
$("#search-clear").onclick = () => {
  const i = $("#search");
  i.value = "";
  applyFilter();
  renderFilters();
  syncClear();
  i.focus();
};
$("#search").onfocus = renderSuggest;
$("#search").onblur = () => setTimeout(() => { $("#suggest").hidden = true; }, 120);
document.addEventListener("keydown", e => {
  if (e.key === "Escape" && !$("#suggest").hidden) { $("#suggest").hidden = true; }
}, true);
addEventListener("keydown", e => {
  if (!$("#dup-review").hidden) {
    if (e.key === "Escape") closeDuplicateReview();
    if (e.key === "ArrowRight") stepDuplicate(1);
    if (e.key === "ArrowLeft") stepDuplicate(-1);
    if (e.key.toLowerCase() === "k") reviewDuplicateKeep();
    return;
  }
  if (!$("#lightbox").hidden) {
    if (S.cropping) {
      if (e.key === "Escape") { endCrop(false); return; }
      if (e.key === "Enter") { endCrop(true); return; }
      return;
    }
    if (e.key === "Escape") S.zoom > 1 ? resetZoom() : closeLightbox();
    // Arrows pan while zoomed in, and step between photos otherwise.
    if (e.key === "ArrowRight") { if (S.zoom > 1) { S.panX -= 60; applyZoom(); } else step(1); }
    if (e.key === "ArrowLeft") { if (S.zoom > 1) { S.panX += 60; applyZoom(); } else step(-1); }
    if (e.key === "ArrowUp" && S.zoom > 1) { S.panY += 60; applyZoom(); }
    if (e.key === "ArrowDown" && S.zoom > 1) { S.panY -= 60; applyZoom(); }
    if (e.key === "+" || e.key === "=") zoomAt(1.4, innerWidth / 2, innerHeight / 2);
    if (e.key === "-" || e.key === "_") zoomAt(1 / 1.4, innerWidth / 2, innerHeight / 2);
    if (e.key === "0") resetZoom();
    // F narrows to this photograph's folder, and back out again.
    if (e.key.toLowerCase() === "f" && !S.cropping) {
      setLbScope(S.lbScope === "folder" ? "view" : "folder");
    }
    return;
  }
  if (e.key === "Escape") { hideCtx(); if (!$("#sheet").hidden) $("#sheet").hidden = true; else if (S.sel.size) clearSel(); }
  if (e.key === "/" && document.activeElement !== $("#search")) { e.preventDefault(); $("#search").focus(); }
  // ⌘F goes to the search field rather than to a finder of its own: the field
  // already matches filename and path, and two places to type a name is one too many.
  // Before the typing guard, because ⌘F is expected to work from inside a field too.
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "f") {
    e.preventDefault();
    $("#search").focus();
    $("#search").select();
    return;
  }
  const typing = /^(INPUT|TEXTAREA)$/.test(document.activeElement?.tagName || "");
  if (typing) return;
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "a") { e.preventDefault(); S.view.forEach(p => S.sel.add(p.hash)); paintSel(); }
  // Arrow keys walk the grid; holding shift extends the selection as it goes.
  const arrows = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: 2, ArrowUp: -2 };
  if (e.key in arrows && $("#lightbox").hidden) {
    e.preventDefault();
    moveSel(arrows[e.key], e.shiftKey);
  }
  if (e.key === " " && S.lastIndex >= 0 && $("#lightbox").hidden) {
    e.preventDefault();
    toggleSel(S.view[S.lastIndex]);
  }
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "z") {
    e.preventDefault();
    if (!refusePeek("undo changes")) doUndo();
  }
  if ((e.key === "Backspace" || e.key === "Delete") && S.sel.size) {
    e.preventDefault();
    if (!refusePeek("delete photographs"))
      S.folder === TRASH ? restoreSelected() : deleteSelected();
  }
});
let rt; addEventListener("resize", () => { clearTimeout(rt); rt = setTimeout(renderGrid, 120); });
$("#main").addEventListener("scroll", () => paintViewport(), { passive: true });

/* A library changed underneath us — someone dropped photographs into a folder in
   Finder. Reload only the library actually on screen: rescanning one nobody is looking
   at should not move the grid they are looking at. */
listen("library-changed", async ({ payload }) => {
  const [root, n] = payload;
  if (root !== S.source) return;
  await refreshSources();
  await loadPhotos();
  toast(`${n} change${n === 1 ? "" : "s"} on disk — updated`, "ok");
});

(async function init() {
  renderWelcome();
  loadFolderState();
  checkUpdates(false); // deliberately not awaited: a network check never delays the library
  await refreshSources();
  const pending = await invoke("take_open_paths").catch(() => []);
  if (pending.length) {
    for (const path of [...new Set(pending)]) await handleOpenPath(path);
  } else if (!S.source && S.sources.length) {
    const first = S.sources.find(s => !s.missing);
    if (first) await selectSource(first.path);
  }
})();


/* ---------------- glass dialogs ----------------
   Native prompt()/confirm() render as OS sheets that ignore the design language and
   block the webview. These are the same questions in our own glass; both resolve
   null/false on Cancel, backdrop click or Escape. */
function dialogFrame(title, bodyKids) {
  let box;
  let resolveFn;
  const done = v => {
    document.removeEventListener("keydown", onKey, true);
    box.remove();
    resolveFn(v);
  };
  const onKey = e => {
    if (e.key !== "Escape") return;
    e.stopPropagation();
    e.preventDefault();
    done(null);
  };
  box = el("div", {
    class: "sheet",
    onclick: e => { if (e.target === box) done(null); }
  },
    el("div", { class: "sheet-panel small", role: "dialog", "aria-modal": "true", "aria-label": title },
      el("div", { class: "sheet-head" }, el("h2", {}, title)),
      el("div", { class: "sheet-body" }, bodyKids)));
  return { box, done, onKey, attach: r => { resolveFn = r; } };
}

function confirmDialog(title, text, okLabel, danger = false) {
  return new Promise(resolve => {
    const d = dialogFrame(title, [
      el("p", { class: "asktext" }, text),
      el("div", { class: "askrow" },
        el("button", { class: "btn ghost", onclick: () => d.done(false) }, "Cancel"),
        el("button", {
          class: "btn" + (danger ? " solid-danger" : ""),
          onclick: () => d.done(true)
        }, okLabel)),
    ]);
    d.attach(resolve);
    document.addEventListener("keydown", d.onKey, true);
    document.body.append(d.box);
  }).then(v => v === null ? false : v);   // Esc/backdrop (null) also means "not confirmed"
}

/** Ask for a folder: the ones that exist, one click each, or a name of your own.
    Offering the existing folders first is what stops a second spelling of a folder
    that is already there. */
function pickFolderPrompt(title, placeholder) {
  return new Promise(resolve => {
    let d;
    const folders = (S.sources.find(x => x.path === S.source)?.folders || [])
      .filter(f => f.path && f.path !== TRASH && !f.path.startsWith(TRASH + "/"))
      .slice(0, 10);
    const input = el("input", {
      class: "nameinput", type: "text", value: "", placeholder: placeholder || "",
      "aria-label": title, style: "width:100%",
      onkeydown: e => {
        e.stopPropagation();
        if (e.key === "Escape") d.done(null);
        if (e.key === "Enter") d.done(input.value.trim() || null);
      }
    });
    d = dialogFrame(title, [
      folders.length
        ? el("div", { class: "movepicks" }, folders.map(f =>
            el("button", { class: "sugg-pill", title: f.path, onclick: () => d.done(f.path) }, f.name)))
        : null,
      input,
      el("div", { class: "askrow" },
        el("button", { class: "btn ghost", onclick: () => d.done(null) }, "Cancel"),
        el("button", { class: "btn", onclick: () => d.done(input.value.trim() || null) }, "Move")),
    ].filter(Boolean));
    d.attach(resolve);
    document.addEventListener("keydown", d.onKey, true);
    document.body.append(d.box);
    setTimeout(() => input.focus(), 40);
  });
}

function promptDialog(title, value) {
  return new Promise(resolve => {
    let d;
    const input = el("input", {
      class: "nameinput", type: "text", value: value || "", "aria-label": title,
      style: "width:100%",
      onkeydown: e => {
        e.stopPropagation();
        if (e.key === "Escape") d.done(null);
        if (e.key === "Enter") d.done(input.value.trim());
      }
    });
    d = dialogFrame(title, [
      input,
      el("div", { class: "askrow" },
        el("button", { class: "btn ghost", onclick: () => d.done(null) }, "Cancel"),
        el("button", { class: "btn", onclick: () => d.done(input.value.trim()) }, "Save")),
    ]);
    d.attach(resolve);
    document.addEventListener("keydown", d.onKey, true);
    document.body.append(d.box);
    setTimeout(() => { input.focus(); input.select(); }, 40);
  });
}

/* ---------------- the command grammar ----------------
   Sentences that *act*, not only ask (ADR-0012). Deliberately a grammar rather than a
   model: what a sentence selects is already resolved exactly by parseQuery, and what it
   does is about eight verbs — small enough to enumerate, and enumerating beats guessing
   when the output moves files.

       utterance := clause (("and" | "then") clause)*
       clause    := verb selector? target?

   Nothing here touches the disk. Every clause compiles to a preview, and the preview is
   the only route to a Plan. */

const VERBS = {
  move:   ["move", "put", "file", "shift", "relocate"],
  rate:   ["rate", "star"],
  label:  ["label", "colour", "color", "tag"],
  delete: ["delete", "remove", "bin", "trash", "chuck"],
  show:   ["show", "find", "search", "list", "open"],
  save:   ["save"],
};

/** Words that mean "the photographs we were just talking about". */
const REFERENTS = ["them", "these", "those", "it", "results", "there", "that"];

/** Prepositions that introduce a destination. */
const INTO = ["to", "into", "in", "under"];

/* Words that are grammar rather than selection. "move all my august photos" selects
   exactly what "move august photos" selects, and leaving them in sends "my" to the
   scene search, where it means nothing and costs a confident wrong answer.

   Stripped only in the command path: a *question* keeps its phrasing intact, because
   "the beach" is a better phrase for the text encoder than "beach". */
const FILLER = ["all", "my", "mine", "the", "a", "an", "any", "some", "every",
                "please", "just", "of", "from", "with"];

function stripFiller(text) {
  const kept = text.split(/\s+/).filter(w => w && !FILLER.includes(w.toLowerCase()));
  // If filler was all there was, keep the original rather than produce an empty
  // selector that would be refused for the wrong reason.
  return kept.length ? kept.join(" ") : text.trim();
}

function verbOf(word) {
  const w = word.toLowerCase().replace(/[.,!?]+$/, "");
  for (const [verb, words] of Object.entries(VERBS)) if (words.includes(w)) return verb;
  return null;
}

/** The closest verb we know, for an error that helps rather than just refuses.
    Edit distance rather than a shared prefix: the commonest typo is a transposition,
    and "mvoe" shares exactly one letter of prefix with "move". */
function editDistance(a, b) {
  const prev = Array.from({ length: b.length + 1 }, (_, i) => i);
  for (let i = 1; i <= a.length; i++) {
    let diag = prev[0];
    prev[0] = i;
    for (let j = 1; j <= b.length; j++) {
      const tmp = prev[j];
      prev[j] = Math.min(
        prev[j] + 1,                                   // deletion
        prev[j - 1] + 1,                               // insertion
        diag + (a[i - 1] === b[j - 1] ? 0 : 1),        // substitution
      );
      diag = tmp;
    }
  }
  return prev[b.length];
}

function nearestVerb(word) {
  const w = word.toLowerCase().replace(/[^a-z]/g, "");
  if (w.length < 3) return null;
  let best = null, bestD = Infinity;
  for (const words of Object.values(VERBS)) {
    for (const cand of words) {
      const d = editDistance(w, cand);
      if (d < bestD) { bestD = d; best = cand; }
    }
  }
  // Two edits covers a transposition, which needs four letters to happen at all.
  // Below that, two edits is most of the word and would "correct" real nouns.
  const limit = w.length >= 4 ? 2 : 1;
  return bestD <= limit ? best : null;
}

/** Split on "and"/"then", but not when "and" sits inside a destination name. */
function splitClauses(text) {
  return text
    .split(/\s+(?:and then|then|and)\s+/i)
    .map(t => t.trim())
    .filter(Boolean);
}

/* Parse one clause into { verb, rest, target, referent }.
   The verb must lead: "move august to X". A clause with no leading verb is a question,
   which is what the panel did before this existed. */
function parseClause(text) {
  const words = text.split(/\s+/).filter(Boolean);
  if (!words.length) return null;
  const verb = verbOf(words[0]);
  if (!verb) return null;

  let rest = words.slice(1);
  let target = null;
  // Scan from the right for the last preposition, so a destination containing one
  // ("photos in Trip/Greece in 2026") still binds to the outermost.
  for (let i = rest.length - 2; i >= 0; i--) {
    if (INTO.includes(rest[i].toLowerCase())) {
      target = rest.slice(i + 1).join(" ");
      rest = rest.slice(0, i);
      break;
    }
  }
  // Verbs that carry a value hold it inside the selector — "rate them 5 stars". Lift
  // it out here, so what remains is purely a selector and can be recognised as the
  // referent it is. Doing this in the executor instead left "them 5 stars" to be
  // searched for as a phrase.
  let selector = stripFiller(rest.join(" "));
  let value = null;
  if (verb === "rate") {
    const m = selector.match(/(\d)\s*\+?\s*(?:stars?)?\b/);
    if (m) {
      value = Math.min(5, +m[1]);
      selector = (selector.slice(0, m.index) + " " + selector.slice(m.index + m[0].length)).trim();
    }
  } else if (verb === "label") {
    const found = LABEL_NAMES.find(l => new RegExp(`\\b${l}\\b`, "i").test(selector));
    if (found) {
      value = found;
      selector = selector.replace(new RegExp(`\\b${found}\\b`, "i"), " ").replace(/\s+/g, " ").trim();
    }
  }
  const selWords = selector ? selector.split(/\s+/) : [];
  // An *explicit* referent borrows the last answer. An *empty* selector does not:
  // "move to Trip" must not quietly inherit whatever was on screen, or a stray
  // sentence plans the whole library (criterion 5).
  const referent = selWords.length === 1 && REFERENTS.includes(selWords[0].toLowerCase())
    ? selWords[0].toLowerCase()
    : null;

  return { verb, rest: referent ? "" : selector, target, referent, value,
           empty: !referent && !selWords.length, source: text };
}

/** Does this utterance ask for something to happen, or is it a question? */
function isCommand(text) {
  return splitClauses(text).some(c => parseClause(c) !== null);
}

/* Resolve a clause's selector into actual photographs.
   Reuses answerQuestion so the command layer and the question layer can never disagree
   about what "august 2026" means — there is one selector language (criterion 10). */
async function resolveSelector(clause, ctx) {
  if (clause.referent) {
    if (!ctx.photos || !ctx.photos.length) {
      return { error: "I do not know which photographs you mean — ask for some first." };
    }
    return { photos: ctx.photos, described: ctx.described || "those photographs" };
  }
  const answer = await answerQuestion(clause.rest);
  if (answer.kind !== "results") return { error: null, answer };
  if (!answer.photos.length) {
    return { error: `Nothing matches \u201C${clause.rest}\u201D, so there is nothing to ${clause.verb}.` };
  }
  return { photos: answer.photos, described: clause.rest, parsed: answer.parsed };
}

/* Turn one clause into something previewable. Never acts. */
async function planClause(clause, ctx) {
  const sel = await resolveSelector(clause, ctx);
  if (sel.answer) return { kind: "passthrough", answer: sel.answer };
  if (sel.error) return { kind: "note", text: sel.error };

  const photos = sel.photos;
  const hashes = photos.map(p => p.hash);

  switch (clause.verb) {
    case "show":
      return { kind: "results", ...(sel.parsed ? { parsed: sel.parsed } : {}), photos,
               phrase: sel.described, semCount: 0, query: clause.rest };

    case "move": {
      const dest = clause.target || ctx.lastFolder;
      if (!dest) {
        // A missing slot is a question, not a guess (ADR-0012).
        return { kind: "ask", slot: "destination", clause, photos,
                 text: `Where should ${photos.length} photo${photos.length === 1 ? "" : "s"} go?` };
      }
      let view;
      try {
        view = await invoke("plan_move", { path: S.source, hashes, dest });
      } catch (e) {
        return { kind: "note", text: String(e) };
      }
      return { kind: "move", dest, view, photos, described: sel.described };
    }

    case "rate": {
      const rating = clause.value;
      if (rating === null || rating === undefined) {
        return { kind: "ask", slot: "rating", clause, photos,
                 text: "How many stars? (0 clears it)" };
      }
      return { kind: "rate", rating, photos, described: sel.described };
    }

    case "label": {
      const colour = clause.value;
      if (!colour) {
        return { kind: "ask", slot: "label", clause, photos,
                 text: `Which colour? ${LABEL_NAMES.join(", ")}` };
      }
      return { kind: "label", colour, photos, described: sel.described };
    }

    case "delete":
      // Always a preview, whatever the phrasing (criterion 9).
      return { kind: "delete", photos, described: sel.described };

    case "save":
      return { kind: "savequery", query: clause.rest || ctx.lastQuery || "" };

    default:
      return { kind: "note", text: `I do not know how to ${clause.verb} yet.` };
  }
}

/* ---------------- ask panel ----------------
   The natural-language surface. Everything it does composes commands the app
   already shipped: the question is parsed exactly like the omnibar (dates, people,
   albums, field:value), leftover words go to the CLIP text encoder, and the answer
   is a card of real photos. The thread is per-session memory — nothing is stored. */

const ASK_HINTS = ["sunset over the mountains", "photos of a dog",
                   "move my august photos to Trip", "rate the night sky 5 stars",
                   "food on a plate"];

function toggleAsk(open) {
  const panel = $("#askpanel");
  const next = open === undefined ? panel.hidden : open;
  panel.hidden = !next;
  $("#btn-ask").setAttribute("aria-pressed", String(next));
  if (next) {
    renderAskEmpty();
    setTimeout(() => $("#ask-input").focus(), 60);
  }
}

function renderAskEmpty() {
  const t = $("#ask-thread");
  if (t.children.length) return;
  t.replaceChildren(el("div", { class: "ask-empty" },
    el("div", { class: "aico" }, el("span", {}, "✦")),
    el("h3", {}, "Ask about your photos"),
    el("p", {}, "Dates, people and scenes work together — the way you would describe a photo to a friend."),
    el("div", { class: "ask-hints" }, ASK_HINTS.map(h =>
      el("button", { class: "ask-hint", onclick: () => { $("#ask-input").value = h; askSubmit(); } }, h)))));
}

async function askSubmit() {
  const input = $("#ask-input");
  const q = input.value.trim();
  if (!q) return;
  input.value = "";
  const thread = $("#ask-thread");
  thread.querySelector(".ask-empty")?.remove();
  thread.append(el("div", { class: "ask-q" }, q));
  const card = el("div", { class: "ask-a pending", role: "status" }, "Reading your library");
  thread.append(card);
  thread.scrollTop = thread.scrollHeight;
  try {
    const answer = await runUtterance(q);
    fillAskCard(card, q, answer);
  } catch (e) {
    card.classList.remove("pending");
    card.replaceChildren(el("p", { class: "asentence" }, `That went wrong: ${e}`));
  }
  thread.scrollTop = thread.scrollHeight;
}

/* Per-thread memory: what the last answer was about, so "them" and "there" mean
   something. Nothing is stored on disk — it lives as long as the panel is open. */
const CTX = { photos: null, described: null, lastFolder: null, lastQuery: null, pending: null };

/* The entry point: a sentence is either a command or a question.
   A command that names no photographs is refused rather than interpreted generously —
   "move everything" is never what someone meant to type (criterion 5). */
async function runUtterance(q) {
  // A pending question from a previous turn takes the answer literally.
  if (CTX.pending) {
    const { clause, slot } = CTX.pending;
    CTX.pending = null;
    const filled = { ...clause };
    if (slot === "destination") filled.target = q.trim();
    else filled.rest = `${filled.rest} ${q.trim()}`.trim();
    return await planClause(filled, CTX);
  }

  const clauses = splitClauses(q);
  if (!clauses.some(c => parseClause(c))) {
    // Not a command. Before answering it as a question, catch the near miss: a first
    // word close to a verb *and* a destination is a mistyped instruction, not a search.
    // The destination is what makes this safe — "movie night photos" has no "to", so it
    // stays a search rather than being read as a broken "move".
    const words = q.trim().split(/\s+/);
    const near = words.length > 1 ? nearestVerb(words[0]) : null;
    const shaped = words.slice(1, -1).some(w => INTO.includes(w.toLowerCase()));
    if (near && shaped) {
      return { kind: "note",
        text: `I do not know \u201C${words[0]}\u201D. Did you mean \u201C${near} ` +
              `${words.slice(1).join(" ")}\u201D?` };
    }
    return await answerQuestion(q);
  }

  const results = [];
  for (const text of clauses) {
    const clause = parseClause(text);
    if (!clause) {
      // Inside a command, a clause with no verb is a selector for the previous one
      // rather than a fresh question.
      const first = text.split(/\s+/)[0];
      const near = nearestVerb(first);
      results.push({ kind: "note",
        text: near
          ? `I do not know \u201C${first}\u201D. Did you mean \u201C${near}\u201D?`
          : `I understood \u201C${text}\u201D as a search, not something to do.` });
      continue;
    }
    if (clause.empty && clause.verb !== "save") {
      const hint = CTX.photos && CTX.photos.length
        ? ` Say \u201C${clause.verb} them${clause.target ? ` to ${clause.target}` : ""}\u201D if you mean the ${CTX.photos.length} just found.`
        : "";
      results.push({ kind: "note",
        text: `\u201C${clause.verb}\u201D needs to know which photographs \u2014 a date, ` +
              `a person, or what is in them.${hint}` });
      continue;
    }
    const out = await planClause(clause, CTX);
    // Advance the context *here*, not when the card renders: within one utterance the
    // second clause runs before anything is drawn, and "rate them" has to see what
    // "show ..." just found.
    if (out.photos && out.photos.length) {
      CTX.photos = out.photos;
      CTX.described = out.described || clause.rest || CTX.described;
    }
    if (out.kind === "move" && out.dest) CTX.lastFolder = out.dest;
    results.push(out);
  }
  return results.length === 1 ? results[0] : { kind: "multi", steps: results };
}

/* A question becomes an answer in three steps: parse it the way the omnibar would,
   fetch scene scores for whatever words are left over, then keep the photos that
   match either half. Nothing below the semantic threshold is offered (ADR-0008). */
async function answerQuestion(q) {
  if (!S.source) {
    return { kind: "note", text: "Add a folder first — there is nothing to look through yet." };
  }
  if (!S.photos.length) await loadPhotos();
  const names = S.people.filter(p => p.name).map(p => p.name);
  const parsed = parseQuery(q, names, S.albums.map(a => a[0]));
  const { want, text } = parsed;
  const phrase = text.join(" ");

  if (phrase && !S.semanticReady) await refreshSemanticStatus();
  const ready = S.semanticReady;
  // Whether the leftover words already match something by name. Checked *before*
  // reporting that scenes are not indexed: "swiss" matches the Swiss Day1 folder
  // literally, and offering to embed the library instead is a wrong answer to a
  // question that had a right one.
  const literalHay = p => {
    const hay = [p.name, p.folder, p.people.join(" ")].join(" ").toLowerCase();
    return text.every(t => hay.includes(t));
  };
  const literalHits = phrase
    ? S.photos.filter(p => p.folder !== TRASH &&
        (!parsed.hasFilter || matchesStructured(p, want)) && literalHay(p)).length
    : 0;
  if (phrase && !literalHits && ready && !ready.available) return { kind: "models" };
  if (phrase && !literalHits && ready && ready.embedded === 0) return { kind: "embed" };

  let semScores = new Map();
  if (phrase) {
    try {
      const hits = await invoke("semantic_search", { path: S.source, query: phrase });
      semScores = new Map(hits.map(h => [h.hash, h.score]));
    } catch (e) {
      console.warn("ask: semantic search failed:", e);
    }
  }

  const inHay = literalHay;
  const photos = S.photos.filter(p =>
    p.folder !== TRASH &&
    (!parsed.hasFilter || matchesStructured(p, want)) &&
    (!text.length || inHay(p) || semScores.has(p.hash)));
  // Literal answers sort by date; answers that only matched on what they show sort
  // by how well they matched.
  photos.sort((a, b) => {
    const la = parsed.hasFilter || inHay(a), lb = parsed.hasFilter || inHay(b);
    if (la !== lb) return la ? -1 : 1;
    if (!la) return (semScores.get(b.hash) ?? 0) - (semScores.get(a.hash) ?? 0);
    return (b.taken_at || 0) - (a.taken_at || 0);
  });
  return { kind: "results", parsed, phrase, semCount: semScores.size, photos };
}

/** The intent chips for an answer — queryChips without the omnibar's live state. */
function askChips({ want }, phrase, semCount) {
  const out = [];
  const date = [];
  if (want.day !== null) date.push(String(want.day));
  if (want.month !== null) date.push(MONTHS[want.month - 1].replace(/^./, c => c.toUpperCase()));
  if (want.year !== null) date.push(String(want.year));
  if (date.length) out.push(["date", date.join(" ")]);
  if (want.person) out.push(["person", want.person]);
  if (want.album) out.push(["album", want.album]);
  if (want.kind) out.push(["type", want.kind === "video" ? "Videos" : "Photos"]);
  if (want.ext) out.push(["type", want.ext]);
  if (want.label) out.push(["label", want.label]);
  if (want.minRating) out.push(["rating", "★".repeat(want.minRating) + "+"]);
  if (want.fav) out.push(["rating", "★★★★★"]);
  if (phrase) out.push(["sem", `✨ ${phrase}${semCount ? ` · ${semCount}` : ""}`]);
  return out.map(([kind, txt]) => el("span", { class: `qc qc-${kind}` }, txt));
}

function fillAskCard(card, q, answer) {
  card.classList.remove("pending");
  card.replaceChildren();

  if (answer.kind === "multi") {
    // Each clause of a compound instruction gets its own card, in order.
    for (const step of answer.steps) {
      const sub = el("div", { class: "ask-step" });
      card.append(sub);
      fillAskCard(sub, q, step);
    }
    return;
  }
  if (answer.kind === "passthrough") return fillAskCard(card, q, answer.answer);

  if (answer.kind === "ask") {
    // Hold the clause; the next thing typed answers this question (criterion 4).
    CTX.pending = { clause: answer.clause, slot: answer.slot };
    CTX.photos = answer.photos;
    card.append(el("p", { class: "asentence" }, answer.text));
    if (answer.slot === "destination") {
      const folders = (S.sources.find(x => x.path === S.source)?.folders || [])
        .filter(f => f.path && f.path !== TRASH).slice(0, 6);
      if (folders.length) {
        card.append(el("div", { class: "ask-acts" }, folders.map(f =>
          el("button", { class: "ask-act", onclick: () => { $("#ask-input").value = f.path; askSubmit(); } },
            f.name))));
      }
    }
    return;
  }

  if (answer.kind === "move") return fillMoveCard(card, answer);
  if (answer.kind === "rate" || answer.kind === "label" || answer.kind === "delete") {
    return fillActionCard(card, answer);
  }
  if (answer.kind === "savequery") {
    card.append(el("p", { class: "asentence" }, "Name it in the sidebar:"));
    showInLibrary(answer.query);
    saveSearchPrompt();
    return;
  }

  if (answer.kind === "note") {
    card.append(el("p", { class: "asentence" }, answer.text));
    return;
  }
  if (answer.kind === "models") {
    card.append(
      el("p", { class: "asentence" },
        "I can look for what a photo shows, but the search models are not installed yet."),
      el("div", { class: "ask-acts" },
        el("button", { class: "ask-act primary", onclick: async () => {
          const msg = await busy("Downloading search models…", () => invoke("models_fetch"));
          toast(msg, "ok");
          await refreshSemanticStatus();
          card.querySelector(".asentence").textContent = "Models installed — ask me again.";
          card.querySelector(".ask-acts")?.remove();
        } }, "Download models")));
    return;
  }
  if (answer.kind === "embed") {
    card.append(
      el("p", { class: "asentence" },
        "I know these files, but not yet what they show. Learning that takes one pass per photo, once."),
      el("div", { class: "ask-acts" },
        el("button", { class: "ask-act primary", onclick: async () => {
          await understand();
          card.querySelector(".asentence").textContent = "Done — ask me again.";
          card.querySelector(".ask-acts")?.remove();
        } }, "✨ Understand these photos")));
    return;
  }

  const { parsed, phrase, semCount, photos } = answer;
  // Remember this answer, so a following "rate them 5 stars" knows what "them" is.
  CTX.photos = photos;
  CTX.described = phrase || q;
  CTX.lastQuery = answer.query || q;
  card.append(el("div", { class: "qchips" }, askChips(parsed, phrase, semCount)));

  // People named in the question get their faces into the answer — recognising a
  // face beats reading a name.
  const ql = q.toLowerCase();
  const mentioned = S.people.filter(p => p.name && ql.includes(p.name.toLowerCase()));
  if (mentioned.length) {
    card.append(el("div", { class: "askpeople" }, mentioned.slice(0, 5).map(p =>
      el("button", { class: "askperson", onclick: () => selectPerson(p.name) },
        p.cover ? el("img", { src: photoUrl(p.cover), alt: "" }) : null,
        el("span", {}, p.name)))));
  }

  const n = photos.length;
  if (!n) {
    card.append(el("p", { class: "asentence" },
      "Nothing matches that — I would rather say so than show you something close enough."));
    return;
  }
  card.append(el("p", { class: "asentence" },
    el("b", {}, String(n)), ` photo${n === 1 ? "" : "s"}`,
    phrase ? ` for “${phrase}”` : ""));

  const THUMBS = 8;
  const shown = n > THUMBS ? photos.slice(0, THUMBS - 1) : photos;
  card.append(el("div", { class: "askthumbs" },
    shown.map((p, i) => el("img", {
      src: photoUrl(p.path) + "?t=" + p.hash, alt: p.name, loading: "lazy", decoding: "async",
      onclick: () => openViewer(photos, i)
    })),
    n > shown.length
      ? el("div", { class: "more", onclick: () => showInLibrary(q) }, `+${n - shown.length}`)
      : null));

  card.append(el("div", { class: "ask-acts" },
    el("button", { class: "ask-act primary", onclick: () => showInLibrary(q) }, "Show in library"),
    el("button", { class: "ask-act", onclick: () => {
      S.sel = new Set(photos.map(p => p.hash));
      paintSel();
      toast(`${S.sel.size} selected`, "ok");
    } }, "Select results"),
    el("button", { class: "ask-act", onclick: () => { showInLibrary(q); saveSearchPrompt(); } },
      "Save this search\u2026")));
}

/* A move, previewed. Nothing has happened yet — this card is the only route to a
   Plan, which is what makes acting on a sentence safe without a model's judgement. */
function fillMoveCard(card, a) {
  const n = a.view.moves.length;
  CTX.photos = a.photos;
  CTX.described = a.described;
  CTX.lastFolder = a.dest;

  card.append(el("p", { class: "asentence" },
    n ? [el("b", {}, String(n)), ` photo${n === 1 ? "" : "s"} will move into `, el("b", {}, a.dest)]
      : [`Nothing to move into `, el("b", {}, a.dest), " — they are already there."]));

  if (a.view.skipped.length) {
    card.append(el("p", { class: "asub" },
      `${a.view.skipped.length} left alone: ${a.view.skipped.slice(0, 2).map(x => x[1]).join("; ")}` +
      (a.view.skipped.length > 2 ? "\u2026" : "")));
  }
  if (!n) return;

  card.append(el("div", { class: "askthumbs" },
    a.photos.slice(0, 7).map((p, i) => el("img", {
      src: photoUrl(p.path) + "?t=" + p.hash, alt: p.name, loading: "lazy",
      onclick: () => openViewer(a.photos, i)
    }))));

  card.append(el("div", { class: "ask-acts" },
    el("button", { class: "ask-act primary", onclick: async e => {
      e.target.disabled = true;
      // An exception in an async handler is an unhandled rejection and vanishes; for
      // an action that moves files, silence is the worst possible report.
      try {
        const msg = await busy("Moving\u2026", () => invoke("apply_move", {
          path: S.source, hashes: a.photos.map(p => p.hash), dest: a.dest }));
        toast(msg + " \u2014 \u2318Z to undo", "ok");
        await refreshSources(); await loadPhotos();
        card.replaceChildren(el("p", { class: "asentence" }, msg));
      } catch (err) {
        e.target.disabled = false;
        card.append(el("p", { class: "asub" }, `That failed: ${err}`));
        console.error("apply_move failed:", err);
      }
    } }, `Move ${n}`),
    el("button", { class: "ask-act", onclick: () => { S.folder = a.dest; applyFilter(); } },
      "Show the destination")));
}

/* Rating, labelling and deleting. Deleting is a preview like everything else. */
function fillActionCard(card, a) {
  CTX.photos = a.photos;
  CTX.described = a.described;
  const n = a.photos.length;
  const what = a.kind === "rate" ? (a.rating ? `${"\u2605".repeat(a.rating)}` : "no stars")
             : a.kind === "label" ? a.colour
             : "the Trash";
  const verb = a.kind === "delete" ? "move to" : a.kind === "rate" ? "rate" : "label";

  card.append(el("p", { class: "asentence" },
    el("b", {}, String(n)), ` photo${n === 1 ? "" : "s"} will be ${verb === "move to" ? "moved to" : verb + "d"} `,
    el("b", {}, what), a.kind === "delete" ? " (recoverable)" : ""));

  card.append(el("div", { class: "askthumbs" },
    a.photos.slice(0, 7).map((p, i) => el("img", {
      src: photoUrl(p.path) + "?t=" + p.hash, alt: p.name, loading: "lazy",
      onclick: () => openViewer(a.photos, i)
    }))));

  card.append(el("div", { class: "ask-acts" },
    el("button", { class: "ask-act primary" + (a.kind === "delete" ? " danger" : ""),
      onclick: async e => {
        e.target.disabled = true;
        try {
        const hashes = a.photos.map(p => p.hash);
        let msg;
        if (a.kind === "rate") {
          await invoke("set_rating", { path: S.source, hashes, rating: a.rating });
          msg = `Rated ${n}`;
        } else if (a.kind === "label") {
          await invoke("set_label", { path: S.source, hashes, label: a.colour });
          msg = `Labelled ${n} ${a.colour}`;
        } else {
          msg = await busy("Moving to Trash\u2026",
            () => invoke("delete_photos", { path: S.source, hashes }));
        }
        toast(msg + " \u2014 \u2318Z to undo", "ok");
        await loadPhotos();
        // Deletions change the folder and Trash counts as much as any other move.
        await refreshSources();
        card.replaceChildren(el("p", { class: "asentence" }, msg));
        } catch (err) {
          e.target.disabled = false;
          card.append(el("p", { class: "asub" }, `That failed: ${err}`));
          console.error("action failed:", err);
        }
      } }, a.kind === "delete" ? `Move ${n} to Trash` : `Yes, ${verb} ${n}`),
    el("button", { class: "ask-act", onclick: () => card.replaceChildren(
      el("p", { class: "asentence" }, "Left alone.")) }, "Cancel")));
}

/* The omnibar takes the question over: same parse, same semantic union, so the grid
   always agrees with the card. */
function showInLibrary(q) {
  $("#search").value = q;
  applyFilter();
  renderFilters();
  syncClear();
}

$("#btn-ask").onclick = () => toggleAsk();
$("#ask-close").onclick = () => toggleAsk(false);
$("#ask-form").addEventListener("submit", e => { e.preventDefault(); askSubmit(); });
$("#ask-input").addEventListener("keydown", e => {
  if (e.key === "Escape") { e.stopPropagation(); toggleAsk(false); }
});
addEventListener("keydown", e => {
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
    e.preventDefault();
    toggleAsk();
  }
});
