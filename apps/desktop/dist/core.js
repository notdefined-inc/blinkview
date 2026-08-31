/* Shared core of the two view layers (desktop app.js, mobile mobile.js).
   Everything here is pure: no DOM, no `S`, no transport — state goes in, values
   come out. Extracted 2026-08-31 (spec docs/SPECS/active/2026-08-31-mobile-ui.md);
   the desktop imports this file, so a change here is a change to both UIs at once. */

/* Grouping + row layout over a view. Parameterised so both UIs feed their own
   state: `view` is the filtered list, `opts` carries what the desktop reads from
   its `S` (sort, peek, group, folder) plus the row metrics. Returns the layout;
   the caller owns where it is stored. */
export function computeLayout(view, opts, width) {
  const rowH = opts.rowH ?? 200, gap = opts.gap ?? 3, headH = opts.headH ?? 46;
  const blocks = [];
  let y = 0;
  if (opts.sort === "custom" || opts.peek) {
    for (const r of justify(view, width, rowH, gap)) {
      blocks.push({ kind: "row", y, h: r.h, items: r.items });
      y += r.h + gap;
    }
    return { blocks, height: y, width };
  }
  const groups = new Map();
  for (const p of view) {
    const key = opts.group === "folder" ? sectionFor(p, opts.folder || "")
              : (p.taken_at ? DAY(p.taken_at) : "Undated");
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(p);
  }
  const ordered = opts.group === "folder"
    ? [...groups.entries()].sort((a, b) => a[0].localeCompare(b[0]))
    : groups.entries();
  for (const [day, items] of ordered) {
    blocks.push({ kind: "head", y, h: headH, day, n: items.length,
                  month: items[0]?.taken_at ? monthKey(items[0].taken_at) : "",
                  hashes: items.map(p => p.hash) });
    y += headH;
    for (const r of justify(items, width, rowH, gap)) {
      blocks.push({ kind: "row", y, h: r.h, items: r.items });
      y += r.h + gap;
    }
    y += 18; // breathing room between days
  }
  return { blocks, height: y, width };
}

/* Which section a photograph falls under when grouping by folder, relative to
   `base` (the selected folder, "" for the root). Sections are the immediate
   children of wherever you are, not full paths: standing in `Trip`, the useful
   headings are `Greece Day1` and `Swiss Day1`, not the same prefix on every row. */
export function sectionFor(p, base) {
  const rel = base ? p.folder.slice(base.length).replace(/^\//, "") : p.folder;
  if (!rel) return base ? base.split("/").pop() : "Loose photos";
  return rel.split("/")[0];
}

/* Load-race guards, keyed by kind *and* source. A single counter per kind was
   wrong in a way that showed up as an empty grid: a background load for the
   library you just left would bump the counter, discarding the load for the
   library you just opened. `source` is a getter so the guard always compares
   against where the caller is *now*. */
export function loadGuard(source) {
  const seq = {};
  return {
    begin(kind) {
      const key = `${kind}\u0000${source() || ""}`;
      seq[key] = (seq[key] || 0) + 1;
      return { key, seq: seq[key], source: source() };
    },
    stillCurrent(t) {
      return t.seq === seq[t.key] && t.source === source();
    },
  };
}


/* Derives the per-photo fields the wire format omits (name, folder, ext) and
   pairs Apple Live Photos — a still and a same-stem MOV beside each other. The
   wire stays small; deriving is a string split per photograph. */
export function hydrate(list) {
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

/* Camera RAW and HEIC: formats the webview cannot decode itself. RAW keeps the
   camera's embedded preview (ADR-0018), so controls that rewrite a file switch off
   rather than fail on the way to disk. Kept in step with `raw::RAW_EXT`. */
export const RAW_EXT = ["cr2", "cr3", "dng", "nef", "arw", "raf"];
export const isRaw = path => RAW_EXT.includes(String(path).split(".").pop().toLowerCase());

/* Formats the webview cannot decode itself — kept in step with
   `imageio::needs_conversion`. Zooming into one of these asks the backend's `?full=`
   route, which transcodes via `sips`, rather than the plain file the webview would
   choke on. */
export const CONVERT_EXT = [...RAW_EXT, "heic", "heif"];
export const needsConversion = path => CONVERT_EXT.includes(String(path).split(".").pop().toLowerCase());

/* A folder contains everything beneath it (ADR-0009), compared segment-wise so
   `Trip2` never reads as living inside `Trip` — the same rule as `in_folder` in
   the backend, and the two must agree or the grid and the counts disagree. */
export function inFolder(path, folder) {
  if (!folder) return true;
  return path === folder || path.startsWith(folder + "/");
}

export const DAY = ts => {
  const d = new Date(ts * 1000);
  return d.toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long", year: "numeric" });
};
export const TIME = ts => new Date(ts * 1000).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });

