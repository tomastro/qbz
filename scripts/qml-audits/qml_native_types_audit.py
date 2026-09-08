#!/usr/bin/env python3
"""Keep qmllint's tooling metadata aligned with hand-registered C++ types."""

from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parents[2]
CXX = ROOT / "crates" / "qbz-qt" / "cxx"
QMLTYPES = Path(__file__).with_name("qbz-native-types") / "plugin.qmltypes"


def registered_types() -> set[str]:
    out: set[str] = set()
    for source in CXX.glob("*.cpp"):
        out.update(re.findall(r"qmlRegisterType<\s*(\w+)\s*>", source.read_text()))
    return out


def header_properties(type_name: str) -> set[str]:
    for header in CXX.glob("*.h"):
        text = header.read_text()
        if re.search(rf"\bclass\s+{re.escape(type_name)}\b", text):
            return set(re.findall(r"Q_PROPERTY\(\s*\S+\s+(\w+)\s+", text))
    raise RuntimeError(f"no header declares registered type {type_name}")


def tooling_components() -> dict[str, set[str]]:
    text = QMLTYPES.read_text()
    chunks = re.findall(
        r'^    Component \{\n(.*?)(?=^    Component \{|^\})',
        text,
        flags=re.MULTILINE | re.DOTALL,
    )
    out: dict[str, set[str]] = {}
    for chunk in chunks:
        name = re.search(r'^        name: "(\w+)"$', chunk, flags=re.MULTILINE)
        if not name:
            continue
        out[name.group(1)] = set(re.findall(r'Property \{ name: "(\w+)"', chunk))
    return out


def main() -> int:
    registered = registered_types()
    described = tooling_components()
    failures: list[str] = []

    missing_types = registered - described.keys()
    stale_types = described.keys() - registered
    if missing_types:
        failures.append("missing tooling types: " + ", ".join(sorted(missing_types)))
    if stale_types:
        failures.append("stale tooling types: " + ", ".join(sorted(stale_types)))

    for type_name in sorted(registered & described.keys()):
        missing = header_properties(type_name) - described[type_name]
        if missing:
            failures.append(
                f"{type_name} is missing Q_PROPERTY metadata: "
                + ", ".join(sorted(missing))
            )

    if failures:
        print("qml_native_types_audit: FAIL", file=sys.stderr)
        for failure in failures:
            print(f"  {failure}", file=sys.stderr)
        return 1

    property_count = sum(len(header_properties(name)) for name in registered)
    print(
        f"qml_native_types_audit: {len(registered)} registered types, "
        f"{property_count} declared properties represented"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
