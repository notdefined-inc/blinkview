import cv2, os, numpy as np, pickle, time
SP="/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/"
SRC="/Volumes/Notdefined/Swissgreece"
LONG=1280; MINF=44; SCORE=0.75

det=cv2.FaceDetectorYN.create(SP+"models/yunet.onnx","",(320,320),SCORE,0.3,5000)
rec=cv2.FaceRecognizerSF.create(SP+"models/sface.onnx","")

files=sorted(f for f in os.listdir(SRC) if f.lower().endswith(".jpg") and not f.startswith("._"))
faces=[]; nofaces=0; errs=[]
t0=time.time()
for i,f in enumerate(files):
    try:
        img=cv2.imread(os.path.join(SRC,f), cv2.IMREAD_REDUCED_COLOR_2)
        if img is None: img=cv2.imread(os.path.join(SRC,f))
        if img is None: errs.append(f); continue
        h,w=img.shape[:2]; s=LONG/max(h,w)
        if s<1: img=cv2.resize(img,(int(w*s),int(h*s)),interpolation=cv2.INTER_AREA)
        h,w=img.shape[:2]
        det.setInputSize((w,h))
        n,fc=det.detect(img)
        if fc is None: nofaces+=1; continue
        kept=0
        for row in fc:
            if row[2]<MINF or row[14]<SCORE: continue
            emb=rec.feature(rec.alignCrop(img,row)).flatten().astype(np.float32)
            emb/=np.linalg.norm(emb)+1e-9
            faces.append({"file":f,"box":row[:4].astype(int).tolist(),
                          "score":float(row[14]),"size":float(row[2]),"emb":emb})
            kept+=1
        if kept==0: nofaces+=1
    except Exception as e:
        errs.append(f)
    if (i+1)%250==0:
        print(f"  {i+1}/{len(files)}  faces={len(faces)}  {time.time()-t0:.0f}s",flush=True)

pickle.dump(faces,open(SP+"faces.pkl","wb"))
print(f"\nphotos={len(files)}  faces={len(faces)}  photos_with_face={len({x['file'] for x in faces})}  no_face={nofaces}  errors={len(errs)}")
