#!/usr/bin/env python3
"""Compare two panels' command sets.

Extracts the request and cyclic-report names from each panel's REPORT_FORMAT and reports
the union, intersection and per-device exclusives. The intersection is the shared control
surface; the exclusives are what one device can do and the other cannot.

Usage
-----
    extract_commands.py <panel_a.py> <panel_b.py> [--label-a NAME] [--label-b NAME]

Example
-------
    extract_commands.py refs/decompiled/quadro/app_report_format.py \
                        refs/decompiled/studio/zenstudiotb_report_format.py \
                        --label-a quadro --label-b studio
"""
import argparse
import re
import sys

def extract(path):
    txt = open(path).read()
    # requests block: 'requests': { ... }  (balanced-ish; stop at top-level close)
    m = re.search(r"'requests':\s*\{", txt)
    if not m:
        return set(), set()
    i = m.end() - 1
    depth = 0
    for j in range(i, len(txt)):
        if txt[j] == "{":
            depth += 1
        elif txt[j] == "}":
            depth -= 1
            if depth == 0:
                block = txt[i:j + 1]
                break
    # NOTE: the original pattern was r"'([a-z_]+)':\s*\{'header'", which requires the
    # opening brace and 'header' to be adjacent. The decompiler wraps long dicts, so any
    # entry formatted as "'name': {\n 'header'..." was silently skipped — that undercounted
    # Studio+ as 79 requests when it really declares 121. Allow whitespace between them.
    keys = re.findall(r"'([a-z_0-9]+)':\s*\{\s*'header'", block)
    # cyclic report ids
    cm = re.search(r"'cyclic_reports':\s*\{(.*?)\n\s*\}\s*,?\s*'requests'", txt, re.S)
    cyc = re.findall(r"'(0x[0-9a-fA-F]+)':", cm.group(1)) if cm else []
    return set(keys), set(cyc)

def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("panel_a")
    ap.add_argument("panel_b")
    ap.add_argument("--label-a", default="a")
    ap.add_argument("--label-b", default="b")
    args = ap.parse_args()

    a_req, a_cyc = extract(args.panel_a)
    b_req, b_cyc = extract(args.panel_b)
    if not a_req or not b_req:
        print("error: no 'requests' block found; is that a panel REPORT_FORMAT module?",
              file=sys.stderr)
        return 2

    for label, req, cyc in ((args.label_a, a_req, a_cyc), (args.label_b, b_req, b_cyc)):
        print(f"=== {label}: {len(req)} requests, cyclic ids {sorted(cyc)} ===")

    print(f"\n=== UNION ({len(a_req | b_req)}) ===", ", ".join(sorted(a_req | b_req)))
    print(f"\n=== INTERSECTION ({len(a_req & b_req)}) ===", ", ".join(sorted(a_req & b_req)))
    print(f"\n=== {args.label_a}-only ({len(a_req - b_req)}) ===", ", ".join(sorted(a_req - b_req)))
    print(f"\n=== {args.label_b}-only ({len(b_req - a_req)}) ===", ", ".join(sorted(b_req - a_req)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
