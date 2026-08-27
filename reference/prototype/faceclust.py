import pickle, numpy as np
from sklearn.cluster import AgglomerativeClustering
from collections import Counter
SP="/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/"
F=pickle.load(open(SP+"faces.pkl","rb"))
X=np.vstack([f["emb"] for f in F])
print(f"{len(F)} faces, dim={X.shape[1]}")
for thr in (0.45,0.55,0.65):
    lab=AgglomerativeClustering(n_clusters=None,distance_threshold=thr,
        metric="cosine",linkage="average").fit_predict(X)
    c=Counter(lab); big=[n for n in c.values() if n>=5]
    print(f"  dist<={thr}: {len(c):>3} clusters, {len(big):>2} with >=5 faces, "
          f"largest={sorted(c.values(),reverse=True)[:8]}")
lab=AgglomerativeClustering(n_clusters=None,distance_threshold=0.55,
    metric="cosine",linkage="average").fit_predict(X)
pickle.dump(lab,open(SP+"labels.pkl","wb"))
c=Counter(lab)
top=[k for k,_ in c.most_common(18)]
print("\ntop clusters (id, faces, distinct photos):")
for k in top:
    idx=[i for i,l in enumerate(lab) if l==k]
    print(f"  #{k:<4} faces={len(idx):<4} photos={len({F[i]['file'] for i in idx})}")
pickle.dump(top,open(SP+"top.pkl","wb"))
