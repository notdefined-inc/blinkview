/* openfoto desktop.
   The engine lives in Rust; this file is presentation and interaction only. */
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

/* Photos are served by our own `photo://` scheme (see serve_photo in lib.rs), which
   only serves files inside folders the user has added as a source. */
const photoUrl = p => "photo://localhost" + encodeURIComponent(p).replace(/%2F/g, "/");
const dialog = window.__TAURI__.dialog;

const S = {
  sources: [],
  source: null,          // active source path
  folder: null,          // active subfolder, null = whole source
  person: null,
  cluster: null,           // an unnamed group being viewed
  clusterHashes: null,
  people: [],              // named + unnamed, from people_overview
  peopleCollapsed: true,
  albums: [],
  sort: "newest",
  photos: [],
  view: [],              // filtered/sorted photos currently on screen
  lbIndex: -1,
  lbList: [],
  sel: new Set(),          // selected photo hashes
  lastIndex: -1,           // anchor for shift-range selection
  zoom: 1, panX: 0, panY: 0,
  edit: null,              // pending, unsaved edit on the open photo
  cropping: false,
  crop: null,              // {x,y,w,h} fractions of the displayed image
  cropAR: null,            // locked aspect ratio, or null for free
  keepOriginal: true,      // safe editing, remembered per session
};

const TRASH = "Trash";

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

async function busy(msg, fn) {
  const label = el("span", {}, msg);
  const pct = el("span", { class: "pct num" });
  const fill = el("span");
  const bar = el("div", { class: "tbar", hidden: true }, fill);
  const t = el("div", { class: "toast", "data-kind": "busy", role: "status" },
    el("div", { class: "trow" }, el("span", { class: "sp" }), label, pct), bar);
  $("#toasts").append(t);
  liveToast = { label, pct, bar, fill, msg };
  try { return await fn(); }
  catch (e) { toast(String(e), "error"); throw e; }
  finally { liveToast = null; t.remove(); }
}

const OP_LABEL = {
  faces: "Detecting faces", thumbs: "Building thumbnails",
  clusters: "Grouping faces", plan: "Analysing photos", apply: "Analysing photos",
  models: "Downloading face models",
};

