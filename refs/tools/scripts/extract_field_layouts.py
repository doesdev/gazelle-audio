#!/usr/bin/env python3
"""Extract full field layouts for the in-scope command set from a panel REPORT_FORMAT.

The decompiled report_format.py files are valid Python, so we import their REPORT_FORMAT
dict directly (via ast.literal_eval on the module) rather than regex-slicing.

For each command name in the device's scope (see SCOPES), emit:
  { name, report_id, ext2, ext3, payload_id, auto_send_notification?,
    params: [ {name, type, size?}, ... ],
    returns: [ ... ] (from the matching cyclic report, if any) }

Type grammar (as emitted by the panels):
  scalar : "ubyte" | "byte" | "short" | "uint16" | "int16" | "uint32" | "int32" | ...
  inline array : "ubyte * N"
  nested struct: {'fields': [[name, type, size?], ...], 'count': N}

Sizes are computed so the Rust side can lay out the wire buffer without the Python source.
"""
import argparse, ast, json, os, sys

# Scope is per device. Taken verbatim from extract_commands.py output; do NOT hand-filter:
# the shared 35 include a few AFX-adjacent commands (get_afx_available_instances/links/order,
# set_afx_bypass/order) that both devices carry and are therefore in scope.
SHARED = [
 "get_adats_links","get_afx_available_instances","get_afx_links","get_afx_order",
 "get_mixer","get_mixer_links","get_preamps_links","get_reverb_config","get_routing",
 "get_spdifs_links","get_tb_latency","preset_recall","preset_save","set_adat_gain",
 "set_afx_bypass","set_afx_order","set_brightness","set_config_feature","set_mute",
 "set_none","set_peak_source","set_power","set_pre_gain","set_pre_phantom","set_pre_type",
 "set_reverb_config","set_routing","set_samp_rate","set_sine_gen","set_spdif_gain",
 "set_spdif_src","set_stereo_link","set_sync_source","set_tb_latency","set_volume",
]
QUADRO_ONLY = [
 "get_afx_max_available_instances","get_afx_remaining_featured_instances",
 "get_afx_strip_order","get_assignment_request","get_assignment_status",
 "get_cmd_set_assignment","get_daw_mode","get_feature_mask","get_mic_emulations",
 "get_panning_law","get_reverb_returns","get_reverb_sends","get_sonarworks_ir",
 "get_trim_configs","get_upload_result","set_dc_coupled","set_dim","set_hard_mute",
 "set_mic_emulation","set_mixer","set_monitor_out","set_panning_law","set_pre_phase_inv",
 "set_predefined_preset","set_reverb_return","set_reverb_send","set_trim_config",
 "set_usb_channels",
]
# Studio+'s own mix/monitoring commands: its equivalents of set_mixer, set_trim_config and
# set_pre_phase_inv under different names, plus line gain and talkback. Its AFX-only
# commands stay out of scope.
STUDIO_ONLY = [
 "get_lines_links","set_line_gain","set_mixer_cfg","set_pre_phaseinv",
 "set_talk","set_tbk_enable","set_tbk_vol","set_trim",
]
SCOPES = {
    "zenquadrosc_usb2": ("intersection(35) + quadro-only(28)", SHARED + QUADRO_ONLY),
    "zenstudiotb": ("intersection(35) + studio-only mix/monitoring(8)", SHARED + STUDIO_ONLY),
}
assert len(SHARED) == 35 and len(QUADRO_ONLY) == 28 and len(STUDIO_ONLY) == 8

TYPE_SIZE = {
 "bit":1,"bool":1,"ubyte":1,"byte":1,"uint8":1,"int8":1,"char":1,"sbyte":1,
 "ushort":2,"short":2,"uint16":2,"int16":2,"wchar":2,
 "uint":4,"int":4,"uint32":4,"int32":4,"float":4,
 "ulong":8,"long":8,"uint64":8,"int64":8,"double":8,
}

def _coerce(spec):
    """A nested struct may arrive as a dict or as a JSON string (dicts are dumped
    to strings in norm_fields). Parse JSON-string dicts back to dicts."""
    if isinstance(spec, str):
        s = spec.strip()
        if s.startswith("{"):
            try:
                return json.loads(s)
            except Exception:
                return spec
    return spec


