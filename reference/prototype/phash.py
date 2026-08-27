import os, json, sys
from PIL import Image

SRC = "/Volumes/Notdefined/Swissgreece"

def dhash(img, s=8):
    g = img.convert("L").resize((s+1, s), Image.LANCZOS)
    px = list(g.getdata())
    bits = 0
    for r in range(s):
        row = px[r*(s+1):(r+1)*(s+1)]
        for c in range(s):
            bits = (bits << 1) | (1 if row[c] > row[c+1] else 0)
    return bits

def ahash(img, s=8):
    g = img.convert("L").resize((s, s), Image.LANCZOS)
    px = list(g.getdata())
    avg = sum(px)/len(px)
    bits = 0
    for p in px:
        bits = (bits << 1) | (1 if p > avg else 0)
    return bits

files = sorted(f for f in os.listdir(SRC)
               if f.lower().endswith(".jpg") and not f.startswith("._"))
out, errs = [], []
for i, f in enumerate(files):
    p = os.path.join(SRC, f)
    try:
        with Image.open(p) as im:
            im.draft("L", (256, 256))          # fast DCT-domain JPEG downscale
            im.load()
            w, h = im.size
            out.append({"f": f, "d": dhash(im), "a": ahash(im),
                        "w": w, "h": h,
                        "sz": os.path.getsize(p), "mt": os.path.getmtime(p)})
    except Exception as e:
        errs.append((f, repr(e)))
    if (i+1) % 400 == 0:
        print(f"  ...{i+1}/{len(files)}", flush=True)

json.dump(out, open("/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/hashes.json","w"))
print(f"hashed={len(out)} errors={len(errs)}")
for f, e in errs[:10]:
    print("  ERR", f, e)
