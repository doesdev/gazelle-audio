#!/usr/bin/env python3
"""Disassemble CPython 3.5-3.8 bytecode without the matching interpreter.

Why
---
uncompyle6 fails on certain control flow (notably try/finally) and, when it does, writes a
``Parse error at or near ...`` line into the output instead of the function body. The
bytecode is still complete — only the *source reconstruction* failed. For understanding
protocol semantics a disassembly is sufficient, and it is more trustworthy than a
decompiler's guess because it is a direct read of what the interpreter executes.

``dis`` in a modern Python cannot do this: opcode numbers change between versions, so
disassembling 3.8 bytecode with 3.14's tables produces confident nonsense.

Usage
-----
    pyc_dis.py <blob.pyc> [--py 3.8] [--func NAME] [--list]

    --list     name every code object in the module
    --func     disassemble just this function (searched recursively)

Verify the tables before trusting output on a new version:

    pyc_dis.py --selftest
"""

from __future__ import annotations

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pyc_inspect import Code, load_blob, walk  # noqa: E402

# Opcode tables. Names and numbers follow CPython's Lib/opcode.py for each release.
# 3.6 introduced "wordcode": every instruction is exactly 2 bytes. In 3.5 an instruction
# is 1 byte, plus a 2-byte little-endian argument when the opcode is >= HAVE_ARGUMENT.
HAVE_ARGUMENT = 90

_COMMON = {
    1: "POP_TOP", 2: "ROT_TWO", 3: "ROT_THREE", 4: "DUP_TOP", 5: "DUP_TOP_TWO",
    9: "NOP", 10: "UNARY_POSITIVE", 11: "UNARY_NEGATIVE", 12: "UNARY_NOT",
    15: "UNARY_INVERT", 16: "BINARY_MATRIX_MULTIPLY", 17: "INPLACE_MATRIX_MULTIPLY",
    19: "BINARY_POWER", 20: "BINARY_MULTIPLY", 22: "BINARY_MODULO", 23: "BINARY_ADD",
    24: "BINARY_SUBTRACT", 25: "BINARY_SUBSCR", 26: "BINARY_FLOOR_DIVIDE",
    27: "BINARY_TRUE_DIVIDE", 28: "INPLACE_FLOOR_DIVIDE", 29: "INPLACE_TRUE_DIVIDE",
    50: "GET_AITER", 51: "GET_ANEXT", 52: "BEFORE_ASYNC_WITH",
    55: "INPLACE_ADD", 56: "INPLACE_SUBTRACT", 57: "INPLACE_MULTIPLY",
    59: "INPLACE_MODULO", 60: "STORE_SUBSCR", 61: "DELETE_SUBSCR",
    62: "BINARY_LSHIFT", 63: "BINARY_RSHIFT", 64: "BINARY_AND", 65: "BINARY_XOR",
    66: "BINARY_OR", 67: "INPLACE_POWER", 68: "GET_ITER", 69: "GET_YIELD_FROM_ITER",
    70: "PRINT_EXPR", 71: "LOAD_BUILD_CLASS", 72: "YIELD_FROM", 73: "GET_AWAITABLE",
    75: "INPLACE_LSHIFT", 76: "INPLACE_RSHIFT", 77: "INPLACE_AND", 78: "INPLACE_XOR",
    79: "INPLACE_OR", 80: "WITH_CLEANUP_START", 81: "WITH_CLEANUP_FINISH",
    83: "RETURN_VALUE", 84: "IMPORT_STAR", 85: "SETUP_ANNOTATIONS", 86: "YIELD_VALUE",
    87: "POP_BLOCK", 89: "POP_EXCEPT",
    90: "STORE_NAME", 91: "DELETE_NAME", 92: "UNPACK_SEQUENCE", 93: "FOR_ITER",
    94: "UNPACK_EX", 95: "STORE_ATTR", 96: "DELETE_ATTR", 97: "STORE_GLOBAL",
    98: "DELETE_GLOBAL", 100: "LOAD_CONST", 101: "LOAD_NAME", 102: "BUILD_TUPLE",
    103: "BUILD_LIST", 104: "BUILD_SET", 105: "BUILD_MAP", 106: "LOAD_ATTR",
    107: "COMPARE_OP", 108: "IMPORT_NAME", 109: "IMPORT_FROM", 110: "JUMP_FORWARD",
    111: "JUMP_IF_FALSE_OR_POP", 112: "JUMP_IF_TRUE_OR_POP", 113: "JUMP_ABSOLUTE",
    114: "POP_JUMP_IF_FALSE", 115: "POP_JUMP_IF_TRUE", 116: "LOAD_GLOBAL",
    122: "SETUP_FINALLY", 124: "LOAD_FAST", 125: "STORE_FAST", 126: "DELETE_FAST",
    130: "RAISE_VARARGS", 131: "CALL_FUNCTION", 132: "MAKE_FUNCTION",
    133: "BUILD_SLICE", 135: "LOAD_CLOSURE", 136: "LOAD_DEREF", 137: "STORE_DEREF",
    138: "DELETE_DEREF", 141: "CALL_FUNCTION_KW", 142: "CALL_FUNCTION_EX",
    143: "SETUP_WITH", 144: "EXTENDED_ARG", 145: "LIST_APPEND", 146: "SET_ADD",
    147: "MAP_ADD", 148: "LOAD_CLASSDEREF", 149: "BUILD_LIST_UNPACK",
    150: "BUILD_MAP_UNPACK", 151: "BUILD_MAP_UNPACK_WITH_CALL",
    152: "BUILD_TUPLE_UNPACK", 153: "BUILD_SET_UNPACK", 154: "SETUP_ASYNC_WITH",
    155: "FORMAT_VALUE", 156: "BUILD_CONST_KEY_MAP", 157: "BUILD_STRING",
    158: "BUILD_TUPLE_UNPACK_WITH_CALL",
}

