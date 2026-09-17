#!/usr/bin/env python3
"""Evaluate module and class bodies from CPython 3.5-3.8 bytecode, symbolically.

Why
---
The vendor panels declare most of what a generator needs as plain class attributes: an effect's
``description = OrderedDict([('threshold', ['uint8', 0, 120, 90]), ...])``, a widget's
``min_value = 0``, ``Effect.to_control('threshold')`` decorating a widget class. Reading those by
pattern-matching instruction pairs (as ``afx_catalogue.py`` does for one attribute) does not scale
to nested literals, so this runs the body on a small stack machine instead: literals are built for
real, names and attributes it cannot know stay symbolic (:class:`Sym`), calls stay as :class:`Call`
records, and nested ``class`` statements become :class:`ClassDef` with their own evaluated
attributes. Nothing is imported and no vendor code runs.

Anything it does not model (loops, comprehensions, jumps) clears the stack, so an attribute built
that way reads as missing rather than wrong. Callers must treat a missing or symbolic value as
unknown, never as zero.
"""

from __future__ import annotations

import os
import sys
from typing import Any, Callable

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pyc_dis  # noqa: E402
from pyc_inspect import Code  # noqa: E402


class Sym:
    """A name or attribute path the body refers to but does not define, e.g. ``constants.AfxType.POWERGATE``."""

    def __init__(self, path: str):
        self.path = path

    def __repr__(self) -> str:
        return f"Sym({self.path})"

    def __eq__(self, other: object) -> bool:
        return isinstance(other, Sym) and other.path == self.path

    def __hash__(self) -> int:
        return hash(self.path)


class Call:
    """A call the evaluator does not perform: the callee and its evaluated arguments."""

    def __init__(self, func: Any, args: list, kwargs: dict | None = None):
        self.func, self.args, self.kwargs = func, args, kwargs or {}

    def __repr__(self) -> str:
        return f"Call({self.func!r}, {self.args!r})"

    def __getitem__(self, index: Any) -> "Call":
        return Call(Sym("__getitem__"), [self, index])


class Func:
    """A function object: its code, not evaluated."""

    def __init__(self, code: Code):
        self.code = code

    def __repr__(self) -> str:
        return f"Func({getattr(self.code, 'name', '?')})"


class Attr:
    """An attribute of a call's result, e.g. the `move` of `Widget(self).move(...)`."""

    def __init__(self, receiver: Call, name: str):
        self.receiver, self.name = receiver, name

    def __repr__(self) -> str:
        return f"Attr({self.receiver!r}.{self.name})"


def receiver(value: Any) -> Any:
    """The innermost call of a method chain: `Widget(self)` for `Widget(self).move(1, 2).show()`."""
    while isinstance(value, Call) and isinstance(value.func, Attr):
        value = value.func.receiver
    return value


def function(value: Any) -> Code | None:
    """The code of a method, seeing through a decorator call such as `reconfigure_effect(func)`."""
    while isinstance(value, Call) and len(value.args) == 1:
        value = value.args[0]
    return value.code if isinstance(value, Func) else None


class Unknown:
    def __repr__(self) -> str:
        return "?"


UNKNOWN = Unknown()


class ClassDef:
    """A class statement: name, bases as evaluated, attributes from its body, decorators applied."""

    def __init__(self, name: str, body: Code, bases: list):
        self.name, self.body, self.bases = name, body, bases
        self.attrs: dict[str, Any] = {}
        self.decorators: list[Any] = []
        self.module = ""

    def __repr__(self) -> str:
        return f"ClassDef({self.name})"

    def lookup(self, attr: str, default: Any = None) -> Any:
        """An attribute, from this class or the first base (single inheritance along evaluated bases) that has it."""
        seen = set()
        cls: Any = self
        while isinstance(cls, ClassDef) and id(cls) not in seen:
            seen.add(id(cls))
            if attr in cls.attrs:
                return cls.attrs[attr]
            cls = next((b for b in reversed(cls.bases) if isinstance(b, ClassDef)), None)
        return default

    def ancestry(self) -> list[str]:
        """Every base, as a dotted name for symbolic ones and a class name for evaluated ones, nearest first."""
        out: list[str] = []
        stack = list(reversed(self.bases))
        while stack:
            base = stack.pop()
            if isinstance(base, ClassDef):
                out.append(base.name)
                stack.extend(reversed(base.bases))
            elif isinstance(base, Sym):
                out.append(base.path)
        return out