/* The justified-row grid engine (Flickr-style): rows are filled to a target
   height and scaled to the container, so aspect ratios survive. */
export function justify(photos, containerWidth, target = 200, gap = 3) {
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

export function labelColour(l) {
  return { red: "#f87171", orange: "#fb923c", yellow: "#fbbf24", green: "#4ade80",
           blue: "#60a5fa", purple: "#c4b5fd", grey: "#9ca3af" }[l] || "#888";
}

export function monthKey(ts) {
  const d = new Date(ts * 1000);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}`;
}

export const MONTHS = ["january","february","march","april","may","june",
                "july","august","september","october","november","december"];
export const LABEL_NAMES = ["red","orange","yellow","green","blue","purple","grey"];

export function parseQuery(q, people = [], albums = []) {
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

/* The structured half of a query — everything decidable from a photo's metadata.
   Shared by the omnibar and the Ask panel so the two can never disagree. */
export function matchesStructured(p, want) {
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

export function bytesLabel(bytes) {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let n = Math.max(0, Number(bytes) || 0), i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  return `${n >= 10 || i === 0 ? n.toFixed(0) : n.toFixed(1)} ${units[i]}`;
}

export const cosine = (a, b) => { let s = 0; for (let i = 0; i < a.length; i++) s += a[i] * b[i]; return s; };

/* Straightening over-zooms just enough that no blank corners survive — less, and
   it shows the user something they will not get. */
export function straightenZoom(w, h, degrees) {
  if (!w || !h || !degrees) return 1;
  const a = w / h;
  const rad = Math.abs(degrees) * Math.PI / 180;
  const sin = Math.abs(Math.sin(rad)), cos = Math.abs(Math.cos(rad));
  const v = Math.min(w / (2 * (a * cos + sin)), h / (2 * (a * sin + cos)));
  const kw = 2 * a * v;
  return kw > 0 ? w / kw : 1;
}

export function project(lon, lat) {
  const x = (lon + 180) / 360;
  const s = Math.sin(Math.max(-85.05, Math.min(85.05, lat)) * Math.PI / 180);
  const y = 0.5 - Math.log((1 + s) / (1 - s)) / (4 * Math.PI);
  return [x, y];
}
export function unproject(x, y) {
  const lon = x * 360 - 180;
  const lat = Math.atan(Math.sinh(Math.PI * (1 - 2 * y))) * 180 / Math.PI;
  return [lon, lat];
}

export const VERBS = {
  move:   ["move", "put", "file", "shift", "relocate"],
  rate:   ["rate", "star"],
  label:  ["label", "colour", "color", "tag"],
  delete: ["delete", "remove", "bin", "trash", "chuck"],
  show:   ["show", "find", "search", "list", "open"],
  save:   ["save"],
};

/** Words that mean "the photographs we were just talking about". */
export const REFERENTS = ["them", "these", "those", "it", "results", "there", "that"];

/** Prepositions that introduce a destination. */
export const INTO = ["to", "into", "in", "under"];

/* Words that are grammar rather than selection. "move all my august photos" selects
   exactly what "move august photos" selects, and leaving them in sends "my" to the
   scene search, where it means nothing and costs a confident wrong answer.

   Stripped only in the command path: a *question* keeps its phrasing intact, because
   "the beach" is a better phrase for the text encoder than "beach". */
export const FILLER = ["all", "my", "mine", "the", "a", "an", "any", "some", "every",
                "please", "just", "of", "from", "with"];

export function stripFiller(text) {
  const kept = text.split(/\s+/).filter(w => w && !FILLER.includes(w.toLowerCase()));
  // If filler was all there was, keep the original rather than produce an empty
  // selector that would be refused for the wrong reason.
  return kept.length ? kept.join(" ") : text.trim();
}

export function verbOf(word) {
  const w = word.toLowerCase().replace(/[.,!?]+$/, "");
  for (const [verb, words] of Object.entries(VERBS)) if (words.includes(w)) return verb;
  return null;
}

/** The closest verb we know, for an error that helps rather than just refuses.
    Edit distance rather than a shared prefix: the commonest typo is a transposition,
    and "mvoe" shares exactly one letter of prefix with "move". */
export function editDistance(a, b) {
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

export function nearestVerb(word) {
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
export function splitClauses(text) {
  return text
    .split(/\s+(?:and then|then|and)\s+/i)
    .map(t => t.trim())
    .filter(Boolean);
}

/* Parse one clause into { verb, rest, target, referent }.
   The verb must lead: "move august to X". A clause with no leading verb is a question,
   which is what the panel did before this existed. */
export function parseClause(text) {
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
export function isCommand(text) {
  return splitClauses(text).some(c => parseClause(c) !== null);
}
