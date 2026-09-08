#!/usr/bin/env bash
# QBZ — prune stale incremental-compilation caches under crates/target.
#
# WHY: `cargo check` writes an incremental cache dir per crate, keyed by a hash
# of the build config (RUSTFLAGS / codegen-units / opt-level). Because
# slint-run.sh flips those knobs per RAM tier, EVERY distinct config spawns a
# NEW `<crate>-<hash>/` dir and Cargo NEVER deletes the old ones. Over months
# they pile up (1000+ dirs, hundreds of GB of pure garbage).
#
# This keeps only the MOST-RECENTLY-MODIFIED incremental dir per crate (that's
# the one that actually speeds up your next check) and deletes the rest.
#
# NOTE: release builds (slint-run.sh / slint-dev.sh) do NOT use incremental at
# all, so this never slows them down. It only affects `cargo check` speed, and
# only prunes stale snapshots you're not using.
#
# Usage:
#   ./scripts/prune-incremental.sh            # prune, keep newest per crate
#   DRY=1 ./scripts/prune-incremental.sh      # show what WOULD be deleted
#   KEEP=2 ./scripts/prune-incremental.sh     # keep the 2 newest per crate
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/.."

TARGET="crates/target"
KEEP="${KEEP:-1}"
DRY="${DRY:-0}"

freed=0
for incdir in "$TARGET"/*/incremental; do
  [[ -d "$incdir" ]] || continue

  declare -A seen=()   # reset per profile (debug/, release/, etc.)

  # Feed "<mtime>\t<crate>\t<path>" sorted by crate asc, then mtime desc, so the
  # newest dir of each crate arrives first.
  while IFS=$'\t' read -r _mtime crate path; do
    [[ -n "$path" ]] || continue
    seen["$crate"]=$(( ${seen["$crate"]:-0} + 1 ))
    (( seen["$crate"] <= KEEP )) && continue   # keep the newest KEEP per crate
    sz=$(du -sm "$path" 2>/dev/null | cut -f1)
    freed=$(( freed + ${sz:-0} ))
    if [[ "$DRY" == 1 ]]; then
      echo "would delete: $path (${sz}M)"
    else
      rm -rf "$path"
      echo "deleted: $path (${sz}M)"
    fi
  done < <(
    for d in "$incdir"/*/; do
      [[ -d "$d" ]] || continue
      name=$(basename "$d")
      printf '%s\t%s\t%s\n' "$(stat -c %Y "$d")" "${name%-*}" "${d%/}"
    done | sort -t$'\t' -k2,2 -k1,1rn
  )

  unset seen
done

if [[ "$DRY" == 1 ]]; then
  echo "--- DRY run: would free ~${freed} MB (KEEP=$KEEP) ---"
else
  echo "--- freed ~${freed} MB (KEEP=$KEEP) ---"
fi