BINOPS: dict[str, Callable[[Any, Any], Any]] = {
    "BINARY_ADD": lambda a, b: a + b,
    "BINARY_SUBTRACT": lambda a, b: a - b,
    "BINARY_MULTIPLY": lambda a, b: a * b,
    "BINARY_TRUE_DIVIDE": lambda a, b: a / b,
    "BINARY_FLOOR_DIVIDE": lambda a, b: a // b,
    "BINARY_OR": lambda a, b: a | b,
    "BINARY_AND": lambda a, b: a & b,
    "BINARY_LSHIFT": lambda a, b: a << b,
    "BINARY_POWER": lambda a, b: a**b,
    "BINARY_MODULO": lambda a, b: a % b,
}


def _numeric(*values: Any) -> bool:
    return all(isinstance(v, (int, float)) and not isinstance(v, bool) for v in values)


def evaluate(code: Code, py: tuple[int, int], scope: dict | None = None, on_class: Callable[[ClassDef], None] | None = None, built: list | None = None) -> dict[str, Any]:
    """Run a module, class or function body and return the names it stores.

    `on_class` sees every class statement, nested ones too. `built`, when given, collects every tuple
    and list the body builds and every call it makes, so a literal that is only iterated over, or a
    widget that is created and never stored, can still be found.
    """
    local: dict[str, Any] = {} if scope is None else scope
    stack: list[Any] = []

    def pop(n: int = 1) -> list[Any]:
        out = [stack.pop() if stack else UNKNOWN for _ in range(n)]
        return out[::-1]

    for ins in pyc_dis.disassemble(code, py):
        op, arg = ins.opname, ins.arg
        try:
            if op == "LOAD_CONST":
                value = code.consts[arg]
                stack.append(Func(value) if isinstance(value, Code) else value)
            elif op in ("LOAD_NAME", "LOAD_GLOBAL"):
                name = code.names[arg]
                stack.append(local[name] if name in local else Sym(name))
            elif op == "LOAD_FAST":
                # A function's argument or local: symbolic, so calls on `self` still record their arguments.
                name = code.varnames[arg]
                stack.append(local[name] if name in local else Sym(name))
            elif op in ("LOAD_ATTR", "LOAD_METHOD"):
                (obj,) = pop()
                name = code.names[arg]
                if isinstance(obj, Sym):
                    stack.append(Sym(f"{obj.path}.{name}"))
                elif isinstance(obj, ClassDef):
                    stack.append(obj.lookup(name, Sym(f"{obj.name}.{name}")))
                elif isinstance(obj, Call):
                    # A method of a call's result, e.g. `Widget(self).move(10, 20)`: the call is kept as the receiver.
                    stack.append(Attr(obj, name))
                else:
                    stack.append(UNKNOWN)
            elif op in ("BUILD_LIST", "BUILD_TUPLE", "BUILD_SET"):
                items = pop(arg)
                value = list(items) if op == "BUILD_LIST" else tuple(items)
                if built is not None:
                    built.append(value)
                stack.append(value)
            elif op == "BUILD_MAP":
                items = pop(2 * arg)
                stack.append({_key(items[i]): items[i + 1] for i in range(0, len(items), 2)})
            elif op == "BUILD_CONST_KEY_MAP":
                (keys,) = pop()
                values = pop(arg)
                stack.append({_key(k): v for k, v in zip(keys, values)})
            elif op == "LOAD_BUILD_CLASS":
                stack.append(Sym("__build_class__"))
            elif op == "MAKE_FUNCTION":
                fn, _qualname = pop(2)
                # 3.6+: one extra item below the code per flag bit; 3.5: the low byte counts defaults.
                pop(bin(arg).count("1") if py >= (3, 6) else (arg & 0xFF) + ((arg >> 8) & 0xFF) * 2)
                stack.append(fn)
            elif op in ("CALL_FUNCTION", "CALL_METHOD"):
                if py < (3, 6) and op == "CALL_FUNCTION":
                    keywords, positional = (arg >> 8) & 0xFF, arg & 0xFF
                    raw = pop(2 * keywords)
                    args = pop(positional)
                    kwargs = {raw[i]: raw[i + 1] for i in range(0, len(raw), 2)}
                else:
                    args, kwargs = pop(arg), {}
                (fn,) = pop()
                result = _call(fn, list(args), kwargs, py, on_class)
                if built is not None and isinstance(result, Call):
                    built.append(result)
                stack.append(result)
            elif op == "CALL_FUNCTION_KW":
                if py < (3, 6):
                    stack.clear()
                    continue
                (names,) = pop()
                values = pop(arg)
                positional = arg - len(names)
                (fn,) = pop()
                stack.append(_call(fn, list(values[:positional]), dict(zip(names, values[positional:])), py, on_class))
            elif op == "UNARY_NEGATIVE":
                (value,) = pop()
                stack.append(-value if _numeric(value) else UNKNOWN)
            elif op in BINOPS:
                a, b = pop(2)
                stack.append(BINOPS[op](a, b) if _numeric(a, b) else UNKNOWN)
            elif op == "BINARY_SUBSCR":
                obj, index = pop(2)
                if isinstance(obj, Sym):
                    stack.append(Sym(f"{obj.path}[{index!r}]"))
                else:
                    try:
                        stack.append(obj[index])
                    except Exception:
                        stack.append(UNKNOWN)
            elif op == "STORE_NAME":
                (value,) = pop()
                local[code.names[arg]] = value
            elif op == "STORE_FAST":
                (value,) = pop()
                local[code.varnames[arg]] = value
            elif op == "UNPACK_SEQUENCE":
                (value,) = pop()
                if isinstance(value, (list, tuple)) and len(value) == arg:
                    stack.extend(reversed(value))
                elif isinstance(value, Call):
                    stack.extend(value[i] for i in reversed(range(arg)))
                else:
                    stack.extend([UNKNOWN] * arg)
            elif op == "POP_TOP":
                pop()
            elif op == "DUP_TOP":
                stack.append(stack[-1] if stack else UNKNOWN)
            elif op == "IMPORT_NAME":
                stack.clear()
                stack.append(Sym(code.names[arg]))
            elif op == "IMPORT_FROM":
                module = stack[-1] if stack else UNKNOWN
                stack.append(Sym(f"{module.path}.{code.names[arg]}") if isinstance(module, Sym) else UNKNOWN)
            elif op == "EXTENDED_ARG":
                continue
            else:
                stack.clear()
        except Exception:
            stack.clear()
    return local


