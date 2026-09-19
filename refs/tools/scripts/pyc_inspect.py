#!/usr/bin/env python3
"""Read CPython marshal blobs without the matching interpreter.

Why this exists
---------------
PyInstaller's ``PYZ-00.pyz`` stores raw ``marshal.dumps(code_object)`` blobs with no
``.pyc`` header. The usual way to read them is to run ``marshal.loads`` under the exact
CPython that produced them (3.5 or 3.8 for the Antelope bundles), which means provisioning
legacy interpreters before you can inspect anything.

This module implements the marshal format directly, so **any** modern Python can:

* list a module's constants, names and docstrings,
* recover module-level constant assignments (``NAME = <literal>``),
* verify that decompiled source actually kept what the bytecode contained.

It does **not** decompile. Recovering statements still needs uncompyle6 (see
``docs/reverse-engineering.md``); this covers the data-extraction and verification
cases, which is most of what a protocol-recovery project needs.

Supported: CPython 3.5-3.8 marshal version 4 (the format PyInstaller emits). Type codes
follow CPython's ``Python/marshal.c``.

Usage
-----
    pyc_inspect.py consts   <blob.pyc> [--py 3.8]   # all constants, recursively
    pyc_inspect.py names    <blob.pyc> [--py 3.8]   # co_names at module level
    pyc_inspect.py assigns  <blob.pyc> [--py 3.8]   # module-level NAME = literal
    pyc_inspect.py strings  <blob.pyc> [--py 3.8]   # every string constant
    pyc_inspect.py get      <blob.pyc> NAME [--py 3.8]

``--py`` selects the code-object field layout; it defaults to 3.8. Use 3.5/3.6/3.7 for
older bundles (3.8 added ``posonlyargcount``).
"""

from __future__ import annotations

import argparse
import struct
import sys
from dataclasses import dataclass, field
from typing import Any

FLAG_REF = 0x80

# Type codes from CPython Python/marshal.c.
TYPE_NULL = "0"
TYPE_NONE = "N"
TYPE_FALSE = "F"
TYPE_TRUE = "T"
TYPE_STOPITER = "S"
TYPE_ELLIPSIS = "."
TYPE_INT = "i"
TYPE_INT64 = "I"
TYPE_FLOAT = "f"
TYPE_BINARY_FLOAT = "g"
TYPE_COMPLEX = "x"
TYPE_BINARY_COMPLEX = "y"
TYPE_LONG = "l"
TYPE_STRING = "s"
TYPE_INTERNED = "t"
TYPE_REF = "r"
TYPE_TUPLE = "("
TYPE_LIST = "["
TYPE_DICT = "{"
TYPE_CODE = "c"
TYPE_UNICODE = "u"
TYPE_UNKNOWN = "?"
TYPE_SET = "<"
TYPE_FROZENSET = ">"
TYPE_ASCII = "a"
TYPE_ASCII_INTERNED = "A"
TYPE_SMALL_TUPLE = ")"
TYPE_SHORT_ASCII = "z"
TYPE_SHORT_ASCII_INTERNED = "Z"


@dataclass
class Code:
    """A decoded code object. Field names mirror CPython's."""

    name: str = ""
    filename: str = ""
    firstlineno: int = 0
    argcount: int = 0
    flags: int = 0
    code: bytes = b""
    consts: tuple = ()
    names: tuple = ()
    varnames: tuple = ()
    freevars: tuple = ()
    cellvars: tuple = ()
    children: list = field(default_factory=list)

    def __repr__(self) -> str:
        return f"<Code {self.name!r} at {self.filename}:{self.firstlineno}>"


class MarshalError(Exception):
    pass


