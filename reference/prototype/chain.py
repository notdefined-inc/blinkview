import pickle, math, itertools
SP="/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/"
cl=pickle.load(open(SP+"verified.pkl","rb")); TH=pickle.load(open(SP+"thumbs.pkl","rb"))
norm={}
for f,px in TH.items():
    m=sum(px)/len(px); sd=math.sqrt(sum((p-m)**2 for p in px)/len(px)) or 1.0
    norm[f]=[(p-m)/sd for p in px]
def rmse(a,b): return math.sqrt(sum((x-y)**2 for x,y in zip(norm[a],norm[b]))/len(norm[a]))
big=cl[0]
d=[rmse(a,b) for a,b in itertools.combinations(big,2)]
print(f"85-cluster all-pairs RMSE: min={min(d):.2f} max={max(d):.2f} mean={sum(d)/len(d):.2f}")
print(f"  pairs actually <=0.45: {sum(1 for x in d if x<=0.45)}/{len(d)}  ({100*sum(1 for x in d if x<=0.45)/len(d):.0f}%)")
print("\ncluster 'diameter' (max intra-pair RMSE) per cluster:")
bad=0
for c in cl:
    if len(c)<3: continue
    dd=max(rmse(a,b) for a,b in itertools.combinations(c,2))
    if dd>0.45: bad+=1
print(f"  clusters (size>=3) whose diameter exceeds 0.45: {bad} of {sum(1 for c in cl if len(c)>=3)}")