OPNAME = {
    (3, 5): {**_COMMON, 48: "END_FINALLY", 88: "END_FINALLY",
             120: "SETUP_LOOP", 121: "SETUP_EXCEPT", 119: "BREAK_LOOP",
             82: "WITH_CLEANUP_FINISH", 117: "CONTINUE_LOOP",
             134: "MAKE_CLOSURE", 140: "CALL_FUNCTION_VAR",
             139: "CALL_FUNCTION_VAR_KW"},
    (3, 6): {**_COMMON, 88: "END_FINALLY", 120: "SETUP_LOOP", 121: "SETUP_EXCEPT",
             119: "BREAK_LOOP", 117: "CONTINUE_LOOP", 160: "LOAD_METHOD",
             161: "CALL_METHOD"},
    (3, 7): {**_COMMON, 88: "END_FINALLY", 120: "SETUP_LOOP", 121: "SETUP_EXCEPT",
             119: "BREAK_LOOP", 117: "CONTINUE_LOOP", 160: "LOAD_METHOD",
             161: "CALL_METHOD"},
    # 3.8 removed SETUP_LOOP/SETUP_EXCEPT/BREAK_LOOP/CONTINUE_LOOP and added
    # ROT_FOUR / BEGIN_FINALLY / END_ASYNC_FOR / CALL_FINALLY.
    (3, 8): {**_COMMON, 6: "ROT_FOUR", 53: "BEGIN_FINALLY", 54: "END_ASYNC_FOR",
             88: "END_FINALLY", 162: "CALL_FINALLY", 163: "POP_FINALLY",
             160: "LOAD_METHOD", 161: "CALL_METHOD"},
}

# Which name table an argument indexes into.
HAS_NAME = {"STORE_NAME", "DELETE_NAME", "STORE_ATTR", "DELETE_ATTR", "STORE_GLOBAL",
            "DELETE_GLOBAL", "LOAD_NAME", "LOAD_ATTR", "IMPORT_NAME", "IMPORT_FROM",
            "LOAD_GLOBAL", "LOAD_METHOD"}
HAS_LOCAL = {"LOAD_FAST", "STORE_FAST", "DELETE_FAST"}
HAS_CONST = {"LOAD_CONST"}
HAS_FREE = {"LOAD_CLOSURE", "LOAD_DEREF", "STORE_DEREF", "DELETE_DEREF",
            "LOAD_CLASSDEREF"}
HAS_JREL = {"JUMP_FORWARD", "FOR_ITER", "SETUP_FINALLY", "SETUP_WITH",
            "SETUP_ASYNC_WITH", "SETUP_LOOP", "SETUP_EXCEPT", "CALL_FINALLY"}
HAS_JABS = {"JUMP_ABSOLUTE", "JUMP_IF_FALSE_OR_POP", "JUMP_IF_TRUE_OR_POP",
            "POP_JUMP_IF_FALSE", "POP_JUMP_IF_TRUE", "CONTINUE_LOOP"}
CMP_OP = ("<", "<=", "==", "!=", ">", ">=", "in", "not in", "is", "is not",
          "exception match", "BAD")


class Instr:
    __slots__ = ("offset", "opcode", "opname", "arg", "argrepr", "target")

    def __init__(self, offset, opcode, opname, arg, argrepr, target):
        self.offset, self.opcode, self.opname = offset, opcode, opname
        self.arg, self.argrepr, self.target = arg, argrepr, target


