/* Blinkview's phone view. The bridge supplies window.__TAURI__; this file owns only
   presentation and interaction, while every read and write uses the desktop engine. */
import { computeLayout, hydrate, parseQuery, matchesStructured, DAY, TIME, bytesLabel, labelColour } from "./core.js";

const $ = s => document.querySelector(s);
const el = (tag, attrs = {}, ...children) => {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === null || value === undefined || value === false) continue;
    if (key === "class") node.className = value;
    else if (key === "style" && typeof value === "object") for (const [prop, css] of Object.entries(value)) prop.startsWith("--") ? node.style.setProperty(prop, css) : node.style[prop] = css;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else node.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children.flat()) if (child !== null && child !== undefined) node.append(child.nodeType ? child : String(child));
  return node;
};

// A deterministic visual fixture for the native screenshot harness. It is unreachable
// through the HTTP bridge, so a paired browser can never mistake demo photos for data.
const DEMO = location.protocol === "tauri:" && new URLSearchParams(location.search).has("demo");
const bridge = DEMO ? demoBridge() : window.__TAURI__ || demoBridge();
const { invoke } = bridge.core;
const { listen } = bridge.event;
const S = { sources: [], source: null, folder: null, photos: [], people: [], searches: [],
  sort: "newest", tab: "photos", search: "", searchView: [], viewer: [], at: -1, loading: true,
  zoom: 1, panX: 0, panY: 0, pointers: new Map(), detail: null };
const grids = new Map();
let toastTimer = 0;

function demoBridge() {
  const names = ["Lake District", "Sunday market", "Maya at the coast", "Late train", "Kitchen light", "North pier", "After the rain", "First swim", "Quiet museum", "Garden table", "Long way home", "Blue hour"];
  const photos = names.map((name, i) => ({ kind: i % 7 === 0 ? "video" : "photo", hash: `demo-${i}`, path: `demo:${i}`, bytes: 1800000 + i * 83000,
    width: [1600, 1200, 1800, 1300][i % 4], height: [1067, 1600, 1200, 1300][i % 4], taken_at: Date.UTC(2026, 7, 31 - Math.floor(i / 4), 10 + i) / 1000,
    rating: i % 6, label: i === 2 ? "blue" : null, people: i % 3 === 0 ? ["Maya"] : [], faces: i % 3 === 0 ? 1 : 0, name: `${name}.jpg`, folder: i > 7 ? "Trips/Coast" : "", ext: "JPG", albums: [] }));
  const sources = [{ name: "Summer archive", path: "/Demo/Summer archive", photos: photos.length, videos: 2, missing: false, indexing: false,
    folders: [{ name: "All photos", path: "", depth: 0, count: photos.length }, { name: "Trips", path: "Trips", depth: 1, count: 4 }, { name: "Coast", path: "Trips/Coast", depth: 2, count: 4 }] }];
  return { core: { invoke: async (cmd) => {
    if (cmd === "list_sources") return sources;
    if (cmd === "photos") return photos.map(({ name, folder, ext, ...p }) => p);
    if (cmd === "people_overview") return { entries: [{ name: "Maya", photos: 4, cover: "demo:3" }], dismissed: 0 };
    if (cmd === "list_searches") return [];
    if (cmd === "photo_detail") return { bytes: photos[S.at]?.bytes || 0, exif: { make: "Fujifilm", model: "X-T5" } };
    return null;
  } }, event: { listen: async () => () => {} } };
}

function demoImage(id) {
  const i = Number(String(id).split(":")[1] || 0), hue = (i * 37 + 195) % 360;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="900" height="700"><defs><linearGradient id="g" x2="1" y2="1"><stop stop-color="hsl(${hue} 55% 62%)"/><stop offset="1" stop-color="hsl(${(hue + 70) % 360} 38% 20%)"/></linearGradient></defs><rect width="100%" height="100%" fill="url(#g)"/><circle cx="${190 + i * 37 % 500}" cy="${160 + i * 53 % 320}" r="120" fill="rgba(255,255,255,.18)"/></svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}

function photoUrl(path, mode = "t", hash = "") {
  if (String(path).startsWith("demo:")) return demoImage(path);
  const abs = path.startsWith("/") ? path : `${S.source}/${path}`;
  const base = "/photo" + abs.split("/").map(encodeURIComponent).join("/");
  return hash ? `${base}?${mode}=${encodeURIComponent(hash)}` : base;
}

