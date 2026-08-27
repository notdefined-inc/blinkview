import pickle, numpy as np
from collections import defaultdict
SP="/private/tmp/claude-501/-Volumes-Notdefined-Swissgreece/bfd190a1-9e05-49a2-ad64-d5eb27bcefa9/scratchpad/"
F=pickle.load(open(SP+"faces.pkl","rb")); lab=pickle.load(open(SP+"labels.pkl","rb"))
by=defaultdict(list)
for i,l in enumerate(lab): by[l].append(i)
N=9
def display_order(k):                      # reproduce facesheet.py exactly
    idx=sorted(by[k], key=lambda i:-F[i]["size"])
    step=max(1,len(idx)//N)
    return idx, idx[::step][:N]

ME=[]; NOT=[]
for k in (18,9,0):                          # fully mine
    ME += by[k]
idx38,disp38 = display_order(38)
ME  += [i for i in idx38 if i is not disp38[-1] and i!=disp38[-1]]
NOT += [disp38[-1]]                         # "#38 except last photo"
idx10,disp10 = display_order(10)
ME  += disp10[:2]                           # "#10 initial 2 photos"
NOT += [i for i in idx10 if i not in disp10[:2]]
NOT += by[3] + by[5]                        # the two big clusters that are other people

ME=sorted(set(ME)); NOT=sorted(set(NOT))
print(f"confirmed ME  faces={len(ME)}  photos={len({F[i]['file'] for i in ME})}")
print(f"confirmed NOT faces={len(NOT)} photos={len({F[i]['file'] for i in NOT})}")
print(f"unlabeled     faces={len(F)-len(ME)-len(NOT)}")
print("\n#38 excluded:", F[disp38[-1]]['file'])
print("#10 kept    :", [F[i]['file'] for i in disp10[:2]])
print("#10 excluded:", [F[i]['file'] for i in idx10 if i not in disp10[:2]])
pickle.dump({"me":ME,"not":NOT},open(SP+"seed.pkl","wb"))
