#!/usr/bin/env python3
"""Every `QbzFoo.member` a .qml touches must exist on the Rust bridge.

QML resolves singleton members lazily too: calling a method the bridge does not
have compiles clean and throws at runtime. This audit caught 30 missing members
in one round (the LocalLibrary QML and its Rust half were written in parallel).

Per bridge it collects `#[qproperty(T, name)]` (-> `name` + `setName`),
`#[qinvokable] fn name` and `#[qsignal] fn name` (-> handler `onName`),
snake_case converted to camelCase, then diffs the QML usage against it.

It ALSO checks that each bridge file is listed in `build.rs`'s cxx-qt
`rust_files`. That list is what generates and compiles the C++ half; a bridge
missing from it still builds and still links — the Rust side is intact and only
the QML type registration is gone — so the failure is a runtime
`ReferenceError: QbzFoo is not defined` the first time a user opens the view.
Which is exactly the class of defect this audit exists for, and it missed one
(`disc_meta_bridge.rs`, 2026-08-21) because it only ever read the members.
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else
                    "/home/blitzkriegfc/Personal/qbz/qbz-worktrees/qbz-qt/crates/qbz-qt")

def camel(s: str) -> str:
    head, *rest = s.split("_")
    return head + "".join(w[:1].upper() + w[1:] for w in rest)

def decomment(src: str) -> str:
    src = re.sub(r"/\*.*?\*/", "", src, flags=re.S)
    return re.sub(r"(?<!:)//[^\n]*", "", src)

# ---- what each singleton actually offers -------------------------------
bridges = {}
for rs in sorted((ROOT / "src").glob("*.rs")):
    src = decomment(rs.read_text(errors="ignore"))
    for m in re.finditer(r"((?:#\[[^\]]*\]\s*)+)\s*type\s+(Qbz\w+)\s*=", src):
        attrs, name = m.group(1), m.group(2)
        if "qml_element" not in attrs:
            continue
        members = {camel(p) for p in re.findall(r"#\[qproperty\(\s*[^,]+,\s*(\w+)", attrs)}
        members |= {"set" + camel(p)[:1].upper() + camel(p)[1:]
                    for p in re.findall(r"#\[qproperty\(\s*[^,]+,\s*(\w+)", attrs)}
        # cxx-qt auto-generates a `<prop>Changed` signal per qproperty; it is a
        # real signal, so it belongs in BOTH sets (a Connections handler on it
        # is legitimate — 14 false positives came from omitting it here).
        prop_changed = {camel(p) + "Changed"
                        for p in re.findall(r"#\[qproperty\(\s*[^,]+,\s*(\w+)", attrs)}
        members |= prop_changed
        # invokables + signals live in the extern blocks of the same file
        members |= {camel(n) for n in re.findall(r"#\[qinvokable\][\s\S]{0,120}?fn\s+(\w+)", src)}
        sigs = {camel(n) for n in re.findall(r"#\[qsignal\][\s\S]{0,120}?fn\s+(\w+)", src)}
        members |= sigs
        sigs |= prop_changed
        bridges[name] = {"members": members, "signals": sigs, "file": rs.name}

# ---- is each bridge actually REGISTERED with cxx-qt? --------------------
# `CxxQtBuilder`'s `rust_files` is the list that generates the C++ half. A
# bridge left out of it compiles, links, and simply never becomes a QML type.
build_rs = (ROOT / "build.rs").read_text(errors="ignore")
unregistered = sorted(
    f"{name} ({info['file']})"
    for name, info in bridges.items()
    if f'"src/{info["file"]}"' not in build_rs
)

# ---- what the QML asks for ---------------------------------------------
problems = []
for f in sorted((ROOT / "qml").rglob("*.qml")):
    src = decomment(f.read_text(errors="ignore"))
    for lineno, line in enumerate(src.splitlines(), 1):
        for sing, member in re.findall(r"\b(Qbz[A-Z]\w*)\.(\w+)\b", line):
            if sing not in bridges:
                problems.append(f"{f.relative_to(ROOT)}:{lineno}  unknown singleton {sing}")
                continue
            if member not in bridges[sing]["members"]:
                problems.append(f"{f.relative_to(ROOT)}:{lineno}  {sing}.{member} "
                                f"not on {bridges[sing]['file']}")
    # Connections { target: QbzX ... function onFoo }
    for blk in re.finditer(r"Connections\s*\{([\s\S]*?)\n\s*\}", src):
        body = blk.group(1)
        tm = re.search(r"target:\s*(?:.*?)\b(Qbz[A-Z]\w*)", body)
        if not tm or tm.group(1) not in bridges:
            continue
        sing = tm.group(1)
        for h in re.findall(r"function\s+(on[A-Z]\w*)", body):
            sig = h[2].lower() + h[3:]
            if sig not in bridges[sing]["signals"]:
                problems.append(f"{f.relative_to(ROOT)}  {sing} has no signal for {h}")

if unregistered:
    for u in unregistered:
        print(f"  {u} is NOT in build.rs rust_files — QML will not see this type")
    print(f"FAIL — {len(unregistered)} bridge(s) never registered with cxx-qt")
    sys.exit(1)

print(f"{len(bridges)} bridges: " + ", ".join(sorted(bridges)))
if problems:
    print(f"MISSING ({len(problems)}):")
    for p in problems:
        print("  " + p)
    sys.exit(1)
print("OK — every QML singleton member exists on its bridge")
