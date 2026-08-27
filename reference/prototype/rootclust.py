import pickle, numpy as np
from sklearn.cluster import AgglomerativeClustering
from collections import defaultdict, Counter
SP="/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/"
F=pickle.load(open(SP+"faces.pkl","rb")); S=pickle.load(open(SP+"seed.pkl","rb"))
A=pickle.load(open(SP+"assign.pkl","rb")); R=pickle.load(open(SP+"rootemb.pkl","rb"))
X=np.vstack([f["emb"] for f in F])
REF={"Me":      X[S["me"]],
     "Person1": X[sorted(set(A["P"]["Person1"])|{i for i,_ in A["assign"]["Person1"]})],
     "Person2": X[sorted(set(A["P"]["Person2"])|{i for i,_ in A["assign"]["Person2"]})]}
print("reference faces:", {k:len(v) for k,v in REF.items()})

Y=np.vstack([r["emb"] for r in R])
lab=AgglomerativeClustering(n_clusters=None,distance_threshold=0.55,
    metric="cosine",linkage="average").fit_predict(Y)
by=defaultdict(list)
for i,l in enumerate(lab): by[l].append(i)
order=sorted(by,key=lambda k:-len(by[k]))
print(f"{len(R)} faces -> {len(by)} clusters ({sum(1 for k in order if len(by[k])>=3)} with >=3)\n")

rows=[]
print(f"{'cluster':>7} {'faces':>5} {'photos':>6}   {'best match':<9} {'sim':>5}  {'runner-up':<9} {'sim':>5}")
for k in order:
    idx=by[k]
    cen=Y[idx].mean(0); cen/=np.linalg.norm(cen)
    sc=sorted(((n,float((v@cen).max())) for n,v in REF.items()), key=lambda t:-t[1])
    ph={R[i]["file"] for i in idx}
    rows.append({"cluster":k,"n":len(idx),"photos":len(ph),
                 "best":sc[0][0],"bs":sc[0][1],"second":sc[1][0],"ss":sc[1][1]})
    if len(idx)>=2 or sc[0][1]>=0.5:
        print(f"{k:>7} {len(idx):>5} {len(ph):>6}   {sc[0][0]:<9} {sc[0][1]:>5.2f}  {sc[1][0]:<9} {sc[1][1]:>5.2f}")
print(f"\n(+{sum(1 for r in rows if r['n']==1 and r['bs']<0.5)} singleton clusters with weak matches, likely strangers)")
pickle.dump({"lab":lab,"by":dict(by),"order":order,"rows":rows},open(SP+"rootclust.pkl","wb"))
