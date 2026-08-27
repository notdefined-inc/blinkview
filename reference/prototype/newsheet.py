import pickle, cv2, numpy as np, os
SP="/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/"
SRC="/Volumes/Notdefined/Swissgreece"; LONG=1280
F=pickle.load(open(SP+"faces.pkl","rb")); rows=pickle.load(open(SP+"unl.pkl","rb"))
hit=[r for r in rows if r[1]>=0.40 and r[1]>r[2]]
hit.sort(key=lambda r:-r[1])
T=126; N=10; PAD=6; LBL=15
R=(len(hit)+N-1)//N
W=PAD+N*(T+PAD); H=PAD+R*(T+PAD+LBL)
sh=np.full((H,W,3),28,np.uint8)
for n,(i,a,b,m) in enumerate(hit):
    f=F[i]; r_,c_=divmod(n,N)
    img=cv2.imread(os.path.join(SRC,f["file"]),cv2.IMREAD_REDUCED_COLOR_2)
    h,w=img.shape[:2]; s=LONG/max(h,w)
    if s<1: img=cv2.resize(img,(int(w*s),int(h*s)),interpolation=cv2.INTER_AREA)
    x0,y0,bw,bh=f["box"]; mg=int(bw*0.35)
    crop=img[max(0,y0-mg):y0+bh+mg, max(0,x0-mg):x0+bw+mg]
    if crop.size==0: continue
    y=PAD+r_*(T+PAD+LBL); x=PAD+c_*(T+PAD)
    sh[y:y+T,x:x+T]=cv2.resize(crop,(T,T),interpolation=cv2.INTER_AREA)
    col=(120,230,150) if a>=0.5 else (110,190,240)
    cv2.putText(sh,f"{a:.2f}",(x+2,y+T+11),cv2.FONT_HERSHEY_SIMPLEX,0.42,col,1,cv2.LINE_AA)
cv2.imwrite(SP+"newmatches.jpg",sh,[cv2.IMWRITE_JPEG_QUALITY,92])
print(f"{len(hit)} new faces, {len({F[i]['file'] for i,_,_,_ in hit})} photos")
print("similarity range:", f"{hit[-1][1]:.2f} .. {hit[0][1]:.2f}")
