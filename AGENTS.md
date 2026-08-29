# Repository Guardrails

Global contract applies (~/.codex/AGENTS.md). Repo specifics below override it.

## Read order
1. docs/CURRENT/STATUS.md
2. docs/CURRENT/ARCHITECTURE.md
3. docs/CURRENT/ROADMAP.md
4. docs/DECISIONS/*
5. docs/SPECS/active/*

## Project rules

**The vault invariant — this is the product.**
- Photos on disk are the only source of truth. `.openfoto/` is 100% derived and must be
  safe to `rm -rf` at any moment; `openfoto scan` rebuilds it fully. Never store anything
  in the index that cannot be recomputed from the photos themselves.
- File identity is the BLAKE3 content hash, never the path. Users rename and move folders
  in Finder while the tool is running; that is normal behavior, not an error state.
- Every mutation writes a journal entry to `.openfoto/journal/` before touching the disk,
  and `openfoto undo` must restore the exact prior tree.

**Safety rules (each was learned by getting it wrong — see ADR-0003).**
- Clustering is complete-linkage. Single-linkage chaining is a correctness bug, not a tuning knob.
- A perceptual-hash match is a *candidate*, never a conclusion — always confirm with pixel comparison.
- Identity assignment is discriminative (nearest-of-many-people), never a bare threshold.
- No destructive command runs without a dry-run path producing the full plan first.
- Filenames must be unique across the whole library, not per-folder.
- Low confidence never auto-moves. Unclear items stay where they are and get reported.

**Platform reality**
- exFAT is a first-class target: reject `" * / : < > ? \ |` in filenames, and carry
  AppleDouble `._` sidecars with their parent file on every move/rename.

## Build / test / run
- `cargo build --workspace`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
  **`--all-targets` is not optional.** Without it clippy skips test targets and will
  report a clean workspace while a test fails to compile.
- `cargo run -p openfoto-cli -- <cmd>` · `cargo run -p openfoto-desktop`
- `node apps/desktop/tests/grammar.test.mjs` — the command grammar. The frontend has no
  build step, so this evaluates `dist/app.js` with browser globals stubbed rather than
  importing it. **`cargo test` does not run it**; run it after touching the parser.

## Verifying
Never report a check as passing from filtered output. `cmd | grep ...; echo "clean"`
prints "clean" whatever happened — that pattern has already caused a commit on top of a
failing clippy run here. Branch on the exit code:

    cargo clippy --workspace --all-targets -q -- -D warnings \
      && echo PASS || { echo FAIL; ... }

Running the check and the commit in **one** shell command repeats the same mistake by a
different route: the commit runs whatever the check reported. Gate it —
`clippy && test && git commit`, never `clippy; git commit`.

## Mistakes already made here — do not repeat them

Each of these cost real time in this project, and every one produced a *confident wrong
answer* rather than an error. They are listed because they recur.

**1. Running a stale binary.** Happened twice. `cargo build -p openfoto-core` does not
rebuild `openfoto-cli`; `cargo build --workspace` does not build examples. Both times a
fix looked like it had failed ("face crops cached: 0", "22 faces found") when the code
was correct and the binary was old. **Build the exact artefact you are about to run**,
or `cargo build --workspace --all-targets`.

**2. A check that cannot fail.** `cargo clippy | grep error; echo "clean"` prints
"clean" whatever happened — this produced a commit on top of a failing clippy run.
Likewise `cargo clippy` without `--all-targets` skips test targets and reported a green
workspace while a test target would not compile. **Branch on the exit code**:
`cmd && echo PASS || { echo FAIL; ... }`.

**3. Trusting a fixture you generated.** A 20,000-file test library contained only 768
distinct images, because the generator's colour offset wrapped at 256. Thumbnails are
content-addressed, so "only 768 built" was *correct* — and nearly triggered an
optimisation of code that was working. A synthetic "blurred" frame was likewise so
unlike its sharp twin that it failed to group, and the instinct was to loosen the
threshold rather than fix the fixture. **Verify the fixture has the property you think
it has before drawing conclusions from it.**

**4. Measuring the wrong moment.** Reading a computed CSS transform immediately after
setting it returns the *start* of a pending transition, not the value. This produced two
false conclusions in a row on the same feature: "rotation is broken" (it was working),
then "my fix broke it" (it had not). The inline style was correct both times. Related:
`getBoundingClientRect` on a rotated element returns the enclosing box of the rotated
shape, not the element's frame — use `offsetWidth`/`offsetTop` when you want the layout
box. **When a measurement disagrees with the code, suspect the measurement.**

**5. Assuming where the time goes.** The dedupe bottleneck was diagnosed as the O(n^2)
pair scan and a multi-index-hashing rewrite was planned. The real cost was two lines
inside `rmse`, which normalised both thumbnails and allocated on *every* call. Fixing
that gave 67x without touching the pair count. **Read the hot function before
optimising the algorithm.**

**6. Shell metacharacters in commit messages.** Backticks inside a double-quoted
`git commit -m` are command substitution; `` `openfoto thumbs` `` executed and left a
hole in the message. **Write multi-paragraph messages to a file and use `-F`.**

**7. Touching a database another process holds.** Deleting `index.sqlite-wal` while the
app was running corrupted the index. The vault is disposable so recovery was cheap, but
`people.json` is *not* recomputable — back it up before rebuilding a vault.

**8. Judging UI from code.** Five rendering bugs in this project were invisible in
source and obvious in a screenshot, including a CSS `display` rule silently overriding
the `hidden` attribute. **Screenshot the running window; the DOM is not the paint.**

**9. Applying a lesson to one half of a pair.** ADR-0008 rejected the quantised *vision*
model because quantisation compressed the embedding space, then quantised the *text*
model in the same document, reasoning it "runs once per query, where quantisation costs
little". That conflated **cost** with **error**: running once per query makes it cheap in
time and says nothing about whether the vector is right. int8 text diverged from fp32 by
cosine 0.89, nearly doubled the matches clearing the threshold, and was not reproducible
between onnxruntime builds. **When you reject a technique for one component, state
explicitly why the sibling component is exempt — or apply it there too.**

**10. Cross-runtime parity is a property of the model, not just the code.** A dynamically
quantised graph (`MatMulInteger`, `DynamicQuantizeLinear`) derives its scale from each
input's own activation range, so two onnxruntime builds give input-dependent differences
— some inputs identical, others off by cosine 0.99. That pattern (a few exact matches
mixed with near-misses) means a kernel or fusion difference, not float noise; uniform
small error would mean float noise. **If a parity test fails on some inputs and passes
exactly on others, suspect quantisation before suspecting your code.**

**11. Recording the change after making it.** `Plan::apply` moved files and wrote the
journal last, so when a plan label containing `/` made the journal filename invalid,
twenty-three photographs moved with no journal entry — unreachable by undo, while the UI
reported failure. The order now is files, journal, metadata, and any failure rolls the
files back. **In a system whose contract is reversibility, the record is part of the
operation, not a receipt printed afterwards.**

**12. Values that become filenames.** Anything user- or code-supplied that ends up in a
path needs sanitising at the point it becomes a path, not at every call site. The label
was fine as a label; it was only wrong as a filename.

**13. A comment claiming an optimisation that is not there.** `thumbs.rs` said decoding
"uses the JPEG decoder's DCT downscaling" — it never did; that trick lives only in
`imagesig`. The comment sent a whole investigation to the wrong place. **Before
optimising, check the claim in the comment against the code.**

**14. Measure the library you actually link.** `image` 0.25 already decodes JPEG with
zune-jpeg, so "switch to a faster decoder" was worth nothing, and turbojpeg measured
identical (60.4 ms vs 60.1 ms on 12 MP) — a C dependency for zero gain. The win was
elsewhere entirely: not decoding at all, by using the preview the camera already
embedded. **Benchmark the alternative before adopting it, and benchmark what you have
before assuming it is slow.**

**15. A lock held across the slow part.** `open_lib` scanned a library while holding the
*registry* mutex — the one every command for every library needs — so adding a 25GB
folder froze the whole window until it finished. Per-library locks did not help, because
the contention was one level up. **Look up the handle under the lock, release it, then
do the work.**

**16. One global banner for concurrent work.** A single `liveToast` was repainted by
whatever progress event arrived, so two operations at once fought over it and a
background scan narrated itself over the thing the user was doing. Progress events now
carry the library they belong to, and the banner ignores the ones that are not its own.

## Traps hit here before
- cargo silently ignores `cfg(debug_assertions)` when selecting dependencies. Dev-only
  dependencies must be real features, or they ship in release builds.
- Synchronous `#[tauri::command]` functions run on the UI thread. Anything touching the
  disk must be `async` or the window freezes.
- A `display` value in a class rule outranks the `hidden` attribute; overlays need an
  explicit `[hidden]{display:none!important}`.
- `backdrop-filter` on a full-screen container can stop WKWebView painting its children
  altogether — the element measures correctly and never appears.
- Theme overrides can outweigh state selectors. `:root[data-theme="light"] .fopt` is
  (0,3,0) and beats `.fopt[aria-pressed="true"]` at (0,2,0), so every selected filter
  chip rendered as white-on-white. **A theme rule that sets `background` must exclude
  the states that also set one** — `:not([aria-pressed="true"])`.
- `replaceChildren` stringifies `null` children into literal "null" text nodes — filter
  arrays before spreading them in (the `el()` helper already skips nulls).
- The Tauri app embeds `dist/` at compile time; editing frontend files needs
  `cargo build -p openfoto-desktop` and an app restart — `location.reload()` still
  serves the stale bundle.
