#!/usr/bin/env python3
"""Compare two command schemas produced by extract_field_layouts.py, command by command.

    python3 refs/tools/scripts/compare_schemas.py refs/schemas/quadro_commands.json OTHER.json

Prints each in-scope command whose header (report_id, ext2, ext3), payload id, params or returns
differ, and cyclic report fields that differ. Used to check the committed schema, extracted from the
panel's bundled format, against the format a device supplies to the manager.
"""
import json, sys


def load(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def main():
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    a, b = load(sys.argv[1]), load(sys.argv[2])
    print(f"A: {sys.argv[1]} (version {a.get('report_version')}, authoritative {a.get('authoritative')})")
    print(f"B: {sys.argv[2]} (version {b.get('report_version')}, authoritative {b.get('authoritative')})")
    names = sorted(set(a["commands"]) | set(b["commands"]))
    differing = 0
    for name in names:
        ca, cb = a["commands"].get(name), b["commands"].get(name)
        if ca is None or cb is None:
            print(f"- {name}: only in {'B' if ca is None else 'A'}")
            differing += 1
            continue
        for key in ("report_id", "ext2", "ext3", "payload_id", "params", "returns", "auto_send_notification"):
            if ca.get(key) != cb.get(key):
                print(f"- {name}.{key}:\n    A {json.dumps(ca.get(key))}\n    B {json.dumps(cb.get(key))}")
                differing += 1
    for rid in sorted(set(a.get("cyclic_reports", {})) | set(b.get("cyclic_reports", {}))):
        fa = {f["name"]: f for f in a.get("cyclic_reports", {}).get(rid, {}).get("fields", [])}
        fb = {f["name"]: f for f in b.get("cyclic_reports", {}).get(rid, {}).get("fields", [])}
        order_a = list(fa)
        order_b = list(fb)
        if order_a != order_b:
            print(f"- cyclic {rid}: field order or set differs ({len(order_a)} vs {len(order_b)} fields)")
            differing += 1
        for field in sorted(set(fa) & set(fb)):
            if fa[field] != fb[field]:
                print(f"- cyclic {rid}.{field}:\n    A {json.dumps(fa[field])}\n    B {json.dumps(fb[field])}")
                differing += 1
    print(f"{differing} difference(s) across {len(names)} commands")


if __name__ == "__main__":
    main()
