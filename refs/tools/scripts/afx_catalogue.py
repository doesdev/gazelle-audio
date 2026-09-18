#!/usr/bin/env python3
"""Generate the effect catalogue (AFX type id -> the panel's name) from both panels' bytecode.

Why
---
A chain slot on the wire is ``{type, inst}``: ``type`` is ``antelope.ui.afx.constants.AfxType``,
``inst`` an instance of that type. The schemas say nothing about what a type id is called. Each
panel keeps one class per effect with ``name = '<display name>'`` and
``type_id = constants.AfxType.<MEMBER>`` in its class body (``antelope/ui/afx/*.py``), so this
script reads the enum and those class bodies, per panel, rather than anyone transcribing 70 names.

Only the names are taken: no parameter descriptions, defaults or presets (those are the vendor's
data, and the app does not send effect parameters).

Usage
-----
    afx_catalogue.py <quadro extracted PYZ> <studio extracted PYZ> [--out FILE]

Each directory is an extracted ``PYZ-00.pyz`` (see ``extract_pyz.py``): Quadro bytecode is 3.8,
Studio+ 3.5.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import pyc_dis  # noqa: E402  (a sibling script, not a package)
from no_dashes import refuse_dashes  # noqa: E402

CONSTANTS = "antelope_ui_afx_constants.pyc"


def afx_type(blob: Path, py: tuple[int, int]) -> dict[str, int]:
    """The members of `AfxType`: `LOAD_CONST <int>` / `STORE_NAME <member>` pairs in its class body."""
    code = pyc_dis.load_blob(str(blob), py)
    body = pyc_dis.find(code, "AfxType")
    if body is None:
        raise SystemExit(f"{blob}: no AfxType class")
    members: dict[str, int] = {}
    pending = None
    for instruction in pyc_dis.disassemble(body, py):
        if instruction.opname == "LOAD_CONST":
            value = body.consts[instruction.arg]
            pending = value if isinstance(value, int) and not isinstance(value, bool) else None
        elif instruction.opname == "STORE_NAME" and pending is not None:
            members[body.names[instruction.arg]] = pending
            pending = None
        else:
            pending = None
    return members


def effect_classes(blob: Path, py: tuple[int, int]) -> list[tuple[str, str, str]]:
    """Every class body in a module assigning both `name = '<str>'` and `type_id = constants.AfxType.<M>`.

    Returns (class, display name, AfxType member). A class inheriting either attribute is left
    out: the id and the name must be the class's own to be read here.
    """
    code = pyc_dis.load_blob(str(blob), py)
    out = []
    for body in pyc_dis.walk(code):
        if not body.name[:1].isupper():
            continue
        name = member = None
        loaded: list[tuple[str, object]] = []
        for instruction in pyc_dis.disassemble(body, py):
            op = instruction.opname
            if op == "LOAD_CONST":
                loaded = [("const", body.consts[instruction.arg])]
            elif op in ("LOAD_NAME", "LOAD_GLOBAL"):
                loaded = [("name", body.names[instruction.arg])]
            elif op == "LOAD_ATTR":
                loaded.append(("attr", body.names[instruction.arg]))
            elif op == "STORE_NAME":
                target = body.names[instruction.arg]
                if target == "name" and len(loaded) == 1 and loaded[0][0] == "const" and isinstance(loaded[0][1], str):
                    name = loaded[0][1]
                elif target == "type_id" and len(loaded) == 3 and loaded[1] == ("attr", "AfxType") and loaded[2][0] == "attr":
                    member = str(loaded[2][1])
                loaded = []
            else:
                loaded = []
        if name is not None and member is not None:
            out.append((body.name, name, member))
    return out


def catalogue(extracted: Path, py: tuple[int, int]) -> dict[int, str]:
    members = afx_type(extracted / CONSTANTS, py)
    names: dict[int, str] = {}
    for blob in sorted(extracted.glob("antelope_ui_afx_*.pyc")):
        for cls, display, member in effect_classes(blob, py):
            if member not in members:
                raise SystemExit(f"{blob.name}: {cls} names AfxType.{member}, which the enum lacks")
            type_id = members[member]
            if type_id in names and names[type_id] != display:
                raise SystemExit(f"{blob.name}: {cls} gives type {type_id} a second name ({display!r} and {names[type_id]!r})")
            names[type_id] = display
    return names


def render(family: str, names: dict[int, str]) -> str:
    rows = "\n".join(f"    [{type_id}, {display!r}]," for type_id, display in sorted(names.items()))
    return f"  {family}: new Map<number, string>([\n{rows.replace(chr(39), chr(34))}\n  ]),"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("quadro", type=Path)
    parser.add_argument("studio", type=Path)
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()

    quadro = catalogue(args.quadro, (3, 8))
    studio = catalogue(args.studio, (3, 5))
    # The ids are the firmware's, so a type both panels name must be the same effect. The names may
    # still differ in spelling between releases; say so rather than pick one silently.
    for type_id in sorted(set(quadro) & set(studio)):
        if quadro[type_id] != studio[type_id]:
            print(f"note: type {type_id} is {quadro[type_id]!r} on the Quadro and {studio[type_id]!r} on the Studio+", file=sys.stderr)

    text = (
        "// Generated by refs/tools/scripts/afx_catalogue.py from both panels' bytecode; do not edit.\n"
        "// Each effect type id (`AfxType`, the `type` of a chain slot) and the name the vendor panel shows for\n"
        "// it, read from the effect classes' own `name` and `type_id`. Names only: no parameters or presets.\n"
        "\n"
        "export const EFFECT_NAMES: Readonly<Record<\"quadro\" | \"studio\", ReadonlyMap<number, string>>> = {\n"
        f"{render('quadro', quadro)}\n"
        f"{render('studio', studio)}\n"
        "};\n"
    )
    text = refuse_dashes(text, "effect-catalogue.ts")
    if args.out is None:
        sys.stdout.write(text)
    else:
        args.out.write_text(text, encoding="utf-8", newline="\n")
        print(f"wrote {args.out}: {len(quadro)} Quadro and {len(studio)} Studio+ effects", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
