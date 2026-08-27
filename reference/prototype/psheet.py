import pickle, cv2, numpy as np, os
SP="/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/"
SRC="/Volumes/Notdefined/Swissgreece"; LONG=1280
F=pickle.load(open(SP+"faces.pkl","rb")); A=pickle.load(open(SP+"assign.pkl","rb"))
P,asg,amb=A["P"],A["assign"],A["amb"]

def crop(i,T):
    f=F[i]; img=cv2.imread(os.path.join(SRC,f["file"]),cv2.IMREAD_REDUCED_COLOR_2)
    if img is None: return None
    h,w=img.shape[:2]; s=LONG/max(h,w)
    if s<1: img=cv2.resize(img,(int(w*s),int(h*s)),interpolation=cv2.INTER_AREA)
    x0,y0,bw,bh=f["box"]; m=int(bw*0.35)
    c=img[max(0,y0-m):y0+bh+m, max(0,x0-m):x0+bw+m]
    return None if c.size==0 else cv2.resize(c,(T,T),interpolation=cv2.INTER_AREA)

rows=[]
for k in ("Person1","Person2"):
    seed=sorted(P[k],key=lambda i:-F[i]["size"])
    st=max(1,len(seed)//12)
    rows.append((f"{k}  -  existing cluster ({len(P[k])} faces)", [(i,None) for i in seed[::st][:12]]))
    new=sorted(asg[k],key=lambda t:-t[1])
    rows.append((f"{k}  -  NEWLY ADDED ({len(new)})", [(i,s) for i,s in new][:12]))
rows.append((f"AMBIGUOUS - between two people ({len(amb)})",
             [(i,s1) for i,_,s1,_,_ in amb][:12]))

T=118; N=12; PAD=6; LBL=17
W=PAD+N*(T+PAD); H=PAD+len(rows)*(T+PAD+LBL+6)
sh=np.full((H,W,3),28,np.uint8)
for ri,(title,items) in enumerate(rows):
    y=PAD+ri*(T+PAD+LBL+6)
    cv2.putText(sh,title,(PAD,y+11),cv2.FONT_HERSHEY_SIMPLEX,0.42,(140,220,255),1,cv2.LINE_AA)
    for ci,(i,s) in enumerate(items):
        c=crop(i,T)
        if c is None: continue
        x=PAD+ci*(T+PAD); yy=y+LBL
        sh[yy:yy+T,x:x+T]=c
        if s is not None:
            cv2.putText(sh,f"{s:.2f}",(x+2,yy+T-4),cv2.FONT_HERSHEY_SIMPLEX,0.42,(120,230,150),1,cv2.LINE_AA)
cv2.imwrite(SP+"persons.jpg",sh,[cv2.IMWRITE_JPEG_QUALITY,92])

ph1={F[i]["file"] for i in set(P["Person1"])|{i for i,_ in asg["Person1"]}}
ph2={F[i]["file"] for i in set(P["Person2"])|{i for i,_ in asg["Person2"]}}
print(f"Person1 photos={len(ph1)}  Person2 photos={len(ph2)}  in BOTH={len(ph1&ph2)}")
pickle.dump({"ph1":sorted(ph1),"ph2":sorted(ph2)},open(SP+"pphotos.pkl","wb"))
