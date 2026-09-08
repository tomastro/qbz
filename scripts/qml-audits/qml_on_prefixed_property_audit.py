#!/usr/bin/env python3
"""A QML property must not be named `on<Upper>…`.

`on` + a capital letter is the signal-HANDLER form (`onClicked`,
`onAlbumsChanged`). The engine resolves a binding by that name as a handler
FIRST, and only falls back to "it is a property" when no target exists. So

    property var albums: []
    readonly property bool onAlbums: root.tab !== "tracks"

parses, compiles, and every read of `root.onAlbums` compiles — but because a
member called `albums` exists on the same object, `onAlbums: …` is filed as a
handler, NOTHING is bound, and the property sits at its type default (`false`,
`""`, `0`) for the life of the component. No error, no warning, no qmllint
finding, and nothing downstream can tell: a ternary reading the property
short-circuits on the default and captures no other dependency, so it never
re-evaluates either. (A literal initializer, `onAlbums: true`, at least fails
the load with "Cannot assign a value to a signal"; a script binding is
swallowed silently.)

Measured 2026-09-01 with the `qml` runtime, Qt 6.11: WITHOUT a sibling member
`albums` the same declaration binds normally. That is exactly why the name is
banned wholesale and not just when it collides today — `onFoo` is correct
right up to the day someone adds a property or signal `foo` next to it, and
then it breaks with no diagnostic. That single name blanked Purchases > Albums
for every account with real purchases: `albums: (root.onAlbums &&
!root.grouped) ? root.albums : []` evaluated once to `[]` and never again.
Four more `on<Upper>` properties were found the same day (kiosk, theme,
QbzCircleAction, SongCardStamp); they had no sibling yet and still worked,
and were renamed so the rule can be exact.

The rule IS exact: a `property` declaration whose name matches `on[A-Z]` is
reported, every time. Signal handlers themselves (`onClicked: …`, `function
onFooChanged()` inside a Connections) are not property declarations and are
never touched.
"""
import pathlib
import re
import sys

DEFAULT_ROOT = pathlib.Path(__file__).resolve().parents[2] / "crates/qbz-qt"
ROOT = pathlib.Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else DEFAULT_ROOT
QML = ROOT / "qml"

# `property <type> onFoo`, with any of the modifiers QML allows in front and
# any type (including `list<…>` / dotted names). The declaration keyword is
# what pins this to a declaration; a handler binding has no `property`.
DECL = re.compile(
    r"^\s*(?:(?:readonly|required|default)\s+)*property\s+"
    r"[A-Za-z_][\w.<>]*\s+(on[A-Z]\w*)\b",
    re.MULTILINE,
)


def main() -> int:
    files = sorted(QML.rglob("*.qml"))
    if not files:
        print(f"no .qml files under {QML}")
        return 1
    findings = []
    for path in files:
        src = path.read_text(errors="ignore")
        for m in DECL.finditer(src):
            line = src[:m.start()].count("\n") + 1
            findings.append((path.relative_to(ROOT), line, m.group(1)))

    print(f"scanned {len(files)} qml files for `on<Upper>` property names")
    if not findings:
        print("OK — no property is named like a signal handler")
        return 0
    for rel, line, name in findings:
        print(f"  {rel}:{line}: property `{name}` is parsed as a signal handler — "
              f"its initializer never binds; rename it")
    noun = "property" if len(findings) == 1 else "properties"
    print(f"\n{len(findings)} `on<Upper>` {noun} — rename (e.g. `albumsTab`, "
          f"`collectionsTab`); the value silently stays at the type default otherwise")
    return 1


if __name__ == "__main__":
    sys.exit(main())