def _key(key: Any) -> Any:
    return key if isinstance(key, (str, int, float, tuple)) else repr(key)


def _call(fn: Any, args: list, kwargs: dict, py: tuple[int, int], on_class: Callable[[ClassDef], None] | None) -> Any:
    if isinstance(fn, Sym) and fn.path == "__build_class__" and len(args) >= 2 and isinstance(args[0], Func):
        cls = ClassDef(args[1], args[0].code, args[2:])
        cls.attrs = evaluate(args[0].code, py, {}, on_class)
        if on_class is not None:
            on_class(cls)
        return cls
    if isinstance(fn, Sym) and fn.path.split(".")[-1] == "OrderedDict" and len(args) == 1 and isinstance(args[0], list):
        try:
            return {k: v for k, v in args[0]}
        except (TypeError, ValueError):
            return Call(fn, args, kwargs)
    # A decorator applied to a class statement: keep the class, and remember the decorator.
    if isinstance(fn, (Call, Sym)) and len(args) == 1 and isinstance(args[0], ClassDef) and not kwargs:
        args[0].decorators.append(fn)
        return args[0]
    if isinstance(fn, Sym) and fn.path.split(".")[-1] in ("staticmethod", "classmethod", "property") and args:
        return args[0]
    return Call(fn, args, kwargs)


def module_classes(path: str, py: tuple[int, int]) -> tuple[dict[str, Any], list[ClassDef]]:
    """A module's stored names and every class statement in it (nested ones included), in order."""
    from pyc_inspect import load_blob

    found: list[ClassDef] = []
    names = evaluate(load_blob(path, py), py, None, found.append)
    module = os.path.basename(path)
    for cls in found:
        cls.module = module
    return names, found
