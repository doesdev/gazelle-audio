"""Generate ground-truth request bytes from the decompiled Antelope Payload/Request
classes, for cross-checking against the Rust implementation.

Run:  python3 ground_truth.py
Output: JSON mapping command name -> hex bytes (with default/zero values).
"""
import sys, ctypes, struct, json
import os
_HERE = os.path.dirname(os.path.abspath(__file__))
_ROOT = os.path.abspath(os.path.join(_HERE, "..", "..", ".."))
IN_SCOPE = os.path.join(_ROOT, "refs", "schemas", "in_scope_commands.json")
DECOMPILED_MANAGER = os.path.join(_ROOT, "refs", "decompiled", "manager")
OUT = os.path.join(_HERE, "ground_truth.json")

sys.path.insert(0, DECOMPILED_MANAGER)

# Reimplement the intact Payload/Request logic (the decompiled module has
# garbled uncompyle6 lines, so we mirror the recovered logic directly).
import zlib


class Field:
    def __init__(self, name, ctype, bitlen=None, default=None):
        self.name = name
        self.ctype = self._parse_ctype(ctype)
        self.bitlen = self._validate_bitlen(bitlen)
        self.default = default

    @property
    def isarray(self):
        return issubclass(self.ctype, ctypes.Array)

    @property
    def isstruct(self):
        return issubclass(self.ctype, ctypes.Structure)

    @property
    def isfloat(self):
        return issubclass(self.ctype, ctypes.c_float)

    @property
    def tuple(self):
        if self.isarray or self.isstruct or self.isfloat:
            return (self.name, self.ctype)
        return (self.name, self.ctype, self.bitlen)

    @staticmethod
    def _getctype(ct):
        return getattr(ctypes, "c_{}".format(ct))

    @staticmethod
    def _parse_ctype(ct):
        # The JSON stores nested struct types as a JSON-encoded string.
        if isinstance(ct, str) and ct.strip().startswith("{"):
            ct = json.loads(ct)
        if isinstance(ct, str):
            segments = [s.strip() for s in ct.split("*")]
            if len(segments) == 1:
                return Field._getctype(ct)
            else:
                try:
                    t = Field._getctype(segments[0])
                    length = int(segments[1])
                except ValueError:
                    length, t = int(segments[0]), Field._getctype(segments[1])
                return t * length
        elif isinstance(ct, dict):
            if "fields" in ct:
                class HelperType(ctypes.Structure):
                    _pack_ = 1
                    _fields_ = [Field(*fd).tuple for fd in ct["fields"]]
                if "count" in ct:
                    return HelperType * ct["count"]
                return HelperType
            elif "elem_type" in ct:
                return Field._parse_ctype(ct["elem_type"]) * ct["count"]

    def _validate_bitlen(self, bitlen):
        if self.isarray:
            if bitlen is None:
                return ctypes.sizeof(self.ctype) * 8
            raise ValueError
        else:
            atmost = ctypes.sizeof(self.ctype) * 8
            if bitlen is None:
                return atmost
            if bitlen <= atmost:
                return bitlen
            raise ValueError


