"""The generators' half of the product's no-dash rule.

The app holds no en dash (U+2013) and no em dash (U+2014) anywhere in its source, and
`xtask/tests/no_dashes.rs` fails on one. The files these scripts write sit inside that check, so a
dash here would be caught there too; this catches it where it is made, before the file is written,
and names the line. A dash that comes from the vendor's own data (an effect or microphone name)
stops the script as well: rewrite it in the script's own tables rather than let it through.

Escapes count as dashes too, since `json.dumps` writes non-ASCII as `\\u2014` and that still
renders as one.
"""

from __future__ import annotations

import sys

NEEDLES = ("–", "—", "\\u2013", "\\u2014", "\\u{2013}", "\\u{2014}")


def dash_lines(text: str) -> list[tuple[int, str]]:
    """`(line, text)` for every line holding an en or em dash, or an escape of one, from 1."""
    return [(i, line.strip()) for i, line in enumerate(text.split("\n"), 1) if any(n in line.lower() for n in NEEDLES)]


def refuse_dashes(text: str, what: str) -> str:
    """Return `text` unchanged, or stop the script naming each line of `what` that holds a dash."""
    found = dash_lines(text)
    if found:
        lines = "\n".join(f"  {what}:{n}: {line}" for n, line in found)
        sys.exit(f"refusing to write {what}: the product holds no en or em dashes, and this would add {len(found)}:\n{lines}")
    return text
