#!/usr/bin/env python3
"""Decompile PyInstaller PYZ .pyc files (no pyc header) with uncompyle6.

PyInstaller's PYZ-00.pyz stores raw marshal code objects WITHOUT the standard
16-byte pyc header (magic + bitfield + timestamp + source size). uncompyle6 /
xdis expect that header, so they fail with "unknown type code".

Fix: prepend a header whose 4-byte magic is copied verbatim from the PYZ's own
magic (bytes 4:8 of the archive). The magic determines the CPython version, so
xdis routes to its version-aware marshal reader instead of the host's
`marshal.loads` (which can only read the host version's type codes).

Usage:
    decompile_panel.py <PYZ_SOURCE_OR_EXTRACTED_DIR> [OUT_DIR] [module1 ...]

If no modules given, decompiles every antelope_*.pyc found under the dir.
"""
import glob
import os
import shutil, sys, subprocess, marshal

PYZ = sys.argv[1]

# --- Locate the PYZ-00.pyz source archive -----------------------------------
# Prefer the archive sitting next to the extracted dir, then immediate parents.
# Do NOT scan the whole repo: multiple bundles ship different Python versions
# and a broad walk can pick the wrong one (e.g. the 3.8 managerserver PYZ when
# decompiling the 3.5 Studio+ panel), which silently corrupts the header.
def find_pyz(start):
    if os.path.isfile(start) and start.endswith("PYZ-00.pyz"):
        return start
    for base in (start, os.path.dirname(start)):
        cand = os.path.join(base, "PYZ-00.pyz")
        if os.path.exists(cand):
            return cand
    p = os.path.abspath(start)
    for _ in range(6):
        p = os.path.dirname(p)
        cand = os.path.join(p, "PYZ-00.pyz")
        if os.path.exists(cand):
            return cand
    raise SystemExit(f"Could not locate PYZ-00.pyz near {start}")

pyz_path = find_pyz(PYZ)

# Optional explicit dir to walk for .pyc modules (defaults to the PYZ's parent).
WALK = sys.argv[2] if len(sys.argv) > 2 else os.path.dirname(pyz_path)
OUT = sys.argv[3] if len(sys.argv) > 3 else os.path.join(os.path.dirname(PYZ), "pyc_panel")
os.makedirs(OUT, exist_ok=True)

# --- Read the archive's own magic and build a matching 16-byte header -------
with open(pyz_path, "rb") as _f:
    _magic = _f.read(8)[4:8]  # bytes 4:8 hold the CPython magic
_MAGIC_TO_VERSION = {
    b"\x17\x0d\x0d\x0a": "3.5",
    b"\x1c\x0d\x0d\x0a": "3.6",
    b"\x1d\x0d\x0d\x0a": "3.7",
    b"\x1f\x0d\x0d\x0a": "3.8",
    b"\x55\x0d\x0d\x0a": "3.8",
}
PY_VERSION = _MAGIC_TO_VERSION.get(_magic)
if PY_VERSION is None:
    raise SystemExit(f"Unsupported PYZ magic {_magic.hex()} (from {pyz_path})")
HEADER = _magic + bytes(12)  # magic + 12 zero bytes (bitfield/ts/size)
print(f"[pyz={pyz_path}] magic={_magic.hex()} py{PY_VERSION}")

# Match the decompiler interpreter to the bytecode version. uncompyle6 3.x runs
# on 3.6+, but 3.5 needs the legacy uncompyle6 2.11 on a 3.5 interpreter.
#
# Interpreters are DISCOVERED, not hardcoded, so this runs on any machine. In order:
#
#   1. ANTELOPE_PY35 / ANTELOPE_PY36 / ANTELOPE_PY37 / ANTELOPE_PY38 env vars
#   2. a matching `pythonX.Y` on PATH
#   3. common conda/pyenv locations under $HOME
#
# 3.6-3.8 bytecode can all be decompiled by uncompyle6 3.x running on a 3.8 interpreter,
# so they fall back to the 3.8 one. 3.5 is different: it needs the legacy uncompyle6 2.11
# on an actual 3.5 interpreter, because uncompyle6 3.x will not run there.
def _discover(version):
    """Find an interpreter for `version`, or None."""
    env = os.environ.get("ANTELOPE_PY" + version.replace(".", ""))
    if env and os.path.exists(env):
        return env
    found = shutil.which("python" + version)
    if found:
        return found
    home = os.path.expanduser("~")
    for pattern in (
        os.path.join(home, "miniconda3", "envs", "py" + version.replace(".", ""), "bin", "python"),
        os.path.join(home, "anaconda3", "envs", "py" + version.replace(".", ""), "bin", "python"),
        os.path.join(home, ".pyenv", "versions", version + ".*", "bin", "python"),
    ):
        for candidate in sorted(glob.glob(pattern)):
            if os.path.exists(candidate):
                return candidate
    return None


def _interpreter_for(version):
    """Interpreter for a bytecode version, falling back to the 3.8 toolchain."""
    direct = _discover(version)
    if direct:
        return direct
    if version != "3.5":
        # 3.6/3.7/3.8 bytecode all decompile under a 3.8 interpreter.
        fallback = _discover("3.8")
        if fallback:
            return fallback
    raise SystemExit(
        "No interpreter found for Python %s bytecode.\n"
        "\n"
        "Set ANTELOPE_PY%s to an interpreter that has uncompyle6 installed, e.g.:\n"
        "    conda create -n py%s python=%s && conda run -n py%s pip install uncompyle6\n"
        "    export ANTELOPE_PY%s=$(conda run -n py%s which python)\n"
        "\n"
        "Python 3.5 additionally needs the legacy uncompyle6 2.11, since uncompyle6 3.x\n"
        "cannot run on 3.5. See docs/reverse-engineering.md.\n"
        % (version, version.replace(".", ""), version.replace(".", ""), version,
           version.replace(".", ""), version.replace(".", ""), version.replace(".", ""))
    )


PY = _interpreter_for(PY_VERSION)
print(f"[interpreter] py{PY_VERSION} -> {PY}")

def find_pyc(name):
    """Locate a module's .pyc anywhere under the extracted dir (handles subpackages)."""
    for root, _dirs, files in os.walk(WALK):
        if name + ".pyc" in files:
            return os.path.join(root, name + ".pyc")
    return None

if len(sys.argv) > 4:
    names = sys.argv[4:]
else:
    names = sorted(
        f[:-4] for root, _dirs, files in os.walk(WALK)
        for f in files
        if f.endswith(".pyc") and f.startswith("antelope_")
    )

def decompile(name):
    src = find_pyc(name)
    if src is None:
        print(f"[MISSING] {name}")
        return
    # The PYZ stores raw marshalled code objects (no pyc header). The blob is
    # already a valid code object for the archive's own Python version, so we
    # load it with the matching-version marshal and feed it straight to
    # uncompyle6's code-object API (decompile(version_float, co, out)). This
    # avoids the header-trick, which misaligns the stream. Everything runs in
    # the matching-version interpreter (py35 for 3.5 bytecode).
    dst = os.path.join(OUT, name.replace("/", "_") + ".py")
    helper = os.path.join(os.path.dirname(__file__), "decompile_one.py")
    r = subprocess.run(
        [PY, helper, src, dst, PY_VERSION],
        capture_output=True, text=True,
    )
    status = "OK" if r.returncode == 0 else f"FAIL(rc={r.returncode})"
    print(f"[{status}] {name}")
    if r.stderr.strip():
        for line in r.stderr.strip().splitlines()[:3]:
            print("     " + line)

for name in names:
    decompile(name)