function showToast(message) {
  const toast = $("#toast"); toast.textContent = message; toast.hidden = false;
  clearTimeout(toastTimer); toastTimer = setTimeout(() => { toast.hidden = true; }, 2600);
}
function empty(stage, symbol, title, note) {
  stage.style.height = "";
  stage.replaceChildren(el("div", { class: "empty" }, el("span", { class: "symbol", "aria-hidden": "true" }, symbol), el("b", {}, title), el("small", {}, note)));
}

function visiblePhotos() {
  const list = S.photos.filter(p => !p.liveStill && p.folder !== "Trash" && (!S.folder || p.folder === S.folder || p.folder.startsWith(S.folder + "/")));
  return list.sort((a, b) => S.sort === "oldest" ? (a.taken_at || 0) - (b.taken_at || 0) : (b.taken_at || 0) - (a.taken_at || 0));
}

function drawGrid(name, photos) {
  const scroll = $(`#${name}-scroll`), stage = $(`#${name}-stage`);
  // A resize, sort or filter changes every row's geometry. Reusing blocks with the
  // same numeric index keeps their old pixel widths and makes the last cell overflow.
  stage.replaceChildren();
  if (!photos.length) return empty(stage, name === "search" ? "⌕" : "◇", name === "search" && !S.search ? "Search your library" : "Nothing here yet", name === "search" && !S.search ? "Try a person, date, filename, rating or colour label." : "No photographs match this view.");
  const width = Math.max(280, stage.clientWidth || scroll.clientWidth - 16);
  const layout = computeLayout(photos, { sort: S.sort, group: "date", folder: S.folder || "", rowH: 138, gap: 3, headH: 42 }, width);
  stage.style.height = `${layout.height}px`; grids.set(name, { photos, layout }); paintGrid(name);
}

function paintGrid(name) {
  const state = grids.get(name); if (!state) return;
  const scroll = $(`#${name}-scroll`), stage = $(`#${name}-stage`), top = scroll.scrollTop - 600, bottom = scroll.scrollTop + scroll.clientHeight + 600;
  const wanted = new Set(), fragment = document.createDocumentFragment();
  state.layout.blocks.forEach((block, index) => {
    if (block.y + block.h < top || block.y > bottom) return;
    wanted.add(index); if (stage.querySelector(`[data-block="${index}"]`)) return;
    const node = block.kind === "head"
      ? el("div", { class: "grid-head", "data-block": index, style: { top: `${block.y}px`, height: `${block.h}px` } }, el("b", {}, block.day), el("span", {}, `${block.n} items`))
      : el("div", { class: "grid-row", "data-block": index, style: { top: `${block.y}px`, height: `${block.h}px` } }, block.items.map(({ p, r }) => gridCell(p, r * block.h, block.h, state.photos)));
    fragment.append(node);
  });
  stage.append(fragment);
  for (const node of [...stage.children]) if (!wanted.has(Number(node.dataset.block))) node.remove();
}

function gridCell(photo, width, height, list) {
  const image = el("img", { alt: photo.name, loading: "lazy", decoding: "async", src: photoUrl(photo.path, "t", photo.hash) });
  const cell = el("button", { class: "grid-cell", type: "button", style: { width: `${Math.max(44, width)}px`, height: `${height}px` }, "aria-label": `Open ${photo.name}`, onclick: () => openViewer(list, photo) }, image, photo.kind === "video" ? el("span", { class: "kind" }, "▶") : null);
  image.addEventListener("load", () => cell.classList.add("loaded"), { once: true });
  return cell;
}

function renderPhotos() {
  const list = visiblePhotos(), src = S.sources.find(s => s.path === S.source);
  $("#photos-title").textContent = S.folder ? S.folder.split("/").pop() : "Photos";
  $("#photos-kicker").textContent = `${list.length.toLocaleString()} items · ${S.sort === "newest" ? "Newest first" : "Oldest first"}`;
  $("#source-name").textContent = src?.name || "Choose library";
  if (S.loading) return empty($("#photos-stage"), "◴", "Opening library…", "Reading the index on your desktop.");
  drawGrid("photos", list);
}