def elem_size(spec):
    """Byte size of one element: a scalar/inline-array type string, or a nested dict."""
    spec = _coerce(spec)
    if isinstance(spec, dict):
        return struct_size(spec)
    if isinstance(spec, str):
        spec = spec.strip()
        # inline array, with or without a space: "ubyte * 2" / "ubyte*2" / "uint8*300"
        for sep in (" *", "*"):
            if sep in spec:
                name, n = spec.split(sep, 1)
                return TYPE_SIZE.get(name.strip(), 1) * int(n.strip())
        return TYPE_SIZE.get(spec, 1)
    return 0


def field_size(t):
    """Return byte size of a single field entry (scalar, inline array, or nested struct)."""
    return elem_size(t)


def struct_size(d):
    """Size of a nested struct.

    Two shapes appear in the panels:
      1. {'fields': [[name, type, bit_width?], ...], 'count': N}
         -> ceil(sum(field bits) / 8) * N
      2. {'elem_type': 'ubyte * K', 'count': N}               -> elem_size * N

    A struct element packs its fields bit by bit (reference/protocol.md, "Cyclic reports"),
    so a field with a bit width contributes those bits, not its type's whole bytes. Summing
    byte sizes made `preamps` 5 bytes per element instead of 1; the live Studio+ capture puts
    `peaks_mixer` and `peaks_preamp` where the bit-packed layout does.
    """
    if "fields" in d:
        bits = 0
        for f in d["fields"]:
            width = f[2] if len(f) > 2 and f[2] else None
            bits += width if width else elem_size(f[1]) * 8
        return (bits + 7) // 8 * d.get("count", 1)
    if "elem_type" in d:
        return elem_size(d["elem_type"]) * d.get("count", 1)
    return 0

def norm_fields(fields):
    out = []
    for f in fields:
        name = f[0]
        t = _coerce(f[1])
        # Unresolved constants serialize as 0 so extraction still completes, but they
        # are recorded in `unresolved_constants` and, when they leave a zero-length array
        # in an in-scope command, called out in `in_scope_zero_counts`. Never silent.
        entry = {"name": name,
                 "type": t if isinstance(t, str) else json.dumps(t, default=lambda o: 0)}
        # Only inline arrays and nested structs carry an explicit size; bare scalars
        # are a single known type and need no size field.
        if isinstance(t, dict):
            entry["size"] = struct_size(t)
        elif isinstance(t, str) and any(s in t for s in (" *", "*")):
            entry["size"] = field_size(t)
        # Field tuples carry an optional bit width and an optional default:
        #
        #   ["name", "type"]                     -- full native width, no default
        #   ["name", "type", bit_width]          -- sub-byte field, no default
        #   ["name", "type", bit_width, default] -- both
        #
        # The original module's own docstring states the form as
        # ('field_name_str', 'field_type_str', bitlength, default_value) with bitlength
        # "optional for all the default types" (recovered via pyc_inspect.py).
        #
        # The 3-element form was previously ignored, which silently discarded the bit
        # width on 21 Quadro fields. set_mixer's pan(6)/mute(1)/solo(1) then occupied
        # three whole bytes instead of one, making the command's wire layout wrong.
        if len(f) >= 3 and isinstance(t, str):
            entry["bit_width"] = f[2]
        if len(f) >= 4 and isinstance(t, str):
            entry["default"] = f[3]
        out.append(entry)
    return out

class _Default:
    """Chainable zero-like stub for AFX-only module constants.

    Stubs module-level objects (afx_pool.PHY_*, constants.MAX_*) that appear ONLY in
    AFX commands, which are out of scope. Attribute access returns another _Default so
    chained lookups like `constants.CHANNEL_STRIP.EQUALIZER` resolve instead of raising
    AttributeError on an int, and the object behaves as 0 wherever a number is needed.

    Only out-of-scope AFX entries ever observe these values, so their exactness does not
    matter -- but note that genuine module-level constants (e.g. Studio+'s
    SET_AFX_REQUEST_PAYLOAD_ID) are resolved for real in load_report_format, NOT stubbed.
    """
    #: Every attribute path that resolved to a stub rather than a real value.
    #: Silent zeros here are how a wrong array count shipped in the Quadro registry.
    SEEN = set()

    def __init__(self, path="?"):
        object.__setattr__(self, "_path", path)

    def __getattr__(self, item):
        path = "%s.%s" % (object.__getattribute__(self, "_path"), item)
        _Default.SEEN.add(path)
        return _Default(path)
    def __getitem__(self, item):
        return _Default(object.__getattribute__(self, "_path"))
    def __call__(self, *a, **k):
        return _Default(object.__getattribute__(self, "_path"))
    def __int__(self):
        return 0
    def __index__(self):
        return 0
    def __repr__(self):
        return "0"
    def __eq__(self, other):
        return other == 0
    def __hash__(self):
        return hash(0)
    def __len__(self):
        return 0
    def __iter__(self):
        return iter(())
    def __bool__(self):
        return False


