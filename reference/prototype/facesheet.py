import pickle, cv2, numpy as np, os
from collections import defaultdict
SP="/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/"
SRC="/Volumes/Notdefined/Swissgreece"; LONG=1280
F=pickle.load(open(SP+"faces.pkl","rb")); lab=pickle.load(open(SP+"labels.pkl","rb"))
by=defaultdict(list)
for i,l in enumerate(lab): by[l].append(i)
order=sorted(by, key=lambda k:-len(by[k]))[:12]

T=132; N=9; PAD=8; LBL=26
W=PAD+N*(T+PAD)+230; H=PAD+len(order)*(T+PAD+LBL)
sheet=np.full((H,W,3),28,np.uint8)
for ri,k in enumerate(order):
    idx=sorted(by[k], key=lambda i:-F[i]["size"])
    step=max(1,len(idx)//N); pick=idx[::step][:N]
    y=PAD+ri*(T+PAD+LBL)
    for ci,i in enumerate(pick):
        f=F[i]
        img=cv2.imread(os.path.join(SRC,f["file"]), cv2.IMREAD_REDUCED_COLOR_2)
        h,w=img.shape[:2]; s=LONG/max(h,w)
        if s<1: img=cv2.resize(img,(int(w*s),int(h*s)),interpolation=cv2.INTER_AREA)
        x0,y0,bw,bh=f["box"]; m=int(bw*0.35)
        crop=img[max(0,y0-m):y0+bh+m, max(0,x0-m):x0+bw+m]
        if crop.size==0: continue
        crop=cv2.resize(crop,(T,T),interpolation=cv2.INTER_AREA)
        x=PAD+ci*(T+PAD); sheet[y:y+T, x:x+T]=crop
    cv2.putText(sheet,f"CLUSTER #{k}  -  {len(by[k])} photos",(PAD,y+T+19),
                cv2.FONT_HERSHEY_SIMPLEX,0.55,(120,230,150),1,cv2.LINE_AA)
cv2.imwrite(SP+"people.jpg",sheet,[cv2.IMWRITE_JPEG_QUALITY,90])
print("rows:", [(f"#{k}",len(by[k])) for k in order])
