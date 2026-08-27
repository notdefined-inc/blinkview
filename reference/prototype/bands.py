import pickle, cv2, numpy as np, os, collections
SP="/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/"
SRC="/Volumes/Notdefined/Swissgreece"; LONG=1280
O=pickle.load(open(SP+"root_faces.pkl","rb"))

# effect of also demanding a confident detection
for sc in (0.55,0.75,0.90):
    b=collections.Counter()
    for o in O:
        r=max([f["ratio"] for f in o["faces"] if f["score"]>=sc], default=0.0)
        b["none" if r==0 else "<4%" if r<0.04 else "4-7%" if r<0.07 else "7-12%" if r<0.12 else ">=12%"]+=1
    print(f"score>={sc}: " + "  ".join(f"{k}={b[k]}" for k in ["none","<4%","4-7%","7-12%",">=12%"]))

BANDS=[("2-4%",0.02,0.04),("4-7%",0.04,0.07),("7-12%",0.07,0.12),(">=12%",0.12,9.0)]
TW,TH_,PAD,LBL=210,158,6,18
N=6
W=PAD+N*(TW+PAD); H=PAD+len(BANDS)*(TH_+PAD+LBL+6)
sh=np.full((H,W,3),28,np.uint8)
for ri,(name,lo,hi) in enumerate(BANDS):
    sel=[o for o in O if lo<=max([f["ratio"] for f in o["faces"] if f["score"]>=0.75],default=0)<hi]
    sel.sort(key=lambda o:-o["maxr"])
    step=max(1,len(sel)//N); pick=sel[::step][:N]
    y=PAD+ri*(TH_+PAD+LBL+6)
    cv2.putText(sh,f"{name}   ({len(sel)} photos, score>=0.75)",(PAD,y+12),
                cv2.FONT_HERSHEY_SIMPLEX,0.45,(140,220,255),1,cv2.LINE_AA)
    for ci,o in enumerate(pick):
        img=cv2.imread(os.path.join(SRC,o["file"]),cv2.IMREAD_REDUCED_COLOR_2)
        if img is None: continue
        h,w=img.shape[:2]; s=LONG/max(h,w)
        if s<1: img=cv2.resize(img,(int(w*s),int(h*s)),interpolation=cv2.INTER_AREA)
        for f in o["faces"]:
            if f["score"]<0.75: continue
            x0,y0,bw,bh=f["box"]; cv2.rectangle(img,(x0,y0),(x0+bw,y0+bh),(80,240,120),3)
        ih,iw=img.shape[:2]; sc2=min(TW/iw,TH_/ih)
        img=cv2.resize(img,(int(iw*sc2),int(ih*sc2)),interpolation=cv2.INTER_AREA)
        x=PAD+ci*(TW+PAD); yy=y+LBL
        sh[yy:yy+img.shape[0], x:x+img.shape[1]]=img
cv2.imwrite(SP+"bands.jpg",sh,[cv2.IMWRITE_JPEG_QUALITY,90])
print("\nwrote bands.jpg")