class Payload:
    def __init__(self, payload_id, fields):
        self._default = {}
        self._struct = None
        self.fields = []
        user_fields = [Field(*args) for args in fields]
        total = sum(f.bitlen for f in user_fields)
        if total % 8 != 0:
            raise ValueError("total field size should be byte-complete")
        self.bytesize = total // 8
        if payload_id is not None:
            self._default["payload_id"] = payload_id
            self.fields = [Field("payload_id", "ubyte", 6), Field("nparams", "ubyte", 2)]
            if self.bytesize < 4:
                # <4 user bytes: single header byte, nparams = bytesize - 1,
                # no nbytes byte. (The decompiled source added 2 here too, which
                # inflated seq by 2 for every such command; that is a bug.)
                # Guard the degenerate ub=0 case so nparams saturates at 0
                # rather than wrapping to 3.
                self._default["nparams"] = max(0, self.bytesize - 1)
                self.bytesize += 1
            else:
                self.fields.append(Field("nbytes", "ubyte"))
                self._default["nparams"] = 3
                self._default["nbytes"] = self.bytesize
                self.bytesize += 2
        self.fields += user_fields
        for f in user_fields:
            self._default[f.name] = f.default

    @property
    def struct(self):
        if self._struct is None:
            class PayloadStruct(ctypes.Structure):
                _pack_ = 1
                _fields_ = [f.tuple for f in self.fields]
            self._struct = PayloadStruct
        return self._struct

    def create(self, *args, **kwargs):
        composed = self._compose_kwargs(*args, **kwargs)
        inst = self.struct()
        self._populate(inst, composed)
        return inst

    def _compose_kwargs(self, *args, **kwargs):
        composed = {}
        args_iter = iter(args)
        for f in self.fields:
            if self._default[f.name] is None:
                if f.name in kwargs:
                    composed[f.name] = kwargs[f.name]
                else:
                    composed[f.name] = next(args_iter)
            else:
                composed[f.name] = kwargs.get(f.name, self._default[f.name])
        return composed

    @staticmethod
    def _populate(centity, python):
        if isinstance(python, dict):
            for field in centity._fields_:
                fname, ftype = field[0], field[1]
                if fname in python:
                    if hasattr(ftype, "_length_") and ftype._type_ == ctypes.c_char:
                        setattr(centity, fname, python[fname].encode())
                    elif hasattr(ftype, "_length_") or issubclass(ftype, ctypes.Structure):
                        Payload._populate(getattr(centity, fname), python[fname])
                    else:
                        setattr(centity, fname, python[fname])
        elif isinstance(python, list):
            etype = centity._type_
            for i, v in enumerate(python):
                if hasattr(etype, "_length_") or issubclass(etype, ctypes.Structure):
                    Payload._populate(centity[i], v)
                else:
                    centity[i] = v
        else:
            if isinstance(python, bytes):
                for i, v in enumerate(python):
                    centity[i] = v


class Request:
    def __init__(self, fmt):
        self.format_dict = fmt
        self._request_struct = None
        self.payload = None

    def create_request(self, mode, *args, **kwargs):
        if self._request_struct is None:
            payload_field = []
            if "params" in self.format_dict:
                params = self.format_dict["params"]
                # params may be a dict {payload_id, fields} or a list of field dicts
                if isinstance(params, dict):
                    payload_id = params.get("payload_id")
                    fields = params["fields"]
                else:
                    payload_id = self.format_dict.get("payload_id")
                    # bit_width must be carried through: Payload sizes itself from the
                    # sum of field BIT lengths, so passing None here silently widened
                    # every sub-byte field to a whole byte (set_mixer's pan/mute/solo
                    # became 3 bytes instead of 1).
                    fields = [
                        [f["name"], f["type"], f.get("bit_width"), f.get("default", 0)]
                        for f in params
                    ]
                self.payload = Payload(payload_id, fields)
                payload_field = [("payload", self.payload.struct)]
            class RequestStruct(ctypes.Structure):
                _pack_ = 1
                _fields_ = [
                    ("report_id", ctypes.c_uint32),
                    ("seq", ctypes.c_uint32),
                    ("ext2", ctypes.c_uint32),
                    ("ext3", ctypes.c_uint32)] + payload_field
            self._request_struct = RequestStruct
        inst = self._request_struct(
            int(self.format_dict["report_id"], 0), 0,
            kwargs.get("ext2", self.format_dict.get("ext2", 0)),
            kwargs.get("ext3", self.format_dict.get("ext3", 0)))
        if self.payload is not None:
            inst.payload = self.payload.create(*args, **kwargs)
        return self._filter(inst, mode.endswith("boot"))

    def _filter(self, inst, crc32=False):
        if crc32:
            inst.seq = zlib.crc32(inst)
        else:
            if self.payload is not None:
                inst.seq = 16 + self.payload.bytesize
            else:
                inst.seq = 16
        return inst


def main():
    doc = json.load(open(IN_SCOPE))
    cmds = doc["commands"]
    out = {}
    for name, c in cmds.items():
        r = Request(c)
        inst = r.create_request("app")
        b = bytes(memoryview(inst).tobytes())
        out[name] = b.hex()
    result = json.dumps(out, indent=1)
    with open(OUT, "w") as f:
        f.write(result)
    print(result)


if __name__ == "__main__":
    main()
