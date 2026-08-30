# Contributing

Issues and pull requests are welcome.

## Before you start

Read [`AGENTS.md`](AGENTS.md). It is mostly a list of mistakes already made in this
repository, written down so nobody has to make them twice — a lock held across a slow
scan, a comment claiming an optimisation that was not there, a benchmark that measured
the wrong thing, a test that passed for the wrong reason.

The decisions that shaped the design are in [`docs/DECISIONS/`](docs/DECISIONS), and
what is true today is in [`docs/CURRENT/`](docs/CURRENT). If code and docs disagree, the
code wins and the docs get fixed.

## Running it

```bash
./tools/build-ffmpeg.sh                  # once: builds the bundled ffmpeg sidecar
cargo run -p openfoto-desktop            # the app, with the UI-verification bridge
cargo run -p openfoto-cli -- --help      # the command line
```

The sidecar is declared in `tauri.conf.json` as an `externalBin`, which Tauri treats as
required at build time, so the desktop app will not compile until that script has run
once. It needs `pkg-config` and a C toolchain (`nasm` too on x86_64) and tells you which
are missing rather than letting ffmpeg's configure fail obscurely.

Analysis in a debug build is roughly a hundred times slower than release — our own
pixel loops are unoptimised there. Use `--release` before concluding anything is slow.

## Checks

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
node apps/desktop/tests/grammar.test.mjs
```

`--all-targets` is not optional: without it clippy skips test targets and reports a
clean workspace while a test fails to compile. Gate the commit on the checks —
`clippy && test && git commit`, never `clippy; git commit`.

## What a good change looks like

- **Measure before optimising.** Twice in this project the obvious bottleneck was not
  the real one, and the fix went to the wrong place. `examples/bench`,
  `examples/passes` and `examples/throughput` exist so a claim can be checked.
- **Never silently change what a model outputs.** The thresholds in ADR-0003 and
  ADR-0008 were measured against particular values; `tests/analyze_pass.rs` compares
  against them so a faithful-looking refactor cannot invalidate both without failing.
- **Prefer refusing to guessing.** Search returns nothing below its threshold, faces
  are left unassigned when the match is not clear, and destructive operations preview
  first. A confidently wrong answer is worse than an honest shrug.
- **Look at the UI.** No interface change is done until a screenshot of it has been
  looked at. Five rendering bugs here were invisible in source and obvious on screen.
