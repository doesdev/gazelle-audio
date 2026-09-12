#!/usr/bin/env python3
"""Extract .pyc files from a PyInstaller PYZ-00.pyz.

Layout: b'PYZ\0' + 4-byte python magic + big-endian int TOC position +
marshalled dict {name: (ispkg, pos, length)} + zlib-compressed pyc blobs.
"""
import struct, zlib, sys, os, marshal

PYZ = sys.argv[1]
OUTDIR = sys.argv[2]
os.makedirs(OUTDIR, exist_ok=True)

with open(PYZ, "rb") as f:
    data = f.read()

assert data[:4] == b"PYZ\0", data[:8]
pycmagic = data[4:8]
toc_pos = struct.unpack(">i", data[8:12])[0]
print(f"pycmagic={pycmagic.hex()} toc_pos={toc_pos}")

f = open(PYZ, "rb")
f.seek(toc_pos)
toc = marshal.load(f)
if isinstance(toc, list):
    toc = dict(toc)
print(f"entries={len(toc)}")

n = 0
for key, (ispkg, pos, length) in toc.items():
    name = key.decode("utf-8") if isinstance(key, bytes) else key
    f.seek(pos)
    raw = f.read(length)
    try:
        raw = zlib.decompress(raw)
    except Exception as e:
        print(f"  decompress fail {name}: {e}")
        continue
    suffix = "/__init__.pyc" if ispkg else ".pyc"
    outp = os.path.join(OUTDIR, name.replace("/", "_").replace(".", "_") + suffix)
    os.makedirs(os.path.dirname(outp), exist_ok=True)
    with open(outp, "wb") as o:
        o.write(raw)
    n += 1
    print(f"  wrote {name} ({len(raw)} bytes)")
print(f"total {n} files")
f.close()
