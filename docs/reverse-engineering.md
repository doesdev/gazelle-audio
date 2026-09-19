# Reverse engineering

How the command definitions in `refs/schemas/` were recovered from Antelope Audio's own control
software, so that anyone with their own copy of that software can check them or regenerate
them. This is a developer document, not part of the user manual. What the protocol turned out
to be is in [The control protocol](protocol.md).

This repository does not contain Antelope's software or anything decompiled from it. Those
inputs stay on your own machine, under paths this repository ignores (`refs/bin/`,
`refs/extracted/`, `refs/decompiled/`). The tools here operate on software you must already
have, for interoperability with hardware you own. Gazelle is not affiliated with or endorsed by
Antelope Audio.

## What the vendor software is

Antelope's control panels and their background manager service are Python programs packaged
with PyInstaller. Each executable holds a `PYZ-00.pyz` archive of compiled Python modules, and
the device's vocabulary (every command, its header and its fields) is a Python data structure in
the **panel** executables, not in the service: the service speaks a fixed wire protocol and is
handed each device's command format at run time.

| Bundle | Python bytecode | Magic |
|---|---|---|
| Manager service (`AntelopeAudioServer.exe`) | 3.8, x64 | `550d0d0a` |
| Zen Quadro panel (`zenquadrosc_usb2.exe`) | 3.8, x64 | `550d0d0a` |
| Zen Studio+ panel (`zenstudiotb.exe`) | 3.5, x86 | `170d0d0a` |

The versions that produced the committed schemas were Quadro panel 1.0.4 (report format
version 20), Studio+ panel 1.4.1 and manager service 1.8.20. Each schema records its `source`
and `report_version`.

## The tools

Everything lives in `refs/tools/scripts/`; its `README.md` has a table of what each tool does.
Most of them run on any modern Python 3, because `pyc_inspect.py` reads CPython's marshal
format directly instead of loading bytecode in a matching interpreter. Only recovering whole
statements as source needs an old interpreter and a decompiler.

| Task | Tool | Needs a legacy interpreter? |
|---|---|---|
| Unpack the outer `.exe` into `PYZ-00.pyz` | pyinstxtractor (upstream, not included) | No |
| Unpack `PYZ-00.pyz` into `.pyc` blobs | `extract_pyz.py` | No |
| Read constants, names and docstrings from a blob | `pyc_inspect.py` | No |
| Disassemble a function a decompiler could not render | `pyc_dis.py` | No |
| Read class attributes and literals (an effect's parameters, a widget's range) | `bytecode_eval.py` | No |
| Check decompiled source against its bytecode | `verify_decompiled.py` | No |
| Extract the command schemas | `extract_field_layouts.py` | No |
| Compare two panels' command sets, or two schemas | `extract_commands.py`, `compare_schemas.py` | No |
| Extract routing topology | `extract_topology.py` | No |
| Effect parameter commands and editor metadata | `afx_parameters.py` | No |
| Effect names per type id | `afx_catalogue.py` | No |
| Microphone emulations per microphone | `mic_emulations.py` | No |
| Recover full statements as source | `decompile_panel.py`, which runs `decompile_one.py` | Yes |

## 1. Get the archive out of the executable

The outer executable is unpacked with **pyinstxtractor**. It is not included here: it is
GPL-3.0 and this repository is MIT, so shipping it would put the tree under conflicting terms.
Install it from upstream:

```bash
git clone https://github.com/extremecoders-re/pyinstxtractor
python3 pyinstxtractor/pyinstxtractor.py refs/bin/quadro/zenquadrosc_usb2.exe
```

That yields `PYZ-00.pyz`. When your Python differs from the one the bundle was built with,
pyinstxtractor warns and skips unpacking the inner archive; that is expected. Unpack it with
`extract_pyz.py` instead:

```bash
python3 refs/tools/scripts/extract_pyz.py refs/extracted/quadro/PYZ-00.pyz refs/extracted/quadro/PYZ_extracted
```

