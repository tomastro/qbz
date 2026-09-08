#!/usr/bin/env bash
# Count Qt Quick scene-graph BATCHES for a scene you have driven to by hand.
#
# WHY THIS EXISTS. GPU utilisation is a terrible optimisation target: it moved by
# nothing across four different hypotheses (shader resolution, pane layering,
# animation tick, duplicated clocks) because none of them touched the thing that
# actually costs. QSG_RENDER_TIMING then showed render=2ms / sync=0 / polish=0 —
# the scene is cheap to COMPUTE — and QSG_RENDERER_DEBUG=render showed
# "Alpha: 533 nodes in 449 batches", i.e. 1.2 nodes per batch and ~28k draw calls
# a second. Batch count is the number that moves when the tree improves, and it
# is deterministic: same view, same batches. No thermal ramp, no measurement
# window to argue about, no 20% spread between consecutive readings.
#
# YOU DRIVE, THEN IT SAMPLES. The first cut of this script launched the app and
# killed it after a fixed timeout, which meant it sampled whatever was on screen
# during startup — and since the dynamic background only mounts once a track is
# loaded, the "reference scene" it measured was the one with the background OFF.
# It now waits for you: get to the view, start playback, let it settle, press
# Enter, and only then does the sample window open.
#
# Usage:
#   ./scripts/qt-batches.sh              # launch, you drive, Enter, 8s sample
#   SECS=15 ./scripts/qt-batches.sh      # longer sample window
#   KEEP=1  ./scripts/qt-batches.sh      # leave the app running afterwards
#   ATLAS_OVERLAY=1 ./scripts/qt-batches.sh   # tint atlased textures (diagnostic)
#   ATLAS=8192 ./scripts/qt-batches.sh        # force a bigger texture atlas
#   BIN=crates/target/debug/qbz ./scripts/qt-batches.sh
#   QBZ_PULSE_MS=25 ./scripts/qt-batches.sh   # 40 Hz shell pulse instead of 30
#
# QBZ_PULSE_MS is the shared repaint clock (settings_qt::shell_pulse_ms,
# default 33). It is THE knob this script's "presents/s" line measures, so the
# summary below prints the value it ran with: two logs whose pulse cannot be
# told apart are not an A/B.
#
# THE REFERENCE SCENE, so two runs are comparable: album page, NPB Large, the
# spectrum band up (the eye toggle on), a track PLAYING, dynamic background on.
# The count is per-scene — comparing Home against an album page tells you
# nothing, and comparing background-on against background-off tells you only
# that translucency moved the tree into the alpha pass.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/.."

BIN="${BIN:-crates/target/release/qbz}"
SECS="${SECS:-8}"
LOG="$(mktemp "${TMPDIR:-/tmp}/qbz-batches-XXXXXX.log")"

[[ -x "$BIN" ]] || { echo "no binary at $BIN — build first" >&2; exit 1; }

# ~1.2 MB/s of renderer chatter, so a long drive makes a big file. Harmless, but
# say so rather than let it be a surprise.
# ATLAS_OVERLAY=1 tints every texture that made it INTO the shared atlas.
# Untinted covers mean the atlas filled up: there is one Atlas instance, and
# once allocate() fails each subsequent image becomes a standalone texture with
# a unique comparisonKey() and is permanently unmergeable. That is a bigger
# lever than any clip, so it is worth ruling in or out before reading a delta.
# It is NOT on by default: the tint makes the UI unusable to look at, and this
# script's main job is the batch count.
atlas_env=()
if [[ "${ATLAS_OVERLAY:-0}" == 1 ]]; then
  atlas_env+=(QSG_ATLAS_OVERLAY=1)
  echo "[batches] ATLAS OVERLAY ON — tinted = in the atlas, untinted = its own texture"
fi
# ATLAS=8192 raises the atlas past the window-derived default, the actual fix
# if the overlay shows overflow.
if [[ -n "${ATLAS:-}" ]]; then
  atlas_env+=(QSG_ATLAS_WIDTH="$ATLAS" QSG_ATLAS_HEIGHT="$ATLAS")
  echo "[batches] atlas forced to ${ATLAS}x${ATLAS}"
fi

echo "[batches] shell pulse = ${QBZ_PULSE_MS:-33} ms (~$((1000 / ${QBZ_PULSE_MS:-33})) Hz)"
echo "[batches] launching $BIN (renderer debug is ~1 MB/s of log -> $LOG)"
env "${atlas_env[@]}" QSG_RENDERER_DEBUG=render QSG_RENDER_TIMING=1 "$BIN" > "$LOG" 2>&1 &
pid=$!

