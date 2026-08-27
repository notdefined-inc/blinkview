# Reference implementation

The Python scripts in `prototype/` are the original, working implementation of this
workflow, run against a real 2519-file library on 2026-08-27. They are **not** built,
tested, or shipped — they are here as executable documentation.

When porting a stage to Rust, the corresponding script is the specification of record,
and `tests/fixtures/*.csv` are its verified outputs on the real library.

| Stage | Scripts |
|---|---|
| perceptual hash + dedupe | `phash.py`, `thumbs.py`, `verify.py`, `tight.py`, `chain.py`, `plan.py` |
| face detect + embed | `scan.py`, `scan2.py`, `rootscan.py` |
| cluster + assign identity | `faceclust.py`, `label.py`, `match.py`, `people2.py`, `others.py`, `rootclust.py` |
| contact sheets (review UI) | `facesheet.py`, `sheet.py`, `sheet2.py`, `newsheet.py`, `psheet.py`, `rootsheet.py`, `bands.py` |
| rename + de-collision | `rename.py`, `decollide.py` |

`chain.py` is worth reading first: it is the diagnostic that proved single-linkage
clustering was chaining unrelated photos, which is the single most important correctness
finding behind ADR-0003.