# Arithmetic and comparison on the stub all collapse to another stub, so expressions
# like `constants.MAX_SLOTS // afx_pool.PHY_PER_SLOT` inside AFX-only entries evaluate
# instead of raising. Defined programmatically to avoid twenty near-identical methods.
for _op in ("add", "sub", "mul", "floordiv", "truediv", "mod", "pow",
            "lshift", "rshift", "and", "or", "xor"):
    for _fmt in ("__%s__", "__r%s__"):
        setattr(_Default, _fmt % _op, lambda self, other: _Default(object.__getattribute__(self, "_path")))
for _op in ("lt", "le", "gt", "ge"):
    setattr(_Default, "__%s__" % _op, lambda self, other: False)
del _op, _fmt


class _Ns:
    """Namespace backed by real module-level constants, falling back to a stub."""
    def __init__(self, values, name):
        object.__setattr__(self, "_v", values)
        object.__setattr__(self, "_n", name)
    def __getattr__(self, item):
        v = object.__getattribute__(self, "_v")
        if item in v:
            return v[item]
        return _Default("%s.%s" % (object.__getattribute__(self, "_n"), item))


def _sibling_ns(report_format_path, filename, nsname, blob_dir=None):
    """Load module-level constants for one of the panels' AFX namespaces.

    REPORT_FORMAT references constants from the AFX modules (constants.NUM_AFX_IN_CHANNEL,
    afx_pool.PHY_AFX_STRIP_SIZE). They are resolved from, in order:

    1. a sibling **decompiled** module, when one exists;
    2. the **raw bytecode blob**, read with `pyc_inspect` — no legacy interpreter needed;
    3. a tracked stub, so the omission is reported rather than silently becoming 0.

    Step 2 matters: the Quadro panel's afx_pool was never decompiled, so
    PHY_AFX_STRIP_SIZE was unresolvable and three in-scope `slots` fields shipped with
    count 0. Reading it straight from the bytecode fixes that without uncompyle6.
    """
    values = {}
    sib = os.path.join(os.path.dirname(os.path.abspath(report_format_path)), filename)
    if os.path.exists(sib):
        try:
            tree = ast.parse(open(sib, "r", encoding="utf-8").read())
        except SyntaxError:
            tree = None
        if tree is not None:
            for node in tree.body:
                if isinstance(node, ast.Assign):
                    for tgt in node.targets:
                        if isinstance(tgt, ast.Name):
                            try:
                                values[tgt.id] = ast.literal_eval(node.value)
                            except Exception:
                                pass

    if blob_dir:
        blob = os.path.join(blob_dir, filename[:-3] + ".pyc")
        if os.path.exists(blob):
            try:
                sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
                from pyc_inspect import load_blob, module_assignments
                code = load_blob(blob, _blob_py_version(blob_dir))
                for k, v in module_assignments(code).items():
                    values.setdefault(k, v)
            except Exception as e:
                print("note: could not read constants from %s: %s" % (blob, e),
                      file=sys.stderr)
    return _Ns(values, nsname)


def _blob_py_version(blob_dir):
    """Infer the bytecode version from the sibling PYZ archive's magic, else assume 3.8."""
    for parent in (blob_dir, os.path.dirname(blob_dir.rstrip("/"))):
        pyz = os.path.join(parent, "PYZ-00.pyz")
        if os.path.exists(pyz):
            with open(pyz, "rb") as f:
                magic = f.read(8)[4:8]
            return {b"\x17\r\r\n": (3, 5), b"\x1c\r\r\n": (3, 6),
                    b"\x1d\r\r\n": (3, 7)}.get(magic, (3, 8))
    return (3, 8)