listen("progress", ({ payload }) => {
  if (!liveToast) return;
  const { op, done, total } = payload;
  if (!total) return;
  const p = Math.round((done / total) * 100);
  liveToast.label.textContent = OP_LABEL[op] || liveToast.msg;
  liveToast.pct.textContent = `${done} / ${total}`;
  liveToast.bar.hidden = false;
  liveToast.fill.style.width = p + "%";
});

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
    return el("button", {
      class: "row" + (s.missing ? " missing" : ""), "aria-current": String(active),
      title: s.path,
      onclick: () => { if (!s.missing) selectSource(s.path); },
      oncontextmenu: e => { e.preventDefault(); removeSource(s.path); }
    },
      el("span", { class: "dotmark" }),
      el("span", { class: "grow" }, s.missing ? `${s.name} (missing)` : s.name),
      el("span", { class: "n num" }, s.missing ? "" : String(s.photos)));
  }));

  const src = S.sources.find(s => s.path === S.source);
  const pb = $("#people-block"), fb = $("#folders-block");
  if (!src) { pb.hidden = true; fb.hidden = true; return; }

  pb.hidden = false;
  const collapsed = S.peopleCollapsed;
  const named = S.people.filter(p => p.name);
  const unnamed = S.people.filter(p => !p.name);
  const face = p => p.cover
    ? el("img", { class: "avatar", src: photoUrl(p.cover), alt: "", loading: "lazy" })
    : el("span", { class: "avatar blank" });

  const rows = named.map(p => el("button", {
    class: "row", "aria-current": String(S.person === p.name),
    onclick: () => selectPerson(p.name)
  }, face(p), el("span", { class: "grow" }, p.name), el("span", { class: "n num" }, String(p.photos))));

  // Unnamed groups are shown too. Detection finding 243 faces and the sidebar still
  // reading "None named yet" is what made face detection look broken.
  for (const u of unnamed.slice(0, 12)) {
    rows.push(el("button", {
      class: "row unnamed", "aria-current": String(S.cluster === u.cluster),
      title: u.suggestion ? `Looks like ${u.suggestion}` : "Unnamed person",
      onclick: () => selectCluster(u.cluster)
    }, face(u),
      el("span", { class: "grow" }, u.suggestion ? `${u.suggestion}?` : "Who is this?"),
      el("span", { class: "n num" }, String(u.photos))));
  }
  if (!rows.length) {
    rows.push(el("div", { class: "row", style: "color:var(--text-faint)" },
      src.faces_analysed < src.photos ? "Not scanned yet" : "No faces found"));
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

  const trash = src.folders.find(f => f.path === TRASH);
  const tb = $("#trash-block");
  tb.hidden = !trash;
  if (trash) {
    $("#trash").replaceChildren(
      el("button", {
        class: "row", "aria-current": String(S.folder === TRASH),
        onclick: () => selectFolder(TRASH)
      }, el("span", { class: "grow" }, "Deleted photos"), el("span", { class: "n num" }, String(trash.count))),
      el("button", {
        class: "row", title: "Hand these to the macOS Trash — openfoto can no longer undo it",
        onclick: emptyTrash
      }, el("span", { class: "grow", style: "color:var(--text-faint)" }, "Empty…")));
  }

  const folders = src.folders.filter(f => f.path !== "" && f.path !== TRASH);
  fb.hidden = folders.length === 0;
  $("#folders").replaceChildren(...folders.map(f => el("button", {
    class: `row indent-${Math.min(f.depth, 2)}`, "aria-current": String(S.folder === f.path),
    onclick: () => selectFolder(f.path)
  },
    el("span", { class: "grow" }, f.name),
    el("span", { class: "n num" }, String(f.count)))));
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

function computeLayout(width) {
  const blocks = [];
  let y = 0;
  const groups = new Map();
  for (const p of S.view) {
    const key = p.taken_at ? DAY(p.taken_at) : "Undated";
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(p);
  }
  for (const [day, items] of groups) {
    blocks.push({ kind: "head", y, h: HEAD_H, day, n: items.length });
    y += HEAD_H;
    for (const r of justify(items, width, ROW_H, GAP)) {
      blocks.push({ kind: "row", y, h: r.h, items: r.items });
      y += r.h + GAP;
    }
    y += 18; // breathing room between days
  }
  LAYOUT = { blocks, height: y, width };
}

function cellFor(p, w, h) {
  const img = el("img", { alt: p.name, loading: "lazy", decoding: "async" });
  // Ask for the *original* with ?t=<hash>: the handler serves the cached thumbnail
  // or renders it now. Only visible cells ever ask, so thumbnails are produced in the
  // order they are looked at.
  img.src = photoUrl(p.path) + "?t=" + p.hash;
  img.addEventListener("load", () => img.classList.add("on"), { once: true });
  return el("div", {
    class: "cell" + (S.sel.has(p.hash) ? " sel" : ""),
    style: `width:${Math.max(40, w)}px;height:${h}px`,
    title: p.name,
    "data-hash": p.hash,
    onclick: e => {
      if (e.metaKey || e.ctrlKey) { toggleSel(p); return; }
      if (e.shiftKey) { rangeSel(p); return; }
      if (S.sel.size) { toggleSel(p); return; }
      openLightbox(p);
    },
    oncontextmenu: e => {
      e.preventDefault();
      if (!S.sel.has(p.hash)) { S.sel.clear(); toggleSel(p); }
      showCtx(e.clientX, e.clientY);
    }
  }, img,
    el("button", {
      class: "pick", "aria-label": `Select ${p.name}`, tabindex: "-1",
      onclick: e => { e.stopPropagation(); toggleSel(p); }
    }, "\u2713"),
    p.kind === "video" ? el("span", { class: "play" }, "\u25B6") : null,
    p.people.length ? el("span", { class: "badge" }, p.people.join(", ")) : null);
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
      ? el("div", { class: "dayhead", "data-b": i, style: `top:${b.y}px` },
          el("b", {}, b.day), el("span", { class: "num" }, String(b.n)))
      : el("div", { class: "jrow", "data-b": i, style: `top:${b.y}px;height:${b.h}px` },
          b.items.map(({ p, r }) => cellFor(p, r * b.h, b.h)));
    frag.append(node);
  });
  stage.append(frag);
  for (const n of [...stage.children]) {
    const i = Number(n.dataset.b);
    if (!wanted.has(i)) n.remove();
  }
}

