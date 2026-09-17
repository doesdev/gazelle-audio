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
import pyc_dis  # noqa: E402
from pyc_inspect import load_blob, walk  # noqa: E402

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

GUITAR_AMP_MODELS_MODULE = "antelope_ui_afx_models_guitar_amp_models.pyc"


def guitar_amp_layouts(panel: Panel, cls: be.ClassDef, parameters: list[dict[str, Any]]) -> dict[str, Any]:
    """Which parameters each amp model shows, and how its switches work.

    `GuitarAmp.model_classes` lists one view class per model; each view's `_setup_ui` builds
    `controlw = OrderedDict([(field, WidgetClass(self).move(x, y)), ...])`, and only those fields have a
    control while that model is chosen (`GuitarAmp.switch_to_model` sets the chosen view's widgets from
    the shared `params`; `bind` sends every field whatever the model). So a field a model does not list
    goes back as read. A knob must have the parameter's own range; a switch is a `Button` whose
    positions are its `button_values`, two (a switch) or three (a menu). The model menu and the level are
    the effect's own widgets and every model's.
    """
    by_name = {p["name"]: p for p in parameters}
    offered = [value for value, _ in by_name["model"]["options"]]
    ids = panel.classes["GuitarAmpModelID"].attrs
    models: dict[int, list[dict[str, Any]]] = {}
    for view in cls.lookup("model_classes") or []:
        view_cls = panel.find(view.path, GUITAR_AMP_MODELS_MODULE) if isinstance(view, be.Sym) else None
        member = view_cls.attrs.get("id") if view_cls is not None else None
        if not (isinstance(member, be.Sym) and ".GuitarAmpModelID." in f".{member.path}"):
            fail(f"{cls.name}: model view {view!r} has no GuitarAmpModelID ({member!r})")
        model_id = ids[member.path.rsplit(".", 1)[1]]
        if model_id not in offered:
            continue
        code = be.function(view_cls.attrs.get("_setup_ui"))
        controls = be.evaluate(code, panel.py, {}).get("controlw") if code is not None else None
        if not isinstance(controls, dict) or not controls:
            fail(f"{view_cls.name}: _setup_ui builds no controlw")
        stem = view_cls.name.removeprefix("GA_")
        entries: dict[str, dict[str, Any]] = {}
        for field, built in controls.items():
            call = be.receiver(built)
            widget = panel.find(call.func.path, GUITAR_AMP_MODELS_MODULE) if isinstance(call, be.Call) and isinstance(call.func, be.Sym) else None
            parameter = by_name.get(field)
            if widget is None or parameter is None or field in ("model", "level"):
                fail(f"{view_cls.name}.{field}: widget {built!r} for parameter {parameter!r}")
            low, high, initial = panel.widget_range(widget)
            if field in MODEL_DEPENDENT:
                entries[field] = amp_switch(panel, view_cls.name, stem, field, widget, (low, high, initial))
            elif (low, high) != (parameter["min"], parameter["max"]) or "hidden" in parameter:
                fail(f"{view_cls.name}.{field}: {widget.name} is {low}..{high}, the parameter {parameter['min']}..{parameter['max']}")
            else:
                entries[field] = {"name": field}
        models[model_id] = [entries[p["name"]] for p in parameters if p["name"] in entries]
    if sorted(models) != sorted(offered):
        fail(f"{cls.name}: layouts for models {sorted(models)}, menu offers {sorted(offered)}")
    used = {c["name"] for layout in models.values() for c in layout}
    return {"by": "model", "fields": [p["name"] for p in parameters if p["name"] in used], "models": {str(k): v for k, v in sorted(models.items())}}


