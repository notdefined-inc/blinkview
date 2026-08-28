// Grammar tests for the command layer (ADR-0012, spec criteria 1, 3, 5-8, 10).
//
// app.js is a browser script, not a module, so the functions under test are extracted
// by evaluating it with the browser globals it touches stubbed out. That keeps the
// shipping file free of test scaffolding, which would otherwise be dead weight in
// every window.
import { readFileSync } from "node:fs";
import { strict as assert } from "node:assert";

const src = readFileSync(new URL("../dist/app.js", import.meta.url), "utf8");

// Everything app.js reaches for at load time. It registers handlers on elements and
// listens for Tauri events; none of that is needed to parse a sentence.
const noop = () => {};
const stubEl = new Proxy({}, {
  get: (_, k) => (k === "value" ? "" : k === "hidden" ? false
    : k === "classList" ? { add: noop, remove: noop, contains: () => false }
    : k === "style" ? {} : k === "children" ? [] : stubFn),
  set: () => true,
});
const stubFn = new Proxy(noop, { get: () => stubFn, apply: () => stubEl });

const sandbox = {
  window: { __TAURI__: { core: { invoke: async () => [] }, event: { listen: noop } },
            addEventListener: noop, matchMedia: () => ({ matches: false, addEventListener: noop }) },
  document: {
    querySelector: () => stubEl, querySelectorAll: () => [],
    createElement: () => stubEl, addEventListener: noop, body: stubEl,
    documentElement: stubEl,
  },
  localStorage: { getItem: () => null, setItem: noop, removeItem: noop },
  addEventListener: noop, setTimeout: noop, clearTimeout: noop, setInterval: noop,
  requestAnimationFrame: noop, performance: { now: () => 0 },
  console, Image: function () {}, Event: function () {}, IntersectionObserver: function () {
    return { observe: noop, disconnect: noop };
  },
};

const exported = ["parseClause", "splitClauses", "verbOf", "nearestVerb", "isCommand",
                  "parseQuery", "inFolder", "sectionFor", "stripFiller"];
const fn = new Function(
  ...Object.keys(sandbox),
  `${src}\n;return { ${exported.join(", ")} };`
);
const G = fn(...Object.values(sandbox));

// --- verbs -----------------------------------------------------------------

assert.equal(G.verbOf("move"), "move");
assert.equal(G.verbOf("Move"), "move", "case must not matter");
assert.equal(G.verbOf("chuck"), "delete", "synonyms are a table, not a model");
assert.equal(G.verbOf("bin"), "delete");
assert.equal(G.verbOf("photos"), null, "a noun is not a verb");

// A helpful error beats a flat refusal (criterion 6).
assert.equal(G.nearestVerb("mov"), "move");
assert.equal(G.nearestVerb("delet"), "delete");
// Transposition is the commonest typo and a shared-prefix score cannot see it.
assert.equal(G.nearestVerb("mvoe"), "move");
assert.equal(G.nearestVerb("dlete"), "delete");
// Real nouns must never be "corrected" into verbs.
assert.equal(G.nearestVerb("photos"), null);
assert.equal(G.nearestVerb("august"), null);
assert.equal(G.nearestVerb("mountains"), null);

// --- clause shape ----------------------------------------------------------

let c = G.parseClause("move my august photos to Trip");
assert.equal(c.verb, "move");
assert.equal(c.rest, "august photos", "'my' is stripped as filler");
assert.equal(c.target, "Trip");

// Nested destinations survive (criterion 1).
c = G.parseClause("move august to Trip/Greece Day3");
assert.equal(c.target, "Trip/Greece Day3");

// The *last* preposition wins, so a selector containing one still works.
c = G.parseClause("move photos in august to Trip");
assert.equal(c.target, "Trip");
assert.equal(c.rest, "photos in august");  // "in" here is not filler, it is a selector word

// No verb means it is a question, not a command (criterion 10).
assert.equal(G.parseClause("photos of a dog"), null);
assert.equal(G.isCommand("photos of a dog"), false);
assert.equal(G.isCommand("move august to Trip"), true);

// --- referents (criteria 7, 8) ---------------------------------------------

c = G.parseClause("rate them 5 stars");
assert.equal(c.verb, "rate");
assert.equal(c.value, 5, "the rating is lifted out of the selector");
assert.equal(c.referent, "them", "what remains is the referent");
assert.equal(c.rest, "", "and nothing is left to search for");

c = G.parseClause("rate the august photos 4 stars");
assert.equal(c.value, 4);
assert.equal(c.rest, "august photos");

c = G.parseClause("label them red");
assert.equal(c.value, "red");
assert.equal(c.referent, "them");

c = G.parseClause("label the greece photos blue");
assert.equal(c.value, "blue");
assert.equal(c.rest, "greece photos");

c = G.parseClause("delete them");
assert.equal(c.referent, "them");

c = G.parseClause("move them to Trip");
assert.equal(c.referent, "them");
assert.equal(c.target, "Trip");

// --- clause splitting ------------------------------------------------------

assert.deepEqual(G.splitClauses("move a to X and move b to Y"),
                 ["move a to X", "move b to Y"]);
assert.deepEqual(G.splitClauses("show august then rate them 5 stars"),
                 ["show august", "rate them 5 stars"]);
assert.equal(G.splitClauses("move august to Trip").length, 1);

// --- the empty selector must be refused (criterion 5) ----------------------

c = G.parseClause("move to Trip");
assert.equal(c.rest, "", "no selector at all");
assert.equal(c.empty, true, "must be flagged empty so the caller refuses it");
assert.equal(c.referent, null, "an empty selector is NOT an implicit referent — that is how\n  'move to Trip' silently planned the whole previous result");

// An explicit referent is different: it deliberately borrows the last answer.
c = G.parseClause("move them to Trip");
assert.equal(c.empty, false);
assert.equal(c.referent, "them");

// --- one selector language (criterion 10) ----------------------------------

const q = G.parseQuery("august 2026 4stars+", [], []);
assert.equal(q.want.month, 8);
assert.equal(q.want.year, 2026);
assert.equal(q.want.minRating, 4);

// --- filler words are grammar, not selection -------------------------------

assert.equal(G.stripFiller("all my august photos"), "august photos");
assert.equal(G.stripFiller("the beach"), "beach");
// Filler-only must not collapse to an empty selector, which would be refused for
// entirely the wrong reason.
assert.equal(G.stripFiller("all of them"), "them");

c = G.parseClause("move all my august photos to Sorted");
assert.equal(c.rest, "august photos", "'all my' is grammar, not a search term");
assert.equal(c.target, "Sorted");

// A bare referent survives filler stripping.
c = G.parseClause("delete all of them");
assert.equal(c.referent, "them");
assert.equal(c.rest, "");

// --- a near miss is only a near miss when it looks like an instruction -------

// "movie night photos" must stay a search, even though "movie" is two letters from
// "move" — the absence of a destination is what keeps it one.
assert.equal(G.nearestVerb("movie"), "move", "close enough to suggest...");
assert.equal(G.isCommand("movie night photos"), false, "...but still not a command");
assert.equal(G.isCommand("mvoe the greece photos to X"), false,
  "a typo'd verb is not a command either — the caller offers a correction");

console.log("grammar: all assertions passed");