The archive is `b"PYZ\0"`, the 4-byte CPython magic, a big-endian table-of-contents offset, a
marshalled `{name: (ispkg, pos, length)}` dictionary and zlib-compressed blobs.
`extract_pyz.py` writes each blob with `/` and `.` in its name replaced by `_`.

## 2. Read what you need without decompiling

Most of what a protocol needs is data, and data survives in the bytecode's constants:

```bash
# what does a module contain?
python3 refs/tools/scripts/pyc_inspect.py assigns refs/extracted/quadro/PYZ_extracted/antelope_ui_afx_platform_afx_pool.pyc --py 3.8
python3 refs/tools/scripts/pyc_inspect.py get     refs/extracted/quadro/PYZ_extracted/antelope_ui_afx_platform_afx_pool.pyc PHY_AFX_STRIP_SIZE --py 3.8

# a function the decompiler lost
python3 refs/tools/scripts/pyc_dis.py <blob.pyc> --func <name> --py 3.8
```

`pyc_inspect.py` reads marshal version 4 (CPython 3.5 to 3.8). It recovers module-level
`NAME = <literal>` assignments only; computed values are skipped rather than guessed.

## 3. Decompile, when you need statements

Recovering source needs `uncompyle6` running under an interpreter of the bundle's own Python
version:

| Bytecode | Interpreter | uncompyle6 |
|---|---|---|
| 3.5 | a Python 3.5 | 2.11.0 (uncompyle6 3.x does not run on 3.5) |
| 3.6, 3.7, 3.8 | a Python 3.8 | 3.9.3 |

```bash
conda create -n py38 python=3.8 && conda run -n py38 pip install uncompyle6
export ANTELOPE_PY38=$(conda run -n py38 which python)

conda create -n py35 python=3.5 && conda run -n py35 pip install 'uncompyle6==2.11.0'
export ANTELOPE_PY35=$(conda run -n py35 which python)
```

The tools find interpreters from `ANTELOPE_PY3x`, then `pythonX.Y` on `PATH`, then common conda
and pyenv locations. Then:

```bash
python3 refs/tools/scripts/decompile_panel.py <PYZ file or extracted dir> [WALK_DIR] [OUT_DIR] [module ...]
```

It finds `PYZ-00.pyz` next to the target, reads the magic from bytes 4 to 8, picks the
interpreter, and runs `decompile_one.py` once per module in it. Module names start at the
fourth argument; with none, it walks for every `antelope_*.pyc`.

### Why it is done this way

- **The blobs have no `.pyc` header.** PyInstaller stores bare marshalled code objects, without
  the 16-byte header (magic, flags, mtime, size) a `.pyc` file starts with, and the decompiler's
  file loader fails on them with "unknown type code". Prepending a fake header does not work
  either: it misaligns the stream. The blob is loaded with `marshal.loads` and the code object
  handed to the decompiler directly.
- **The magic is in the archive**, at bytes 4 to 8 of `PYZ-00.pyz`, and it decides both the
  interpreter and the decompiler's version argument. Never hard-code it: a wrong magic corrupts
  everything downstream without an error.
- **`marshal.loads` must run in the matching interpreter.** A 3.8 interpreter cannot read 3.5
  type codes.
- **`uncompyle6.main.decompile(co, (major, minor), out)`**: the code object first, the version
  second, and the version as a tuple, not a float. The package-level helpers want real `.pyc`
  files and fail on bare blobs.
- **Do not search a whole tree for `PYZ-00.pyz`.** Bundles ship different Python versions, and
  picking up the manager's 3.8 archive while decompiling the 3.5 Studio+ panel silently
  corrupts the result.

### Verify before you trust

Decompilers fail quietly: uncompyle6 writes a `Parse error at or near ...` line and carries on,
so a file can look complete while whole function bodies are gone. A tail parse error with the
rest intact is normal; `decompile_one.py` keeps partial output rather than losing it.

