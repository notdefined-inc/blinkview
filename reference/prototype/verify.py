import json, pickle, itertools, math
from collections import defaultdict

SP = "/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/"
H = json.load(open(SP+"hashes.json"))
TH = pickle.load(open(SP+"thumbs.pkl","rb"))
n = len(H)
def ham(x,y): return bin(x^y).count("1")

# normalize each thumb (contrast-invariant) so exposure shifts don't hide a true dupe
norm = {}
for f, px in TH.items():
    m = sum(px)/len(px)
    sd = math.sqrt(sum((p-m)**2 for p in px)/len(px)) or 1.0
    norm[f] = [(p-m)/sd for p in px]

def rmse(a, b):
    A, B = norm[a], norm[b]
    return math.sqrt(sum((x-y)**2 for x, y in zip(A, B))/len(A))

# broad candidate generation, then pixel verification
cand = []
for i, j in itertools.combinations(range(n), 2):
    if ham(H[i]["d"], H[j]["d"]) <= 12:
        cand.append((i, j))
print(f"candidate pairs from dHash<=12: {len(cand)}")

verified = []
for i, j in cand:
    r = rmse(H[i]["f"], H[j]["f"])
    if r <= 0.45:                      # ~identical framing
        verified.append((i, j, r))
print(f"pixel-verified pairs (norm RMSE<=0.45): {len(verified)}")

parent = list(range(n))
def find(x):
    while parent[x]!=x: parent[x]=parent[parent[x]]; x=parent[x]
    return x
for i, j, _ in verified:
    a,b = find(i), find(j)
    if a!=b: parent[a]=b
g = defaultdict(list)
for i in range(n): g[find(i)].append(i)
cl = sorted([v for v in g.values() if len(v)>1], key=len, reverse=True)
print(f"clusters: {len(cl)}, photos: {sum(len(c) for c in cl)}, would move: {sum(len(c)-1 for c in cl)}")

def day(f): return f[:8]
spread = [c for c in cl if len({day(H[i]['f']) for i in c}) > 1]
print(f"clusters spanning >1 day (suspicious): {len(spread)}")
print("\ntop clusters:")
for c in cl[:12]:
    fs = sorted(H[i]["f"] for i in c)
    print(f"  [{len(c)}] {fs[0]} .. {fs[-1]}  days={len({f[:8] for f in fs})}")
pickle.dump([sorted(H[i]['f'] for i in c) for c in cl], open(SP+"verified.pkl","wb"))
