import os, re, csv, sys, pickle, shutil, collections
SRC="/Volumes/Notdefined/Swissgreece"
SP="/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/"
APPLY="--apply" in sys.argv
DIRS=['Me','Nikhil','saurabh','Scenery','.','Duplicates','Videos']   # priority order
PRIO={d:i for i,d in enumerate(DIRS)}
def real(x): return sorted(f for f in os.listdir(os.path.join(SRC,x))
                           if f.lower().endswith(('.jpg','.mp4')) and not f.startswith('._'))
loc=collections.defaultdict(list)
for d in DIRS:
    for f in real(d): loc[f].append(d)
allnames=set(loc)
coll={k:sorted(v,key=lambda d:PRIO[d]) for k,v in loc.items() if len(v)>1}
print(f"colliding names: {len(coll)}  files affected: {sum(len(v) for v in coll.values())}")

stem_re=re.compile(r'^(\d{2}-\d{2}-\d{2}_[ap]m_\d{2}_[a-z]{3}_\d{4})(?:_(\d+))?\.(jpg|mp4)$', re.I)
plan=[]
taken=set(allnames)
for name in sorted(coll):
    keep, *rest = coll[name]
    m=stem_re.match(name)
    if not m: raise SystemExit(f'ABORT: unparseable name {name}')
    base,ext=m.group(1),m.group(3)
    for d in rest:
        n=2
        while f"{base}_{n}.{ext}" in taken: n+=1
        new=f"{base}_{n}.{ext}"
        taken.add(new)
        plan.append({"folder":d,"old":name,"new":new,"kept_in":keep})
print(f"renames planned: {len(plan)}")
print("\nsample:")
for r in plan[:6]:
    print(f"  {r['folder']:<11} {r['old']:<30} -> {r['new']:<32} (kept in {r['kept_in']})")

# validate
bad=[r for r in plan if r["new"] in allnames]
per=collections.Counter((r["folder"],r["new"]) for r in plan)
print(f"\nvalidation: clashes-with-existing={len(bad)}  duplicate-targets={sum(1 for v in per.values() if v>1)}")
with open(SP+"decollide_plan.csv","w",newline="") as fh:
    w=csv.DictWriter(fh,fieldnames=["folder","old","new","kept_in"]); w.writeheader(); w.writerows(plan)
if not APPLY:
    print("\nDRY RUN - nothing changed."); sys.exit(0)
if bad or any(v>1 for v in per.values()):
    print("ABORT"); sys.exit(1)

done=0
for r in plan:
    p=os.path.join(SRC,r["folder"])
    os.rename(os.path.join(p,r["old"]), os.path.join(p,r["new"])); done+=1
    sa,sb=os.path.join(p,"._"+r["old"]), os.path.join(p,"._"+r["new"])
    if os.path.exists(sa) and not os.path.exists(sb): os.rename(sa,sb)
print(f"renamed {done} files")

# update folder manifests
ren={(r["folder"],r["old"]):r["new"] for r in plan}
for d in ['Me','Nikhil','saurabh','Scenery','Duplicates']:
    p=os.path.join(SRC,d,"_manifest.csv")
    rows=list(csv.DictReader(open(p))); fn=list(rows[0].keys()); n=0
    for row in rows:
        if (d,row["file"]) in ren: row["file"]=ren[(d,row["file"])]; n+=1
        if "kept_instead" in row and row["kept_instead"]:
            hits=[(dd,nn) for (dd,nn) in ren if nn==row["kept_instead"]]
            # only rewrite when the referenced photo is unambiguous
            if len(hits)==1: row["kept_instead"]=ren[hits[0]]
    with open(p,"w",newline="") as fh:
        w=csv.DictWriter(fh,fieldnames=fn); w.writeheader(); w.writerows(rows)
    print(f"  {d}/_manifest.csv: {n} filenames updated")
