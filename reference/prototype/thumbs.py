import os, json, pickle
from PIL import Image
SRC = "/Volumes/Notdefined/Swissgreece"
S = 32
H = json.load(open("/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/hashes.json"))
th = {}
for i, r in enumerate(H):
    with Image.open(os.path.join(SRC, r["f"])) as im:
        im.draft("L", (128, 128)); im.load()
        th[r["f"]] = list(im.convert("L").resize((S, S), Image.LANCZOS).getdata())
    if (i+1) % 600 == 0: print(f"  ...{i+1}", flush=True)
pickle.dump(th, open("/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/thumbs.pkl","wb"))
print("thumbs:", len(th))
