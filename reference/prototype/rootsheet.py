import pickle, cv2, numpy as np, os
SP="/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/"
SRC="/Volumes/Notdefined/Swissgreece"; LONG=1920
R=pickle.load(open(SP+"rootemb.pkl","rb")); C=pickle.load(open(SP+"rootclust.pkl","rb"))
by,rows=C["by"],{r["cluster"]:r for r in C["rows"]}
show=[k for k in C["order"] if len(by[k])>=2]
T=104; N=8; PAD=5; LBL=15; SIDE=250
W=SIDE+PAD+N*(T+PAD); H=PAD+len(show)*(T+PAD+LBL)
sh=np.full((H,W,3),26,np.uint8)
cache={}
for ri,k in enumerate(show):
    idx=sorted(by[k],key=lambda i:-R[i]["size"])
    st=max(1,len(idx)//N); pick=idx[::st][:N]
    y=PAD+ri*(T+PAD+LBL)
    r=rows[k]
    strong = r["bs"]>=0.70 and r["bs"]-r["ss"]>=0.10
    col=(120,235,150) if strong else (110,190,245) if r["bs"]>=0.55 else (130,130,200)
    cv2.putText(sh,f"#{k}  ({r['photos']} photos)",(PAD,y+18),
                cv2.FONT_HERSHEY_SIMPLEX,0.52,(235,235,235),1,cv2.LINE_AA)
    tag=f"-> {r['best']} {r['bs']:.2f}" if r["bs"]>=0.55 else "-> unclear"
    cv2.putText(sh,tag,(PAD,y+40),cv2.FONT_HERSHEY_SIMPLEX,0.52,col,1,cv2.LINE_AA)
    cv2.putText(sh,f"(2nd {r['second']} {r['ss']:.2f})",(PAD,y+59),
                cv2.FONT_HERSHEY_SIMPLEX,0.40,(150,150,150),1,cv2.LINE_AA)
    for ci,i in enumerate(pick):
        f=R[i]
        if f["file"] not in cache:
            im=cv2.imread(os.path.join(SRC,f["file"]))
            h,w=im.shape[:2]; s=LONG/max(h,w)
            if s<1: im=cv2.resize(im,(int(w*s),int(h*s)),interpolation=cv2.INTER_AREA)
            cache.clear(); cache[f["file"]]=im
        im=cache[f["file"]]
        x0,y0,bw,bh=f["box"]; m=int(bw*0.4)
        c=im[max(0,y0-m):y0+bh+m, max(0,x0-m):x0+bw+m]
        if c.size==0: continue
        x=SIDE+PAD+ci*(T+PAD)
        sh[y:y+T,x:x+T]=cv2.resize(c,(T,T),interpolation=cv2.INTER_AREA)
cv2.imwrite(SP+"rootclusters.jpg",sh,[cv2.IMWRITE_JPEG_QUALITY,92])
print(f"rendered {len(show)} clusters covering "
      f"{len({R[i]['file'] for k in show for i in by[k]})} photos")
