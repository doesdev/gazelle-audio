#!/usr/bin/env python3
"""Generate each effect type's parameter commands and editor metadata from both panels' bytecode.

Why
---
Effect parameters travel as one command pair per effect type (``set_<effect>_conf`` /
``get_<effect>_conf``), not as a generic command, and their ranges and defaults are the panels'
code (``specs/2026-09-17-effects-and-reverb.md``). This script reads them so nobody transcribes
seventy effects by hand, and writes:

* ``refs/schemas/afx_parameters.json``: per model, every supported effect's commands as the panel
  sends them (the set's field order, the get's ``ext3``, instance parameter and reply count), each
  parameter's range, default and presentation, and every effect left out with the reason.
  ``extract_field_layouts.py --afx`` reads it to add the commands to the command schemas.
* ``web/apps/web/src/store/effect-parameters.ts``: the same, for the web app's editor.

Where each value comes from
---------------------------
* **Quadro, most effects** (``using_description``): the class's ``description``,
  ``OrderedDict([(field, [wire type, min, max, initial]), ...])``. The panel builds the set and get
  commands from it (``base.Effect._create_effect_commands``), and its docstring requires every widget
  to take its range from it and the initial values to match the firmware's. The layout is checked
  against the device-supplied format (``report_format_1.0.4.json``, the source of
  ``quadro_commands.json``); any disagreement stops the script.
* **Quadro PowerFFC and Guitar Amp**, which declare their commands by hand: the class's own command
  descriptions, ``_defaults`` and widget classes, named field by field in :data:`HAND_MAPPED`.
* **Studio+**: its ``report_format`` declares the commands. A parameter's range and default come
  from the widget class its control is built from in the effect's ``_setup_ui`` (``controlw``), the
  default from ``antelope.ui.afx.defaults`` (the state the panel resets an added effect to) where it
  has a flat entry. Where the controls are not built as a literal, the Quadro description of the
  same effect type and field is used and the parameter says so (``range_from: "quadro"``).

Only values are taken: names, ranges, defaults, value tables and display rules found in code. No
artwork, presets, per-model factory tables or impulse responses. Where the code gives no unit (most
vintage emulations draw their scales in their images), none is invented: the editor shows the value
the device takes.

Usage
-----
    afx_parameters.py <quadro PYZ_extracted> <studio PYZ_extracted> \\
        --quadro-format <device report_format JSON> --studio-format <zenstudiotb_report_format.py> \\
        [--json FILE] [--ts FILE]
"""

from __future__ import annotations

import argparse
import ast
import json
import re
import sys
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import bytecode_eval as be  # noqa: E402
from pyc_inspect import walk  # noqa: E402

QUADRO_PY = (3, 8)
STUDIO_PY = (3, 5)

#: Bit 31 of a Quadro effect read's `ext3`: read one instance, named in the request's `id` byte
#: (`base.Effect.get_data_from_device`: `request(get_cmd, self.id, ext3=0x80000000 | type_id)`).
SINGLE_INSTANCE = 0x80000000

#: Fields that are not a user's parameter. Every set carries every field, so these go back with the
#: value last read (or the default), and have no control.
HIDDEN = {
    # The sidechain source: a routing source and channel picked from a menu of the routing model
    # (`afx_menu.SideSourceMenuStripButton`), with no fixed default (`initial` is None).
    "sideSource": "sidechain",
    "sideChanN": "sidechain",
    # Follows the chains' stereo link and the per-slot link button (`VintageCompressor`), not a control.
    "linked": "link",
    # BBD Chorus carries its bypass and meter among its parameters; bypass is `set_afx_bypass`.
    "bypass": "internal",
    "peakmeter": "internal",
}

#: Hidden fields of one effect class only (the names mean something else elsewhere).
CLASS_HIDDEN = {
    # PowerFFC's `ctrl` is always sent as 0 (`Compressor.bind`) and has no widget.
    ("Compressor", "ctrl"): "internal",
    # Guitar Amp: no widget in any model; sent as stored.
    ("GuitarAmp", "midfreq"): "unused",
    ("GuitarAmp", "density"): "unused",
}

WIRE_TYPES = {
    "uint8": "u8", "ubyte": "u8", "byte": "i8", "int8": "i8",
    "uint16": "u16", "ushort": "u16", "int16": "i16", "short": "i16",
    "uint32": "u32", "uint": "u32", "int32": "i32", "int": "i32",
}

#: Effects left out, by model and type id, with the reason. Each was read and could not be driven
#: with confidence; see `specs/2026-09-17-effects-and-reverb.md`, "Effect parameters".
UNSUPPORTED: dict[str, dict[int, str]] = {
    "quadro": {
        1: "The panel's read names no instance (get_eq_configs has no parameters, so its instance id never reaches the wire), and each change sends one band; which EQ the device answers is unknown.",
        4: "Its sound is an impulse response and filters computed on the computer by the vendor's cabinet library from these settings (set_impulse_part, set_biquads), and its microphone positions are floats, which the protocol crate does not carry.",
        52: "Its ranges are set in the panel's layout code rather than declared, its note mask is a 12-byte array, and the device format's read parameters are malformed (a list, not a field table).",
        75: "Its parameters are floats, which the protocol crate does not carry; the panel also writes x2 as a float but reads it as an integer, and swaps morph and bias between its write and its read.",
    },
    "studio": {
        1: "The panel reads it in two parts with ext3 = 0 or 1 overriding the header (instances 0-7, then 8-15), and each change sends one band.",
        4: "Its sound is an impulse response and filters computed on the computer by the vendor's cabinet library from these settings (set_impulse_part, set_biquads), and its microphone positions are floats, which the protocol crate does not carry.",
    },
}