```bash
python3 refs/tools/scripts/verify_decompiled.py --batch \
    refs/extracted/manager/PYZ_extracted refs/decompiled/manager --py 3.8
```

`verify_decompiled.py` checks every identifier and module-level constant in the bytecode
against the decompiled text and exits non-zero when something is missing. It proves that names
and constants survived, not that the recovered logic is right. Where it matters (how a reply is
matched to a request, for example), the logic in Gazelle was read from the disassembly with
`pyc_dis.py`, not from decompiled source.

## 4. Regenerate the schemas

```bash
# the command schemas (the blob directory resolves constants from modules never decompiled)
python3 refs/tools/scripts/extract_field_layouts.py \
    refs/decompiled/quadro/app_report_format.py \
    refs/schemas/quadro_commands.json \
    refs/extracted/quadro/PYZ_extracted --device zenquadrosc_usb2

python3 refs/tools/scripts/extract_field_layouts.py \
    refs/decompiled/studio/zenstudiotb_report_format.py \
    refs/schemas/studio_commands.json \
    refs/extracted/studio/PYZ_extracted --device zenstudiotb

# routing topology
python3 refs/tools/scripts/extract_topology.py --family quadro --out refs/schemas/quadro_topology.json
python3 refs/tools/scripts/extract_topology.py --family studio --out refs/schemas/studio_topology.json

# effect parameters (then extract_field_layouts.py --afx adds them to the schemas)
python3 refs/tools/scripts/afx_parameters.py refs/extracted/quadro/PYZ_extracted refs/extracted/studio/PYZ_extracted \
    --quadro-format <the Quadro's report format JSON> --studio-format refs/decompiled/studio/zenstudiotb_report_format.py

# the web app's effect names and microphone emulations
python3 refs/tools/scripts/afx_catalogue.py refs/extracted/quadro/PYZ_extracted refs/extracted/studio/PYZ_extracted
python3 refs/tools/scripts/mic_emulations.py refs/extracted/quadro/PYZ_extracted

# how two panels' command sets differ
python3 refs/tools/scripts/extract_commands.py \
    refs/decompiled/quadro/app_report_format.py \
    refs/decompiled/studio/zenstudiotb_report_format.py \
    --label-a quadro --label-b studio
```

`extract_field_layouts.py` reports anything it could not resolve in the schema itself
(`unresolved_constants`, `in_scope_zero_counts`, `unresolved_reply_counts`) rather than writing
a silent zero. Then run the tests: the Rust suite loads the schemas, and
`corepack pnpm -C web test` fails if the web client's generated types no longer match them
(`corepack pnpm -C web gen-types` regenerates them).

## 5. The test vectors

`crates/gazelle-audio-protocol/tests/ground_truth.py` and `gen_cyclic_gt.py` produce the byte
vectors the Rust protocol crate is tested against (`ground_truth.json` and `cyclic_gt.json`).
They are written in Python with `ctypes`, the way the vendor software serialises, and
independently of the Rust code, so the two check each other. `ground_truth.py` builds every
request in a schema with default values; `gen_cyclic_gt.py` carries the Studio+ `0x73` report
layout as data and builds a report from it.

## Other hardware

The same method should carry over to other Antelope interfaces whose panels are PyInstaller
bundles:

1. Read the magic and map it to a CPython version (CPython's `MAGIC_NUMBER` table in
   `importlib/_bootstrap_external.py`).
2. uncompyle6 covers roughly 2.7 to 3.8. For 3.9 and 3.10 there are the `decompyle3` forks;
   3.11 and later changed bytecode substantially, and `pycdc` or plain disassembly is the
   fallback. The data-extraction tools here need no decompiler at all, but `pyc_inspect.py`
   reads only marshal version 4 (3.5 to 3.8).
3. Check the result: the command format structure should be present and parseable. A tail
   parse error is acceptable; a missing top-level structure is not.
4. Recover every per-platform variant of a panel you care about: they share the wire protocol
   but differ in transport.

None of this has been tried on 3.6, 3.7, 3.9 or 3.10 bytecode.
