#!/usr/bin/env python3
"""Every .qml file on disk must be listed in build.rs's `qml_files`.

WHY THIS EXISTS
---------------
A .qml file that is not in that list is not in the QML module. It compiles
(there is nothing to compile), `cargo check` is silent (cargo cannot see QML),
and `qml_resolution_audit.py` is ALSO silent — that audit reads the files on
disk and resolves component names between them, so an unregistered file looks
perfectly resolvable to it.

The app then fails at RUNTIME with:

    qrc:/.../LocalLibrarySettings.qml:181:5: MediaServerSettings is not a type
    qrc:/.../SettingsView.qml:285:25: Type LocalLibrarySettings unavailable

...and because the failure cascades UP through every parent that instantiates
it, the visible symptom is a whole SETTINGS PAGE rendering blank. Nothing in
the build says a word.

That is exactly what happened on 2026-08-20 while adding the media-server
panel: three green audits, a clean release build, and an empty screen. This
audit is the missing check, and it is the cheap kind — a set difference.

The reverse direction is checked too: a file listed in build.rs but MISSING
from disk is a rename that only half landed, and it fails the qmlcachegen step
with a much less obvious message.

USAGE
-----
    python3 qml_module_registration_audit.py <path-to-qbz-qt-crate>
"""

import re
import sys
from pathlib import Path


def listed_in_build_rs(build_rs: Path) -> set[str]:
    """Every "qml/....qml" string literal in build.rs.

    Deliberately a dumb regex over the whole file rather than a parse of the
    `qml_files` array: the array is split across several `&[...]` blocks, and a
    literal that appears anywhere in build.rs is registered somewhere. A false
    NEGATIVE here (missing a real registration) would make this audit cry wolf,
    which is how an audit gets switched off.
    """
    text = build_rs.read_text(encoding="utf-8")
    return set(re.findall(r'"(qml/[^"]+\.qml)"', text))


def on_disk(crate: Path) -> set[str]:
    qml_dir = crate / "qml"
    return {
        str(p.relative_to(crate)).replace("\\", "/")
        for p in qml_dir.rglob("*.qml")
    }


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: qml_module_registration_audit.py <qbz-qt crate dir>", file=sys.stderr)
        return 2
    crate = Path(sys.argv[1]).resolve()
    build_rs = crate / "build.rs"
    if not build_rs.is_file():
        print(f"FAIL — no build.rs at {build_rs}", file=sys.stderr)
        return 2

    listed = listed_in_build_rs(build_rs)
    present = on_disk(crate)

    unregistered = sorted(present - listed)
    missing = sorted(listed - present)

    print(f"scanned {len(present)} qml files against {len(listed)} build.rs entries")

    if not unregistered and not missing:
        print("OK — every QML file is registered, and every registration exists")
        return 0

    for f in unregistered:
        print(
            f"FAIL — {f} is on disk but NOT in build.rs: it is not in the QML "
            f"module, so instantiating it fails at runtime with "
            f'"<Component> is not a type" and blanks the page that mounts it',
            file=sys.stderr,
        )
    for f in missing:
        print(
            f"FAIL — build.rs lists {f} but it does not exist: a half-finished "
            f"rename, which fails the qmlcachegen step",
            file=sys.stderr,
        )
    return 1


if __name__ == "__main__":
    sys.exit(main())