function renderGrid() {
  const stage = $("#stage");
  if (!S.source) return renderWelcome();
  if (!S.view.length) {
    stage.className = "";
    stage.style.height = "";
    stage.replaceChildren(el("div", { class: "welcome" },
      el("div", { class: "art" }, "\u25C7"),
      el("h2", {}, "Nothing here yet"),
      el("p", {}, S.photos.length
        ? "No photos match this filter."
        : "This folder has no photos openfoto can read, or it has not finished indexing."),
      S.photos.length ? el("button", { class: "btn ghost", onclick: () => selectSource(S.source) }, "Show all photos") : null));
    return;
  }
  stage.className = "virt";
  computeLayout(stage.clientWidth || $("#main").clientWidth - 48 || 1000);
  stage.style.height = LAYOUT.height + "px";
  stage.replaceChildren();
  $("#main").scrollTop = 0;
  paintViewport();
}

function renderWelcome() {
  $("#stage").replaceChildren(el("div", { class: "welcome" },
    el("div", { class: "art" }, "◎"),
    el("h2", {}, "Your folders, your photos"),
    el("p", {}, "openfoto reads folders you already have. Nothing is copied into a database, and nothing moves unless you ask. Add a folder to begin."),
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
  if (one) items.push(item("Rename…", "", () => renamePhoto(one)));
  if (S.person) items.push(item(`Not ${S.person}`, "", untagSelected));
  items.push(el("hr"));
  if (S.folder === TRASH) items.push(item(`Restore ${n}`, "", restoreSelected));
  else items.push(item(`Move ${n} to Trash`, "⌫", deleteSelected, "danger"));
  menu.replaceChildren(...items);

  menu.hidden = false;
  const r = menu.getBoundingClientRect();
  menu.style.left = Math.min(x, innerWidth - r.width - 8) + "px";
  menu.style.top = Math.min(y, innerHeight - r.height - 8) + "px";
}
function hideCtx() { $("#ctx").hidden = true; }

/* ---------------- editing ---------------- */
async function deleteSelected() {
  const hashes = [...S.sel];
  if (!hashes.length) return;
  const msg = await busy(`Moving ${hashes.length} to Trash…`,
    () => invoke("delete_photos", { path: S.source, hashes }));
  toast(msg + " — press ⌘Z to undo", "ok");
  clearSel(); await reload();
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
  const name = prompt("Rename photo", p.name);
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
  if (!confirm("Move everything in Trash to the macOS Trash?\n\nopenfoto can no longer undo this — Finder can still recover the files.")) return;
  const msg = await busy("Emptying Trash…", () => invoke("empty_trash", { path: S.source }));
  toast(msg, "ok");
  if (S.folder === TRASH) S.folder = null;
  await refreshSources(); await reload();
}

/* ---------------- lightbox ---------------- */
function openLightbox(photo) {
  // Navigation follows what you are actually looking at. Filtered to a person or a
  // folder, stepping through stays inside that set; browsing unfiltered, it falls back
  // to the photo's own folder, which is the Picasa behaviour.
  const filtered = S.person || S.folder || $("#search").value.trim();
  S.lbList = filtered ? S.view.slice() : S.photos.filter(p => p.folder === photo.folder);
  S.lbIndex = S.lbList.findIndex(p => p.hash === photo.hash);
  $("#lightbox").hidden = false;
  paintLightbox();
}
function paintLightbox() {
  const p = S.lbList[S.lbIndex];
  if (!p) return;
  const stage = document.querySelector(".lb-stage");
  const isVideo = p.kind === "video";
  stage.querySelector("video")?.remove();
  const img = $("#lb-img");
  img.hidden = isVideo;
  if (isVideo) {
    const v = el("video", { id: "lb-video", src: photoUrl(p.path), controls: true, autoplay: true });
    stage.append(v);
  } else {
    // Pass the hash so the handler can cache a transcode when the format needs one.
    img.src = photoUrl(p.path) + "?full=" + p.hash;
  }
  resetZoom();
  $("#lb-name").textContent = p.name;
  $("#lb-meta").textContent = [
    p.taken_at ? `${DAY(p.taken_at)} · ${TIME(p.taken_at)}` : "Undated",
    p.width ? `${p.width}×${p.height}` : null,
    p.people.length ? p.people.join(", ") : (p.faces ? `${p.faces} face${p.faces > 1 ? "s" : ""}` : null),
  ].filter(Boolean).join("   ·   ");
  const scope = S.person ? `👤 ${S.person}` : (S.folder || p.folder || "root");
  $("#lb-folder").textContent = `${scope} · ${S.lbIndex + 1} of ${S.lbList.length}`;

  // Render a window around the current photo rather than the whole folder. A few
  // hundred thumbnails in one flex row is both slow and, on WKWebView, enough to
  // destabilise the layout of the sibling image.
  const strip = $("#lb-strip");
  const WIN = 30;
  const from = Math.max(0, S.lbIndex - WIN);
  const to = Math.min(S.lbList.length, S.lbIndex + WIN + 1);
  strip.replaceChildren(...S.lbList.slice(from, to).map((q, k) => {
    const i = from + k;
    return el("img", {
      src: photoUrl(q.path) + "?t=" + q.hash, alt: q.name, loading: "lazy", decoding: "async",
      "aria-current": String(i === S.lbIndex),
      onclick: () => { S.lbIndex = i; paintLightbox(); }
    });
  }));
  strip.querySelector('[aria-current="true"]')?.scrollIntoView({ inline: "center", block: "nearest" });
  paintStars();
  if (!$("#infopanel").hidden) { $("#infopanel").hidden = true; toggleInfo(); }
}
function closeLightbox() {
  $("#lightbox").hidden = true;
  $("#lb-img").src = "";
  document.querySelector(".lb-stage")?.querySelector("video")?.remove();
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
  if (!S.source) return;
  try {
    S.people = await invoke("people_overview", { path: S.source, distance: 0.55 });
  } catch { S.people = []; }
  try { S.albums = await invoke("list_albums", { path: S.source }); } catch { S.albums = []; }
  await refreshSources();
}
async function loadPhotos() {
  S.photos = await invoke("photos", { path: S.source, folder: null, person: null });
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
  const tokens = q.toLowerCase().split(/[\s,]+/).filter(Boolean);
  const names = people.filter(Boolean).map(n => n.toLowerCase());
  const albumNames = albums.map(a => a.toLowerCase());

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
    if (LABEL_NAMES.includes(raw)) { want.label = raw; continue; }
    if (raw === "video" || raw === "videos") { want.kind = "video"; continue; }
    if (raw === "photo" || raw === "photos") { want.kind = "photo"; continue; }
    if (raw === "fav" || raw === "favourite" || raw === "favorite") { want.fav = true; continue; }
    const stars = raw.match(/^(\d)\+?(?:star|stars)$/);
    if (stars) { want.minRating = +stars[1]; continue; }

    text.push(raw);
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
  for (const t of text) out.push({ kind: "text", text: t });
  return out;
}

/* Show what the query was understood to mean. Someone typing "sam august 2026" should
   see it become a person and a date, not wonder why nothing matched. */
function showQueryChips(parsed) {
  const bar = $("#qchips");
  if (!parsed || (!parsed.hasFilter && !parsed.text.length)) { bar.hidden = true; return; }
  bar.hidden = false;
  bar.replaceChildren(...queryChips(parsed).map(c =>
    el("span", { class: `qc qc-${c.kind}` }, c.text)));
}

function matchesQuery(p, parsed) {
  const { want, text } = parsed;
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
  if (!text.length) return true;
  const hay = [p.name, p.folder, p.people.join(" ")].join(" ").toLowerCase();
  return text.every(t => hay.includes(t));
}

function applyFilter() {
  document.querySelector(".namebar")?.remove();
  const q = $("#search").value.trim();
  const parsed = q
    ? parseQuery(q, S.people.filter(p => p.name).map(p => p.name), S.albums.map(a => a[0]))
    : null;
  showQueryChips(parsed);
  S.view = S.photos.filter(p =>
    // Trash is a real folder, but it should not appear in the library view unless
    // the user deliberately opens it.
    (S.folder === TRASH || p.folder !== TRASH) &&
    (!S.folder || p.folder === S.folder) &&
    (!S.person || p.people.includes(S.person)) &&
    (!S.clusterHashes || S.clusterHashes.has(p.hash)) &&
    (!parsed || matchesQuery(p, parsed)));
  sortView();
  const src = S.sources.find(s => s.path === S.source);
  $("#crumb").textContent = [
    src?.name, S.folder,
    S.person ? `\u{1F464} ${S.person}` : null,
    S.cluster !== null ? "\u{1F464} unnamed person" : null,
  ].filter(Boolean).join("  \u203A  ") + `   \u00B7   ${S.view.length} photos`;
  renderGrid();
  paintSel();
  renderFilters();
}
async function selectSource(path) {
  S.source = path; S.folder = null; S.person = null;
  S.cluster = null; S.clusterHashes = null; S.people = [];
  renderSidebar();
  await busy("Loading library…", loadPhotos);
  refreshPeople();
  // Thumbnails are produced on demand by the photo:// handler as cells scroll into
  // view, so nothing blocks the first paint. A background pass backfills the rest so
  // later scrolling is instant, but it is an optimisation, not a prerequisite.
  const src = S.sources.find(s => s.path === path);
  if (src && src.thumbs_ready < src.photos) {
    invoke("build_thumbs", { path })
      .then(() => refreshSources())
      .catch(() => {});
  }
}
function selectFolder(f) { S.folder = (S.folder === f ? null : f); S.person = null; renderSidebar(); applyFilter(); }
function selectPerson(p) {
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

function namePrompt(id) {
  const u = S.people.find(p => p.cluster === id);
  const bar = el("div", { class: "namebar" },
    u?.cover ? el("img", { class: "avatar lg", src: photoUrl(u.cover), alt: "" }) : null,
    el("span", { class: "grow" }, "Who is this?"),
    el("input", {
      class: "nameinput", type: "text", placeholder: u?.suggestion || "Add a name",
      "aria-label": "Person name",
      onkeydown: async e => {
        if (e.key !== "Enter") return;
        const v = e.target.value.trim() || u?.suggestion;
        if (!v) return;
        await busy(`Learning ${v}…`,
          () => invoke("name_cluster", { path: S.source, distance: 0.55, cluster: id, name: v }));
        toast(`Named ${v}`, "ok");
        S.cluster = null; S.clusterHashes = null;
        await refreshPeople(); await loadPhotos();
      }
    }),
    el("button", { class: "btn ghost sm", onclick: () => { S.cluster = null; S.clusterHashes = null; renderSidebar(); applyFilter(); } }, "Skip"));
  $("#stage").before(bar);
  setTimeout(() => bar.querySelector("input")?.focus(), 60);
}

async function addSource() {
  const picked = await dialog.open({ directory: true, multiple: false, title: "Add a photo folder" });
  if (!picked) return;
  await busy("Indexing folder…", async () => {
    await invoke("add_source", { path: picked });
    await refreshSources();
    await selectSource(picked);
  });
  toast("Folder added", "ok");
  autodetect(picked);
}

/* Finding people is the point of the app, so a newly added folder is scanned for
   faces without being asked. Silent if the models are not installed. */
async function autodetect(path) {
  try {
    const msg = await busy("Looking for people…", () => invoke("autodetect_faces", { path }));
    if (msg.includes("models not installed")) return;
    await refreshPeople();
    const unnamed = S.people.filter(p => !p.name).length;
    if (unnamed) toast(`${unnamed} people found — name them in the sidebar`, "ok");
  } catch { /* reported by busy */ }
}
async function removeSource(path) {
  if (!confirm(`Remove ${path} from openfoto?\n\nYour photos are not touched.`)) return;
  await invoke("remove_source", { path });
  if (S.source === path) { S.source = null; S.photos = []; S.view = []; renderWelcome(); }
  await refreshSources();
}

/* ---------------- organize sheet ---------------- */
const OPS = [
  { id: "dedupe",  title: "Find duplicates",  desc: "Group burst shots and set all but the sharpest aside." },
  { id: "scenery", title: "Split out scenery", desc: "Move photos with no close-up person into Scenery." },
  { id: "file",    title: "File by person",    desc: "Move each photo into a folder named for the person in it." },
  { id: "rename",  title: "Rename by date",    desc: "Give every file a date-and-time filename." },
];
function openSheet() {
  if (!S.source) return toast("Add a folder first");
  $("#sheet-title").textContent = "Organize";
  $("#sheet-body").replaceChildren(
    ...OPS.map(op => {
      const out = el("div", { class: "planout", hidden: true });
      const apply = el("button", { class: "btn", disabled: true, onclick: () => runApply(op, out, apply) }, "Apply");
      return el("div", { class: "op" },
        el("div", { class: "txt" }, el("b", {}, op.title), el("span", {}, op.desc), out),
        el("button", { class: "btn ghost", onclick: () => runPreview(op, out, apply) }, "Preview"),
        apply);
    }),
    el("div", { class: "op", id: "op-faces" },
      el("div", { class: "txt" }, el("b", {}, "Find people"),
        el("span", { id: "faces-note" }, "Detect faces, then name the groups openfoto finds.")),
      el("button", { class: "btn ghost", onclick: analyze }, "Detect faces"),
      el("button", { class: "btn", onclick: openReview }, "Review people")),
    el("div", { class: "op" },
      el("div", { class: "txt" }, el("b", {}, "Undo"), el("span", {}, "Reverse the most recent change.")),
      el("button", { class: "btn ghost", onclick: doUndo }, "Undo last")));
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
  const msg = await busy("Detecting faces… this runs once per photo", () => invoke("analyze_faces", { path: S.source }));
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
  const cl = await busy("Grouping faces…", () => invoke("clusters", { path: S.source, distance: 0.55 }));
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
  let added = 0;
  for (const p of paths) {
    try {
      await busy(`Adding ${p.split("/").pop()}…`, () => invoke("add_source", { path: p }));
      added++;
    } catch (e) { /* reported by busy */ }
  }
  if (added) {
    await refreshSources();
    await selectSource(paths[0]);
    toast(`Added ${added} folder${added > 1 ? "s" : ""}`, "ok");
    autodetect(paths[0]);
  }
});

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
            "openfoto can keep the untouched original so you can go back to it."),
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

  const sorts = [["newest", "Newest"], ["oldest", "Oldest"], ["name", "Name"],
                 ["rating", "Rating"], ["size", "Size"]];
  $("#f-sort").replaceChildren(...sorts.map(([k, label]) =>
    el("button", {
      class: "fopt", "aria-pressed": String(S.sort === k),
      onclick: () => { S.sort = k; applyFilter(); renderFilters(); }
    }, label)));
}

