#!/usr/bin/env python3
"""Extract full field layouts for the in-scope command set from a panel REPORT_FORMAT.

The decompiled report_format.py files are valid Python, so we import their REPORT_FORMAT
dict directly (via ast.literal_eval on the module) rather than regex-slicing.

For each command name in IN_SCOPE, emit:
  { name, report_id, ext2, ext3, payload_id, auto_send_notification?,
    params: [ {name, type, size?}, ... ],
    returns: [ ... ] (from the matching cyclic report, if any) }

Type grammar (as emitted by the panels):
  scalar : "ubyte" | "byte" | "short" | "uint16" | "int16" | "uint32" | "int32" | ...
  inline array : "ubyte * N"
  nested struct: {'fields': [[name, type, size?], ...], 'count': N}

Sizes are computed so the Rust side can lay out the wire buffer without the Python source.
"""
import ast, json, os, sys

# In-scope = INTERSECTION(35) + Quadro-only(28) = 63, taken verbatim from
# extract_commands.py output. Do NOT hand-filter: the shared 35 includes a few
# AFX-adjacent commands (get_afx_available_instances/links/order, set_afx_bypass/order)
# that are shared by both devices and therefore in scope.
IN_SCOPE = [
 "get_adats_links","get_afx_available_instances","get_afx_links","get_afx_order",
 "get_mixer","get_mixer_links","get_preamps_links","get_reverb_config","get_routing",
 "get_spdifs_links","get_tb_latency","preset_recall","preset_save","set_adat_gain",
 "set_afx_bypass","set_afx_order","set_brightness","set_config_feature","set_mute",
 "set_none","set_peak_source","set_power","set_pre_gain","set_pre_phantom","set_pre_type",
 "set_reverb_config","set_routing","set_samp_rate","set_sine_gen","set_spdif_gain",
 "set_spdif_src","set_stereo_link","set_sync_source","set_tb_latency","set_volume",
 "get_afx_max_available_instances","get_afx_remaining_featured_instances",
 "get_afx_strip_order","get_assignment_request","get_assignment_status",
 "get_cmd_set_assignment","get_daw_mode","get_feature_mask","get_mic_emulations",
 "get_panning_law","get_reverb_returns","get_reverb_sends","get_sonarworks_ir",
 "get_trim_configs","get_upload_result","set_dc_coupled","set_dim","set_hard_mute",
 "set_mic_emulation","set_mixer","set_monitor_out","set_panning_law","set_pre_phase_inv",
 "set_predefined_preset","set_reverb_return","set_reverb_send","set_trim_config",
 "set_usb_channels",
]

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
      1. {'fields': [[name, type, size?], ...], 'count': N}   -> sum(field sizes) * N
      2. {'elem_type': 'ubyte * K', 'count': N}               -> elem_size * N
    """
    if "fields" in d:
        total = sum(elem_size(f[1]) for f in d["fields"])
        return total * d.get("count", 1)
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
        # 4-tuple form: ["name", "type", bit_width, default_value]. The 3rd element is
        # the field's bit width and the 4th its default value (e.g. ["density","ubyte",8,100],
        # ["buffer_adjust_in","int32",32,0]). Capture both; they matter for wire layout.
        if len(f) >= 4 and isinstance(t, str):
            entry["bit_width"] = f[2]
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
    tree = ast.parse(open(path, "r", encoding="utf-8").read())
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
        entry["returns"] = norm_fields(req["returns"].get("fields", []))
    return entry

def main():
    path = sys.argv[1]
    out = sys.argv[2] if len(sys.argv) > 2 else "in_scope_commands.json"
    # Optional: a directory of raw bytecode blobs, used to resolve constants whose
    # module was never decompiled.
    blob_dir = sys.argv[3] if len(sys.argv) > 3 else None
    rf = load_report_format(path, blob_dir)
    result = {}
    missing = []
    for name in IN_SCOPE:
        e = build(rf, name)
        if e is None:
            missing.append(name)
        else:
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
    doc = {
        "source": os.path.basename(path),
        "unresolved_constants": unresolved,
        "in_scope_zero_counts": zero_counts,
        "report_version": rf.get("version"),
        "authoritative": rf.get("authorative"),
        "scope": "intersection(35) + quadro-only(28)",
        "count": len(result),
        "missing": missing,
        "commands": result,
    }
    with open(out, "w", encoding="utf-8") as f:
        # Unresolved constants collapse to 0 here too (sizes as well as counts);
        # they are surfaced via unresolved_constants / in_scope_zero_counts.
        json.dump(doc, f, indent=2, default=lambda o: 0)
    print("wrote %s (%d commands, %d missing)" % (out, len(result), len(missing)))
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

if __name__ == "__main__":
    main()
