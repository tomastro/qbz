#!/usr/bin/env python3
"""A tab body must not be BUILT while its tab is not the active one.

`visible: false` hides an item; it does not stop QML from instantiating it.
An item declared as a sibling per tab and gated only on `visible:` is therefore
built in full every time the view mounts — every Repeater under it runs, every
delegate is constructed — so a four-tab view pays four times its mount cost to
show one tab. Nothing warns about it: the view looks correct, the audits pass,
and the only symptom is that entering the section takes seconds.

Measured on Discover (2026-08-17): a Home mount spent ~360 ms of ~410 ms inside
the four tab Columns, and 19 of its 28 section rails belonged to tabs nobody
was looking at. The reference does it the other way round — HomeView.slint:321
runs ONE repeater whose model is picked by a ternary on the active tab, and
mounts the remaining tabs behind `if`.

The fix is always the same shape:

    Loader {
        active: root.activeTab === "albums"
        visible: active          // else the parent keeps a spacing slot
        sourceComponent: <the body that used to be here>
    }

NOT `asynchronous: true`: incubation is time-sliced, so it spreads the same
work over more frames instead of removing it (tried 2026-08-13, reverted the
same day — see ContentRouter.qml).

Only HEAVY bodies are reported, on two counts, because an audit that cries
wolf gets switched off:

  * the body must instantiate a Repeater / ListView / GridView / TableView —
    that is what turns one hidden item into hundreds; a toolbar button that
    shows for one tab costs nothing to build, and there are dozens of those;
  * that view's `model:` must be UNBOUNDED USER DATA — a queue, a library
    feed, a playlist, a history, a result set — not an inline `[...]` literal,
    not a count, and not a fixed option list. The sort popups in
    LibraryToolbar.qml are Repeaters over four fixed entries: hidden or not,
    they are four items, and reporting them would bury the finding that
    matters.

The narrowness is deliberate. The wide rule reports 145 sites and a build gate
that noisy gets switched off within a day. This one should fire rarely, and
every time it fires it should be a freeze waiting for a big enough account —
which is exactly what shipped: a 1935-row play queue built ~4000 cards for a
panel nobody had opened, and the app came up frozen for 28 seconds.
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else
                    "/home/blitzkriegfc/Personal/qbz/qbz-worktrees/qbz-qt/crates/qbz-qt")
QML = ROOT / "qml"

HEAVY = ("Repeater", "ListView", "GridView", "TableView")
# The gates that decide WHICH body is wanted. Kept as a list rather than a
# regex so a new one is an obvious one-line change.
#
# It is not only tabs: the coverflow that froze the app for 28 s at launch was
# gated on `QbzImmersive.mode`, and an earlier version of this audit missed it
# for exactly that reason.
TAB_PROPS = (
    "activeTab", "activeSubTab", "currentTab",
    # panel / overlay selectors — same shape, same failure
    "QbzImmersive.mode", "QbzImmersive.viewMode", "QbzImmersive.splitPanel",
    "QbzShell.queueOpen", "QbzShell.lyricsOpen",
)

# The models whose length is decided by USER DATA and therefore has no upper
# bound: the play queue, the library feed, a playlist, a history, a result set.
# THIS is the dangerous subset and the only one this audit fails the build on.
#
# A wide rule — "any heavy body gated only by `visible:`" — reports 145 sites,
# nearly all of them modals over short lists. A build gate that noisy gets
# switched off within a day, and then it protects nothing. So the rule is
# narrow ON PURPOSE: it should fire rarely, and every time it fires it should
# be a real freeze waiting for a big enough account.
#
# Matched against the model EXPRESSION, so `root.cfDoc.tracks` /
# `root.upcoming` / `root.visibleRows` hit and `[ ... ]` literals do not.
UNBOUNDED_MODELS = (
    "tracks", "upcoming", "history", "historyRows", "queue",
    "visibleRows", "feed", "albums", "playlists", "artists",
    "rows", "items", "results", "cards",
)


def blank_noise(src: str) -> str:
    """Comments and string bodies out, LENGTH AND LINE COUNT PRESERVED.

    Offsets have to keep pointing at the same characters as the original, so
    everything is replaced with same-length filler instead of removed — the
    braces this audit counts live in real code, and a `{` inside a comment or a
    string would otherwise shift every block boundary after it.
    """
    out = list(src)
    i, n = 0, len(src)
    while i < n:
        two = src[i:i + 2]
        if two == "/*":
            j = src.find("*/", i + 2)
            j = n if j < 0 else j + 2
            for k in range(i, j):
                if out[k] != "\n":
                    out[k] = " "
            i = j
        elif two == "//":
            j = src.find("\n", i)
            j = n if j < 0 else j
            for k in range(i, j):
                out[k] = " "
            i = j
        elif src[i] in "\"'":
            q = src[i]
            j = i + 1
            while j < n and src[j] != q:
                if src[j] == "\\":
                    j += 1
                j += 1
            for k in range(i + 1, min(j, n)):
                if out[k] != "\n":
                    out[k] = " "
            i = min(j + 1, n)
        else:
            i += 1
    return "".join(out)


def visible_expr(code: str, start: int) -> str:
    """The WHOLE `visible:` expression, continuation lines included.

    Reading only the first line is not a detail: the mount that froze the app
    was written as

        visible: QbzShaderScene.scene === 0
            && QbzImmersive.viewMode === 0 && QbzImmersive.mode === 2

    so a first-line-only read saw `QbzShaderScene.scene === 0`, matched no
    known gate, and skipped it — verified against the pre-fix tree.
    """
    lines = code[start:].split("\n")
    out = [lines[0]]
    for nxt in lines[1:]:
        stripped = nxt.strip()
        prev = out[-1].rstrip()
        continues = prev.endswith(("&&", "||", "?", ":", "(", ",", "+", "==="))
        starts_op = stripped.startswith(("&&", "||", "?", ":", ".", ")"))
        if not (continues or starts_op):
            break
        out.append(nxt)
    return " ".join(out)


def enclosing_block(code: str, pos: int):
    """(type_name, body_start, body_end) of the object block containing `pos`."""
    depth = 0
    i = pos
    while i > 0:
        i -= 1
        if code[i] == "}":
            depth += 1
        elif code[i] == "{":
            if depth == 0:
                break
            depth -= 1
    else:
        return None
    open_brace = i
    m = re.search(r"([A-Za-z_][\w.]*)\s*$", code[:open_brace])
    name = m.group(1) if m else "?"
    depth = 0
    j = open_brace
    while j < len(code):
        if code[j] == "{":
            depth += 1
        elif code[j] == "}":
            depth -= 1
            if depth == 0:
                break
        j += 1
    return name, open_brace, j


def data_driven_views(code: str, start: int, end: int) -> set:
    """The heavy views inside [start, end) whose model is data, not a literal.

    A `Repeater { model: [ ... ] }` builds exactly as many items as the author
    typed, so hiding it wastes a fixed handful. A `Repeater { model:
    root.doc.tracks }` builds one per row of whatever the backend sent, which
    is the case this audit exists for.
    """
    found = set()
    for m in re.finditer(r"\b(%s)\s*\{" % "|".join(HEAVY), code[start:end]):
        view = m.group(1)
        brace = start + m.end() - 1
        depth, j = 0, brace
        while j < end:
            if code[j] == "{":
                depth += 1
            elif code[j] == "}":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        expr_m = re.search(r"\bmodel\s*:\s*([^\n]*)", code[brace:j])
        if not expr_m:
            # No model at all is the ListView-with-delegates-in-QML case:
            # treat it as data rather than silently passing it.
            found.add(view)
            continue
        expr = expr_m.group(1).strip()
        if expr[:1] in "[0123456789":
            continue
        # Narrow to the unbounded-user-data subset (see UNBOUNDED_MODELS).
        low = expr.lower()
        if not any(k.lower() in low for k in UNBOUNDED_MODELS):
            continue
        found.add(view)
    return found


def component_index(files) -> dict:
    """component name -> does mounting it (however deep) build an unbounded view?

    THE CASE THIS EXISTS FOR. The gate that froze the app was written as

        CoverflowPanel { visible: ... }      // in ImmersiveView.qml

    while the two Repeaters over the whole play queue live in
    CoverflowPanel.qml. A same-file rule cannot see across that boundary and
    reported the tree clean — verified, on the pre-fix tree, before this
    function existed. So the index is transitive: a component counts as heavy
    if its own file has an unbounded view OR if it mounts something that does.
    """
    own, refs = {}, {}
    for path in files:
        raw = path.read_text(errors="ignore")
        code = blank_noise(raw)
        own[path.stem] = bool(data_driven_views(code, 0, len(code)))
        refs[path.stem] = set(re.findall(r"\b([A-Z]\w+)\s*\{", code))
    heavy = {n for n, v in own.items() if v}
    # Fixed point. The tree is small and shallow; iterate until nothing new.
    changed = True
    while changed:
        changed = False
        for name, used in refs.items():
            if name in heavy:
                continue
            if used & heavy:
                heavy.add(name)
                changed = True
    return heavy


def main() -> int:
    findings = []
    files = sorted(QML.rglob("*.qml"))
    heavy_components = component_index(files)
    for path in files:
        raw = path.read_text(errors="ignore")
        code = blank_noise(raw)
        for m in re.finditer(r"\bvisible\s*:", code):
            expr = visible_expr(code, m.end())
            if not any(p in expr for p in TAB_PROPS):
                continue
            block = enclosing_block(code, m.start())
            if block is None:
                continue
            name, start, end = block
            # Already gated: a Loader's own `visible` is the companion of
            # `active`, which is the fix, not the defect.
            if name == "Loader":
                continue
            hits = sorted(data_driven_views(code, start, end))
            # Cross-file: the gated thing may BE a component whose own file
            # (or something it mounts) builds the unbounded view.
            if not hits and name in heavy_components:
                hits = ["%s (its own tree)" % name]
            if not hits:
                continue
            findings.append((path.relative_to(ROOT), raw[:m.start()].count("\n") + 1,
                             name, ", ".join(hits)))

    print(f"scanned {len(files)} qml files for eagerly-built tab bodies")
    if not findings:
        print("OK — every heavy per-tab body is gated on instantiation, not just visibility")
        return 0
    for rel, line, name, hits in findings:
        print(f"  {rel}:{line}: `{name}` is gated only by `visible:` but builds {hits}")
    noun = "body" if len(findings) == 1 else "bodies"
    print(f"\n{len(findings)} eagerly-built tab {noun} — gate with "
          f"`Loader {{ active: ...; visible: active; sourceComponent: ... }}`")
    return 1


if __name__ == "__main__":
    sys.exit(main())
