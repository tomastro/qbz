#!/usr/bin/env bash
# scripts/packaging/desktop_entry_check.sh — the MPRIS DesktopEntry / .desktop
# / icon invariant, as a gate (qbz-nix-docs/cicd/2026-08-04-desktop-entry-icon-mismatch.md).
#
# GNOME Shell / Plasma resolve the media widget's application icon ONLY through
# the MPRIS `DesktopEntry` property: they look for `<DesktopEntry>.desktop` in
# the XDG application dirs and read its `Icon=`. If the installed basename
# differs from the constant, or `Icon=` names an icon the same package does not
# install, the widget shows no icon — and nothing else breaks, which is why
# this regressed more than once. Two properties, asserted at the SOURCE
# (recipes) and, when `--deb <file>` / `--rpm <file>` / `--appdir <dir>` are
# given, on the ARTIFACT that ships.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

ENTRY=$(grep -oP 'const DESKTOP_ENTRY: &str = "\K[^"]+' crates/qbz-media-controls/src/linux.rs)
[[ -n "$ENTRY" ]] || { echo "::error::DESKTOP_ENTRY constant not found"; exit 1; }
echo "DesktopEntry = $ENTRY"
fail=0
say_fail() { echo "::error::$*"; fail=1; }

icon_of() { grep -m1 '^Icon=' "$1" | cut -d= -f2-; }

# --- source recipes -----------------------------------------------------
# nfpm (deb/rpm): the installed basename must be the constant; Icon= must be
# one of the hicolor names nfpm installs.
NFPM=packaging/nfpm/nfpm.yaml
dst=$(grep -oP 'dst: /usr/share/applications/\K[^ ]+\.desktop' "$NFPM" | head -1)
[[ "$dst" == "$ENTRY.desktop" ]] || say_fail "nfpm installs $dst, DesktopEntry is $ENTRY"
src=$(grep -B1 "dst: /usr/share/applications/$dst" "$NFPM" | grep -oP 'src: \K\S+')
icon=$(icon_of "$src")
grep -q "dst: /usr/share/icons/hicolor/[0-9x]*/apps/${icon}\.png" "$NFPM" \
  || say_fail "nfpm: $src has Icon=$icon but installs no hicolor/*/apps/$icon.png"
# StartupWMClass must be the constant too (window <-> launcher association).
for f in $(git ls-files 'packaging/*.desktop' 'flatpak/*.desktop'); do
  wm=$(grep -m1 '^StartupWMClass=' "$f" | cut -d= -f2-)
  [[ "$wm" == "$ENTRY" ]] || say_fail "$f: StartupWMClass=$wm != $ENTRY"
done
# Recipes that ship a reverse-DNS basename must match the constant exactly.
for f in $(git ls-files 'packaging/*.desktop' 'flatpak/*.desktop' | grep -v '/qbz\.desktop$'); do
  base=$(basename "$f" .desktop)
  [[ "$base" == "$ENTRY" ]] || say_fail "$f: basename $base != $ENTRY"
done

# --- artifacts ------------------------------------------------------------
while [[ $# -gt 0 ]]; do
  case "$1" in
    --deb)
      list=$(dpkg-deb -c "$2" | awk '{print $6}')
      grep -qx "./usr/share/applications/$ENTRY.desktop" <<<"$list" \
        || say_fail "$2 does not install /usr/share/applications/$ENTRY.desktop"
      grep -q "^./usr/share/icons/hicolor/256x256/apps/${icon}\.png$" <<<"$list" \
        || say_fail "$2 does not install hicolor/256x256/apps/$icon.png"
      echo "deb OK: $2"; shift 2 ;;
    --rpm)
      list=$(rpm -qlp "$2" 2>/dev/null || true)
      grep -qx "/usr/share/applications/$ENTRY.desktop" <<<"$list" \
        || say_fail "$2 does not install /usr/share/applications/$ENTRY.desktop"
      grep -q "^/usr/share/icons/hicolor/256x256/apps/${icon}\.png$" <<<"$list" \
        || say_fail "$2 does not install hicolor/256x256/apps/$icon.png"
      echo "rpm OK: $2"; shift 2 ;;
    --appdir)
      d="$2"
      [[ -f "$d/$ENTRY.desktop" ]] || say_fail "$d has no $ENTRY.desktop at its root"
      aicon=$(icon_of "$d/$ENTRY.desktop")
      [[ -f "$d/$aicon.png" || -L "$d/$aicon.png" ]] || say_fail "$d: Icon=$aicon but no $aicon.png at the root"
      # No .DirIcon assertion: linuxdeploy does not write it (appimagetool
      # does, at packaging time) — asserting it here failed a green build.
      echo "AppDir OK: $d"; shift 2 ;;
    *) echo "unknown arg $1"; exit 2 ;;
  esac
done
(( fail == 0 )) && echo "desktop-entry check OK" || exit 1
