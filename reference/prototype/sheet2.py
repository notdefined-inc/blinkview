import csv, os
from collections import defaultdict
from PIL import Image, ImageDraw
SP="/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/"
SRC="/Volumes/Notdefined/Swissgreece"
G=defaultdict(list)
for r in csv.DictReader(open(SP+"plan.csv")): G[int(r["group"])].append(r)
big=sorted(G.values(), key=len, reverse=True)[:6]
TW,TH_,PAD,LBL=200,150,6,16
W=max(len(r) for r in big)*(TW+PAD)+PAD; Hh=len(big)*(TH_+PAD+LBL)+PAD
s=Image.new("RGB",(W,Hh),(24,24,28)); d=ImageDraw.Draw(s)
for ri,grp in enumerate(big):
    grp=sorted(grp,key=lambda r:-float(r["sharpness"]))
    y=PAD+ri*(TH_+PAD+LBL)
    for ci,r in enumerate(grp):
        with Image.open(os.path.join(SRC,r["file"])) as im:
            im.draft("RGB",(TW*2,TH_*2)); im.load(); im=im.convert("RGB"); im.thumbnail((TW,TH_),Image.LANCZOS)
        x=PAD+ci*(TW+PAD); s.paste(im,(x,y))
        col=(90,220,120) if r["action"]=="KEEP" else (190,190,195)
        d.text((x+2,y+TH_+2), f"{r['action']} s={r['sharpness']}", fill=col)
        if r["action"]=="KEEP": d.rectangle([x-2,y-2,x+im.width+1,y+im.height+1],outline=(90,220,120),width=3)
s.save(SP+"keepers.jpg",quality=88)
print("sorted left->right by sharpness; green = kept")