def load_report_format(path, blob_dir=None):
    """Evaluate a panel's REPORT_FORMAT dict without importing the panel package.

    Module-level constants are resolved from the module itself first. The Studio+
    format references bare names such as SET_AFX_REQUEST_PAYLOAD_ID (= 20) and
    GET_AFX_REQUEST_EXT2 (= 7) that are assigned above REPORT_FORMAT; stubbing those
    to zero would silently corrupt payload ids and ext selectors, so they are
    evaluated for real. Only the AFX-only *module* objects (afx_pool, constants),
    whose attributes appear solely in out-of-scope AFX commands, remain stubbed.
    """
    text = open(path, "r", encoding="utf-8").read()
    # A device-supplied format (the manager saves it as panels/report_format_<version>) is plain
    # JSON with the same shape: {"authorative", "version", "requests", "cyclic_reports"}. Every
    # count in it is already a number, so nothing needs resolving.
    if text.lstrip().startswith("{"):
        return json.loads(text)
    tree = ast.parse(text)
    ns = {"afx_pool": _sibling_ns(path, "antelope_ui_afx_platform_afx_pool.py",
                                  "afx_pool", blob_dir),
          "constants": _sibling_ns(path, "antelope_ui_afx_constants.py",
                                   "constants", blob_dir),
          "indexed_cyclic_report": lambda *a, **k: []}

    # Pass 0: stub every module-level import. The panel modules import the AFX model
    # packages (equalizer, compressor, neve, ssl, ...) whose members appear only in
    # out-of-scope AFX entries. Stubbing them generically keeps this working on panels
    # we have not seen yet, instead of hard-coding one name at a time.
    for node in tree.body:
        if isinstance(node, ast.Import):
            for a in node.names:
                ns.setdefault((a.asname or a.name).split(".")[0], _Default())
        elif isinstance(node, ast.ImportFrom):
            for a in node.names:
                ns.setdefault(a.asname or a.name, _Default())

    # Pass 1: resolve module-level constant assignments (Name = <literal expr>).
    for node in tree.body:
        if not isinstance(node, ast.Assign):
            continue
        for tgt in node.targets:
            if not isinstance(tgt, ast.Name) or tgt.id == "REPORT_FORMAT":
                continue
            try:
                ns[tgt.id] = eval(
                    compile(ast.Expression(node.value), "<const>", "eval"),
                    {"__builtins__": {}}, dict(ns))
            except Exception:
                pass  # not literal-evaluable; leave undefined and let pass 2 report it

    # Pass 2: evaluate REPORT_FORMAT with those constants in scope.
    for node in tree.body:
        if isinstance(node, ast.Assign):
            for tgt in node.targets:
                if isinstance(tgt, ast.Name) and tgt.id == "REPORT_FORMAT":
                    code = compile(ast.Expression(node.value), "<rf>", "eval")
                    return eval(code, {"__builtins__": {}}, ns)
    raise SystemExit("REPORT_FORMAT not found in %s" % path)

def find_request(rf, name):
    return rf.get("requests", {}).get(name)

def build(rf, name):
    req = find_request(rf, name)
    if req is None:
        return None
    hdr = req.get("header", {})
    params = req.get("params", {})
    entry = {
        "name": name,
        "report_id": hdr.get("report_id"),
        "ext2": hdr.get("ext2"),
        "ext3": hdr.get("ext3"),
        "payload_id": params.get("payload_id"),
        "params": norm_fields(params.get("fields", [])),
    }
    if req.get("auto_send_notification"):
        entry["auto_send_notification"] = True
    if "returns" in req:
        returns = req["returns"]
        # A top-level 'count' makes the reply that many elements of the fields: the manager's
        # ResponseStruct is `Field("contents", returns)`, a ctypes array when counted
        # (antelope_dev_reports.py:184-189, 500-513), turned into a list of dicts. get_mixer is
        # 33 strips (master first) and each links read one byte per pair. Ignoring the count
        # read only the first element.
        count = returns.get("count")
        if isinstance(count, int) and not isinstance(count, bool) and count > 0:
            entry["returns"] = norm_fields([["entries", {"fields": returns.get("fields", []), "count": count}]])
        else:
            # A count built from constants that could not be resolved (AFX pool sizes) would
            # give a zero or negative length; keep the one-element layout and say so instead.
            if "count" in returns:
                UNRESOLVED_REPLY_COUNTS.append(name)
            entry["returns"] = norm_fields(returns.get("fields", []))
    return entry

