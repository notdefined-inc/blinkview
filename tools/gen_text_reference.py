#!/usr/bin/env python3
"""Regenerate tests/fixtures/clip_text_reference.json.

The Rust text encoder is checked against this file (ADR-0004 parity rule). Run this
only when the text model itself changes, and update the sha256 in faces/fetch.rs to
match — the parity test refuses to run if the two disagree.

    pip install onnxruntime tokenizers numpy
    python3 tools/gen_text_reference.py
"""
import hashlib, json, pathlib, sys
import numpy as np, onnxruntime as ort
from tokenizers import Tokenizer

MODELS = pathlib.Path.home() / ".cache/blinkview/models"
OUT = pathlib.Path(__file__).resolve().parent.parent / "tests/fixtures/clip_text_reference.json"
CTX = 77  # CLIP's fixed context; the encoder rejects anything shorter

QUERIES = [
    "a night sky", "a dog on a beach", "snowy mountains", "a church",
    "a laptop computer", "the sea", "a selfie of a man", "food on a plate",
    "a city street at night", "green trees", "a bridge over water",
    "people sitting together",
    "",  # empty: must still produce a unit vector rather than NaN
    "a very long query about mountains rivers forests and the wide open sky above them all",
]


def main() -> int:
    model = MODELS / "clip-text.onnx"
    if not model.exists():
        print(f"missing {model} — run `blinkview models fetch` first", file=sys.stderr)
        return 1
    tok = Tokenizer.from_file(str(MODELS / "clip-tokenizer.json"))
    sess = ort.InferenceSession(str(model), providers=["CPUExecutionProvider"])
    name = sess.get_inputs()[0].name

    out = {}
    for q in QUERIES:
        ids = tok.encode(q).ids[:CTX]
        ids += [0] * (CTX - len(ids))
        v = sess.run(None, {name: np.array([ids], dtype=np.int64)})[0][0]
        out[q] = [float(x) for x in v / np.linalg.norm(v)]

    OUT.write_text(json.dumps({
        "note": "MobileCLIP-S0 fp32 text tower, L2-normalised. "
                "Regenerate with tools/gen_text_reference.py if the model changes.",
        "model": "clip-text.onnx",
        "model_sha256": hashlib.sha256(model.read_bytes()).hexdigest(),
        "onnxruntime": ort.__version__,
        "embeddings": out,
    }))
    print(f"wrote {OUT} — {len(out)} queries")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
