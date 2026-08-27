import pickle, numpy as np
from sklearn.cluster import AgglomerativeClustering
from collections import Counter
SP="/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/"
F=pickle.load(open(SP+"faces.pkl","rb")); A=pickle.load(open(SP+"assign.pkl","rb"))
X=np.vstack([f["emb"] for f in F])
idx=[i for i,_,_ in A["unk"]]
lab=AgglomerativeClustering(n_clusters=None,distance_threshold=0.55,
    metric="cosine",linkage="average").fit_predict(X[idx])
c=Counter(lab)
print(f"{len(idx)} leftover faces -> {len(c)} groups")
groups=[]
for k,n in c.most_common():
    mem=[idx[j] for j,l in enumerate(lab) if l==k]
    if n>=2: groups.append(mem)
    print(f"  group {k}: {n} face(s)  e.g. {F[mem[0]]['file']}")
pickle.dump(groups,open(SP+"othergroups.pkl","wb"))
print(f"\nrecurring (>=2 faces): {len(groups)} groups covering {sum(len(g) for g in groups)} faces")
print(f"one-off strangers: {sum(1 for k,n in c.items() if n==1)}")