def fail(message: str) -> None:
    raise SystemExit(f"afx_parameters: {message}")


def note(message: str) -> None:
    print(f"note: {message}", file=sys.stderr)


SHORT_WORDS = {"lf", "hf", "mf", "lmf", "hmf", "hpf", "lpf", "eq", "sc", "hr", "pk", "q", "fc", "db"}


def label(field: str) -> str:
    """A readable label from a field name: `hf_boost_freq` -> `HF boost freq`, `compThreshold` -> `Comp threshold`."""
    spaced = re.sub(r"([a-z])([A-Z])", r"\1 \2", field).replace("_", " ")
    out = []
    for i, word in enumerate(spaced.split()):
        lower = word.lower()
        if lower == "db":
            out.append("dB")
        elif lower in SHORT_WORDS:
            out.append(lower.upper())
        elif i == 0:
            out.append(lower[:1].upper() + lower[1:])
        else:
            out.append(lower)
    return " ".join(out)


def number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


# ---------------------------------------------------------------------------------------------
# Reading a panel


class Panel:
    """Every class in a panel's `antelope_ui_afx_*` modules, and its `AfxType` enum."""

    def __init__(self, family: str, extracted: Path, py: tuple[int, int]):
        self.family, self.py, self.extracted = family, py, extracted
        self.modules: dict[str, list[be.ClassDef]] = {}
        self.module_names: dict[str, dict[str, Any]] = {}
        self.classes: dict[str, be.ClassDef] = {}
        for blob in sorted(extracted.glob("antelope_ui_afx_*.pyc")):
            names, classes = be.module_classes(str(blob), py)
            self.modules[blob.name] = classes
            self.module_names[blob.name] = names
            for cls in classes:
                self.classes.setdefault(cls.name, cls)
        afx = self.classes.get("AfxType")
        if afx is None:
            fail(f"{extracted}: no AfxType")
        self.afx_type = {k: v for k, v in afx.attrs.items() if number(v)}
        # Widget base classes live outside the afx modules.
        _, base = be.module_classes(str(extracted / "antelope_ui_base.pyc"), py)
        manipulator = next(c for c in base if c.name == "ValueManipulatorMixin")
        self.manipulator = {k: manipulator.attrs[k] for k in ("min_value", "max_value", "initial_value")}
        _, buttons = be.module_classes(str(extracted / "antelope_ui_buttons.pyc"), py)
        self.button = next(c for c in buttons if c.name == "Button")

    def type_id(self, cls: be.ClassDef) -> int:
        sym = cls.lookup("type_id")
        if not isinstance(sym, be.Sym) or ".AfxType." not in f".{sym.path}":
            fail(f"{cls.module} {cls.name}: type_id is {sym!r}")
        member = sym.path.rsplit(".", 1)[1]
        if member not in self.afx_type:
            fail(f"{cls.module} {cls.name}: AfxType has no {member}")
        return self.afx_type[member]

    def effects(self) -> dict[int, be.ClassDef]:
        """Effect classes by type id: those with a display `name` and a `type_id` of their own."""
        out: dict[int, be.ClassDef] = {}
        for classes in self.modules.values():
            for cls in classes:
                if isinstance(cls.attrs.get("name"), str) and isinstance(cls.attrs.get("type_id"), be.Sym):
                    type_id = self.type_id(cls)
                    if type_id in out:
                        fail(f"type {type_id} has two classes, {out[type_id].name} and {cls.name}")
                    out[type_id] = cls
        return out

    def find(self, name: str, module: str | None = None) -> be.ClassDef | None:
        """A class by name, from `module` first (names repeat across modules), then anywhere."""
        name = name.rsplit(".", 1)[-1]
        if module is not None:
            for cls in self.modules.get(module, []):
                if cls.name == name:
                    return cls
        return self.classes.get(name)

    def widget_range(self, cls: be.ClassDef) -> tuple[Any, Any, Any]:
        """A control widget's (min, max, initial): its class attributes through its bases, a `Button`'s
        maximum from its `button_values` (`Button.__init__`: `max_value = len(button_values) - 1`),
        else `ValueManipulatorMixin`'s. A maximum that only comes from `ValueManipulatorMixin` (1000)
        is not the widget's: None."""
        ancestry = cls.ancestry()
        is_button = any(a.endswith("buttons.Button") for a in ancestry)
        low = cls.lookup("min_value", self.manipulator["min_value"])
        initial = cls.lookup("initial_value", self.manipulator["initial_value"])
        high = cls.lookup("max_value")
        if is_button:
            values = cls.lookup("button_values", self.button.attrs["button_values"])
            if high is None or not number(high):
                high = len(values) - 1 if isinstance(values, list) else None
        return (low if number(low) else None, high if number(high) else None, initial if number(initial) else None)