class Unmarshaller:
    """Minimal CPython marshal reader.

    ``py`` is the (major, minor) version whose code-object layout to expect. The only
    layout difference across 3.5-3.8 is ``posonlyargcount``, added in 3.8.
    """

    def __init__(self, data: bytes, py: tuple[int, int] = (3, 8)) -> None:
        self.data = data
        self.pos = 0
        self.py = py
        self.refs: list[Any] = []

    # -- primitives ---------------------------------------------------------
    def _read(self, n: int) -> bytes:
        if self.pos + n > len(self.data):
            raise MarshalError(
                f"truncated: wanted {n} bytes at offset {self.pos}, "
                f"only {len(self.data) - self.pos} remain"
            )
        b = self.data[self.pos : self.pos + n]
        self.pos += n
        return b

    def _u8(self) -> int:
        return self._read(1)[0]

    def _i32(self) -> int:
        return struct.unpack("<i", self._read(4))[0]

    def _i64(self) -> int:
        return struct.unpack("<q", self._read(8))[0]

    # -- reference table ----------------------------------------------------
    def _reserve(self, flag: bool) -> int | None:
        """Reserve a slot before reading a container, so self-references resolve."""
        if not flag:
            return None
        self.refs.append(None)
        return len(self.refs) - 1

    def _store(self, idx: int | None, value: Any) -> Any:
        if idx is not None:
            self.refs[idx] = value
        return value

    # -- main dispatch ------------------------------------------------------
    def load(self) -> Any:
        code = self._u8()
        flag = bool(code & FLAG_REF)
        t = chr(code & ~FLAG_REF)

        if t in (TYPE_NULL, TYPE_NONE):
            return None
        if t == TYPE_STOPITER:
            return StopIteration
        if t == TYPE_ELLIPSIS:
            return Ellipsis
        if t == TYPE_FALSE:
            return False
        if t == TYPE_TRUE:
            return True

        if t == TYPE_INT:
            return self._store(self._reserve(flag), self._i32())
        if t == TYPE_INT64:
            return self._store(self._reserve(flag), self._i64())
        if t == TYPE_LONG:
            return self._store(self._reserve(flag), self._long())
        if t == TYPE_FLOAT:
            n = self._u8()
            return self._store(self._reserve(flag), float(self._read(n)))
        if t == TYPE_BINARY_FLOAT:
            return self._store(
                self._reserve(flag), struct.unpack("<d", self._read(8))[0]
            )
        if t == TYPE_COMPLEX:
            nr = self._u8(); real = float(self._read(nr))
            ni = self._u8(); imag = float(self._read(ni))
            return self._store(self._reserve(flag), complex(real, imag))
        if t == TYPE_BINARY_COMPLEX:
            real, imag = struct.unpack("<dd", self._read(16))
            return self._store(self._reserve(flag), complex(real, imag))

        if t == TYPE_STRING:
            idx = self._reserve(flag)
            return self._store(idx, self._read(self._i32()))
        if t in (TYPE_UNICODE, TYPE_INTERNED, TYPE_ASCII, TYPE_ASCII_INTERNED):
            idx = self._reserve(flag)
            raw = self._read(self._i32())
            enc = "utf-8" if t in (TYPE_UNICODE, TYPE_INTERNED) else "latin-1"
            return self._store(idx, raw.decode(enc, "surrogateescape"))
        if t in (TYPE_SHORT_ASCII, TYPE_SHORT_ASCII_INTERNED):
            idx = self._reserve(flag)
            raw = self._read(self._u8())
            return self._store(idx, raw.decode("latin-1", "surrogateescape"))

        if t in (TYPE_TUPLE, TYPE_SMALL_TUPLE):
            n = self._u8() if t == TYPE_SMALL_TUPLE else self._i32()
            idx = self._reserve(flag)
            return self._store(idx, tuple(self.load() for _ in range(n)))
        if t == TYPE_LIST:
            n = self._i32()
            idx = self._reserve(flag)
            out: list = []
            self._store(idx, out)
            for _ in range(n):
                out.append(self.load())
            return out
        if t in (TYPE_SET, TYPE_FROZENSET):
            n = self._i32()
            idx = self._reserve(flag)
            items = [self.load() for _ in range(n)]
            value = frozenset(items) if t == TYPE_FROZENSET else set(items)
            return self._store(idx, value)
        if t == TYPE_DICT:
            idx = self._reserve(flag)
            d: dict = {}
            self._store(idx, d)
            while True:
                k = self.load()
                if k is None:
                    break
                d[k] = self.load()
            return d

        if t == TYPE_REF:
            i = self._i32()
            if not 0 <= i < len(self.refs):
                raise MarshalError(f"reference {i} out of range")
            return self.refs[i]

        if t == TYPE_CODE:
            return self._code(flag)

        raise MarshalError(
            f"unknown marshal type {t!r} (0x{code:02x}) at offset {self.pos - 1}"
        )

    def _long(self) -> int:
        n = self._i32()
        if n == 0:
            return 0
        neg = n < 0
        n = abs(n)
        value = 0
        for i in range(n):
            digit = struct.unpack("<H", self._read(2))[0]
            value |= digit << (i * 15)
        return -value if neg else value

    def _code(self, flag: bool) -> Code:
        idx = self._reserve(flag)
        c = Code()
        self._store(idx, c)

        c.argcount = self._i32()
        if self.py >= (3, 8):
            self._i32()  # posonlyargcount, 3.8+
        self._i32()  # kwonlyargcount
        self._i32()  # nlocals
        self._i32()  # stacksize
        c.flags = self._i32()
        c.code = self.load()
        c.consts = self.load() or ()
        c.names = self.load() or ()
        c.varnames = self.load() or ()
        c.freevars = self.load() or ()
        c.cellvars = self.load() or ()
        c.filename = self.load() or ""
        c.name = self.load() or ""
        c.firstlineno = self._i32()
        self.load()  # lnotab

        c.children = [k for k in (c.consts or ()) if isinstance(k, Code)]
        return c


