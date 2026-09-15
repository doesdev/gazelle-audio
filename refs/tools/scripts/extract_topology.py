#!/usr/bin/env python3
"""Extract device topology (routing groups, mixers) from the vendor panels' bytecode.

Why
---
The command schemas carry no topology: which inputs and outputs exist, what they are called,
how many channels each has, and how many mixers there are. The panels define it in small
routings modules (spec §8, corrected by the 2026-09-15 research): tuples of
``(RoutingInputGroupType|RoutingOutputGroupType, name, channels, colour)``. Nothing is
decompiled for those modules, and a decompiler's guess is not provenance anyway, so this reads
the bytecode directly: it evaluates the module-level instructions (constants, names, enum
attribute loads, tuple, list and map builds, a little integer arithmetic) with ``pyc_dis``'s
decoder and stops at the first function definition. Anything it cannot fully resolve is an
error, not a silent gap.

Usage
-----
    extract_topology.py --family quadro --out refs/schemas/quadro_topology.json
    extract_topology.py --family studio --out refs/schemas/studio_topology.json

Blobs are read from ``refs/extracted/<family>/PYZ_extracted/`` (gitignored; regenerate them
with the extraction steps in ``.agent/reference/decompilation.md``). The output records each
blob's path and SHA-256 so the result can be checked against anyone's copy.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import operator
import os
import sys
from dataclasses import dataclass

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pyc_dis import disassemble  # noqa: E402
from pyc_inspect import load_blob, walk  # noqa: E402

ROOT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", ".."))
CONTROLLER = "antelope_components_routing_data_model_routing_model_controller.pyc"

FAMILIES = {
    "quadro": {
        "py": (3, 8),
        "routings": "app_ui_old_widgets_routings.pyc",
        "constants": None,
        # MixerModel (antelope/components/mixers/data_model/mixer_models.py) sends set_mixer with
        # device channel 0 for the master and strip index + 1 otherwise.
        "stereo_link": ("antelope_components_mixers_data_model_mixer_models.pyc", "MixerModel", "STEREO_LINK_ID"),
        "mixer_command": "set_mixer",
        "assumptions": [],
    },
    "studio": {
        "py": (3, 5),
        "routings": "zenstudiotb_ui_tabs_routings.pyc",
        "constants": "zenstudiotb_constants.pyc",
        "stereo_link": ("zenstudiotb_ui_panels.pyc", "ZenStudioTbMixerController", "STEREO_LINK_PERIPH_ID"),
        "mixer_command": "set_mixer_cfg",
        "assumptions": [
            "The Studio+ mixer controller that sends set_mixer_cfg (antelope.components.mixers.data_model."
            "MixerDataModelController) was not extracted; masterChannel 0 and strip index + 1 assume it "
            "matches Quadro's MixerModel. Confirm with a capture of a fader move in the vendor panel.",
        ],
    },
}

COMMON_ASSUMPTIONS = [
    "Group ids follow the routing controller's '%s%d' % (type name, index among earlier groups of the same "
    "type), read from its decompiled source, which is incomplete.",
]


class ExtractError(Exception):
    pass


class Unknown:
    def __repr__(self) -> str:
        return "<unknown>"


UNKNOWN = Unknown()


@dataclass(frozen=True)
class Member:
    enum: str
    name: str
    value: int


class EnumProxy:
    def __init__(self, name: str, members: dict[str, int]):
        self.name, self.members = name, members

    def attr(self, name: str) -> Member:
        if name not in self.members:
            raise ExtractError(f"{self.name} has no member {name}")
        return Member(self.name, name, self.members[name])


class ModuleProxy:
    def __init__(self, name: str, attrs: dict | None = None):
        self.name, self.attrs = name, attrs or {}

    def attr(self, name: str):
        return self.attrs.get(name, UNKNOWN)


BINARY = {
    "BINARY_ADD": operator.add,
    "BINARY_SUBTRACT": operator.sub,
    "BINARY_MULTIPLY": operator.mul,
    "BINARY_FLOOR_DIVIDE": operator.floordiv,
}
STOP = {"MAKE_FUNCTION", "MAKE_CLOSURE", "LOAD_BUILD_CLASS", "RETURN_VALUE"}


def evaluate(code, py, namespace: dict, modules=lambda name: ModuleProxy(name)) -> str | None:
    """Runs module- or class-level instructions into `namespace` until a definition or an
    instruction it does not model; returns the name of the instruction it stopped at, if any."""
    stack: list = []
    for ins in disassemble(code, py):
        op = ins.opname
        if op in STOP:
            return op
        if op == "LOAD_CONST":
            stack.append(code.consts[ins.arg])
        elif op == "LOAD_NAME":
            stack.append(namespace.get(code.names[ins.arg], UNKNOWN))
        elif op == "STORE_NAME":
            namespace[code.names[ins.arg]] = stack.pop()
        elif op == "POP_TOP":
            stack.pop()
        elif op == "LOAD_ATTR":
            obj = stack.pop()
            stack.append(obj.attr(code.names[ins.arg]) if hasattr(obj, "attr") else UNKNOWN)
        elif op == "IMPORT_NAME":
            stack.pop()  # fromlist
            stack.pop()  # level
            stack.append(modules(code.names[ins.arg]))
        elif op == "IMPORT_FROM":
            module = stack[-1]
            stack.append(module.attr(code.names[ins.arg]) if hasattr(module, "attr") else UNKNOWN)
        elif op == "IMPORT_STAR":
            module = stack.pop()
            if isinstance(module, ModuleProxy):
                namespace.update(module.attrs)
        elif op in ("BUILD_TUPLE", "BUILD_LIST"):
            items = stack[len(stack) - ins.arg:]
            del stack[len(stack) - ins.arg:]
            stack.append(tuple(items) if op == "BUILD_TUPLE" else list(items))
        elif op == "BUILD_MAP":
            items = stack[len(stack) - 2 * ins.arg:]
            del stack[len(stack) - 2 * ins.arg:]
            stack.append({items[2 * i]: items[2 * i + 1] for i in range(ins.arg)})
        elif op in BINARY:
            b, a = stack.pop(), stack.pop()
            stack.append(BINARY[op](a, b) if isinstance(a, int) and isinstance(b, int) else UNKNOWN)
        else:
            return op
    return None


def known(value) -> bool:
    if value is UNKNOWN:
        return False
    if isinstance(value, (tuple, list)):
        return all(known(v) for v in value)
    if isinstance(value, dict):
        return all(known(k) and known(v) for k, v in value.items())
    return isinstance(value, (Member, int, str)) or value is None


def find_code(blob_code, name: str):
    matches = [c for c in walk(blob_code) if c.name == name]
    if len(matches) != 1:
        raise ExtractError(f"expected one code object named {name}, found {len(matches)}")
    return matches[0]


class Blobs:
    def __init__(self, family: str, py):
        self.dir = os.path.join(ROOT, "refs", "extracted", family, "PYZ_extracted")
        self.py = py
        self.read: dict[str, set[str]] = {}

    def load(self, file: str, reads: str):
        path = os.path.join(self.dir, file)
        if not os.path.isfile(path):
            raise ExtractError(f"missing blob {os.path.relpath(path, ROOT)}; extract the vendor software first")
        self.read.setdefault(file, set()).add(reads)
        return load_blob(path, self.py)

    def provenance(self) -> list[dict]:
        out = []
        for file in sorted(self.read):
            path = os.path.join(self.dir, file)
            with open(path, "rb") as f:
                digest = hashlib.sha256(f.read()).hexdigest()
            rel = os.path.relpath(path, ROOT).replace(os.sep, "/")
            out.append({"path": rel, "sha256": digest, "read": sorted(self.read[file])})
        return out


def enum_members(blobs: Blobs, enum: str) -> dict[str, int]:
    body = find_code(blobs.load(CONTROLLER, enum), enum)
    namespace = {"__name__": "routing_model_controller"}
    evaluate(body, blobs.py, namespace)
    members = {k: v for k, v in namespace.items() if k.isupper() and isinstance(v, int)}
    if not members:
        raise ExtractError(f"no members read for {enum}")
    return members


def class_constant(blobs: Blobs, file: str, cls: str, name: str) -> int:
    body = find_code(blobs.load(file, f"{cls}.{name}"), cls)
    namespace = {"__name__": cls}
    evaluate(body, blobs.py, namespace)
    value = namespace.get(name, UNKNOWN)
    if not isinstance(value, int):
        raise ExtractError(f"{cls}.{name} is not an integer constant before the class's first method")
    return value


def groups(spec, kind: str) -> list[dict]:
    out, seen = [], {}
    for entry in spec:
        if not (isinstance(entry, tuple) and len(entry) == 4):
            raise ExtractError(f"{kind} entry {entry!r} is not (type, name, channels, colour)")
        member, name, channels, colour = entry
        if not isinstance(member, Member):
            raise ExtractError(f"{kind} entry {entry!r} has no group type")
        index = seen.get(member.name, 0)
        seen[member.name] = index + 1
        out.append({"id": f"{member.name}{index}", "type": member.name, "typeId": member.value, "name": name, "channels": channels, "color": colour})
    return out


def extract(family: str) -> dict:
    config = FAMILIES[family]
    blobs = Blobs(family, config["py"])
    inputs_enum = EnumProxy("RoutingInputGroupType", enum_members(blobs, "RoutingInputGroupType"))
    outputs_enum = EnumProxy("RoutingOutputGroupType", enum_members(blobs, "RoutingOutputGroupType"))
    controller = ModuleProxy("routing_model_controller", {"RoutingInputGroupType": inputs_enum, "RoutingOutputGroupType": outputs_enum})

    constants = ModuleProxy("constants")
    if config["constants"] is not None:
        namespace: dict = {}
        stopped = evaluate(blobs.load(config["constants"], "module constants"), blobs.py, namespace)
        constants = ModuleProxy("constants", {k: v for k, v in namespace.items() if known(v)})
        del stopped

    def modules(name: str) -> ModuleProxy:
        if name.endswith("routing.data_model"):
            return ModuleProxy(name, {"routing_model_controller": controller})
        if name.endswith("routing_model_controller"):
            return controller
        if name.endswith(".constants"):
            return constants
        return ModuleProxy(name)

    namespace = {}
    evaluate(blobs.load(config["routings"], "INPUT_SPEC, OUTPUT_SPEC, SIGNAL_PRESENT_SPEC, AVAILABLE_CHANNELS_SPEC"), blobs.py, namespace, modules)
    for name in ("INPUT_SPEC", "OUTPUT_SPEC", "SIGNAL_PRESENT_SPEC", "AVAILABLE_CHANNELS_SPEC"):
        if name not in namespace or not known(namespace[name]):
            raise ExtractError(f"{name} could not be fully resolved from {config['routings']}: {namespace.get(name, 'missing')!r}")

    inputs = groups(namespace["INPUT_SPEC"], "INPUT_SPEC")
    outputs = groups(namespace["OUTPUT_SPEC"], "OUTPUT_SPEC")
    mixer_ins = [g for g in outputs if g["type"] == "MIXER_IN"]
    mixer_outs = [g for g in inputs if g["type"] == "MIXER_OUT"]
    if not mixer_ins or len({g["channels"] for g in mixer_ins}) != 1:
        raise ExtractError(f"mixer input groups must exist and share a channel count: {mixer_ins}")
    if len(mixer_outs) != len(mixer_ins):
        raise ExtractError(f"{len(mixer_ins)} mixer input groups but {len(mixer_outs)} mixer outputs")

    # Read every value before recording provenance, so each blob a value came from is listed.
    link_file, link_class, link_name = config["stereo_link"]
    stereo_link_id = class_constant(blobs, link_file, link_class, link_name)
    signal_present = []
    for field, (member, channel) in namespace["SIGNAL_PRESENT_SPEC"]:
        signal_present.append({"field": field, "type": member.name, "typeId": member.value, "channel": channel})
    available = [{"type": member.name, "typeId": member.value, "field": field} for member, field in namespace["AVAILABLE_CHANNELS_SPEC"].items()]

    return {
        "family": family,
        "source": {
            "tool": "refs/tools/scripts/extract_topology.py",
            "bytecode": ".".join(str(n) for n in config["py"]),
            "blobs": blobs.provenance(),
        },
        "inputs": inputs,
        "outputs": outputs,
        "signalPresent": signal_present,
        "availableChannels": available,
        "mixers": {
            "count": len(mixer_ins),
            "channels": mixer_ins[0]["channels"],
            "command": config["mixer_command"],
            "masterChannel": 0,
            "stereoLinkId": stereo_link_id,
            "inputGroups": [g["id"] for g in mixer_ins],
            "outputGroups": [g["id"] for g in mixer_outs],
        },
        "assumptions": COMMON_ASSUMPTIONS + config["assumptions"],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--family", required=True, choices=sorted(FAMILIES))
    parser.add_argument("--out", required=True)
    args = parser.parse_args()
    try:
        topology = extract(args.family)
    except ExtractError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1
    with open(args.out, "w", encoding="utf-8", newline="\n") as f:
        json.dump(topology, f, indent=2, ensure_ascii=False)
        f.write("\n")
    mixers = topology["mixers"]
    print(f"wrote {args.out}: {len(topology['inputs'])} input groups, {len(topology['outputs'])} output groups, "
          f"{mixers['count']} mixers x {mixers['channels']} channels")
    return 0


if __name__ == "__main__":
    sys.exit(main())