def display_rule(panel: Panel, class_name: str, module: str) -> dict[str, Any]:
    """How a display class shows a value: divided by its `coef` (or `coeff_f`) when its
    `_update_value` divides, with the decimals and unit of its format string (`'{:.1f}'`, `'{} dB'`)."""
    cls = panel.find(class_name, module)
    if cls is None:
        fail(f"no display class {class_name} in {module}")
    formats: list[str] = []
    divides = False
    current: Any = cls
    seen: set[int] = set()
    while isinstance(current, be.ClassDef) and id(current) not in seen:
        seen.add(id(current))
        code = be.function(current.attrs.get("_update_value"))
        if code is not None:
            formats = [k for c in walk(code) for k in c.consts if isinstance(k, str) and "{" in k]
            divides = any(n in ("coef", "coeff_f") for c in walk(code) for n in c.names)
            break
        current = next((b for b in reversed(current.bases) if isinstance(b, be.ClassDef)), None)
    if len(formats) != 1:
        fail(f"{class_name}: expected one format string in _update_value, found {formats}")
    match = re.fullmatch(r"\{(?::\.(\d)f)?\}\s*(.*)", formats[0])
    if match is None:
        fail(f"{class_name}: format {formats[0]!r} is not a plain number and unit")
    rule: dict[str, Any] = {}
    scale = cls.lookup("coef", cls.lookup("coeff_f", 1))
    if divides and number(scale) and scale != 1:
        rule["scale"] = scale
    if match.group(1) is not None and int(match.group(1)) > 0:
        rule["decimals"] = int(match.group(1))
    if match.group(2):
        rule["unit"] = match.group(2)
    return rule


#: Display classes the panels pair with a control, by model, effect class and field. The pairing is
#: made in each `_setup_ui` in several shapes (a loop over pairs, a keyword, a nested call), so it is
#: named here; scale, decimals and unit are then read from the display class itself.
DISPLAYS: dict[tuple[str, str], dict[str, str]] = {
    # `PowerGate._setup_ui`: `for master, clazz in ((controlw['threshold'], PowerGateThresholdDisplay), ...)`.
    ("quadro", "PowerGate"): {"threshold": "PowerGateThresholdDisplay", "attack": "PowerGateAttackDisplay", "hold": "PowerGateHoldDisplay", "range": "PowerGateRangeDisplay", "decay": "PowerGateDecayDisplay", "gain": "PowerGateGainDisplay"},
    ("studio", "PowerGate"): {"threshold": "PowerGateThresholdDisplay", "attack": "PowerGateAttackDisplay", "hold": "PowerGateHoldDisplay", "range": "PowerGateRangeDisplay", "decay": "PowerGateDecayDisplay", "gain": "PowerGateGainDisplay"},
    # `PowerEx._setup_ui`: the same loop; its ratio has `RatioDisplay` (coef 100).
    ("quadro", "PowerEx"): {"threshold": "PowerExDisplay", "range": "PowerExDisplay", "attack": "PowerExDisplay", "decay": "PowerExDisplay", "ratio": "RatioDisplay", "gain": "PowerExDisplay"},
    # `Compressor._setup_ui`: attack and release share `CompressorDisplay` (coef 1000).
    ("quadro", "Compressor"): {"attack": "CompressorDisplay", "release": "CompressorDisplay", "ratio": "CompressorRatioDisplay", "gain": "CompressorGainDisplay", "threshold": "CompressorThreshDisplay"},
    ("studio", "Compressor"): {"attack": "CompressorDisplay", "release": "CompressorDisplay", "ratio": "CompressorRatioDisplay", "gain": "CompressorGainDisplay", "threshold": "CompressorThreshDisplay"},
    # `GuitarAmp._setup_ui`: `GuitarAmpLevelDisplay(self, GuitarAmpLevelSlider(self))`.
    ("quadro", "GuitarAmp"): {"level": "GuitarAmpLevelDisplay"},
    ("studio", "GuitarAmp"): {"level": "GuitarAmpLevelDisplay"},
}

#: Hand-declared effects: each field's control widget class, or a value table's class attribute
#: (`("options", Class, attribute)`), or an enum pair naming a menu (`("enum", NameEnum, IdEnum)`).
#: Ranges, defaults and tables are read from those classes; only the pairing is named here, from
#: each effect's `_setup_ui`.
HAND_MAPPED: dict[str, dict[str, Any]] = {
    # `compressor.Compressor._setup_ui`; `_defaults` gives the defaults.
    "Compressor": {
        "attack": "AttackPan", "release": "ReleasePan", "taw": ("options", "TawCombo", "taw_items"),
        "ratio": "RatioPan", "gain": "CompressorSlider", "threshold": "ThreshPan", "knee": "KneePan",
    },
    # `guitar_amp.GuitarAmp._setup_ui` and `models.guitar_amp_models`: every tone knob class there is
    # 0..100 starting at 50 (checked below); the level slider and the model menu are the effect's own.
    "GuitarAmp": {
        "model": ("enum", "GuitarAmpModelName", "GuitarAmpModelID"),
        "gain": "knob", "bass": "knob", "mid": "knob", "treble": "knob", "presence": "knob", "volume": "knob", "boost": "knob",
        "level": "GuitarAmpLevelSlider",
    },
}

#: Guitar Amp switches whose range depends on the model (two- or three-way per model's layout).
MODEL_DEPENDENT = {"mode1", "mode2", "mode3", "mode4", "mode5"}


def wire_fields(entry: Any) -> list[list[Any]]:
    if isinstance(entry, dict):
        return [list(f)[:2] for f in entry.get("fields", [])]
    return []