def amp_switch(panel: Panel, view: str, stem: str, field: str, widget: be.ClassDef, widget_range: tuple[Any, Any, Any]) -> dict[str, Any]:
    """An amp model's mode switch: a `Button`, its value the index of its position (`Button.get_value_as_name`
    reads `button_values[value]`). Named by its class where the class says what it is (`DarkfaceBrightButton`
    is "Bright"); positions are named only where the class declares its own `button_values` (the base
    class's are `released`/`pressed` image states, and a subclass inherits another switch's images)."""
    if not any(a.endswith("buttons.Button") for a in widget.ancestry()):
        fail(f"{view}.{field}: {widget.name} is not a button")
    values = widget.lookup("button_values", panel.button.attrs["button_values"])
    low, high, initial = widget_range
    if not isinstance(values, list) or (low, high) != (0, len(values) - 1) or initial != 0:
        fail(f"{view}.{field}: {widget.name} positions {values!r}, range {widget_range}")
    number_label = f"Mode {field.removeprefix('mode')}"
    word = widget.name
    for suffix in ("ThreeWay", "Button", "Switch"):
        word = word.removesuffix(suffix)
    for prefix in (stem, "GuitarAmp"):
        word = word.removeprefix(prefix)
    word = re.sub(r"([a-z])([A-Z])", r"\1 \2", word).strip()
    entry: dict[str, Any] = {"name": field, "label": number_label if word in ("", "Mode") else f"{word} ({number_label.lower()})"}
    if len(values) == 2:
        entry.update(control="switch", min=0, max=1)
    elif len(values) == 3:
        declared = "button_values" in widget.attrs
        names = [v[0] if isinstance(v, tuple) else None for v in values]
        if declared and not all(isinstance(n, str) for n in names):
            fail(f"{view}.{field}: {widget.name} positions {values!r}")
        options = [[i, str(names[i]).capitalize() if declared else f"Position {i + 1}"] for i in range(len(values))]
        entry.update(control="menu", min=0, max=len(values) - 1, options=options)
    else:
        fail(f"{view}.{field}: {widget.name} has {len(values)} positions")
    return entry


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
    layouts = guitar_amp_layouts(panel, cls, parameters) if cls.name == "GuitarAmp" else None
    return effect_entry(type_id, cls, f"{cls.module}:{cls.name} (hand-declared commands)", set_name, get_name, SINGLE_INSTANCE | type_id, "id", 1, parameters, layouts)


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


def effect_entry(type_id: int, cls: be.ClassDef, source: str, set_name: str, get_name: str, ext3: int, instance_param: str | None, count: int, parameters: list[dict[str, Any]], layouts: dict[str, Any] | None = None) -> dict[str, Any]:
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
        # from routing, or a switch whose range depends on the amp model and no layout says how).
        "status": "partial" if "sidechain" in hidden or ("model" in hidden and layouts is None) else "full",
    }
    if layouts is not None:
        missing = {p["name"] for p in parameters if p.get("hidden") == "model"} - set(layouts["fields"])
        if missing:
            fail(f"{cls.name}: no model uses {sorted(missing)}")
        entry["layouts"] = layouts
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
        if cls.name == "Equalizer":
            out.append(studio_equalizer(panel, cls, requests, get_name))
            continue
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
        layouts = guitar_amp_layouts(panel, cls, parameters) if cls.name == "GuitarAmp" else None
        out.append(effect_entry(type_id, cls, f"{cls.module}:{cls.name}", set_name, get_name, type_id, None, count, parameters, layouts))
    return out


#: The Equalizer's band fields as the editor names them (the code's names are `freq`, `qual`, `gain`, `ftype`).
EQ_LABELS = {"freq": "Frequency", "qual": "Q", "gain": "Gain", "ftype": "Type"}

#: What the panel's EQ plot calls each filter name's family (`Equalizer.redraw_plot`: names starting `low` are
#: `LF`, `high` `HF`, `lpf` `LP`, `hpf` `HP`, the rest `peak`), as the editor labels them.
EQ_FILTER_LABELS = {"LF": "Low shelf", "HF": "High shelf", "LP": "Low-pass", "HP": "High-pass", "peak": "Peak"}


