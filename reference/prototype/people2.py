import pickle, numpy as np, os
from collections import defaultdict
SP="/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/"
SRC="/Volumes/Notdefined/Swissgreece"
F=pickle.load(open(SP+"faces.pkl","rb")); lab=pickle.load(open(SP+"labels.pkl","rb"))
S=pickle.load(open(SP+"seed.pkl","rb")); R=pickle.load(open(SP+"rem.pkl","rb"))
X=np.vstack([f["emb"] for f in F]); rem=set(R["rem"]); by=R["by"]

P={"Person1": list(by[3]), "Person2": list(by[5])+list(by[6])}   # #6 merges into #5 (0.77)
ME=[i for i in S["me"]]                                          # user = negative class here
seeded={i for v in P.values() for i in v}
unl=[i for i in rem if i not in seeded]
print(f"seeds: "+", ".join(f"{k}={len(v)}" for k,v in P.items())+f"   unassigned={len(unl)}")

def best(i):
    sc={k: float((X[v]@X[i]).max()) for k,v in P.items()}
    sc["_me"]=float((X[ME]@X[i]).max())
    o=sorted(sc.items(), key=lambda kv:-kv[1])
    return o[0], o[1]

assign=defaultdict(list); amb=[]; unk=[]
for i in unl:
    (k1,s1),(k2,s2)=best(i)
    if k1=="_me": unk.append((i,k1,s1)); continue
    if s1>=0.50 and s1-s2>=0.05: assign[k1].append((i,s1))
    elif s1>=0.50: amb.append((i,k1,s1,k2,s2))
    else: unk.append((i,k1,s1))
for k in P: print(f"  +{len(assign[k]):>3} new faces -> {k}")
print(f"  {len(amb)} ambiguous (two people close), {len(unk)} unmatched/other")
pickle.dump({"P":P,"assign":dict(assign),"amb":amb,"unk":unk},open(SP+"assign.pkl","wb"))

for k,v in P.items():
    tot=set(v)|{i for i,_ in assign[k]}
    ph={F[i]["file"] for i in tot}
    print(f"{k}: {len(tot)} faces / {len(ph)} photos")