function renderSearch() {
  const input = $("#search-input"), query = input.value.trim(); S.search = query; $("#search-clear").hidden = !query;
  if (!query) { S.searchView = []; $("#search-chips").replaceChildren(); return drawGrid("search", []); }
  const names = S.people.map(p => p.name).filter(Boolean), parsed = parseQuery(query, names, []);
  const literal = parsed.text.join(" ").toLowerCase();
  S.searchView = visiblePhotos().filter(p => matchesStructured(p, parsed.want) && (!literal || [p.name, p.folder, p.people.join(" ")].join(" ").toLowerCase().includes(literal)));
  const chips = [];
  if (parsed.want.person) chips.push(`Person · ${parsed.want.person}`);
  if (parsed.want.year) chips.push(String(parsed.want.year));
  if (parsed.want.month) chips.push(new Date(2026, parsed.want.month - 1).toLocaleString(undefined, { month: "long" }));
  if (parsed.want.minRating) chips.push(`${"★".repeat(parsed.want.minRating)}+`);
  if (parsed.want.label) chips.push(parsed.want.label);
  if (literal) chips.push(`Text · ${literal}`);
  $("#search-chips").replaceChildren(...chips.map(c => el("span", { class: "chip" }, c)), el("span", { class: "chip" }, `${S.searchView.length} found`));
  drawGrid("search", S.searchView);
}

function renderPeople() {
  const list = $("#people-list");
  if (!S.people.length) return list.replaceChildren(el("div", { class: "empty" }, el("span", { class: "symbol" }, "◉"), el("b", {}, "No people yet"), el("small", {}, "Run face analysis from the desktop app, then named people appear here.")));
  list.replaceChildren(...S.people.map(person => {
    const visual = person.cover ? el("img", { src: photoUrl(person.cover), alt: "", loading: "lazy" }) : el("div", { class: "fallback" }, "◉");
    return el("button", { class: "person", type: "button", onclick: () => showPerson(person) }, visual, el("span", { class: "person-copy" }, el("b", {}, person.name || "Unnamed person"), el("small", {}, `${person.photos} photos`)));
  }));
}

function showPerson(person) {
  if (!person.name) return showToast("Name this person in the desktop app first.");
  S.folder = null; switchTab("photos");
  const list = visiblePhotos().filter(p => p.people.includes(person.name));
  $("#photos-title").textContent = person.name; $("#photos-kicker").textContent = `${list.length} photos`; drawGrid("photos", list);
}

function sourceRow(source, picker = false) {
  return el("button", { class: `lib-row${source.path === S.source ? " active" : ""}`, type: "button", onclick: () => { closeSheet("source"); loadSource(source.path); if (picker) switchTab("photos"); } }, el("span", { class: "lib-icon", "aria-hidden": "true" }, source.missing ? "!" : "▧"), el("span", { class: "lib-copy" }, el("b", {}, source.name), el("small", {}, source.missing ? "Folder is unavailable" : source.indexing ? "Indexing…" : `${source.photos.toLocaleString()} photos · ${(source.videos || 0).toLocaleString()} videos`)), el("span", { "aria-hidden": "true" }, "›"));
}

function renderLibrary() {
  const root = $("#library-list");
  root.replaceChildren(...S.sources.map(source => {
    const folders = source.path === S.source ? (source.folders || []).filter(f => f.path !== "Trash").map(folder => el("button", { class: `folder${folder.path === (S.folder || "") ? " active" : ""}`, type: "button", style: { "--depth": String(Math.max(0, folder.depth - 1)) }, onclick: () => { S.folder = folder.path || null; renderLibrary(); renderPhotos(); switchTab("photos"); } }, el("span", {}, folder.path ? `⌞ ${folder.name}` : "All photos"), el("span", {}, folder.count))) : [];
    return el("article", { class: "lib-card" }, sourceRow(source), folders.length ? el("div", { class: "folder-list" }, folders) : null);
  }));
}

function switchTab(tab) {
  S.tab = tab;
  for (const panel of document.querySelectorAll("[data-panel]")) panel.hidden = panel.dataset.panel !== tab;
  for (const button of document.querySelectorAll("[data-tab]")) button.classList.toggle("active", button.dataset.tab === tab);
  if (tab === "search") { renderSearch(); setTimeout(() => $("#search-input").focus(), 80); }
  if (tab === "people") renderPeople(); if (tab === "library") renderLibrary(); if (tab === "photos") renderPhotos();
}

async function loadSource(path) {
  S.source = path; S.folder = null; S.photos = []; S.loading = true; grids.clear(); renderPhotos();
  try {
    const [photos, people, searches] = await Promise.all([
      invoke("photos", { path, folder: null, person: null }),
      invoke("people_overview", { path, distance: 0.55 }).catch(() => ({ entries: [] })),
      invoke("list_searches", { path }).catch(() => []),
    ]);
    if (S.source !== path) return;
    S.photos = hydrate(photos); S.people = people.entries || []; S.searches = searches; S.loading = false;
    $("#app").setAttribute("aria-busy", "false"); renderPhotos(); renderPeople(); renderLibrary();
  } catch (error) { S.loading = false; empty($("#photos-stage"), "!", "Couldn’t open this library", String(error)); }
}