def parameter(name: str, wire: str, low: Any, high: Any, default: Any, where: str, hidden: str | None = None) -> dict[str, Any]:
    """One parameter. `where` names the effect class (`PowerGate`, `studio PowerGate`), which also selects `CLASS_HIDDEN`."""
    if wire not in WIRE_TYPES:
        fail(f"{where}.{name}: wire type {wire!r} is not an integer the protocol crate carries")
    out: dict[str, Any] = {"name": name, "label": label(name), "wire": WIRE_TYPES[wire], "min": low, "max": high, "default": default}
    hidden = hidden or HIDDEN.get(name) or CLASS_HIDDEN.get((where.split()[-1], name))
    if hidden is not None:
        out["hidden"] = hidden
        # What goes out before anything is read. The panels send 0 for PowerFFC's `ctrl`, an unlinked
        # effect's `linked` and the amp's unused fields; an amp's mode switches start at 0 in the
        # reset tables of most models. A sidechain source has no default: it is left null.
        if default is None and hidden != "sidechain":
            out["default"] = 0
    elif not (number(low) and number(high)):
        fail(f"{where}.{name}: no range ({low}..{high})")
    return out


def add_displays(panel: Panel, cls: be.ClassDef, parameters: list[dict[str, Any]]) -> None:
    table = DISPLAYS.get((panel.family, cls.name), {})
    by_name = {p["name"]: p for p in parameters}
    for field, display in table.items():
        if field not in by_name:
            fail(f"{cls.name}: display for unknown field {field}")
        by_name[field].update(display_rule(panel, display, cls.module))


def uad_ratio(panel: Panel, cls: be.ClassDef, parameters: list[dict[str, Any]]) -> None:
    """The FET compressors' ratio: four buttons as one bit mask (`UadRatio.get_value` reads
    `int(''.join(ratio_20, ratio_12, ratio_8, ratio_4), 2)`, so the first button is bit 0)."""
    ratio = next((p for p in parameters if p["name"] == "ratio"), None)
    if ratio is None:
        return
    widget = panel.find("UadRatio", cls.module)
    code = be.function(widget.attrs.get("get_value")) if widget is not None else None
    if code is None:
        fail(f"{cls.name}: no UadRatio.get_value")
    order = [k for c in walk(code) for k in c.consts if isinstance(k, str) and k.startswith("ratio_")]
    order += [k for c in walk(code) for t in c.consts if isinstance(t, tuple) for k in t if isinstance(k, str) and k.startswith("ratio_")]
    bits = list(reversed(order))  # most significant first in the join
    if [b for b in bits] != ["ratio_4", "ratio_8", "ratio_12", "ratio_20"]:
        fail(f"{cls.name}: UadRatio bit order {bits}")
    ratio["control"] = "bits"
    ratio["options"] = [[1 << i, f"{name.split('_')[1]}:1"] for i, name in enumerate(bits)]


# ---------------------------------------------------------------------------------------------
# The Quadro


def quadro(panel: Panel, device: dict[str, Any]) -> list[dict[str, Any]]:
    out = []
    for type_id, cls in sorted(panel.effects().items()):
        if type_id in UNSUPPORTED["quadro"]:
            continue
        if isinstance(cls.attrs.get("description"), dict):
            effect = quadro_description_effect(panel, cls, device)
        elif cls.name in HAND_MAPPED:
            effect = quadro_hand_mapped(panel, cls, device)
        else:
            fail(f"quadro type {type_id} ({cls.name}) is neither described, hand-mapped nor listed as unsupported")
        if cls.name in ("Uad1176", "Uad1178"):
            uad_ratio(panel, cls, effect["parameters"])
        add_displays(panel, cls, effect["parameters"])
        out.append(effect)
    return out


def check_device_layout(name: str, set_name: str, get_name: str, fields: list[list[Any]], type_id: int, device: dict[str, Any], set_prefix: list[list[str]]) -> None:
    set_entry, get_entry = device.get(set_name), device.get(get_name)
    if set_entry is None or get_entry is None:
        fail(f"{name}: the device format lacks {set_name} or {get_name}")
    same = lambda a, b: [(n, WIRE_TYPES.get(t, t)) for n, t in a] == [(n, WIRE_TYPES.get(t, t)) for n, t in b]  # noqa: E731
    if not same(wire_fields(set_entry["params"]), set_prefix + fields):
        fail(f"{name}: {set_name} in the device format differs from the panel: {set_entry['params']}")
    reply = wire_fields(get_entry.get("returns"))
    if not reply or reply[0][0] != "enabled" or not same(reply[1:], fields) or get_entry["returns"].get("count") != 1:
        fail(f"{name}: {get_name} reply in the device format differs from the panel: {get_entry.get('returns')}")
    if wire_fields(get_entry.get("params")) != [["id", "ubyte"]] or get_entry["header"].get("ext3") != type_id:
        fail(f"{name}: {get_name} in the device format is not read by instance: {get_entry}")


