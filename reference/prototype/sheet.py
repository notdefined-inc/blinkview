import pickle, os
from PIL import Image, ImageDraw
SP="/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/"
SRC="/Volumes/Notdefined/Swissgreece"
g=pickle.load(open(SP+"tight.pkl","rb"))
rows=g[:4]+g[len(g)//2:len(g)//2+3]+g[-3:]   # big, middle, and smallest clusters
TW,TH_,PAD,LBL=190,140,6,16
W=max(len(r) for r in rows)*(TW+PAD)+PAD
Hh=len(rows)*(TH_+PAD+LBL)+PAD
sheet=Image.new("RGB",(W,Hh),(24,24,28)); d=ImageDraw.Draw(sheet)
for ri,r in enumerate(rows):
    y=PAD+ri*(TH_+PAD+LBL)
    for ci,f in enumerate(r):
        with Image.open(os.path.join(SRC,f)) as im:
            im.draft("RGB",(TW*2,TH_*2)); im.load()
            im=im.convert("RGB"); im.thumbnail((TW,TH_),Image.LANCZOS)
        x=PAD+ci*(TW+PAD)
        sheet.paste(im,(x,y))
        d.text((x+2,y+TH_+2), f.replace(".jpg","")[9:], fill=(200,200,205))
        if ci==0: d.rectangle([x-2,y-2,x+im.width+1,y+im.height+1],outline=(90,200,120),width=2)
sheet.save(SP+"contact.jpg",quality=88)
print("rows (green box = the one KEPT):")
for r in rows: print(f"  [{len(r)}] {r[0]}  +{len(r)-1} more")
