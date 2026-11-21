#!/usr/bin/env python3
"""unpack_pkg.py <scene.pkg> <out dir>: extracts a WE PKGV package."""
import struct,sys,os
def rd(f):
    n=struct.unpack('<I',f.read(4))[0]; return f.read(n).decode()
p,out=sys.argv[1],sys.argv[2]
f=open(p,'rb'); rd(f); c=struct.unpack('<I',f.read(4))[0]
ents=[(rd(f),)+struct.unpack('<II',f.read(8)) for _ in range(c)]
base=f.tell()
for n,o,l in ents:
    f.seek(base+o); dst=os.path.join(out,n); os.makedirs(os.path.dirname(dst),exist_ok=True); open(dst,'wb').write(f.read(l))
