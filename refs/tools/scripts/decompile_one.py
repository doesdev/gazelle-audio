#!/usr/bin/env python
"""Decompile a single PYZ .pyc (raw marshalled code object) to source.

Run with the interpreter matching the bytecode version (e.g. py35 for 3.5).
Usage: decompile_one.py <raw_pyc_path> <output_py_path> <version_float>

uncompyle6's `decompile` signature differs across versions:
  - 2.11.0 (py35 env):  decompile(version, co, out)   -- version FIRST
  - 3.9.3  (py38 env):  decompile(co, version, out)   -- code object FIRST

This helper inspects the installed signature at runtime and calls whichever
convention is present, so the same driver works for both 3.5 and 3.8 targets.

uncompyle6 sometimes raises SourceWalkerError ("Deparsing stopped due to parse
error") on certain try/finally blocks. That is a known limitation: the head of
the file is intact. We write whatever was decompiled before the error so the
recoverable source is preserved instead of being lost.
"""
import sys, marshal, io, contextlib, inspect, traceback
from uncompyle6.main import decompile

src, dst, version = sys.argv[1], sys.argv[2], sys.argv[3]
raw = open(src, "rb").read()
co = marshal.loads(raw)
out = io.StringIO()
major, minor = version.split(".")
ver = (int(major), int(minor))

# Detect the calling convention of the installed uncompyle6.decompile.
params = list(inspect.signature(decompile).parameters)
# 2.11.0 -> first param is the bytecode version (str/tuple); 3.9.3 -> first
# param is the code object. Heuristic: if the first param is named like a
# version, OR the code object is not the first arg, pass version first.
version_first = params and params[0] in ("bytecode_version", "version")
# 2.11.0's get_scanner checks `version in PYTHON_VERSIONS` (a tuple of floats
# like 3.5) then does int(version * 10); it wants the float 3.5. 3.9.3 wants
# the (major, minor) tuple.
ver_arg = float(version) if version_first else (int(major), int(minor))

try:
    with contextlib.redirect_stdout(io.StringIO()):
        if version_first:
            decompile(ver_arg, co, out)
        else:
            decompile(co, ver_arg, out)
except Exception:
    partial = out.getvalue()
    with open(dst, "w") as f:
        f.write(partial)
    sys.stderr.write("[PARTIAL] %s wrote %d bytes before parse error\n"
                     % (src, len(partial)))
    traceback.print_exc(file=sys.stderr)
    sys.exit(2)
with open(dst, "w") as f:
    f.write(out.getvalue())