def studio_equalizer(panel: Panel, cls: be.ClassDef, requests: dict[str, Any], get_name: str) -> dict[str, Any]:
    """The Studio+ Equalizer: five bands per instance, read in parts and set one band per command.

    * **Read** (`zenstudiotb.sync.sync_eqs`): `get_eq_configs` once per part, `ext3` = the part index in
      place of the header's type (`range(EQ_PARTS)`, `EQ_PARTS = 2`); part p's entries are instances
      p * 8 + k (`set_all` pairs `widgets[:8]` / `widgets[8:]` with the entries). Each entry is
      `biquads[num_strips]` then `enabled`.
    * **Write** (`zenstudiotb.bind.bind_eq`): each strip's `value_changed` sends `set_eq_conf(type, instance,
      strip index, freq, qual, gain, adapted ftype)`, with the sender keyed by (instance, strip): one band
      per command.
    * **Controls** (`Equalizer._setup_ui`'s `strips_config`, one tuple of widget classes per band, passed to
      `EqStrip(self, freq_class, gain_class, qual_class, ftype_class)`): a pan with a range is a control, a
      pan without one (`DummyQualityFactorPan`) a value kept as read, and the filter button a menu of the wire
      values its positions map to (`bind._adapt_ftype`), or a value kept as read where every position maps
      to one value (the peak bands).
    * **Pass filters** (`EqStrip._setup_events._ftype_handler`): at the button's position 3 the gain is
      disabled and set to 0.
    """
    type_id = panel.type_id(cls)
    request = requests[get_name]
    returns = request["returns"]
    num_parts, num_strips, max_instances = (cls.lookup(k) for k in ("num_parts", "num_strips", "max_instances"))
    if not all(number(v) for v in (num_parts, num_strips, max_instances)) or returns.get("count") != max_instances // num_parts:
        fail(f"studio {get_name}: count {returns.get('count')} for {max_instances} instances in {num_parts} parts")
    reply = [list(f)[:2] for f in returns["fields"]]
    if [f[0] for f in reply] != ["biquads", "enabled"] or not isinstance(reply[0][1], dict) or reply[0][1].get("count") != num_strips:
        fail(f"studio {get_name}: reply {reply}")
    band_fields = [list(f)[:2] for f in reply[0][1]["fields"]]
    set_name = studio_set_name(requests, get_name)
    declared = [list(f)[:2] for f in requests[set_name]["params"]["fields"]]
    if [(n, WIRE_TYPES.get(t)) for n, t in declared[:3]] != [("type_id", "u8"), ("inst_id", "u8"), ("strip_id", "u8")] or [f[0] for f in declared[3:]] != [f[0] for f in band_fields]:
        fail(f"studio {set_name}: {declared} against the reply's bands {band_fields}")
    check_equalizer_io(panel)

    # The widget classes of each band, by field.
    strip = panel.find("EqStrip", cls.module)
    # Read from the class body's code objects: a method with a closure (`super()`) is built by MAKE_CLOSURE on
    # 3.5, which the evaluator does not model.
    methods = {c.name: c for c in (strip.body.children if strip is not None else [])}
    strip_init, strip_ui = methods.get("__init__"), methods.get("_setup_ui")
    if strip_init is None or strip_ui is None:
        fail("EqStrip: no __init__ or _setup_ui")
    arguments = list(strip_ui.varnames[1:strip_ui.argcount])
    if list(strip_init.varnames[2:strip_init.argcount]) != arguments:
        fail(f"EqStrip.__init__ {strip_init.varnames} does not pass {arguments} in order")
    controls = be.evaluate(strip_ui, panel.py, {}).get("controlw")
    field_argument = {}
    for field, built in (controls or {}).items():
        call = be.receiver(built)
        if not (isinstance(call, be.Call) and isinstance(call.func, be.Sym) and call.func.path in arguments):
            fail(f"EqStrip.{field}: built from {built!r}")
        field_argument[field] = arguments.index(call.func.path)
    if sorted(field_argument) != sorted(f[0] for f in band_fields):
        fail(f"EqStrip controls {sorted(field_argument)} against the bands' fields {band_fields}")
    config = be.evaluate(be.function(cls.attrs.get("_setup_ui")), panel.py, {}).get("strips_config")
    if not (isinstance(config, tuple) and len(config) == num_strips and all(isinstance(b, tuple) and len(b) == len(arguments) for b in config)):
        fail(f"Equalizer.strips_config {config!r}")

    adapt = eq_filter_values(panel)
    pass_position = eq_pass_position(panel, strip)
    displays = eq_displays(panel, cls.module)
    bands = []
    for index, classes in enumerate(config):
        band = []
        for field, wire in band_fields:
            widget = panel.find(classes[field_argument[field]].path, cls.module)
            if widget is None:
                fail(f"Equalizer band {index} {field}: no class {classes[field_argument[field]]!r}")
            band.append(eq_parameter(panel, field, wire, widget, adapt, index))
        by_name = {q["name"]: q for q in band}
        ftype = panel.find(classes[field_argument["ftype"]].path, cls.module)
        names = ftype.lookup("button_values")
        if "options" in by_name["ftype"] and len(names) > pass_position:
            # The position the handler tests is a pass filter: the gain goes to 0 and cannot be changed there.
            by_name["gain"]["off_when"] = {"name": "ftype", "values": [adapt(names[pass_position][0])]}
        for field, rule in displays.items():
            if "hidden" not in by_name[field]:
                by_name[field].update(rule)
        bands.append(band)
    for band in bands:
        for q in band:
            if "control" not in q and "hidden" not in q:
                q["control"] = "range"
    return {
        "type": type_id,
        "name": cls.attrs["name"],
        "class": cls.name,
        "source": f"{cls.module}:{cls.name}, zenstudiotb_sync.pyc:sync_eqs, zenstudiotb_bind.pyc:bind_eq",
        "set": set_name,
        "get": get_name,
        "get_ext3": type_id,
        "reply_count": max_instances // num_parts,
        "read_parts": num_parts,
        "parameters": [],
        "bands": {"param": "strip_id", "list": "biquads", "bands": bands},
        "status": "full",
    }