#: Commands whose reply `count` could not be resolved, so their returns stay one element.
UNRESOLVED_REPLY_COUNTS = []

# Effect parameters (`--afx`). Each effect type has its own set and get; `afx_parameters.py` reads
# them from the panels' bytecode into `refs/schemas/afx_parameters.json`, which says which to add and
# what the report format alone cannot: the read's `ext3` as the panel sends it (the Quadro sets bit 31
# and names the instance in `id`; the Studio+ format's `constants.AfxType` members resolve to stubs
# here), the reply count (`<Class>.max_instances` on the Studio+), and the panel's starting values,
# which go on the reply's fields as `default` so a loopback can answer a read with them.
AFX_FAMILIES = {"zenquadrosc_usb2": "quadro", "zenstudiotb": "studio"}
WIRE_BITS = {"u8": 8, "i8": 8, "u16": 16, "i16": 16, "u32": 32, "i32": 32}


def afx_effects(path, device):
    """The effects `afx_parameters.json` lists for a device: {command name: effect}."""
    with open(path, "r", encoding="utf-8") as f:
        doc = json.load(f)
    effects = {}
    for effect in doc[AFX_FAMILIES[device]]["effects"]:
        effects[effect["set"]] = effect
        effects[effect["get"]] = effect
    return effects


def wire_default(parameter):
    """A parameter's default as the device holds it; a negative value in an unsigned field is its
    two's complement (the panel assigns it to a ctypes unsigned field)."""
    value = parameter.get("default")
    if value is None:
        return None
    if value < 0 and parameter["wire"].startswith("u"):
        value += 1 << WIRE_BITS[parameter["wire"]]
    return value


def apply_afx_get(entry, request, effect):
    """Give an effect read its panel's `ext3`, its reply count, and the reply fields' defaults."""
    entry["ext3"] = effect["get_ext3"]
    defaults = {p["name"]: wire_default(p) for p in effect["parameters"]}
    defaults["enabled"] = 1  # an effect starts processing (`_defaults['bp'] = 0`)
    fields = [list(f)[:2] for f in request["returns"]["fields"]]
    if any(len(list(f)) > 2 for f in request["returns"]["fields"]):
        raise SystemExit("%s: a reply field has a bit width or default already" % entry["name"])
    bands = effect.get("bands")
    if bands is not None:
        # An effect read band by band (the Studio+ Equalizer): each entry holds a list of bands whose count is
        # a class attribute (`equalizer.Equalizer.num_strips`) this script only stubs, so it comes from the
        # catalogue, which read it from the class.
        for field in fields:
            if field[0] == bands["list"]:
                inner = _coerce(field[1])
                stub = inner.get("count") if isinstance(inner, dict) else None
                if not isinstance(inner, dict) or not (isinstance(stub, _Default) or stub == len(bands["bands"])):
                    raise SystemExit("%s: band list %s is %r" % (entry["name"], field[0], inner))
                field[1] = {"fields": [list(f)[:2] for f in inner["fields"]], "count": len(bands["bands"])}
                break
        else:
            raise SystemExit("%s: no band list %s in the reply" % (entry["name"], bands["list"]))
    counted = {"fields": fields, "count": effect["reply_count"]}
    returns = norm_fields([["entries", counted]])
    # Sized from the plain layout above; the defaults ride in the same type in field-object form.
    counted["fields"] = [
        {"name": n, "type": t, "default": defaults[n]} if defaults.get(n) is not None else {"name": n, "type": t}
        for n, t in fields
    ]
    returns[0]["type"] = json.dumps(counted)
    entry["returns"] = returns
    if entry["name"] in UNRESOLVED_REPLY_COUNTS:
        UNRESOLVED_REPLY_COUNTS.remove(entry["name"])

# Cyclic fields whose declared length is a buffer rather than the packet's size, so a report of
# that layout must be decoded at whatever length arrived. Only the LAST field of a report may be
# marked; everything before it stays fixed.
#
# The Quadro's 0x83 declares `afx_meters` as `ubyte * 304`, but the panel walks that buffer effect
# by effect: each loaded effect takes `get_amount_of_meters()` bytes -- two for every effect type
# -- chain by chain, then the mic emulation meters and the surround meters follow
# (`antelope/ui/afx/platform/afx_model_controller.on_cyclic_report` and `afx_meters_model`). With
# nothing loaded the Quadro sends four bytes, one per mic emulation channel; live traffic confirms
# it (an 8053 segment of 20 bytes carrying `60606060`).
VARIABLE_TAIL_FIELDS = {"afx_meters"}


