#!/usr/bin/env python3
"""Generate the mic emulation catalogue from the Quadro panel's own model classes.

Why
---
``set_mic_emulation(preamp_ch, target, emu_model, ch_swap, pattern)`` carries the emulation as two
small integers, and nothing in the schema says what they mean. The panel keeps one class per
emulation, named after the microphone and carrying its ``emu_idx``, grouped in a module per mic
(``edge_solo_models.py`` and friends). The indexes are *per target*: Berlin 47 FET is 2 on the Edge
Solo and 1 on the Edge Duo, so a single flat list would be wrong.

The displayed name comes from the panel's own rule (``utils.deduce_module_name_from_feature``):
lowercase the model name, split off the city it starts with, and join the city capitalised with the
rest upper-cased — with two spellings the panel special-cases. "Berlin 47 FT" really is what it
shows for the 47 FET.

Usage
-----
    mic_emulations.py <dir with the extracted PYZ> [--out FILE]

The directory is an extracted ``PYZ-00.pyz`` (see ``extract_pyz.py``). Writing the table by hand
would be 100 lines of transcription, which is exactly the kind of thing to get quietly wrong.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import pyc_dis  # noqa: E402  (a sibling script, not a package)

# MicTarget in antelope/components/preamps/models/mic_emulation/utils.py, with the module holding
# that target's models. MicTarget.ANY (0) has no models: no Antelope mic is on the preamp.
# MicIdentity also has EDGE_GO = 370, which MicTarget has no member for — the Edge Go is a USB
# microphone, not one on a preamp, so no `target` byte can name it.
TARGETS = [
    (1, "EDGE_DUO", "Edge Duo", "edge_duo"),
    (2, "VERGE", "Verge", "verge"),
    (3, "EDGE_SOLO", "Edge Solo", "edge_solo"),
    (4, "EDGE_QUADRO", "Edge Quadro", "edge_quadro"),
    (5, "ACCORD", "Accord", "accord"),
    (6, "EDGE_NOTE", "Edge Note", "edge_note"),
]

# utils._first_parts_of_a_name and utils._second_parts_of_a_name_edge_cases.
CITIES = ["berlin", "tokyo", "oxford", "vienna", "sacramento", "minnesota", "illinois", "perth", "freiburg", "aalborg", "hamburg"]
EDGE_CASES = {"47fet": " 47 FT", "m25": "/Halske M25"}

SUFFIXES = ["_Solo", "_Duo", "_Go", "_Quadro", "_EdgeNote", "_Verge", "_Accord"]

# `MicModelBaseWithPAngle`'s defaults, for a model that overrides only some of them. `pattern` runs
# from min_pattern to max_pattern and means a polar angle from min_pangle to max_pangle: +1 omni,
# 0 cardioid, -1 figure-8. Only the Edge Duo, Edge Quadro and Accord models inherit it; the others
# inherit `MicModelBase`, whose `pattern_to_pangle` returns nothing at all.
PANGLE_DEFAULTS = {"min_pattern": 0, "max_pattern": 100, "initial_value": 50, "min_pangle": 1, "max_pangle": -1}
WITH_PANGLE = {"edge_duo", "edge_quadro", "accord"}


def label_for(name: str) -> str | None:
    """The panel's displayed name for a model class, or None when it is not a mic's name."""
    feature = name.lower()
    for city in CITIES:
        if not feature.startswith(city):
            continue
        second = feature[len(city) :]
        return city.capitalize() + EDGE_CASES.get(second, f" {second.upper()}")
    return None


def class_attributes(blob: Path) -> list[tuple[str, dict[str, int]]]:
    """Every `MicModel…` class in the module with the constants its body assigns, in module order.

    A class body is a code object of its own whose attributes are `LOAD_CONST` / `STORE_NAME`
    pairs: `emu_idx`, and for a model with an adjustable polar pattern the `min_pattern` /
    `max_pattern` / `initial_value` range and sometimes `min_pangle` / `max_pangle`.
    """
    code = pyc_dis.load_blob(str(blob), (3, 8))
    out = []
    for body in pyc_dis.walk(code):
        if not body.name.startswith("MicModel") or "Base" in body.name:
            continue
        attributes: dict[str, int] = {}
        pending = None
        for instruction in pyc_dis.disassemble(body, (3, 8)):
            if instruction.opname == "LOAD_CONST":
                value = body.consts[instruction.arg] if instruction.arg < len(body.consts) else None
                pending = value if isinstance(value, int) and not isinstance(value, bool) else None
            elif instruction.opname == "STORE_NAME" and pending is not None:
                attributes[body.names[instruction.arg]] = pending
                pending = None
            else:
                pending = None
        out.append((body.name, attributes))
    return out


def strip_suffix(name: str) -> str:
    stem = name[len("MicModel") :]
    for suffix in SUFFIXES:
        if stem.endswith(suffix):
            return stem[: -len(suffix)]
    return stem


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("extracted", type=Path)
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()

    blocks = []
    patterns = []
    for value, member, display, module in TARGETS:
        blob = args.extracted / f"antelope_components_preamps_models_mic_emulation_{module}_models.pyc"
        entries = []
        ranges = []
        for name, attributes in class_attributes(blob):
            index = attributes.get("emu_idx")
            if index is None:
                continue
            label = label_for(strip_suffix(name))
            # Index 0 is the microphone itself, unemulated; the panel falls back to its own name.
            entries.append((index, display if label is None else label))
            if module in WITH_PANGLE:
                ranges.append((index, {key: attributes.get(key, fallback) for key, fallback in PANGLE_DEFAULTS.items()}))
        entries.sort()
        lines = ",\n".join(f'    "{label}"' for _, label in entries)
        expected = list(range(len(entries)))
        if [index for index, _ in entries] != expected:
            raise SystemExit(f"{member}: emu_idx values are not 0..{len(entries) - 1}: {[i for i, _ in entries]}")
        blocks.append(f"  // MicTarget.{member}\n  {value}: [\n{lines},\n  ],")
        if ranges:
            ranges.sort()
            rows = "\n".join(
                f'    {index}: {{ min: {got["min_pattern"]}, max: {got["max_pattern"]}, initial: {got["initial_value"]}, minAngle: {got["min_pangle"]}, maxAngle: {got["max_pangle"]} }},'
                for index, got in ranges
            )
            patterns.append(f"  // MicTarget.{member}\n  {value}: {{\n{rows}\n  }},")

    body = "\n".join(blocks)
    pattern_body = "\n".join(patterns)
    text = f"""// Generated by refs/tools/scripts/mic_emulations.py from the Quadro panel's own model classes.