def quadro_description_effect(panel: Panel, cls: be.ClassDef, device: dict[str, Any]) -> dict[str, Any]:
    type_id = panel.type_id(cls)
    description = cls.attrs["description"]
    create = cls.attrs.get("effect_set_cmd")
    # `effect_set_cmd = _create_effect_commands('<stem>', type_id, description, single)[0]`
    try:
        stem = create.args[0].args[0]
    except (AttributeError, IndexError):
        fail(f"{cls.name}: effect_set_cmd is not built by _create_effect_commands: {create!r}")
    if not isinstance(stem, str):
        fail(f"{cls.name}: command stem {stem!r}")
    if cls.lookup("effect_get_single_settings") is False:
        fail(f"{cls.name}: reads every instance, which the Quadro effects here do not")
    set_name, get_name = f"set_{stem}_conf", f"get_{stem}_conf"
    fields = [[key, spec[0]] for key, spec in description.items()]
    check_device_layout(cls.name, set_name, get_name, fields, type_id, device, [["type_id", "uint8"], ["inst_id", "uint8"]])
    parameters = [parameter(key, spec[0], spec[1], spec[2], spec[3], cls.name) for key, spec in description.items()]
    return effect_entry(type_id, cls, f"{cls.module}:{cls.name}.description", set_name, get_name, SINGLE_INSTANCE | type_id, "id", 1, parameters)


def quadro_hand_mapped(panel: Panel, cls: be.ClassDef, device: dict[str, Any]) -> dict[str, Any]:
    type_id = panel.type_id(cls)
    set_name, get_name = cls.attrs.get("effect_set_cmd"), cls.attrs.get("effect_get_cmd")
    set_desc = cls.attrs.get("effect_set_cmd_desc")
    if not (isinstance(set_name, str) and isinstance(get_name, str) and isinstance(set_desc, dict)):
        fail(f"{cls.name}: no hand-declared commands")
    declared = wire_fields(set_desc["params"])
    if [(n, WIRE_TYPES.get(t)) for n, t in declared[:2]] != [("type_id", "u8"), ("inst_id", "u8")]:
        fail(f"{cls.name}: {set_name} does not start with type_id, inst_id: {declared[:3]}")
    fields = declared[2:]
    check_device_layout(cls.name, set_name, get_name, fields, type_id, device, [["type_id", "ubyte"], ["inst_id", "ubyte"]])
    parameters = hand_mapped_parameters(panel, cls, fields)
    return effect_entry(type_id, cls, f"{cls.module}:{cls.name} (hand-declared commands)", set_name, get_name, SINGLE_INSTANCE | type_id, "id", 1, parameters)


def hand_mapped_parameters(panel: Panel, cls: be.ClassDef, fields: list[list[Any]]) -> list[dict[str, Any]]:
    mapping = HAND_MAPPED[cls.name]
    defaults = cls.lookup("_defaults")
    parameters = []
    for name, wire in fields:
        spec = mapping.get(name)
        options = None
        if name in HIDDEN or name in MODEL_DEPENDENT or (cls.name, name) in CLASS_HIDDEN:
            low = high = default = None
        elif spec == "knob":
            low, high, default = guitar_amp_knob(panel)
        elif isinstance(spec, tuple) and spec[0] == "options":
            holder = panel.find(spec[1], cls.module)
            table = holder.lookup(spec[2]) if holder else None
            if not (isinstance(table, tuple) and all(isinstance(t, tuple) and len(t) == 2 for t in table)):
                fail(f"{cls.name}.{name}: {spec[1]}.{spec[2]} is not a table of (label, value)")
            options = [[value, text] for text, value in table]
            values = [v for v, _ in options]
            low, high, default = min(values), max(values), values[0]
        elif isinstance(spec, tuple) and spec[0] == "enum":
            names, ids = panel.classes.get(spec[1]), panel.classes.get(spec[2])
            if names is None or ids is None:
                fail(f"{cls.name}.{name}: no {spec[1]} / {spec[2]}")
            models = model_ids(panel, cls)
            options = [[ids.attrs[m], names.attrs[m]] for m in models]
            values = [v for v, _ in options]
            low, high, default = min(values), max(values), values[0]
        elif isinstance(spec, str):
            widget = panel.find(spec, cls.module)
            if widget is None:
                fail(f"{cls.name}.{name}: no widget class {spec}")
            low, high, default = panel.widget_range(widget)
        else:
            fail(f"{cls.name}.{name}: not mapped")
        if isinstance(defaults, dict) and number(defaults.get(name)):
            default = defaults[name]
        entry = parameter(name, wire, low, high, default, cls.name, "model" if name in MODEL_DEPENDENT else None)
        if options is not None:
            entry["control"] = "menu"
            entry["options"] = options
        parameters.append(entry)
    return parameters


def model_ids(panel: Panel, cls: be.ClassDef) -> list[str]:
    """The amp models this panel offers, as `GuitarAmpModelID` members. `GuitarAmp.model_classes` lists
    the model views in id order; the Studio+ build replaces that list in `zenstudiotb.config` with the
    ten it offers (no Bass SuperTube VR), so there only the models it names count."""
    from pyc_inspect import all_strings, load_blob

    ids = panel.classes["GuitarAmpModelID"].attrs
    members = sorted((m for m, v in ids.items() if number(v)), key=lambda m: ids[m])
    views = [v.path.rsplit(".", 1)[-1] for v in cls.lookup("model_classes") or [] if isinstance(v, be.Sym)]
    if len(views) != len(members):
        fail(f"{cls.name}: {len(views)} model views for {len(members)} model ids")
    config = panel.extracted / "zenstudiotb_config.pyc"
    if panel.family == "studio":
        offered = {name for name in all_strings(load_blob(str(config), panel.py)) if name.startswith("GA_")}
        if not offered or not offered <= set(views):
            fail(f"studio amp models in zenstudiotb.config: {sorted(offered)}")
        return [member for member, view in zip(members, views) if view in offered]
    return members


