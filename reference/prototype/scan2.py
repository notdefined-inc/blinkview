import cv2, os, pickle, time, numpy as np
SP="/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/"
SRC="/Volumes/Notdefined/Swissgreece"; LONG=1280
det=cv2.FaceDetectorYN.create(SP+"models/yunet.onnx","",(320,320),0.55,0.3,5000)
files=sorted(f for f in os.listdir(SRC) if f.lower().endswith(".jpg") and not f.startswith("._"))
out=[]; t0=time.time()
for i,f in enumerate(files):
    img=cv2.imread(os.path.join(SRC,f),cv2.IMREAD_REDUCED_COLOR_2)
    if img is None: img=cv2.imread(os.path.join(SRC,f))
    if img is None: out.append({"file":f,"n":0,"maxr":0.0,"faces":[]}); continue
    h,w=img.shape[:2]; s=LONG/max(h,w)
    if s<1: img=cv2.resize(img,(int(w*s),int(h*s)),interpolation=cv2.INTER_AREA)
    h,w=img.shape[:2]
    det.setInputSize((w,h))
    n,fc=det.detect(img)
    fs=[]
    if fc is not None:
        for r in fc:
            fs.append({"w":float(r[2]),"h":float(r[3]),"score":float(r[14]),
                       "box":r[:4].astype(int).tolist(),
                       "ratio":float(r[2])/w})          # face width / image width
    out.append({"file":f,"n":len(fs),"maxr":max([x["ratio"] for x in fs],default=0.0),
                "dims":(w,h),"faces":fs})
    if (i+1)%300==0: print(f"  {i+1}/{len(files)}  {time.time()-t0:.0f}s",flush=True)
pickle.dump(out,open(SP+"root_faces.pkl","wb"))
import collections
print(f"\nphotos={len(out)}  with>=1 face={sum(1 for o in out if o['n'])}  none={sum(1 for o in out if not o['n'])}")
b=collections.Counter()
for o in out:
    r=o["maxr"]
    b["no face"      if r==0     else
      "<2%"          if r<0.02   else
      "2-4%"         if r<0.04   else
      "4-7%"         if r<0.07   else
      "7-12%"        if r<0.12   else
      "12-20%"       if r<0.20   else
      ">=20%"] += 1
for k in ["no face","<2%","2-4%","4-7%","7-12%","12-20%",">=20%"]:
    print(f"  {k:>8}: {b[k]:>5}")
