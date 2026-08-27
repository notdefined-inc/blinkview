# Status

_Last updated: 2026-08-27_

## Current work
Phase 0 (bootstrap). Repo scaffolded; ADRs and the v1 spec are written. No implementation
code yet — `openfoto-core` and `openfoto-cli` are empty skeletons.

## Known issues
Nothing shipped yet, so nothing is broken yet.

## Recently shipped
Nothing. The repo was created 2026-08-27.

## Origin
The workflow this tool automates was first executed by hand against a real 2519-file
library (`/Volumes/Notdefined/Swissgreece`): burst-duplicate detection, face clustering
into per-person folders, a scenery split, and a bulk date-time rename. That library and
its CSV manifests are the ground truth for the regression tests. See
docs/DECISIONS/ADR-0003-tuned-thresholds.md for the values that were validated there and
the failures that produced them.
