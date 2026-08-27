import pickle, os, csv
from PIL import Image, ImageFilter, ImageStat
SP="/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/"
SRC="/Volumes/Notdefined/Swissgreece"
groups=pickle.load(open(SP+"tight.pkl","rb"))
LAP=ImageFilter.Kernel((3,3),[0,1,0,1,-4,1,0,1,0],scale=1,offset=128)

def sharp(f):
    with Image.open(os.path.join(SRC,f)) as im:
        im.draft("L",(512,512)); im.load()
        g=im.convert("L")
        g.thumbnail((512,512), Image.LANCZOS)
        return ImageStat.Stat(g.filter(LAP)).var[0]

rows=[]; keeps=0; moves=0
for gi,g in enumerate(groups):
    sc={f: sharp(f) for f in g}
    keep=max(sc, key=sc.get)
    keeps+=1
    for f in sorted(g):
        act = "KEEP" if f==keep else "MOVE"
        if act=="MOVE": moves+=1
        rows.append({"group":gi,"action":act,"file":f,"sharpness":round(sc[f],1)})
    if (gi+1)%60==0: print(f"  ...{gi+1}/{len(groups)}",flush=True)

with open(SP+"plan.csv","w",newline="") as fh:
    w=csv.DictWriter(fh,fieldnames=["group","action","file","sharpness"]); w.writeheader(); w.writerows(rows)
print(f"\ngroups={keeps} keep={keeps} move={moves} total={len(rows)}")