cleanup() { [[ "${KEEP:-0}" == 1 ]] || kill "$pid" 2>/dev/null || true; }
trap cleanup EXIT

echo
echo "[batches] Drive the app to the scene you want to measure:"
echo "          album page · NPB Large · band ON · track PLAYING · background on"
echo "[batches] Let it settle, then press Enter to open the ${SECS}s sample window."
read -r _ || true

# Everything written before this instant is startup and navigation. The sample
# is only what lands after — that is the whole point of waiting for you.
mark=$(stat -c %s "$LOG" 2>/dev/null || stat -f %z "$LOG")
echo "[batches] sampling ${SECS}s..."
sleep "$SECS"
sample="$(mktemp "${TMPDIR:-/tmp}/qbz-batches-sample-XXXXXX.log")"
tail -c "+$((mark + 1))" "$LOG" > "$sample"

if ! kill -0 "$pid" 2>/dev/null; then
  echo "[batches] WARNING: the app exited during the sample window — this is a" >&2
  echo "          real exit, not the script; check the tail of $LOG" >&2
fi

# The LAST block in the window: the settled tree, after any delegate churn from
# scrolling or a track change has finished.
opaque=$(grep -a 'Opaque:' "$sample" | tail -1 || true)
alpha=$(grep -a 'Alpha:' "$sample" | tail -1 || true)

if [[ -z "$alpha" ]]; then
  echo "[batches] no renderer output in the window — did the scene stop repainting?" >&2
  echo "[batches] (a paused player with no animation legitimately emits nothing)" >&2
  echo "[batches] log: $LOG" >&2
  exit 1
fi

echo
echo "  $opaque"
echo "  $alpha"
echo

# nodes-per-batch is the health metric: 1.0 means every node pays its own draw
# call, higher is better. Under ~2 says the tree is fighting the batcher, and
# clip state is the usual reason (Qt cannot merge across different clip roots).
python3 - "$alpha" "$opaque" <<'PY'
import re, sys
def parse(s):
    m = re.search(r'(\d+)\s+nodes in\s+(\d+)\s+batches', s or '')
    return (int(m.group(1)), int(m.group(2))) if m else None
a, o = parse(sys.argv[1]), parse(sys.argv[2])
if a:
    print(f"  alpha nodes/batch = {a[0]/max(a[1],1):.2f}   (1.0 = no batching at all)")
if o:
    print(f"  opaque nodes/batch = {o[0]/max(o[1],1):.2f}")
if a and o:
    print(f"  TOTAL batches per frame = {a[1] + o[1]}   <- the number to drive down")
PY

# PRESENT RATE — the number that decides whether hunting repaint sources is even
# the right hunt. Qt Quick redraws and presents the WHOLE window whenever
# anything dirties the scene, and on a hybrid laptop whose external monitor
# hangs off the dGPU every present is also a frame KWin composites and scans out
# there. So the GPU cost is presents/s x window area, and presents/s is
# MEASURABLE instead of guessable: `polishAndSync: elapsed since last call` is
# the frame period. Read this before proposing any mechanism. ~30/s = one
# source. ~60/s = two unsynchronised ones. 120+/s = something runs at display
# rate. Five hypotheses died in this campaign for want of this one number.
python3 - "$sample" <<'PYFPS'
import re, sys, statistics, os
txt = open(sys.argv[1], errors="ignore").read()
per = [int(m) for m in re.findall(r"elapsed since last call: (\d+) ms", txt)]
per = [p for p in per if p > 0]
if per:
    med = statistics.median(per)
    fast = sum(1 for p in per if p <= 8)
    print(f"  presents: {len(per)} sampled, median period {med:.0f} ms -> ~{1000/max(med,1):.0f}/s"
          f"   (pulse was {os.environ.get('QBZ_PULSE_MS', '33')} ms)")
    print(f"  arriving <=8ms apart: {fast} ({100*fast/len(per):.0f}%)   <- display-rate source if high")
else:
    print("  presents: no timing lines (QSG_RENDER_TIMING not honoured?)")
PYFPS

# Per-frame composition of the last block, which is what a clip audit targets.
last_start=$(grep -an 'Rendering:' "$sample" | tail -1 | cut -d: -f1 || true)
if [[ -n "$last_start" ]]; then
  block=$(tail -n "+$last_start" "$sample")
  echo
  printf '  last frame: clip=%s noclip=%s unmerged=%s\n' \
    "$(grep -c '\[  clip\]' <<<"$block" || true)" \
    "$(grep -c '\[noclip\]' <<<"$block" || true)" \
    "$(grep -c '\[unmerged\]' <<<"$block" || true)"
fi

echo
echo "  sample: $sample"
echo "  full log: $LOG"
