#!/usr/bin/env python3
"""Every component a .qml instantiates must resolve from THAT file's imports.

QML resolves types lazily: a file that instantiates a component nobody can see
compiles clean, passes `cargo check`, and throws at runtime the first time the
view is shown. This is the audit that catches it before a build.

Visible to a file = the .qml siblings in its own directory + the .qml files in
every relative directory it imports + the Qt built-ins + the cxx-qt singletons.
"""
import pathlib
import re
import sys

def qt_qml_dir() -> pathlib.Path:
    """Where Qt's own plugins.qmltypes live. QBZ_QT_QML_DIR wins (CI sets it to
    the aqt install: $QT_ROOT_DIR/qml); then the distro layouts. Failing loudly
    beats the alternative: with no qmltypes every Rectangle reads as unresolved
    and the audit drowns the real findings in noise."""
    import os
    candidates = []
    if os.environ.get("QBZ_QT_QML_DIR"):
        # Explicit means explicit: a wrong value must not fall through to
        # whatever Qt the box happens to have.
        c = pathlib.Path(os.environ["QBZ_QT_QML_DIR"])
        if c.is_dir() and any(c.rglob("plugins.qmltypes")):
            return c
        sys.exit(f"qml_resolution_audit: QBZ_QT_QML_DIR={c} has no plugins.qmltypes")
    if os.environ.get("QT_ROOT_DIR"):
        candidates.append(pathlib.Path(os.environ["QT_ROOT_DIR"]) / "qml")
    candidates += [pathlib.Path(x) for x in (
        "/usr/lib64/qt6/qml", "/usr/lib/x86_64-linux-gnu/qt6/qml",
        "/usr/lib/aarch64-linux-gnu/qt6/qml", "/usr/lib/qt6/qml",
        "/opt/homebrew/share/qt/qml", "/usr/local/share/qt/qml")]
    for c in candidates:
        if c.is_dir() and any(c.rglob("plugins.qmltypes")):
            return c
    sys.exit("qml_resolution_audit: no Qt qml dir with plugins.qmltypes found — "
             "set QBZ_QT_QML_DIR (tried: " + ", ".join(str(c) for c in candidates) + ")")


ROOT = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else
                    "/home/blitzkriegfc/Personal/qbz/qbz-worktrees/qbz-qt/crates/qbz-qt")
QML = ROOT / "qml"

def decomment(src: str) -> str:
    src = re.sub(r"/\*.*?\*/", "", src, flags=re.S)
    # Not `//` inside a string (import "http://..." never happens, but urls do).
    src = re.sub(r"(?<!:)//[^\n]*", "", src)
    return src

def strip(src: str) -> str:
    """Blank out string literals. Imports MUST be read before this runs —
    reading them after is why the first version of this audit reported 519
    false positives."""
    src = decomment(src)
    src = re.sub(r'"(?:[^"\\]|\\.)*"', '""', src)
    src = re.sub(r"'(?:[^'\\]|\\.)*'", "''", src)
    return src

def qt_builtins() -> set:
    names = set()
    for p in qt_qml_dir().rglob("*.qmltypes"):
        try:
            for m in re.finditer(r'"(?:[A-Za-z0-9_.]+)/([A-Z]\w+) [\d.]+"', p.read_text(errors="ignore")):
                names.add(m.group(1))
        except OSError:
            pass
    # Attached/grouped and value types that never appear as exports.
    names |= {"Gradient", "GradientStop", "Behavior", "Transition", "Component",
              "QtObject", "Connections", "Binding", "Timer", "Loader", "Item"}
    return names

BUILTINS = qt_builtins()
SINGLETONS = {"Qbz" + n for n in
              re.findall(r"qml_uri\w*|\w+", "")} or set()

def singletons() -> set:
    out = set()
    for p in (ROOT / "src").glob("*.rs"):
        t = p.read_text(errors="ignore")
        for m in re.finditer(r"#\[qml_element\][\s\S]{0,200}?type\s+(Qbz\w+)", t):
            out.add(m.group(1))
        out |= set(re.findall(r"\btype\s+(Qbz\w+)\s*=", t))
    return out

SING = singletons()

def cpp_registered_types() -> set:
    """QML types registered from hand-written C++ (qmlRegisterType<T>) —
    the QQuickRhiItem of block A4 (cxx/linebed_item.cpp) is the first; the
    audit's .rs scan only sees the cxx-qt singletons."""
    out = set()
    cxx = ROOT / "cxx"
    if cxx.is_dir():
        for p in cxx.rglob("*"):
            if p.suffix in (".cpp", ".cc", ".cxx", ".h", ".hpp"):
                out |= set(re.findall(r"qmlRegisterType<\s*(\w+)",
                                      p.read_text(errors="ignore")))
    return out

CPP_TYPES = cpp_registered_types()
problems = []
for f in sorted(QML.rglob("*.qml")):
    raw = decomment(f.read_text(errors="ignore"))
    src = strip(raw)
    visible = {p.stem for p in f.parent.glob("*.qml")}
    # Qt 6 inline components — `component Foo: Item { }` — are visible to the
    # file that declares them and to nothing else.
    visible |= set(re.findall(r"^\s*component\s+(\w+)\s*:", src, flags=re.M))
    for imp in re.findall(r'^\s*import\s+"([^"]+)"', raw, flags=re.M):
        d = (f.parent / imp).resolve()
        if d.is_dir():
            visible |= {p.stem for p in d.glob("*.qml")}
    used = set(re.findall(r"(?:^|[\s{:\[(])([A-Z]\w*)\s*\{", src, flags=re.M))
    for name in sorted(used):
        if name in visible or name in BUILTINS or name in SING or name in CPP_TYPES:
            continue
        line = next((i for i, l in enumerate(src.splitlines(), 1)
                     if re.search(rf"\b{name}\s*{{", l)), 0)
        problems.append(f"{f.relative_to(ROOT)}:{line}  {name}")

print(f"scanned {len(list(QML.rglob('*.qml')))} qml files, "
      f"{len(BUILTINS)} Qt types, {len(SING)} singletons")
if problems:
    print(f"UNRESOLVED ({len(problems)}):")
    for p in problems:
        print("  " + p)
    sys.exit(1)
print("OK — every instantiated component resolves")