def guitar_amp_knob(panel: Panel) -> tuple[int, int, int]:
    module = "antelope_ui_afx_models_guitar_amp_models.pyc"
    knobs = [c for c in panel.modules.get(module, []) if "max_value" in c.attrs or "initial_value" in c.attrs]
    ranges = {panel.widget_range(c) for c in knobs if not any(a.endswith("buttons.Button") for a in c.ancestry())}
    if ranges != {(0, 100, 50)}:
        fail(f"guitar amp knobs are not all 0..100 from 50: {ranges}")
    return 0, 100, 50


def effect_entry(type_id: int, cls: be.ClassDef, source: str, set_name: str, get_name: str, ext3: int, instance_param: str | None, count: int, parameters: list[dict[str, Any]]) -> dict[str, Any]:
    for p in parameters:
        if "control" not in p and "hidden" not in p:
            p["control"] = "switch" if (p["min"], p["max"]) == (0, 1) else "range"
    hidden = sorted({p["hidden"] for p in parameters if "hidden" in p})
    entry: dict[str, Any] = {
        "type": type_id,
        "name": cls.attrs["name"],
        "class": cls.name,
        "source": source,
        "set": set_name,
        "get": get_name,
        "get_ext3": ext3,
        "reply_count": count,
        "parameters": parameters,
        # Partly: some parameters are sent as read and have no control (a sidechain source picked
        # from routing, or a switch whose range depends on the amp model).
        "status": "partial" if {"sidechain", "model"} & set(hidden) else "full",
    }
    if instance_param is not None:
        entry["instance_param"] = instance_param
    return entry


# ---------------------------------------------------------------------------------------------
# The Studio+


class _Namespace:
    """Resolves `constants.AfxType.X`, `powergate.PowerGate.max_instances` and module constants
    while the Studio+ `REPORT_FORMAT` is evaluated; anything else is a zero stub (only reached by
    commands this script does not read)."""

    def __init__(self, panel: Panel, path: str = ""):
        self._panel, self._path = panel, path

    def __getattr__(self, item: str) -> Any:
        panel, path = self._panel, f"{self._path}.{item}".lstrip(".")
        parts = path.split(".")
        if len(parts) == 3 and parts[1] == "AfxType":
            return panel.afx_type[parts[2]]
        if len(parts) == 3:
            cls = panel.classes.get(parts[1])
            value = cls.lookup(parts[2]) if cls is not None else None
            if number(value):
                return value
        if len(parts) == 2:
            for names in panel.module_names.values():
                if parts[1] in names and number(names[parts[1]]) and names is panel.module_names.get(f"antelope_ui_afx_{parts[0]}.pyc"):
                    return names[parts[1]]
        return _Namespace(panel, path)

    def __index__(self) -> int:
        return 0

    def __int__(self) -> int:
        return 0

    def __floordiv__(self, other: Any) -> int:
        return 0

    def __rfloordiv__(self, other: Any) -> int:
        return 0

    def __format__(self, spec: str) -> str:
        return "0"


def studio_format(path: Path, panel: Panel) -> dict[str, Any]:
    tree = ast.parse(path.read_text(encoding="utf-8"))
    ns: dict[str, Any] = {"indexed_cyclic_report": lambda *a, **k: []}
    for node in tree.body:
        if isinstance(node, ast.ImportFrom):
            for alias in node.names:
                ns[alias.asname or alias.name] = _Namespace(panel, alias.asname or alias.name)
    for node in tree.body:
        if isinstance(node, ast.Assign) and isinstance(node.targets[0], ast.Name):
            value = eval(compile(ast.Expression(node.value), str(path), "eval"), {"__builtins__": {}}, ns)  # noqa: S307 (the panel's own literal)
            ns[node.targets[0].id] = value
    return ns["REPORT_FORMAT"]["requests"]


def studio(panel: Panel, requests: dict[str, Any], quadro_effects: dict[int, dict[str, Any]]) -> list[dict[str, Any]]:
    defaults_names = panel.module_names.get("antelope_ui_afx_defaults.pyc", {})
    reset_state = defaults_names.get("defaults", {})
    gets = {entry["header"]["ext3"]: name for name, entry in requests.items() if name.startswith("get_") and entry["header"].get("ext2") == 7}
    out = []
    for type_id, cls in sorted(panel.effects().items()):
        if type_id in UNSUPPORTED["studio"]:
            continue
        get_name = gets.get(type_id)
        if get_name is None:
            fail(f"studio type {type_id} ({cls.name}): no read with ext3 {type_id}")
        reply = [list(f)[:2] for f in requests[get_name]["returns"]["fields"]]
        count = requests[get_name]["returns"].get("count")
        if reply[0][0] != "enabled" or count != cls.lookup("max_instances"):
            fail(f"studio {get_name}: reply {reply[:1]} count {count}")
        set_name = studio_set_name(requests, get_name)
        declared = [list(f)[:2] for f in requests[set_name]["params"]["fields"]]
        if [(n, WIRE_TYPES.get(t)) for n, t in declared[:2]] != [("type_id", "u8"), ("inst_id", "u8")]:
            fail(f"studio {set_name}: starts {declared[:2]}")
        fields = declared[2:]
        reply_fields = reply[1:]
        # PowerGate's reply ends with a `linked` the set does not carry (sync ignores it).
        if [f[0] for f in reply_fields[: len(fields)]] != [f[0] for f in fields] or any(f[0] != "linked" for f in reply_fields[len(fields):]):
            fail(f"studio {cls.name}: {set_name} fields {fields} and {get_name} reply {reply_fields} differ")
        state = reset_state.get(f"Sym(antelope.ui.afx.constants.AfxType.{member_of(panel, type_id)})")
        # Most entries are a memento, `{'state': {...}, 'bp': 0}`; PowerFFC's and PowerGate's are flat.
        if isinstance(state, dict) and isinstance(state.get("state"), dict):
            state = state["state"]
        if cls.name in HAND_MAPPED:
            parameters = hand_mapped_parameters(panel, cls, fields)
            if isinstance(state, dict):
                for p in parameters:
                    if number(state.get(p["name"])):
                        p["default"] = state[p["name"]]
        else:
            parameters = studio_parameters(panel, cls, fields, state if isinstance(state, dict) else {}, quadro_effects.get(type_id))
        if cls.name in ("Uad1176", "Uad1178"):
            uad_ratio(panel, cls, parameters)
        add_displays(panel, cls, parameters)
        out.append(effect_entry(type_id, cls, f"{cls.module}:{cls.name}", set_name, get_name, type_id, None, count, parameters))
    return out