async function refreshSources() {
  try {
    S.sources = await invoke("list_sources");
    const next = S.sources.find(s => s.path === S.source) || S.sources.find(s => !s.missing);
    if (!next) { $("#app").setAttribute("aria-busy", "false"); empty($("#photos-stage"), "▧", "No folders yet", "Add a photo folder from the desktop app, then it will appear here."); renderLibrary(); return; }
    await loadSource(next.path);
  } catch (error) { empty($("#photos-stage"), "!", "Desktop unavailable", String(error)); }
}

function openViewer(list, photo) {
  S.viewer = list; S.at = Math.max(0, list.findIndex(p => p.hash === photo.hash)); S.detail = null;
  $("#lightbox").hidden = false; document.body.style.overflow = "hidden"; paintViewer();
}
function closeViewer() { $("#lightbox").hidden = true; $("#lb-video").pause(); closeSheet("action"); resetZoom(); }
function stepViewer(delta) { if (S.zoom > 1 || !S.viewer.length) return; S.at = (S.at + delta + S.viewer.length) % S.viewer.length; S.detail = null; paintViewer(); }
function paintViewer() {
  const p = S.viewer[S.at]; if (!p) return closeViewer(); resetZoom();
  const image = $("#lb-image"), video = $("#lb-video"), isVideo = p.kind === "video";
  image.hidden = isVideo; video.hidden = !isVideo; video.pause();
  if (isVideo) video.src = photoUrl(p.path); else { video.removeAttribute("src"); image.src = photoUrl(p.path, "preview", p.hash); image.alt = p.name; }
  $("#lb-name").textContent = p.name; $("#lb-count").textContent = `${S.at + 1} of ${S.viewer.length}`;
  $("#lb-date").textContent = p.taken_at ? DAY(p.taken_at) : "Undated";
  $("#lb-meta").textContent = [p.taken_at ? TIME(p.taken_at) : null, p.width ? `${p.width} × ${p.height}` : null, p.people.join(", ")].filter(Boolean).join(" · ");
}

function resetZoom() { S.zoom = 1; S.panX = S.panY = 0; S.pointers.clear(); applyZoom(); }
function applyZoom() { const image = $("#lb-image"); image.style.transform = `translate(${S.panX}px,${S.panY}px) scale(${S.zoom})`; $("#zoom-note").hidden = S.zoom === 1; $("#zoom-note").textContent = `${Math.round(S.zoom * 100)}%`; }
function zoomTo(next) { S.zoom = Math.max(1, Math.min(6, next)); if (S.zoom === 1) S.panX = S.panY = 0; applyZoom(); }

async function openActions() {
  const p = S.viewer[S.at]; if (!p) return;
  openSheet("action"); $("#sheet-title").textContent = p.name;
  $("#rating-actions").replaceChildren(...[1,2,3,4,5].map(r => el("button", { type: "button", class: p.rating >= r ? "active" : "", "aria-label": `${r} stars`, onclick: () => setRating(r) }, "★")));
  const colours = ["red","orange","yellow","green","blue","purple","grey"];
  $("#label-actions").replaceChildren(...colours.map(label => el("button", { type: "button", class: p.label === label ? "active" : "", style: { background: labelColour(label) }, "aria-label": `${label} label`, onclick: () => setLabel(p.label === label ? null : label) })));
  $("#photo-details").replaceChildren(el("dt", {}, "Folder"), el("dd", {}, p.folder || "Library root"), el("dt", {}, "Type"), el("dd", {}, p.ext), el("dt", {}, "Dimensions"), el("dd", {}, p.width ? `${p.width} × ${p.height}` : "Unknown"), el("dt", {}, "Size"), el("dd", {}, bytesLabel(p.bytes)));
  try { S.detail = await invoke("photo_detail", { path: S.source, hash: p.hash }); if (S.detail?.bytes) $("#photo-details dd:last-child").textContent = bytesLabel(S.detail.bytes); } catch { /* summary remains useful */ }
}

async function setRating(rating) { const p = S.viewer[S.at]; await action(() => invoke("set_rating", { path: S.source, hashes: [p.hash], rating }), `${rating} star${rating === 1 ? "" : "s"}`); p.rating = rating; openActions(); }
async function setLabel(label) { const p = S.viewer[S.at]; await action(() => invoke("set_label", { path: S.source, hashes: [p.hash], label }), label ? `${label} label` : "Label cleared"); p.label = label; openActions(); }
async function action(run, success) { try { await run(); showToast(success); } catch (error) { showToast(String(error)); } }

