#!/usr/bin/env python3
"""Check decompiled source against the bytecode it came from.

Decompilers fail quietly. uncompyle6 emits a ``Parse error at or near ...`` line and keeps
going, so a file can look complete while whole function bodies are missing, and nothing in
the output says which identifiers were lost.

This compares the two directly, using ``pyc_inspect`` to read the marshal blob without
needing the interpreter that produced it:

* every identifier in the bytecode (``co_names``, ``co_varnames``, function and class
  names) is looked for in the decompiled text;
* every module-level literal assignment is checked to have the same value;
* ``Parse error`` markers are counted and the functions they replaced are named.

Anything reported as MISSING is present in the compiled module but absent from the source
you are reading, so any conclusion drawn from that source may be incomplete.

Usage
-----
    verify_decompiled.py <blob.pyc> <source.py> [--py 3.8] [--quiet]
    verify_decompiled.py --batch <blob_dir> <src_dir> [--py 3.8]

Exit status is 1 when identifiers are missing or a constant disagrees, so this can gate CI.
"""

from __future__ import annotations

import argparse
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pyc_inspect import MarshalError, load_blob, module_assignments, walk  # noqa: E402

# Identifiers that are always present in bytecode but need not appear verbatim in source:
# builtins, dunders, and names the compiler synthesises.
IGNORE = {
    "__name__", "__doc__", "__module__", "__qualname__", "__class__", "__dict__",
    "__init__", "__file__", "__builtins__", "__spec__", "__package__", "__loader__",
    "__annotations__", "__orig_bases__", "__set_name__", "__prepare__",
    "print", "len", "range", "int", "str", "bytes", "bool", "float", "list", "dict",
    "set", "tuple", "object", "type", "super", "isinstance", "issubclass", "getattr",
    "setattr", "hasattr", "delattr", "property", "staticmethod", "classmethod",
    "Exception", "ValueError", "TypeError", "KeyError", "IndexError", "RuntimeError",
    "AttributeError", "NotImplementedError", "StopIteration", "OSError", "IOError",
    "min", "max", "sum", "abs", "any", "all", "enumerate", "zip", "map", "filter",
    "sorted", "reversed", "open", "repr", "hash", "id", "iter", "next", "format",
    "None", "True", "False", "self", "cls",
}


def identifiers(code) -> set[str]:
    """Every identifier the compiled module refers to."""
    out: set[str] = set()
    for c in walk(code):
        for n in list(c.names) + list(c.varnames) + list(c.freevars) + list(c.cellvars):
            if isinstance(n, str) and n.isidentifier():
                out.add(n)
        if isinstance(c.name, str) and c.name.isidentifier():
            out.add(c.name)
    return out - IGNORE


def parse_error_context(text: str) -> list[str]:
    """Names of the definitions uncompyle6 replaced with an error marker."""
    out = []
    for m in re.finditer(r"(?:def|class)\s+(\w+)[^\n]*Parse error at or near", text):
        out.append(m.group(1))
    # `def nameParse error ...` — the decompiler runs the name into the marker.
    for m in re.finditer(r"(?:def|class)\s+(\w+?)Parse error at or near", text):
        out.append(m.group(1))
    return sorted(set(out))


def verify(blob: str, src: str, py: tuple[int, int], quiet: bool = False) -> dict:
    code = load_blob(blob, py)
    text = open(src, "r", encoding="utf-8", errors="replace").read()

    wanted = identifiers(code)
    # Word-boundary search, so `foo` does not match `foobar`.
    present = set(re.findall(r"\b\w+\b", text))
    missing = sorted(n for n in wanted if n not in present)

    const_mismatch = []
    for k, v in module_assignments(code).items():
        if not isinstance(v, (int, float, str, bytes, bool)) or isinstance(v, bool):
            continue
        m = re.search(rf"^{re.escape(k)}\s*=\s*(.+)$", text, re.M)
        if not m:
            const_mismatch.append((k, v, "<absent from source>"))
            continue
        literal = m.group(1).strip().rstrip(",")
        try:
            got = eval(literal, {"__builtins__": {}}, {})  # literals only
        except Exception:
            continue
        if got != v:
            const_mismatch.append((k, v, got))

    errs = text.count("Parse error at or near")
    stubbed = parse_error_context(text)

    result = {
        "blob": blob, "src": src,
        "identifiers": len(wanted), "missing": missing,
        "const_mismatch": const_mismatch,
        "parse_errors": errs, "stubbed": stubbed,
    }
    if not quiet:
        name = os.path.basename(src)
        status = "OK" if not missing and not const_mismatch and not errs else "INCOMPLETE"
        print(f"[{status}] {name}")
        print(f"    identifiers in bytecode: {len(wanted)}   missing from source: {len(missing)}")
        if errs:
            print(f"    parse-error markers: {errs}" + (f"  (stubbed: {', '.join(stubbed)})" if stubbed else ""))
        if missing:
            shown = ", ".join(missing[:15])
            more = f" … and {len(missing) - 15} more" if len(missing) > 15 else ""
            print(f"    MISSING: {shown}{more}")
        for k, want, got in const_mismatch:
            print(f"    CONSTANT MISMATCH: {k}: bytecode={want!r} source={got!r}")
    return result


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("blob")
    ap.add_argument("src")
    ap.add_argument("--py", default="3.8")
    ap.add_argument("--batch", action="store_true", help="treat blob/src as directories")
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args()
    py = tuple(int(x) for x in args.py.split("."))

    if not args.batch:
        try:
            r = verify(args.blob, args.src, py, args.quiet)
        except MarshalError as e:
            print(f"error: {e}", file=sys.stderr)
            return 2
        return 1 if (r["missing"] or r["const_mismatch"]) else 0

    total = incomplete = failed = 0
    worst = []
    for fn in sorted(os.listdir(args.src)):
        if not fn.endswith(".py"):
            continue
        blob = os.path.join(args.blob, fn[:-3] + ".pyc")
        if not os.path.exists(blob):
            continue
        total += 1
        try:
            r = verify(blob, os.path.join(args.src, fn), py, quiet=True)
        except MarshalError:
            failed += 1
            continue
        if r["missing"] or r["const_mismatch"] or r["parse_errors"]:
            incomplete += 1
            worst.append((len(r["missing"]), r["parse_errors"], fn))
    worst.sort(reverse=True)
    print(f"checked {total} modules: {total - incomplete} clean, {incomplete} incomplete, "
          f"{failed} unreadable")
    if worst:
        print("\nmost incomplete:")
        print(f"  {'missing':>8} {'errs':>5}  module")
        for miss, errs, fn in worst[:15]:
            print(f"  {miss:8} {errs:5}  {fn}")
    return 1 if incomplete else 0


if __name__ == "__main__":
    sys.exit(main())