def member_of(panel: Panel, type_id: int) -> str:
    return next(m for m, v in panel.afx_type.items() if v == type_id)


def studio_set_name(requests: dict[str, Any], get_name: str) -> str:
    stem = re.sub(r"^get_|_configs$", "", get_name)
    squash = stem.replace("_", "")
    candidates = [n for n in requests if n.startswith("set_") and re.sub(r"^set_|_(conf|cfg)$", "", n).replace("_", "") == squash]
    if len(candidates) != 1:
        fail(f"studio {get_name}: set command candidates {candidates}")
    return candidates[0]


def studio_parameters(panel: Panel, cls: be.ClassDef, fields: list[list[Any]], state: dict[str, Any], quadro_effect: dict[str, Any] | None) -> list[dict[str, Any]]:
    controls: dict[str, Any] = {}
    code = be.function(cls.lookup("_setup_ui"))
    if code is not None:
        local = be.evaluate(code, panel.py, {})
        found = local.get("controlw")
        if isinstance(found, dict):
            controls = found
    quadro_by_name = {p["name"]: p for p in (quadro_effect or {}).get("parameters", [])}
    parameters = []
    for name, wire in fields:
        low = high = initial = None
        range_from = None
        call = be.receiver(controls.get(name))
        widget = panel.find(call.func.path, cls.module) if isinstance(call, be.Call) and isinstance(call.func, be.Sym) else None
        if widget is not None:
            low, high, initial = panel.widget_range(widget)
        hidden = name in HIDDEN or (cls.name, name) in CLASS_HIDDEN
        if (low is None or high is None) and not hidden:
            q = quadro_by_name.get(name)
            if q is None or not (number(q["min"]) and number(q["max"])):
                fail(f"studio {cls.name}.{name}: no control widget and no Quadro description to take its range from")
            low, high, initial, range_from = q["min"], q["max"], q["default"], "quadro"
        elif not hidden and name in quadro_by_name:
            q = quadro_by_name[name]
            if (q["min"], q["max"]) != (low, high):
                note(f"studio {cls.name}.{name}: widget range {low}..{high}, Quadro description {q['min']}..{q['max']}")
        default = state[name] if number(state.get(name)) else initial
        entry = parameter(name, wire, low, high, default, f"studio {cls.name}")
        if range_from is not None:
            entry["range_from"] = range_from
        parameters.append(entry)
    return parameters


# ---------------------------------------------------------------------------------------------
# Output


def summary(effects: list[dict[str, Any]], names: dict[int, str], unsupported: dict[int, str]) -> dict[str, Any]:
    return {
        "full": sum(1 for e in effects if e["status"] == "full"),
        "partial": sum(1 for e in effects if e["status"] == "partial"),
        "unsupported": len(unsupported),
        "with_display_rules": sum(1 for e in effects if any(k in p for p in e["parameters"] for k in ("unit", "scale", "options"))),
    }