def eq_parameter(panel: Panel, field: str, wire: str, widget: be.ClassDef, adapt: Any, index: int) -> dict[str, Any]:
    if any(a.endswith("buttons.Button") for a in widget.ancestry()):
        names = widget.lookup("button_values")
        if not isinstance(names, list) or not all(isinstance(n, tuple) and isinstance(n[0], str) for n in names):
            fail(f"Equalizer band {index} {field}: {widget.name} positions {names!r}")
        values: dict[int, str] = {}
        for name, _ in names:
            values.setdefault(adapt(name), eq_filter_label(name))
        default = adapt(names[0][0])
        if len(values) == 1:
            entry = parameter(field, wire, None, None, default, "Equalizer", "internal")
        else:
            entry = parameter(field, wire, min(values), max(values), default, "Equalizer")
            entry["control"] = "menu"
            entry["options"] = [[v, t] for v, t in values.items()]
    else:
        low, high, initial = panel.widget_range(widget)
        if high is None or widget.lookup("max_value") is None:
            # A pan with no range of its own (`DummyQualityFactorPan`): the value is shown greyed and sent as read.
            entry = parameter(field, wire, None, None, initial, "Equalizer", "internal")
        else:
            entry = parameter(field, wire, low, high, initial, "Equalizer")
    entry["label"] = EQ_LABELS[field]
    return entry


def eq_filter_label(name: str) -> str:
    family = "LF" if name.startswith("low") else "HF" if name.startswith("high") else "LP" if name == "lpf" else "HP" if name == "hpf" else "peak"
    return EQ_FILTER_LABELS[family]


def if_chain(code: Any, py: tuple[int, int]) -> list[tuple[str, Any, Any]]:
    """An `a if test else b if …` chain on one name, as (test, operand, result) with test `startswith` or
    `==`, and a final ("else", None, result)."""
    ins = [i for i in pyc_dis.disassemble(code, py) if i.opname != "EXTENDED_ARG"]
    out: list[tuple[str, Any, Any]] = []
    i = 0
    while i < len(ins):
        op = ins[i].opname
        if op in ("LOAD_FAST", "LOAD_DEREF") and i + 5 < len(ins) and ins[i + 1].opname in ("LOAD_ATTR", "LOAD_METHOD") and code.names[ins[i + 1].arg] == "startswith":
            operand, result = code.consts[ins[i + 2].arg], code.consts[ins[i + 5].arg]
            out.append(("startswith", operand, result))
            i += 6
        elif op in ("LOAD_FAST", "LOAD_DEREF") and i + 4 < len(ins) and ins[i + 1].opname == "LOAD_CONST" and ins[i + 2].opname == "COMPARE_OP" and ins[i + 2].arg == 2:
            out.append(("==", code.consts[ins[i + 1].arg], code.consts[ins[i + 4].arg]))
            i += 5
        elif op == "LOAD_CONST" and i + 1 < len(ins) and ins[i + 1].opname == "STORE_FAST" and out and out[-1][0] != "else":
            out.append(("else", None, code.consts[ins[i].arg]))
            break
        else:
            i += 1
    return out


def const_set(consts: Any) -> set[Any]:
    """A code object's constants that can be compared (numbers and strings)."""
    return {k for k in consts if isinstance(k, (int, float, str))}


def find_code(path: Path, py: tuple[int, int], name: str) -> Any:
    found = [c for c in walk(load_blob(str(path), py)) if c.name == name]
    if len(found) != 1:
        fail(f"{path.name}: {len(found)} code objects named {name}")
    return found[0]


