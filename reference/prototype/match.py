import pickle, numpy as np
SP="/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/"
F=pickle.load(open(SP+"faces.pkl","rb")); S=pickle.load(open(SP+"seed.pkl","rb"))
X=np.vstack([f["emb"] for f in F])
ME,NOT=S["me"],S["not"]
Mm, Mn = X[ME], X[NOT]

def sims(v): return float((Mm@v).max()), float((Mn@v).max())

# sanity: how separable are the two confirmed sets?
sep=[sims(X[i]) for i in ME]
print("confirmed ME faces -> max sim to ME(self excl.) vs NOT:")
selfsim=[float(np.sort(Mm@X[i])[-2]) for i in ME]
notsim=[s[1] for s in sep]
print(f"  vs other ME: mean={np.mean(selfsim):.3f}  vs NOT: mean={np.mean(notsim):.3f}")
print(f"  ME faces where NOT-sim beat ME-sim: {sum(1 for a,b in zip(selfsim,notsim) if b>a)}/{len(ME)}")

unl=[i for i in range(len(F)) if i not in set(ME) and i not in set(NOT)]
rows=[]
for i in unl:
    a,b=sims(X[i]); rows.append((i,a,b,a-b))
rows.sort(key=lambda r:-r[3])
for thr in (0.30,0.35,0.40,0.45):
    hit=[r for r in rows if r[1]>=thr and r[1]>r[2]]
    print(f"  thr={thr}: {len(hit):>3} of {len(unl)} unlabeled faces match ME "
          f"({len({F[r[0]]['file'] for r in hit})} photos)")
pickle.dump(rows,open(SP+"unl.pkl","wb"))