function labelColour(l) {
  return { red: "#f87171", orange: "#fb923c", yellow: "#fbbf24", green: "#4ade80",
           blue: "#60a5fa", purple: "#c4b5fd", grey: "#9ca3af" }[l] || "#888";
}

function sortView() {
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
  panel.replaceChildren(
    el("h3", {}, "Info"),
    row("File", d.path),
    row("Size", `${mb} MB`),
    row("Pixels", d.width ? `${d.width} × ${d.height}` : null),
    row("Taken", d.taken_at ? `${DAY(d.taken_at)} · ${TIME(d.taken_at)}` : "Unknown"),
    row("Date from", d.taken_from),
    row("Kind", d.kind),
    row("Faces", d.faces || null),
    row("People", d.people.join(", ") || null),
    row("Rating", d.meta.rating ? "\u2605".repeat(d.meta.rating) : null),
    row("Label", d.meta.label),
    row("Albums", (d.meta.albums || []).join(", ") || null));
  panel.hidden = false;
}

/* ---------------- wiring ---------------- */
$("#btn-add").onclick = addSource;
$("#btn-tools").onclick = openSheet;
$("#btn-review").onclick = openReview;
$("#sheet-close").onclick = () => ($("#sheet").hidden = true);
$("#sheet").onclick = e => { if (e.target.id === "sheet") $("#sheet").hidden = true; };
$("#lb-close").onclick = closeLightbox;
$("#lb-info").onclick = toggleInfo;
$("#btn-filter").onclick = () => {
  const f = $("#filters");
  f.hidden = !f.hidden;
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
$("#sel-delete").onclick = deleteSelected;
$("#sel-restore").onclick = restoreSelected;
$("#sel-untag").onclick = untagSelected;
addEventListener("click", e => { if (!e.target.closest("#ctx")) hideCtx(); });
$("#lb-prev").onclick = () => step(-1);
$("#lb-next").onclick = () => step(1);
$("#search").oninput = applyFilter;
addEventListener("keydown", e => {
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
    return;
  }
  if (e.key === "Escape") { hideCtx(); if (!$("#sheet").hidden) $("#sheet").hidden = true; else if (S.sel.size) clearSel(); }
  if (e.key === "/" && document.activeElement !== $("#search")) { e.preventDefault(); $("#search").focus(); }
  const typing = /^(INPUT|TEXTAREA)$/.test(document.activeElement?.tagName || "");
  if (typing) return;
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "a") { e.preventDefault(); S.view.forEach(p => S.sel.add(p.hash)); paintSel(); }
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "z") { e.preventDefault(); doUndo(); }
  if ((e.key === "Backspace" || e.key === "Delete") && S.sel.size) {
    e.preventDefault();
    S.folder === TRASH ? restoreSelected() : deleteSelected();
  }
});
let rt; addEventListener("resize", () => { clearTimeout(rt); rt = setTimeout(renderGrid, 120); });
$("#main").addEventListener("scroll", () => paintViewport(), { passive: true });

(async function init() {
  renderWelcome();
  await refreshSources();
  if (S.sources.length) {
    const first = S.sources.find(s => !s.missing);
    if (first) await selectSource(first.path);
  }
})();