// Do not edit by hand: re-run the script against an extracted PYZ instead.
//
// Each Antelope microphone has its own catalogue, and the indexes are per target — Berlin 47 FT is
// 2 on the Edge Solo and 1 on the Edge Duo. Index 0 is the microphone itself, unemulated.

/** `target` in `set_mic_emulation`: which Antelope microphone is on the preamp. */
export const MIC_TARGETS = {{
  ANY: 0,
  EDGE_DUO: 1,
  VERGE: 2,
  EDGE_SOLO: 3,
  EDGE_QUADRO: 4,
  ACCORD: 5,
  EDGE_NOTE: 6,
}} as const;

/** `emu_model` by target: the microphones each one can be made to sound like. */
export const MIC_EMULATIONS: Readonly<Record<number, readonly string[]>> = {{
{body}
}};

/**
 * A model's polar pattern. `pattern` runs from `min` to `max` and means an angle from `minAngle` to
 * `maxAngle`: +1 omni, 0 cardioid, -1 figure-8 (`MicModelBaseWithPAngle`). A model whose `min` and
 * `max` are equal has a fixed pattern. A microphone missing from this table has no polar pattern at
 * all — only the Edge Duo, Edge Quadro and Accord models carry one.
 */
export interface PatternRange {{
  min: number;
  max: number;
  initial: number;
  minAngle: number;
  maxAngle: number;
}}

export const MIC_PATTERNS: Readonly<Record<number, Readonly<Record<number, PatternRange>>>> = {{
{pattern_body}
}};
"""
    if args.out is None:
        sys.stdout.write(text)
    else:
        args.out.write_text(text, encoding="utf-8", newline="\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
