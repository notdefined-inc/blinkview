import json, pickle, math, itertools
from collections import defaultdict
SP="/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/"
H=json.load(open(SP+"hashes.json")); TH=pickle.load(open(SP+"thumbs.pkl","rb"))
norm={}
for f,px in TH.items():
    m=sum(px)/len(px); sd=math.sqrt(sum((p-m)**2 for p in px)/len(px)) or 1.0
    norm[f]=[(p-m)/sd for p in px]
def rmse(a,b): return math.sqrt(sum((x-y)**2 for x,y in zip(norm[a],norm[b]))/len(norm[a]))
def ham(x,y): return bin(x^y).count("1")
n=len(H); F=[r["f"] for r in H]

pairs=[]
for i,j in itertools.combinations(range(n),2):
    if ham(H[i]["d"],H[j]["d"])<=12:
        r=rmse(F[i],F[j])
        if r<=THRESH: pairs.append((r,i,j))
pairs.sort()

# complete-linkage: a photo joins a group only if similar to EVERY member
groups=[]; assigned={}
for r,i,j in pairs:
    gi,gj=assigned.get(i),assigned.get(j)
    if gi is None and gj is None:
        groups.append({i,j}); assigned[i]=assigned[j]=len(groups)-1
    elif gi is not None and gj is None:
        g=groups[gi]
        if all(rmse(F[j],F[k])<=THRESH for k in g): g.add(j); assigned[j]=gi
    elif gj is not None and gi is None:
        g=groups[gj]
        if all(rmse(F[i],F[k])<=THRESH for k in g): g.add(i); assigned[i]=gj
    elif gi!=gj:
        a,b=groups[gi],groups[gj]
        if all(rmse(F[x],F[y])<=THRESH for x in a for y in b):
            a|=b
            for k in b: assigned[k]=gi
            groups[gj]=set()
groups=[g for g in groups if len(g)>1]
groups.sort(key=len, reverse=True)
tot=sum(len(g) for g in groups)
print(f"tight clusters: {len(groups)}  photos: {tot}  would move: {tot-len(groups)}")
md=max(max(rmse(F[x],F[y]) for x,y in itertools.combinations(g,2)) for g in groups)
print(f"max diameter across all clusters: {md:.2f}  (threshold THRESH)")
print(f"clusters spanning >1 day: {sum(1 for g in groups if len({F[i][:8] for i in g})>1)}")
print("\nlargest:")
for g in groups[:10]:
    fs=sorted(F[i] for i in g); print(f"  [{len(g)}] {fs[0]} .. {fs[-1]}")
pickle.dump([sorted(F[i] for i in g) for g in groups], open(SP+"tight.pkl","wb"))
