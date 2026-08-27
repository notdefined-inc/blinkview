# ADR-0002: Rust core with `ort`, not Python

Date: 2026-08-27
Status: Accepted

## Context

The entire workflow was prototyped in Python (OpenCV 4.13 + onnxruntime + scikit-learn) and
it worked well — 2078 photos scanned in ~2 minutes, face embedding and clustering in ~90s.
Reusing it directly is the fastest route to a working CLI.

But the CLI is explicitly step one toward a desktop viewer, and the user's complaint about
every existing viewer is that they are slow and feel bad. Shipping a desktop app whose core
is Python means bundling an interpreter and native wheels (OpenCV alone is ~90MB), which is
the standard route to a large, slow-starting, fragile app — the exact failure being escaped.

`ort` (pykeio) provides mature Rust bindings to ONNX Runtime and is used in production by
SurrealDB, Bloop and Magika. It loads the **same** `yunet.onnx` and `sface.onnx` files
already validated in the prototype.

Toolchain present: rustc 1.93.1, cargo, node 22, pnpm, bun, and Tauri MCP tooling.

## Decision

`openfoto-core` is a Rust library crate. The CLI ships first; the Tauri v2 app later links
the same crate. ONNX inference goes through `ort`, reusing the prototype's model files
unchanged.

Rejected: **Python CLI now, rewrite later** — guarantees two implementations that drift, and
the rewrite lands exactly when the project is least able to afford it.
Rejected: **Python core with a Rust/Tauri shell** — keeps the packaging problem forever.

## Consequences

Good: one engine for CLI and GUI. Single static binary, no runtime to bundle. Fast startup,
which is most of what "feels fast" means. Rust's type system suits the safety invariants in
AGENTS.md (a dry-run plan can be a distinct type from an applied mutation).

Costly: v1 lands slower than a Python assembly of the existing scripts would. The team takes
on Rust ML plumbing, which is less ergonomic than numpy/sklearn — clustering and the pixel
metrics must be written by hand (they are ~200 lines and fully specified in ADR-0003).

Mitigation: the prototype scripts are kept as the executable reference implementation. The
expensive knowledge is the tuning, not the code, and it transfers as numbers.