def render_ts(doc: dict[str, Any]) -> str:
    def parameter_ts(p: dict[str, Any]) -> str:
        keys = ["name", "label", "wire", "min", "max", "default", "control", "options", "scale", "decimals", "unit", "hidden", "range_from"]
        rename = {"range_from": "rangeFrom"}
        parts = []
        for k in keys:
            if k in p:
                parts.append(f"{rename.get(k, k)}: {json.dumps(p[k])}")
        return "{ " + ", ".join(parts) + " }"

    lines = [
        "// Generated by refs/tools/scripts/afx_parameters.py from both panels' bytecode; do not edit.",
        "// Every effect type whose parameters the app can edit: its set and get commands as the panel sends",
        "// them, and each parameter's range, default and presentation as the panel's code defines them. Values",
        "// only: no artwork, presets or impulse responses. A parameter with no unit shows the device's value.",
        "",
        "export type EffectFamily = \"quadro\" | \"studio\";",
        "",
        "export interface EffectParameter {",
        "  /** The field's name in the set and get commands. */",
        "  readonly name: string;",
        "  readonly label: string;",
        "  /** The field's wire type; a negative value in an unsigned field travels as its two's complement. */",
        "  readonly wire: \"u8\" | \"i8\" | \"u16\" | \"i16\" | \"u32\" | \"i32\";",
        "  readonly min: number | null;",
        "  readonly max: number | null;",
        "  /** The panel's starting value; null where it has none (a sidechain source). */",
        "  readonly default: number | null;",
        "  /** How it is edited; absent on a hidden field. `bits`: each option is one bit of the value. */",
        "  readonly control?: \"range\" | \"switch\" | \"menu\" | \"bits\";",
        "  /** [value, label] pairs for a menu, or [bit, label] for bits, from the panel's code. */",
        "  readonly options?: readonly (readonly [number, string])[];",
        "  /** Shown as value / scale, with this many decimals and this unit, as the panel's display does. */",
        "  readonly scale?: number;",
        "  readonly decimals?: number;",
        "  readonly unit?: string;",
        "  /** Sent as read, with no control: a sidechain source, a stereo-link flag, an internal or unused field, or a switch whose range depends on the model. */",
        "  readonly hidden?: \"sidechain\" | \"link\" | \"internal\" | \"unused\" | \"model\";",
        "  /** Where the range came from when not this panel's own control (the Quadro's description of the same effect). */",
        "  readonly rangeFrom?: \"quadro\";",
        "}",
        "",
        "export interface EffectDescription {",
        "  readonly type: number;",
        "  readonly name: string;",
        "  readonly set: string;",
        "  readonly get: string;",
        "  /** The reply's entries: 1 when the read names its instance in `instanceParam` (Quadro), else one per instance, entry k being instance k (Studio+). */",
        "  readonly replyCount: number;",
        "  readonly instanceParam?: string;",
        "  /** In the set command's order, after type_id and inst_id. */",
        "  readonly parameters: readonly EffectParameter[];",
        "  /** partial: some parameters have no control and go back as read. */",
        "  readonly status: \"full\" | \"partial\";",
        "}",
        "",
        "export const EFFECT_PARAMETERS: Readonly<Record<EffectFamily, ReadonlyMap<number, EffectDescription>>> = {",
    ]
    for family in ("quadro", "studio"):
        lines.append(f"  {family}: new Map<number, EffectDescription>([")
        for e in doc[family]["effects"]:
            params = ",\n".join(f"      {parameter_ts(p)}" for p in e["parameters"])
            instance = f", instanceParam: {json.dumps(e['instance_param'])}" if "instance_param" in e else ""
            lines.append(f"    [{e['type']}, {{ type: {e['type']}, name: {json.dumps(e['name'])}, set: {json.dumps(e['set'])}, get: {json.dumps(e['get'])}, replyCount: {e['reply_count']}{instance}, status: {json.dumps(e['status'])}, parameters: [")
            lines.append(params)
            lines.append("    ] }],")
        lines.append("  ]),")
    lines.append("};")
    lines.append("")
    lines.append("/** Effect types the editor leaves out, with the reason. */")
    lines.append("export const UNSUPPORTED_EFFECTS: Readonly<Record<EffectFamily, ReadonlyMap<number, string>>> = {")
    for family in ("quadro", "studio"):
        rows = "\n".join(f"    [{u['type']}, {json.dumps(u['reason'])}]," for u in doc[family]["unsupported"])
        lines.append(f"  {family}: new Map<number, string>([\n{rows}\n  ]),")
    lines.append("};")
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("quadro", type=Path, help="the Quadro panel's extracted PYZ (3.8)")
    parser.add_argument("studio", type=Path, help="the Studio+ panel's extracted PYZ (3.5)")
    parser.add_argument("--quadro-format", type=Path, required=True, help="the Quadro's device-supplied report format (JSON)")
    parser.add_argument("--studio-format", type=Path, required=True, help="the Studio+ panel's decompiled report_format.py")
    parser.add_argument("--json", type=Path)
    parser.add_argument("--ts", type=Path)
    args = parser.parse_args()

    q_panel = Panel("quadro", args.quadro, QUADRO_PY)
    s_panel = Panel("studio", args.studio, STUDIO_PY)
    device = json.loads(args.quadro_format.read_text(encoding="utf-8"))["requests"]
    q_effects = quadro(q_panel, device)
    s_effects = studio(s_panel, studio_format(args.studio_format, s_panel), {e["type"]: e for e in q_effects})

    doc: dict[str, Any] = {"generator": "refs/tools/scripts/afx_parameters.py"}
    for family, panel, effects in (("quadro", q_panel, q_effects), ("studio", s_panel, s_effects)):
        names = {t: c.attrs["name"] for t, c in panel.effects().items()}
        missing = set(names) - {e["type"] for e in effects} - set(UNSUPPORTED[family])
        if missing:
            fail(f"{family}: types neither generated nor listed as unsupported: {sorted(missing)}")
        unsupported = [{"type": t, "name": names.get(t, f"type {t}"), "reason": r} for t, r in sorted(UNSUPPORTED[family].items())]
        doc[family] = {"summary": summary(effects, names, UNSUPPORTED[family]), "effects": effects, "unsupported": unsupported}
        print(f"{family}: {doc[family]['summary']}", file=sys.stderr)

    if args.json is not None:
        args.json.write_text(json.dumps(doc, indent=1) + "\n", encoding="utf-8", newline="\n")
    if args.ts is not None:
        args.ts.write_text(render_ts(doc), encoding="utf-8", newline="\n")
    if args.json is None and args.ts is None:
        sys.stdout.write(json.dumps(doc, indent=1) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
