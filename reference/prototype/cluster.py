import json, itertools
from collections import defaultdict

H = json.load(open("/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/hashes.json"))
n = len(H)
pop = bin

def ham(x, y): return bin(x ^ y).count("1")

# exact dHash collisions
byd = defaultdict(list)
for r in H: byd[r["d"]].append(r["f"])
exact = {k: v for k, v in byd.items() if len(v) > 1}
print(f"exact dHash-identical groups: {len(exact)}  (files: {sum(len(v) for v in exact.values())})")

def clusters(thr, require_ahash=True):
    parent = list(range(n))
    def find(x):
        while parent[x] != x:
            parent[x] = parent[parent[x]]; x = parent[x]
        return x
    def union(a, b):
        ra, rb = find(a), find(b)
        if ra != rb: parent[ra] = rb
    for i, j in itertools.combinations(range(n), 2):
        if ham(H[i]["d"], H[j]["d"]) <= thr:
            if not require_ahash or ham(H[i]["a"], H[j]["a"]) <= thr + 4:
                union(i, j)
    g = defaultdict(list)
    for i in range(n): g[find(i)].append(i)
    return [v for v in g.values() if len(v) > 1]

for thr in (0, 2, 5, 8, 10):
    cs = clusters(thr)
    files = sum(len(c) for c in cs)
    extras = files - len(cs)
    print(f"dHash<={thr:>2}: {len(cs):>4} clusters, {files:>4} photos involved, "
          f"{extras:>4} would move (keep 1 per cluster)")

# save the mid threshold for inspection
import pickle
cs = clusters(5)
cs.sort(key=len, reverse=True)
pickle.dump([[H[i]["f"] for i in c] for c in cs],
            open("/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/clusters5.pkl","wb"))
print("\nlargest clusters at dHash<=5:")
for c in cs[:8]:
    print(f"  [{len(c)}] " + ", ".join(H[i]["f"] for i in c[:6]) + (" ..." if len(c) > 6 else ""))