function openSheet(name) { $(`#${name}-sheet`).hidden = false; $(`#${name === "action" ? "sheet" : "source"}-scrim`).hidden = false; }
function closeSheet(name) { $(`#${name}-sheet`).hidden = true; $(`#${name === "action" ? "sheet" : "source"}-scrim`).hidden = true; }

function bind() {
  for (const button of document.querySelectorAll("[data-tab]")) button.onclick = () => switchTab(button.dataset.tab);
  for (const name of ["photos", "search"]) $(`#${name}-scroll`).addEventListener("scroll", () => paintGrid(name), { passive: true });
  $("#sort-btn").onclick = () => { S.sort = S.sort === "newest" ? "oldest" : "newest"; renderPhotos(); };
  let searchTimer; $("#search-input").oninput = () => { clearTimeout(searchTimer); searchTimer = setTimeout(renderSearch, 100); };
  $("#search-clear").onclick = () => { $("#search-input").value = ""; renderSearch(); $("#search-input").focus(); };
  $("#source-pill").onclick = () => { $("#source-options").replaceChildren(...S.sources.map(s => sourceRow(s, true))); openSheet("source"); };
  $("#source-close").onclick = $("#source-scrim").onclick = () => closeSheet("source");
  $("#sheet-close").onclick = $("#sheet-scrim").onclick = () => closeSheet("action");
  $("#lb-close").onclick = closeViewer; $("#lb-prev").onclick = () => stepViewer(-1); $("#lb-next").onclick = () => stepViewer(1); $("#lb-more").onclick = openActions;
  $("#rename-action").onclick = async () => { const p = S.viewer[S.at], name = prompt("Rename photo", p.name); if (!name || name === p.name) return; await action(() => invoke("rename_photo", { path: S.source, hash: p.hash, name }), "Photo renamed"); closeSheet("action"); await loadSource(S.source); closeViewer(); };
  $("#delete-action").onclick = async () => { const p = S.viewer[S.at]; if (!confirm(`Move “${p.name}” to Trash? You can restore it from Blinkview on the desktop.`)) return; await action(() => invoke("delete_photos", { path: S.source, hashes: [p.hash], dest: null }), "Moved to Trash"); S.photos = S.photos.filter(x => x.hash !== p.hash); closeViewer(); renderPhotos(); };
  bindGestures(); window.addEventListener("resize", () => { grids.clear(); if (S.tab === "search") renderSearch(); else renderPhotos(); });
}

function bindGestures() {
  const stage = $("#lb-stage"); let down = null, pinch = null, lastTap = 0;
  stage.onpointerdown = e => { stage.setPointerCapture(e.pointerId); S.pointers.set(e.pointerId, { x: e.clientX, y: e.clientY }); down = { x: e.clientX, y: e.clientY, panX: S.panX, panY: S.panY }; if (S.pointers.size === 2) { const [a,b] = [...S.pointers.values()]; pinch = { distance: Math.hypot(a.x-b.x,a.y-b.y), zoom: S.zoom }; } };
  stage.onpointermove = e => { if (!S.pointers.has(e.pointerId)) return; S.pointers.set(e.pointerId, { x:e.clientX,y:e.clientY }); if (S.pointers.size === 2 && pinch) { const [a,b]=[...S.pointers.values()]; zoomTo(pinch.zoom * Math.hypot(a.x-b.x,a.y-b.y)/pinch.distance); } else if (down && S.zoom > 1) { S.panX = down.panX + e.clientX-down.x; S.panY = down.panY + e.clientY-down.y; applyZoom(); } };
  const up = e => { const start = down; S.pointers.delete(e.pointerId); if (!S.pointers.size) { if (start && S.zoom === 1 && Math.abs(e.clientX-start.x)>55 && Math.abs(e.clientY-start.y)<70) stepViewer(e.clientX < start.x ? 1 : -1); const now=Date.now(); if (start && Math.hypot(e.clientX-start.x,e.clientY-start.y)<12 && now-lastTap<280) zoomTo(S.zoom>1?1:2.5); lastTap=now; down=pinch=null; } };
  stage.onpointerup = up; stage.onpointercancel = up;
}

listen("blinkview:remote-lost", () => { $("#connection").hidden = false; });
listen("blinkview:remote-connected", () => { $("#connection").hidden = true; refreshSources(); });
listen("source-ready", ({ payload }) => { if (payload === S.source) loadSource(S.source); else refreshSources(); });
listen("library-changed", ({ payload }) => { if (payload?.[0] === S.source) loadSource(S.source); });
bind(); refreshSources();
