import os,re,datetime,collections,csv,sys
SRC="/Volumes/Notdefined/Swissgreece"
SP="/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/"
APPLY = "--apply" in sys.argv
pat=re.compile(r'^(\d{8})_(\d{6})(?:\((\d+)\))?\.(jpg|mp4)$', re.I)
BAD=set('"*/:<>?\\|')
dirs=["."]+[d for d in sorted(os.listdir(SRC)) if os.path.isdir(os.path.join(SRC,d)) and not d.startswith('.')]

plan=[]; problems=[]
for d in dirs:
    p=os.path.join(SRC,d)
    fs=[f for f in os.listdir(p) if not f.startswith("._") and pat.match(f)]
    # group by exact second; base name (no parens) sorts first
    g=collections.defaultdict(list)
    for f in fs:
        m=pat.match(f)
        g[(m.group(1),m.group(2),m.group(4).lower())].append(
            (int(m.group(3)) if m.group(3) is not None else -1, f))
    existing={f.lower() for f in os.listdir(p)}
    for (ymd,hms,ext),items in g.items():
        dt=datetime.datetime.strptime(ymd+hms,"%Y%m%d%H%M%S")
        stem=dt.strftime("%I-%M-%S_%p_%d_%b_%Y").lower()
        for n,(idx,f) in enumerate(sorted(items), start=1):
            new=f"{stem}{'' if n==1 else f'_{n}'}.{ext}"
            if BAD & set(new): problems.append(("invalid-char",d,f,new))
            if new.lower()!=f.lower() and new.lower() in existing:
                problems.append(("target-exists",d,f,new))
            plan.append({"folder":d,"old":f,"new":new})

# global validation
byfolder=collections.defaultdict(collections.Counter)
for r in plan: byfolder[r["folder"]][r["new"].lower()]+=1
for d,c in byfolder.items():
    for nm,n in c.items():
        if n>1: problems.append(("duplicate-target",d,"",nm))

print(f"planned renames: {len(plan)}   problems: {len(problems)}")
for t in collections.Counter(p[0] for p in problems).items(): print("   ",t)
print("\nsample:")
for r in plan[:3]+plan[len(plan)//2:len(plan)//2+3]:
    print(f"  {r['folder']:<11} {r['old']:<28} -> {r['new']}")
mx=max(len(r["new"]) for r in plan)
print(f"\nlongest new name: {mx} chars")
with open(SP+"rename_plan.csv","w",newline="") as fh:
    w=csv.DictWriter(fh,fieldnames=["folder","old","new"]); w.writeheader(); w.writerows(plan)

if not APPLY:
    print("\nDRY RUN - nothing renamed."); sys.exit(0)
if problems:
    print("\nABORT: problems found."); sys.exit(1)

done=side=fail=0
for r in plan:
    p=os.path.join(SRC,r["folder"])
    a,b=os.path.join(p,r["old"]),os.path.join(p,r["new"])
    if r["old"]==r["new"] or not os.path.exists(a): continue
    try:
        os.rename(a,b); done+=1
        sa,sb=os.path.join(p,"._"+r["old"]),os.path.join(p,"._"+r["new"])
        if os.path.exists(sa) and not os.path.exists(sb):
            os.rename(sa,sb); side+=1
    except Exception as e:
        fail+=1; print("  FAIL",r["old"],e)
print(f"renamed={done}  sidecars={side}  failed={fail}")
