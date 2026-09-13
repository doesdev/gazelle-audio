# Recovery toolchain

Tools for recovering the Antelope control protocol from the vendor's PyInstaller bundles,
and for **checking that what came out matches what went in**.

Nothing here needs the original software to run — but the inputs do. Bring your own copy;
see `.agent/reference/decompilation.md` for the full pipeline.

## No legacy interpreter needed

| Task | Tool | Needs uncompyle6? |
|---|---|---|
| Unpack the outer `.exe` into a `PYZ-00.pyz` | **pyinstxtractor** (upstream, GPL-3.0 — not vendored) | no |
| Unpack a `PYZ-00.pyz` into `.pyc` blobs | `extract_pyz.py` | no |
| Read constants, names, docstrings from a blob | `pyc_inspect.py` | **no** |
| Check decompiled source against its bytecode | `verify_decompiled.py` | **no** |
| Extract command field layouts | `extract_field_layouts.py` | no |
| Compare two panels' command sets | `extract_commands.py` | no |
| Disassemble a function uncompyle6 could not render | `pyc_dis.py` | **no** |
| Recover full statements as source | `decompile_panel.py` → `decompile_one.py` | yes |

Only the last row needs a legacy interpreter. Everything else runs on any modern Python,
because `pyc_inspect.py` implements CPython's marshal format directly rather than calling
`marshal.loads` under a matching interpreter.

## Typical run

```bash
# 1. unpack
python3 extract_pyz.py refs/extracted/quadro/PYZ-00.pyz out/quadro

# 2. what does a module actually contain?
python3 pyc_inspect.py assigns out/quadro/antelope_ui_afx_platform_afx_pool.pyc --py 3.8
python3 pyc_inspect.py get     out/quadro/antelope_ui_afx_platform_afx_pool.pyc PHY_AFX_STRIP_SIZE --py 3.8

# 3. extract the command model (blob dir is optional; it resolves constants whose
#    module was never decompiled)
python3 extract_field_layouts.py \
    refs/decompiled/quadro/app_report_format.py \
    refs/schemas/quadro_commands.json \
    refs/extracted/quadro/PYZ_extracted --device zenquadrosc_usb2

python3 extract_field_layouts.py \
    refs/decompiled/studio/zenstudiotb_report_format.py \
    refs/schemas/studio_commands.json \
    refs/extracted/studio/PYZ_extracted --device zenstudiotb

# 4. compare two devices
python3 extract_commands.py \
    refs/decompiled/quadro/app_report_format.py \
    refs/decompiled/studio/zenstudiotb_report_format.py \
    --label-a quadro --label-b studio

# 5. did the decompiler actually keep everything?
python3 verify_decompiled.py --batch \
    refs/extracted/manager/PYZ_extracted refs/decompiled/manager --py 3.8
```

## Verify before you trust

`verify_decompiled.py` exists because decompilers fail quietly: uncompyle6 writes a
`Parse error at or near ...` line and carries on, so a file can look complete while whole
function bodies are gone. It cross-checks every identifier and module-level constant in the
bytecode against the decompiled text and exits non-zero when something is missing, so it
can gate CI.

Running it across this project's own output found that 25 of 32 manager modules are
incomplete, that `HWDevice.request` — the function defining request/response correlation —
was never recovered at all, and that `dev_reports.py` lost a module docstring containing the
author's own specification of the field format.

## Setting up the decompiler (only for `decompile_panel.py`)

```bash
conda create -n py38 python=3.8 && conda run -n py38 pip install uncompyle6
export ANTELOPE_PY38=$(conda run -n py38 which python)

# 3.5 bytecode needs the legacy uncompyle6 2.11 on a real 3.5 interpreter;
# uncompyle6 3.x will not run there.
conda create -n py35 python=3.5 && conda run -n py35 pip install 'uncompyle6==2.11.0'
export ANTELOPE_PY35=$(conda run -n py35 which python)
```

Interpreters are discovered from `ANTELOPE_PY3x`, then `pythonX.Y` on `PATH`, then common
conda/pyenv locations. Nothing is hardcoded to one machine.

## Scope and limits

- `pyc_inspect.py` reads marshal version 4 (CPython 3.5–3.8), which is what PyInstaller
  emits for these bundles. It decodes structure and constants; it does not decompile.
- `module_assignments` recovers `NAME = <literal>` only. Computed values are skipped rather
  than guessed at.
- `verify_decompiled.py` proves identifiers and constants **survived**. It cannot prove the
  recovered *logic* is correct — only real hardware or captured traffic can do that.

## Legal

These tools operate on software you must already possess. They are for interoperability
with hardware you own. Not affiliated with or endorsed by Antelope Audio.