def disassemble(code: Code, py=(3, 8)) -> list[Instr]:
    """Decode a code object's bytecode into instructions."""
    table = OPNAME.get(tuple(py))
    if table is None:
        raise ValueError(f"no opcode table for Python {py[0]}.{py[1]}")
    b = code.code
    wordcode = tuple(py) >= (3, 6)
    out: list[Instr] = []
    i = 0
    ext = 0
    while i < len(b):
        offset = i
        op = b[i]
        name = table.get(op, f"<{op}>")
        if wordcode:
            arg = b[i + 1] | ext if op >= HAVE_ARGUMENT else None
            i += 2
        else:
            if op >= HAVE_ARGUMENT:
                arg = (b[i + 1] | (b[i + 2] << 8)) | ext
                i += 3
            else:
                arg = None
                i += 1
        if name == "EXTENDED_ARG":
            ext = (arg << 8) if wordcode else (arg << 16)
            out.append(Instr(offset, op, name, arg, "", None))
            continue
        ext = 0

        argrepr, target = "", None
        if arg is not None:
            if name in HAS_CONST:
                v = code.consts[arg] if arg < len(code.consts) else "?"
                argrepr = repr(v) if not isinstance(v, Code) else f"<code {v.name}>"
            elif name in HAS_NAME:
                argrepr = code.names[arg] if arg < len(code.names) else "?"
            elif name in HAS_LOCAL:
                argrepr = code.varnames[arg] if arg < len(code.varnames) else "?"
            elif name in HAS_FREE:
                pool = list(code.cellvars) + list(code.freevars)
                argrepr = pool[arg] if arg < len(pool) else "?"
            elif name == "COMPARE_OP":
                argrepr = CMP_OP[arg] if arg < len(CMP_OP) else "?"
            elif name in HAS_JREL:
                target = i + arg
                argrepr = f"to {target}"
            elif name in HAS_JABS:
                target = arg
                argrepr = f"to {target}"
            else:
                argrepr = str(arg)
        out.append(Instr(offset, op, name, arg, argrepr, target))
    return out


def render(code: Code, py=(3, 8)) -> str:
    instrs = disassemble(code, py)
    targets = {i.target for i in instrs if i.target is not None}
    lines = [f"# {code.name}  ({code.filename}:{code.firstlineno})",
             f"# args={code.argcount} locals={list(code.varnames)}"]
    for ins in instrs:
        mark = ">>" if ins.offset in targets else "  "
        arg = "" if ins.arg is None else f"{ins.arg:>4}"
        lines.append(f"{mark} {ins.offset:>4} {ins.opname:<24} {arg} {ins.argrepr}")
    return "\n".join(lines)


def find(code: Code, name: str):
    for c in walk(code):
        if c.name == name:
            return c
    return None


def selftest() -> int:
    """Disassemble this very module under the host Python and compare against `dis`.

    Validates the decoder's structure (offsets, argument resolution) on a version we can
    check. The per-version opcode *numbers* still need checking against a module whose
    source is known — see the README.
    """
    import dis as _dis
    import types
    host = sys.version_info[:2]
    if host not in OPNAME:
        print(f"host Python {host[0]}.{host[1]} has no table here; "
              f"selftest validates structure only on 3.5-3.8", file=sys.stderr)
        return 0
    src = "def f(a, b):\n    x = a + b\n    if x > 3:\n        return [x, a]\n    return None\n"
    ns: dict = {}
    exec(compile(src, "<selftest>", "exec"), ns)
    fn: types.FunctionType = ns["f"]
    c = Code(name=fn.__code__.co_name, code=fn.__code__.co_code,
             consts=fn.__code__.co_consts, names=fn.__code__.co_names,
             varnames=fn.__code__.co_varnames, argcount=fn.__code__.co_argcount)
    mine = [(i.offset, i.opname) for i in disassemble(c, host)]
    theirs = [(i.offset, i.opname) for i in _dis.get_instructions(fn)]
    ok = mine == theirs
    print("selftest:", "PASS" if ok else "FAIL")
    if not ok:
        for a, b in zip(mine, theirs):
            if a != b:
                print("  mine:", a, " dis:", b)
    return 0 if ok else 1


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("blob", nargs="?")
    ap.add_argument("--py", default="3.8")
    ap.add_argument("--func")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    if not args.blob:
        ap.error("a blob is required unless --selftest")
    py = tuple(int(x) for x in args.py.split("."))
    code = load_blob(args.blob, py)
    if args.list:
        for c in walk(code):
            print(f"{c.name:32} {c.filename}:{c.firstlineno}  ({len(c.code)} bytes)")
        return 0
    target = find(code, args.func) if args.func else code
    if target is None:
        print(f"error: no code object named {args.func!r}", file=sys.stderr)
        return 1
    print(render(target, py))
    return 0


if __name__ == "__main__":
    sys.exit(main())