def load_blob(path: str, py: tuple[int, int] = (3, 8)) -> Code:
    """Load a headerless marshal blob (a PYZ entry) as a code object."""
    with open(path, "rb") as f:
        data = f.read()
    # Tolerate a real .pyc header if one is present.
    if len(data) > 16 and data[2:4] == b"\r\n":
        data = data[16:]
    obj = Unmarshaller(data, py).load()
    if not isinstance(obj, Code):
        raise MarshalError(f"{path}: expected a code object, got {type(obj).__name__}")
    return obj


# -- bytecode-level helpers -------------------------------------------------

def module_assignments(code: Code) -> dict[str, Any]:
    """Recover module-level ``NAME = <literal>`` assignments.

    Scans for the ``LOAD_CONST k; STORE_NAME n`` pair that a simple literal assignment
    compiles to. Only literal assignments are recovered; computed values are skipped
    rather than guessed at.
    """
    LOAD_CONST, STORE_NAME = 100, 90
    out: dict[str, Any] = {}
    b = code.code
    i = 0
    last_const = None
    while i + 1 < len(b):
        op, arg = b[i], b[i + 1]
        if op == LOAD_CONST:
            last_const = code.consts[arg] if arg < len(code.consts) else None
        elif op == STORE_NAME and last_const is not None:
            if arg < len(code.names):
                value = last_const
                if not isinstance(value, Code):
                    out[code.names[arg]] = value
            last_const = None
        elif op not in (LOAD_CONST,):
            # EXTENDED_ARG (144) keeps the pending const; anything else clears it.
            if op != 144:
                last_const = None
        i += 2
    return out


def walk(code: Code):
    yield code
    for child in code.children:
        yield from walk(child)


def all_strings(code: Code) -> set[str]:
    out: set[str] = set()
    for c in walk(code):
        out.update(n for n in c.names if isinstance(n, str))
        out.update(n for n in c.varnames if isinstance(n, str))
        for k in c.consts:
            if isinstance(k, str):
                out.add(k)
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("action", choices=["consts", "names", "assigns", "strings", "get"])
    ap.add_argument("blob")
    ap.add_argument("key", nargs="?")
    ap.add_argument("--py", default="3.8", help="bytecode version, e.g. 3.5 or 3.8")
    args = ap.parse_args()

    major, minor = (int(x) for x in args.py.split("."))
    try:
        code = load_blob(args.blob, (major, minor))
    except MarshalError as e:
        print(f"error: {e}", file=sys.stderr)
        print(
            "hint: try a different --py; 3.8 added posonlyargcount to the code layout.",
            file=sys.stderr,
        )
        return 2

    if args.action == "consts":
        for c in walk(code):
            for k in c.consts:
                if not isinstance(k, Code):
                    print(f"{c.name}: {k!r}")
    elif args.action == "names":
        for n in code.names:
            print(n)
    elif args.action == "assigns":
        for k, v in sorted(module_assignments(code).items()):
            print(f"{k} = {v!r}")
    elif args.action == "strings":
        for s in sorted(all_strings(code)):
            print(s)
    elif args.action == "get":
        if not args.key:
            print("error: 'get' needs a NAME", file=sys.stderr)
            return 2
        vals = module_assignments(code)
        if args.key not in vals:
            print(f"error: {args.key} is not a module-level literal assignment", file=sys.stderr)
            return 1
        print(vals[args.key])
    return 0


if __name__ == "__main__":
    sys.exit(main())