def eq_filter_values(panel: Panel) -> Any:
    """The filter button's position name to the wire's `ftype` (`zenstudiotb.bind`'s `_adapt_ftype`), checked
    against the read's reverse (`zenstudiotb.sync.adapt_ftype`)."""
    forward = if_chain(find_code(panel.extracted / "zenstudiotb_bind.pyc", panel.py, "_adapt_ftype"), panel.py)
    reverse = if_chain(find_code(panel.extracted / "zenstudiotb_sync.pyc", panel.py, "adapt_ftype"), panel.py)
    if len(forward) < 3 or forward[-1][0] != "else" or reverse[-1][0] != "else":
        fail(f"_adapt_ftype chains {forward} / {reverse}")

    def adapt(name: str) -> int:
        for test, operand, result in forward:
            if test == "else" or (test == "startswith" and name.startswith(operand)) or (test == "==" and name == operand):
                return result
        fail(f"_adapt_ftype: no value for {name}")

    for test, value, name in reverse:
        if test == "==" and adapt(name) != value:
            fail(f"adapt_ftype: {value} reads back as {name}, which is sent as {adapt(name)}")
    if adapt(reverse[-1][2]) in [v for t, v, _ in reverse if t == "=="]:
        fail(f"adapt_ftype: its default {reverse[-1][2]} is sent as a value it names otherwise")
    return adapt


def eq_pass_position(panel: Panel, strip: be.ClassDef) -> int:
    """The filter button position at which `_ftype_handler` disables the gain and sets it to 0."""
    code = find_code(panel.extracted / "antelope_ui_afx_equalizer.pyc", panel.py, "_ftype_handler")
    ins = [i for i in pyc_dis.disassemble(code, panel.py) if i.opname != "EXTENDED_ARG"]
    if not (len(ins) > 3 and ins[0].opname == "LOAD_FAST" and code.varnames[ins[0].arg] == "value" and ins[1].opname == "LOAD_CONST" and ins[2].opname == "COMPARE_OP" and ins[2].arg == 2):
        fail("_ftype_handler does not start `if value == <position>`")
    if not {"set_enabled", "set_value"} <= set(code.names) or 0 not in code.consts:
        fail(f"_ftype_handler: {code.names} {code.consts}")
    return code.consts[ins[1].arg]


def eq_displays(panel: Panel, module: str) -> dict[str, dict[str, Any]]:
    """How the strip shows its values: gain and Q as value / 100 (`EqDisplay._update_value`, `round(value / 100,
    2)`; `QualityFactorPan.value_to_text` `'{:.2f}'`), frequency in thousands above 1000 with a `k`
    (`EqFreqDisplay._update_value`)."""
    def consts(class_name: str, method: str) -> list[Any]:
        cls = panel.find(class_name, module)
        code = be.function(cls.attrs.get(method)) if cls is not None else None
        if code is None:
            fail(f"{class_name}.{method} not found")
        return [k for c in walk(code) for k in c.consts]

    if not {100, 2} <= const_set(consts("EqDisplay", "_update_value")):
        fail("EqDisplay._update_value is not round(value / 100, 2)")
    if not ({"{:.2f}", 100} <= const_set(consts("QualityFactorPan", "value_to_text"))):
        fail("QualityFactorPan.value_to_text is not '{:.2f}' of value / 100")
    freq = consts("EqFreqDisplay", "_update_value")
    if not ({1000, "{}k", "{:.1f}"} <= const_set(freq)):
        fail(f"EqFreqDisplay._update_value: {freq}")
    return {"gain": {"scale": 100}, "qual": {"scale": 100, "decimals": 2}, "freq": {"kilo": True}}


def check_equalizer_io(panel: Panel) -> None:
    """What `sync_eqs` and `bind_eq` do, checked where it matters: two parts in ext3, one band per sender."""
    sync = find_code(panel.extracted / "zenstudiotb_sync.pyc", panel.py, "sync_eqs")
    if not ({"get_eq_configs", "ext3", 2} <= const_set(sync.consts) and "EQ_PARTS" in sync.varnames and "range" in sync.names):
        fail(f"sync_eqs: {sync.consts} {sync.varnames}")
    parts = [c for c in walk(sync) if c.name == "set_all"]
    if len(parts) != 1 or 8 not in const_set(parts[0].consts):
        fail(f"sync_eqs.set_all does not split the instances at 8: {[c.consts for c in parts]}")
    bind = find_code(panel.extracted / "zenstudiotb_bind.pyc", panel.py, "bind_eq")
    if not ({"set_eq_conf", "sender"} <= const_set(bind.consts) and {"strip_id"} <= set(bind.varnames)):
        fail(f"bind_eq: {bind.consts} {bind.varnames}")


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
        "with_display_rules": sum(1 for e in effects if any(k in p for p in e["parameters"] + [q for band in e.get("bands", {}).get("bands", []) for q in band] for k in ("unit", "scale", "options"))),
    }


