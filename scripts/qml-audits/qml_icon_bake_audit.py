#!/usr/bin/env python3
"""Every icon name a .qml asks for must have a pre-baked SVG on disk.

WHY THIS EXISTS
---------------
`QbzIcon` does not draw an SVG by path — it picks a PRE-BAKED, per-tint copy
out of `qml/assets/icons/<tint>/<name>.svg`. A name with no bake is not an
error anywhere:

  - nothing to compile, so the build is clean;
  - `cargo check` cannot see QML at all;
  - `qml_resolution_audit.py` resolves TYPES, not asset names;
  - `qml_module_registration_audit.py` checks files, not icon names;
  - no test asserts on a glyph.

The icon simply RENDERS NOTHING. The row keeps its 15px slot, the label sits
where it always did, and the only way to find out is to look at the window.

That is exactly what happened on 2026-08-20 building the Local Library
`Refresh` menu: `"icon": "server"` looked obvious, `server.svg` has no bake,
and the Plex / Jellyfin / Navidrome rows shipped iconless through a clean
build, 434 green tests and four green audits. It was caught by driving the app
under Xvfb and LOOKING at the menu.

WHAT IT CHECKS
--------------
Literal `name:` / `"icon":` assignments in .qml against the UNION of names
present in the tint directories.

Two deliberate limits, both stated so nobody mistakes this for more than it is:

  - Only LITERALS. An icon chosen by expression (`name: cond ? "a" : "b"`, or
    from a model) is out of reach for a static pass. Floor, not ceiling — the
    literal case is the one that keeps happening.

  - Only "missing from EVERY tint". A first draft also flagged names absent
    from SOME tints and produced ~90 findings, all noise: `amber`, `green`,
    `orange` and `favorite` are special-purpose STATUS tints that are supposed
    to hold a handful of glyphs, not the whole set. Deciding which tint a given
    call site actually asks for needs `tintName`, which is usually an
    expression. An audit that cries wolf gets ignored, and then it protects
    nothing.

USAGE
-----
    qml_icon_bake_audit.py <qbz-qt crate dir>
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

# `name: "x"` (QbzIcon / QbzNavButton / QbzIconButton) and `"icon": "x"`
# (the CardMenu entry model). Both are the literal forms in use.
NAME_RE = re.compile(r'(?:^|\s)name:\s*"([a-z0-9][a-z0-9-]*)"')
ICON_RE = re.compile(r'"icon"\s*:\s*"([a-z0-9][a-z0-9-]*)"')

# Properties that are called `name:` but are NOT icons. Kept explicit rather
# than guessed: a wrong exclusion here silently disables the audit for a whole
# component, which is the very failure mode this file exists to prevent.
SKIP_FILES = {"QbzIconTint.qml"}


def main() -> int:
    if len(sys.argv) != 2:
        print(f"usage: {Path(sys.argv[0]).name} <qbz-qt crate dir>", file=sys.stderr)
        return 2
    crate = Path(sys.argv[1]).resolve()
    qml_dir = crate / "qml"
    icons_dir = qml_dir / "assets" / "icons"
    if not qml_dir.is_dir() or not icons_dir.is_dir():
        print(f"FAIL — {qml_dir} or {icons_dir} not found", file=sys.stderr)
        return 2

    tints = sorted(p for p in icons_dir.iterdir() if p.is_dir())
    if not tints:
        print(f"FAIL — no tint directories under {icons_dir}", file=sys.stderr)
        return 2

    # Per-tint sets, so the report can say WHICH tint is missing a bake: a name
    # present in some tints and absent in others renders in one theme state and
    # vanishes in another, which is even harder to spot than a total blank.
    baked: dict[str, set[str]] = {t.name: {p.stem for p in t.glob("*.svg")} for t in tints}
    anywhere = set.union(*baked.values())

    qml_files = sorted(p for p in qml_dir.rglob("*.qml") if p.name not in SKIP_FILES)
    used: dict[str, list[str]] = {}
    for f in qml_files:
        text = f.read_text(encoding="utf-8", errors="replace")
        for rx in (NAME_RE, ICON_RE):
            for m in rx.finditer(text):
                used.setdefault(m.group(1), []).append(str(f.relative_to(crate)))

    missing = {n: srcs for n, srcs in used.items() if n not in anywhere}

    print(
        f"scanned {len(qml_files)} qml files for {len(used)} distinct icon names "
        f"against {len(tints)} tints"
    )
    if not missing:
        print("OK — every literal icon name has a baked svg")
        return 0

    for n, srcs in sorted(missing.items()):
        where = ", ".join(sorted(set(srcs))[:3])
        print(
            f'FAIL — icon "{n}" has NO baked svg in any tint (used in {where}): '
            f"it renders as nothing at runtime, silently",
            file=sys.stderr,
        )
    return 1


if __name__ == "__main__":
    sys.exit(main())