def main():
    ap = argparse.ArgumentParser(description="Extract a device's in-scope command layouts.")
    ap.add_argument("report_format")
    ap.add_argument("out")
    ap.add_argument("blob_dir", nargs="?",
                    help="raw bytecode blobs, to resolve constants whose module was never decompiled")
    ap.add_argument("--device", required=True, choices=sorted(SCOPES))
    ap.add_argument("--afx", help="refs/schemas/afx_parameters.json: add each listed effect's parameter commands")
    args = ap.parse_args()
    path, out, blob_dir = args.report_format, args.out, args.blob_dir
    scope_label, scope = SCOPES[args.device]
    effects = afx_effects(args.afx, args.device) if args.afx else {}
    if effects:
        scope = scope + sorted(effects)
        scope_label = "%s + effect parameters(%d)" % (scope_label, len(effects))
    rf = load_report_format(path, blob_dir)
    result = {}
    missing = []
    for name in scope:
        e = build(rf, name)
        if e is None:
            missing.append(name)
        else:
            if name in effects and name == effects[name]["get"]:
                apply_afx_get(e, find_request(rf, name), effects[name])
            result[name] = e
    # An unresolved constant only matters if it left a zero-length array in an
    # IN-SCOPE command. AFX model constants (Altec436C.max_instances etc.) appear
    # solely in out-of-scope entries and are noise.
    zero_counts = []
    for cname, entry in result.items():
        for sec in ("params", "returns"):
            for f in entry.get(sec, []):
                t = f.get("type", "")
                if isinstance(t, str) and t.startswith("{") and '"count": 0' in t:
                    zero_counts.append("%s.%s.%s" % (cname, sec, f["name"]))
    unresolved = sorted(_Default.SEEN)
    # Cyclic reports are the device -> host direction: unsolicited state pushes keyed by
    # report id. They use the same field grammar as requests, so the same normaliser
    # applies. Without these the event stream has no layout to decode against.
    cyclic = {}
    for rid, spec_ in (rf.get("cyclic_reports") or {}).items():
        fields = spec_.get("fields", []) if isinstance(spec_, dict) else spec_
        norm = norm_fields(fields)
        entry = {"report_id": str(rid), "fields": norm}
        if norm and norm[-1].get("name") in VARIABLE_TAIL_FIELDS:
            entry["variable_tail"] = True
        cyclic[str(rid)] = entry

    doc = {
        "source": os.path.basename(path),
        "device": args.device,
        "cyclic_reports": cyclic,
        "unresolved_constants": unresolved,
        "in_scope_zero_counts": zero_counts,
        "unresolved_reply_counts": sorted(UNRESOLVED_REPLY_COUNTS),
        "report_version": rf.get("version"),
        "authoritative": rf.get("authorative"),
        "scope": scope_label,
        "count": len(result),
        "missing": missing,
        "commands": result,
    }
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        # Unresolved constants collapse to 0 here too (sizes as well as counts);
        # they are surfaced via unresolved_constants / in_scope_zero_counts.
        json.dump(doc, f, indent=2, default=lambda o: 0)
    print("wrote %s (%d commands, %d cyclic reports, %d missing)"
          % (out, len(result), len(cyclic), len(missing)))
    if zero_counts:
        print("WARNING: %d IN-SCOPE field(s) have a zero-length array, which is almost"
              " certainly a constant that could not be resolved: %s"
              % (len(zero_counts), ", ".join(zero_counts)), file=sys.stderr)
    if unresolved:
        print("note: %d constant(s) fell back to 0; all confined to out-of-scope entries"
              " unless listed above (see 'unresolved_constants' in the output)."
              % len(unresolved), file=sys.stderr)
    if missing:
        print("MISSING: %s" % ", ".join(missing))
    if missing:
        # An in-scope command the report format does not define means the scope list or the
        # extraction is wrong. Fail loudly rather than write a quietly smaller registry.
        print("ERROR: %d in-scope command(s) not found for %s: %s"
              % (len(missing), args.device, ", ".join(missing)), file=sys.stderr)
        sys.exit(1)

if __name__ == "__main__":
    main()