def render_ts(doc: dict[str, Any]) -> str:
    def parameter_ts(p: dict[str, Any]) -> str:
        keys = ["name", "label", "wire", "min", "max", "default", "control", "options", "scale", "decimals", "unit", "kilo", "hidden", "range_from", "off_when"]
        rename = {"range_from": "rangeFrom", "off_when": "offWhen"}
        parts = []
        for k in keys:
            if k == "off_when" and k in p:
                parts.append(f"offWhen: {{ name: {json.dumps(p[k]['name'])}, values: {json.dumps(p[k]['values'])} }}")
            elif k in p:
                parts.append(f"{rename.get(k, k)}: {json.dumps(p[k])}")
        return "{ " + ", ".join(parts) + " }"

    def layout_ts(c: dict[str, Any]) -> str:
        return "{ " + ", ".join(f"{k}: {json.dumps(c[k])}" for k in ("name", "label", "control", "min", "max", "options") if k in c) + " }"

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
        "  /** Shown in thousands above 1000 with a k (5k, 11.3k), as the panel's frequency display does. */",
        "  readonly kilo?: boolean;",
        "  /** Sent as read, with no control: a sidechain source, a stereo-link flag, an internal or unused field, or a switch whose control depends on the model (see `EffectDescription.layouts`). */",
        "  readonly hidden?: \"sidechain\" | \"link\" | \"internal\" | \"unused\" | \"model\";",
        "  /** Where the range came from when not this panel's own control (the Quadro's description of the same effect). */",
        "  readonly rangeFrom?: \"quadro\";",
        "  /** While another field of the same band holds one of these values (a pass filter), this one is 0 and cannot be changed. */",
        "  readonly offWhen?: { readonly name: string; readonly values: readonly number[] };",
        "}",
        "",
        "/** One control in a model's layout: the parameter it drives, and what the model changes about it (a switch's name, positions and range). */",
        "export interface EffectControlLayout {",
        "  readonly name: string;",
        "  readonly label?: string;",
        "  readonly control?: NonNullable<EffectParameter[\"control\"]>;",
        "  readonly min?: number;",
        "  readonly max?: number;",
        "  readonly options?: readonly (readonly [number, string])[];",
        "}",
        "",
        "/** Controls that depend on another parameter's value (the Guitar Amp's model), as the panel shows one view per model. */",
        "export interface EffectLayouts {",
        "  /** The parameter whose value picks the layout. */",
        "  readonly by: string;",
        "  /** Every parameter some layout shows: one the chosen layout does not list has no control and goes back as read. */",
        "  readonly fields: readonly string[];",
        "  /** The controls each value shows, in the set command's order. */",
        "  readonly models: ReadonlyMap<number, readonly EffectControlLayout[]>;",
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
        "  /** Read in this many parts, part p in ext3 in place of the type, its entries being instances p * replyCount + k. */",
        "  readonly readParts?: number;",
        "  /** Set one band per command: `param` names the band, and each band has its own parameters, read from each entry's `list`. */",
        "  readonly bands?: { readonly param: string; readonly list: string; readonly bands: readonly (readonly EffectParameter[])[] };",
        "  /** Which of the parameters show, by another parameter's value; absent when every control always shows. */",
        "  readonly layouts?: EffectLayouts;",
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
            if params:
                lines.append(params)
            if "bands" in e:
                rows = ",\n".join("      [\n" + ",\n".join(f"        {parameter_ts(p)}" for p in band) + "\n      ]" for band in e["bands"]["bands"])
                lines.append(f"    ], readParts: {e['read_parts']}, bands: {{ param: {json.dumps(e['bands']['param'])}, list: {json.dumps(e['bands']['list'])}, bands: [\n{rows},\n    ] }} }}],")
            elif "layouts" in e:
                layouts = e["layouts"]
                models = ",\n".join(f"      [{model}, [{', '.join(layout_ts(c) for c in controls)}]]" for model, controls in layouts["models"].items())
                lines.append(f"    ], layouts: {{ by: {json.dumps(layouts['by'])}, fields: {json.dumps(layouts['fields'])}, models: new Map<number, readonly EffectControlLayout[]>([\n{models},\n    ]) }} }}],")
            else:
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
