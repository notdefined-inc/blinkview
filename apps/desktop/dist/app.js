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
  photos: [],
  view: [],              // filtered/sorted photos currently on screen
  lbIndex: -1,
  lbList: [],
  sel: new Set(),          // selected photo hashes
  lastIndex: -1,           // anchor for shift-range selection
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
  $("#people").replaceChildren(
    ...(src.people.length ? src.people.map(p => el("button", {
      class: "row", "aria-current": String(S.person === p.name),
      onclick: () => selectPerson(p.name)
    },
      p.cover ? el("img", { class: "avatar", src: photoUrl(p.cover), alt: "" })
              : el("span", { class: "dotmark" }),
      el("span", { class: "grow" }, p.name),
      el("span", { class: "n num" }, String(p.photos))))
      : [el("div", { class: "row", style: "color:var(--text-faint)" }, "None named yet")]));

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

function renderGrid() {
  const stage = $("#stage");
  if (!S.source) return renderWelcome();
  if (!S.view.length) {
    stage.replaceChildren(el("div", { class: "welcome" },
      el("div", { class: "art" }, "◇"),
      el("h2", {}, "Nothing here yet"),
      el("p", {}, S.photos.length
        ? "No photos match this filter."
        : "This folder has no photos openfoto can read, or it has not finished indexing."),
      S.photos.length ? el("button", { class: "btn ghost", onclick: () => selectSource(S.source) }, "Show all photos") : null));
    return;
  }

  const width = stage.clientWidth || 1000;
  const groups = new Map();
  for (const p of S.view) {
    const key = p.taken_at ? DAY(p.taken_at) : "Undated";
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(p);
  }

  const frag = document.createDocumentFragment();
  for (const [day, items] of groups) {
    const rows = justify(items, width, 200, 3);
    frag.append(el("section", { class: "daygroup" },
      el("div", { class: "dayhead" }, el("b", {}, day), el("span", { class: "num" }, `${items.length}`)),
      ...rows.map(r => el("div", { class: "jrow" },
        r.items.map(({ p, r: ratio }) => {
          const img = el("img", { alt: p.name, loading: "lazy" });
          img.dataset.src = photoUrl(p.thumb);
          img.addEventListener("load", () => img.classList.add("on"));
          io.observe(img);
          const cell = el("div", {
            class: "cell" + (S.sel.has(p.hash) ? " sel" : ""),
            style: `width:${Math.max(40, ratio * r.h)}px;height:${r.h}px`,
            title: p.name,
            "data-hash": p.hash,
            onclick: e => {
              // Modifier-click selects; a plain click opens, unless a selection is
              // already running — then it extends it, as every photo app does.
              if (e.metaKey || e.ctrlKey) { toggleSel(p); return; }
              if (e.shiftKey) { rangeSel(p); return; }
              if (S.sel.size) { toggleSel(p); return; }
              openLightbox(p);
            },
            oncontextmenu: e => { e.preventDefault(); if (!S.sel.has(p.hash)) { S.sel.clear(); toggleSel(p); } showCtx(e.clientX, e.clientY); }
          }, img,
            p.kind === "video" ? el("span", { class: "play" }, "▶") : null,
            p.people.length ? el("span", { class: "badge" }, p.people.join(", ")) : null);
          return cell;
        })))));
  }
  stage.replaceChildren(frag);
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
    img.src = photoUrl(p.path);
  }
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
      src: photoUrl(q.thumb), alt: q.name, loading: "lazy", decoding: "async",
      "aria-current": String(i === S.lbIndex),
      onclick: () => { S.lbIndex = i; paintLightbox(); }
    });
  }));
  strip.querySelector('[aria-current="true"]')?.scrollIntoView({ inline: "center", block: "nearest" });
}
function closeLightbox() { $("#lightbox").hidden = true; $("#lb-img").src = ""; }
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
async function loadPhotos() {
  S.photos = await invoke("photos", { path: S.source, folder: null, person: null });
  applyFilter();
}
/* Search covers what a person would actually type: a filename, a person's name, a
   folder, or a date like "august" or "2026". */
function matches(p, q) {
  const hay = [
    p.name, p.folder, p.people.join(" "),
    p.taken_at ? DAY(p.taken_at) : "",
  ].join(" ").toLowerCase();
  return q.split(/\s+/).every(term => hay.includes(term));
}

function applyFilter() {
  const q = $("#search").value.trim().toLowerCase();
  S.view = S.photos.filter(p =>
    // Trash is a real folder, but it should not appear in the library view unless
    // the user deliberately opens it.
    (S.folder === TRASH || p.folder !== TRASH) &&
    (!S.folder || p.folder === S.folder) &&
    (!S.person || p.people.includes(S.person)) &&
    (!q || matches(p, q)));
  const src = S.sources.find(s => s.path === S.source);
  $("#crumb").textContent = [src?.name, S.folder, S.person ? `👤 ${S.person}` : null]
    .filter(Boolean).join("  ›  ") + `   ·   ${S.view.length} photos`;
  renderGrid();
  paintSel();
}
async function selectSource(path) {
  S.source = path; S.folder = null; S.person = null;
  renderSidebar();
  await busy("Loading library…", loadPhotos);
  const src = S.sources.find(s => s.path === path);
  if (src && src.thumbs_ready < src.photos) {
    busy("Building thumbnails…", async () => {
      await invoke("build_thumbs", { path });
      await refreshSources(); await loadPhotos();
      toast("Thumbnails ready", "ok");
    });
  }
}
function selectFolder(f) { S.folder = (S.folder === f ? null : f); S.person = null; renderSidebar(); applyFilter(); }
function selectPerson(p) { S.person = (S.person === p ? null : p); S.folder = null; renderSidebar(); applyFilter(); }

async function addSource() {
  const picked = await dialog.open({ directory: true, multiple: false, title: "Add a photo folder" });
  if (!picked) return;
  await busy("Indexing folder…", async () => {
    await invoke("add_source", { path: picked });
    await refreshSources();
    await selectSource(picked);
  });
  toast("Folder added", "ok");
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
    el("div", { class: "op" },
      el("div", { class: "txt" }, el("b", {}, "Find people"),
        el("span", {}, "Detect faces, then name the groups openfoto finds.")),
      el("button", { class: "btn ghost", onclick: analyze }, "Detect faces"),
      el("button", { class: "btn", onclick: openReview }, "Review people")),
    el("div", { class: "op" },
      el("div", { class: "txt" }, el("b", {}, "Undo"), el("span", {}, "Reverse the most recent change.")),
      el("button", { class: "btn ghost", onclick: doUndo }, "Undo last")));
  $("#sheet").hidden = false;
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
  await refreshSources(); await loadPhotos();
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

/* ---------------- wiring ---------------- */
$("#btn-add").onclick = addSource;
$("#btn-tools").onclick = openSheet;
$("#btn-review").onclick = openReview;
$("#sheet-close").onclick = () => ($("#sheet").hidden = true);
$("#sheet").onclick = e => { if (e.target.id === "sheet") $("#sheet").hidden = true; };
$("#lb-close").onclick = closeLightbox;
$("#lb-rename").onclick = () => { const p = S.lbList[S.lbIndex]; if (p) renamePhoto(p); };
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
    if (e.key === "Escape") closeLightbox();
    if (e.key === "ArrowRight") step(1);
    if (e.key === "ArrowLeft") step(-1);
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

(async function init() {
  renderWelcome();
  await refreshSources();
  if (S.sources.length) {
    const first = S.sources.find(s => !s.missing);
    if (first) await selectSource(first.path);
  }
})();
