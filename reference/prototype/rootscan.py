import cv2, os, pickle, numpy as np, time
SP="/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/"
SRC="/Volumes/Notdefined/Swissgreece"; LONG=1920
det=cv2.FaceDetectorYN.create(SP+"models/yunet.onnx","",(320,320),0.75,0.3,5000)
rec=cv2.FaceRecognizerSF.create(SP+"models/sface.onnx","")
files=sorted(f for f in os.listdir(SRC) if f.lower().endswith(".jpg") and not f.startswith("._"))
print(f"scanning {len(files)} root photos at {LONG}px")
out=[]; t0=time.time()
for i,f in enumerate(files):
    img=cv2.imread(os.path.join(SRC,f))
    if img is None: continue
    h,w=img.shape[:2]; s=LONG/max(h,w)
    if s<1: img=cv2.resize(img,(int(w*s),int(h*s)),interpolation=cv2.INTER_AREA)
    h,w=img.shape[:2]
    det.setInputSize((w,h))
    n,fc=det.detect(img)
    if fc is None: continue
    for r in fc:
        if r[2]<50: continue                       # need ~50px for a usable embedding
        e=rec.feature(rec.alignCrop(img,r)).flatten().astype(np.float32)
        e/=np.linalg.norm(e)+1e-9
        out.append({"file":f,"box":r[:4].astype(int).tolist(),"score":float(r[14]),
                    "size":float(r[2]),"ratio":float(r[2])/w,"emb":e})
    if (i+1)%150==0: print(f"  {i+1}/{len(files)}  faces={len(out)}  {time.time()-t0:.0f}s",flush=True)
pickle.dump(out,open(SP+"rootemb.pkl","wb"))
print(f"\nfaces={len(out)} across {len({o['file'] for o in out})} photos "
      f"(of {len(files)}); {len(files)-len({o['file'] for o in out})} had no usable face")
