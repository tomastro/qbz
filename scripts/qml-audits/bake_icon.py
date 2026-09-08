#!/usr/bin/env python3
"""Bake one SVG into every general-purpose tint QbzIcon can ask for.

QbzIcon does not tint at runtime — it picks a PRE-BAKED copy out of
`qml/assets/icons/<tint>/<name>.svg`, and a name with no bake renders NOTHING
(no error, no warning; see qml_icon_bake_audit.py). So a new icon is not a
file, it is six.

The six are the general tints. `amber`, `green`, `orange` and `favorite` are
STATUS tints that hold a handful of glyphs by design and are left alone.

The source should be a Lucide-style 24x24 outline: viewBox "0 0 24 24",
fill="none", stroke-width 2, round caps and joins. Whatever colour it declares
is replaced per tint.

    bake_icon.py <source.svg> <name> <qbz-qt crate dir>
"""
import re
import sys
from pathlib import Path

# Read off the existing bakes of `disc.svg`, so a new icon matches its siblings.
TINTS = {
    "primary": "#ffffff",
    "secondary": "#cccccc",
    "muted": "#888888",
    "accent": "#4285f4",
    "black": "#000000",
    "warning": "#fbbf24",
}


def main() -> int:
    if len(sys.argv) != 4:
        print(__doc__, file=sys.stderr)
        return 2
    src, name, crate = Path(sys.argv[1]), sys.argv[2], Path(sys.argv[3])
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]*", name):
        print(f"name must be lowercase-with-dashes, got {name!r}", file=sys.stderr)
        return 2
    svg = src.read_text(encoding="utf-8")

    icons = crate / "qml" / "assets" / "icons"
    if not icons.is_dir():
        print(f"no icon directory at {icons}", file=sys.stderr)
        return 2

    for tint, colour in TINTS.items():
        d = icons / tint
        if not d.is_dir():
            print(f"  skip {tint}: not present")
            continue
        # Replace the stroke colour wherever it is declared, including
        # `currentColor`, which renders BLACK on a dark theme if left alone.
        out = re.sub(r'stroke="[^"]*"', f'stroke="{colour}"', svg)
        # A fill other than "none" would paint the glyph a colour the tint
        # cannot override — these icons are outlines.
        out = re.sub(r'fill="(?!none)[^"]*"', 'fill="none"', out)
        (d / f"{name}.svg").write_text(out, encoding="utf-8")
        print(f"  {tint:<10} {d / (name + '.svg')}")

    # The RUNTIME tint table too. `icon_tint_qt.rs` re-tints from a master at
    # theme-change time, and `masters_cover_every_shipped_glyph` FAILS the
    # build for a glyph that is baked but has no master — which is exactly how
    # this script's first version broke the tests: six files written, one line
    # forgotten. Doing it here is the only way it cannot be forgotten again.
    masters = crate / "src" / "icon_tint_qt.rs"
    line = f'    ("{name}", include_str!("../qml/assets/icons/primary/{name}.svg")),\n'
    if masters.is_file():
        src_txt = masters.read_text(encoding="utf-8")
        if f'("{name}",' in src_txt:
            print(f"\nmaster for {name} already registered")
        else:
            # Insert in alphabetical order among the existing entries.
            lines = src_txt.splitlines(keepends=True)
            at = None
            for i, l in enumerate(lines):
                m = re.match(r'\s*\("([a-z0-9-]+)", include_str!', l)
                if m and m.group(1) > name:
                    at = i
                    break
            if at is None:
                print(f"\nCOULD NOT place the master line — add it by hand:\n{line}", file=sys.stderr)
                return 1
            lines.insert(at, line)
            masters.write_text("".join(lines), encoding="utf-8")
            print(f"\nmaster registered in {masters.name}")
    else:
        print(f"\nNO icon_tint_qt.rs found — add by hand:\n{line}", file=sys.stderr)

    print(f"baked {name} into {len(TINTS)} tints — now run:")
    print(f"  python3 qml_icon_bake_audit.py {crate}")
    print(f"  cargo test -p qbz-qt masters_cover_every_shipped_glyph")
    return 0


if __name__ == "__main__":
    sys.exit(main())
