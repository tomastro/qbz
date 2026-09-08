#!/usr/bin/env bash
# QBZ Qt — RENDERING DIAGNOSTIC. Launches the app with Qt's scene-graph and RHI
# logging on, samples the process's CPU while you drive it, and leaves ONE log
# file you can copy off the machine.
#
# Why this exists: the whole "this port runs on the software renderer" doctrine
# — repeated across ~8 QML file headers and used to justify pre-baked icon
# tints, Canvas-based rounded covers and a blanket no-effects rule — came from
# an observation made OFFSCREEN, and QT_QPA_PLATFORM=offscreen forces software
# by definition. On Linux, QSG_INFO in a real window says otherwise: OpenGL RHI,
# threaded render loop, Intel Arc. macOS is still unmeasured. So: measure, in a
# REAL graphical session, never over ssh and never offscreen.
#
# WHAT TO DO WHILE IT RUNS (the log is only as good as the session):
#   1. Let it sit idle on Home for ~15s.
#   2. Play a track. Let it settle ~15s.
#   3. Switch the now-playing bar through its modes (New / Classic / Small /
#      Large), pausing ~15s on each — Large is the one that mounts the FFT
#      spectrum, which is the suspect.
#   4. Hover a few album cards.
#   5. Close the app normally. The summary is written on exit.
#
# Usage:  ./scripts/qt-diag.sh            # release binary, must already be built
#         DEBUG=1 ./scripts/qt-diag.sh    # debug binary instead
#         INTERVAL=2 ./scripts/qt-diag.sh # CPU sample period in seconds (default 2)
set -uo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/.."

UNAME_S="$(uname -s)"
PROFILE=$([[ "${DEBUG:-0}" == 1 ]] && echo debug || echo release)
BIN="crates/target/${PROFILE}/qbz"
INTERVAL="${INTERVAL:-2}"

[[ -x "${BIN}" ]] || { echo "[qt-diag] no binary at ${BIN} — build first (./scripts/qt-run.sh NORUN=1)" >&2; exit 1; }

# Refuse to produce a log that answers the wrong question. Offscreen forces
# software, and a headless ssh session has no compositor to talk to — either
# would make the backend line meaningless, which is the one line we are here for.
if [[ "${QT_QPA_PLATFORM:-}" == offscreen ]]; then
  echo "[qt-diag] QT_QPA_PLATFORM=offscreen forces the software backend — that is the mistake this script exists to correct. Unset it." >&2
  exit 1
fi
if [[ "${UNAME_S}" != "Darwin" && -z "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]]; then
  echo "[qt-diag] no DISPLAY / WAYLAND_DISPLAY — run this from the machine's own graphical session, not over ssh." >&2
  exit 1
fi

stamp="$(date +%Y%m%d-%H%M%S)"
host="$(hostname -s 2>/dev/null || hostname)"
LOG="${PWD}/qt-diag-${host}-${stamp}.log"

{
  echo "=============================================================="
  echo " QBZ Qt rendering diagnostic"
  echo " host      : ${host}"
  echo " when      : $(date '+%Y-%m-%d %H:%M:%S %Z')"
  echo " os        : $(uname -srm)"
  [[ "${UNAME_S}" == "Darwin" ]] && echo " macos     : $(sw_vers -productVersion)"
  echo " binary    : ${BIN} ($(date -r "${BIN}" '+%Y-%m-%d %H:%M' 2>/dev/null || stat -c %y "${BIN}" 2>/dev/null))"
  echo " profile   : ${PROFILE}"
  echo "=============================================================="
  echo
  echo "---- display ----"
  if [[ "${UNAME_S}" == "Darwin" ]]; then
    system_profiler SPDisplaysDataType 2>/dev/null | grep -iE 'resolution|ui looks like|chipset|vram|metal' || true
  else
    xrandr 2>/dev/null | grep -E ' connected|\*' || echo "(xrandr unavailable — wayland?)"
    echo "session type: ${XDG_SESSION_TYPE:-unknown}"
  fi
  echo
  echo "---- qt ----"
  (qmake6 -query QT_VERSION 2>/dev/null || /opt/homebrew/opt/qt/bin/qmake -query QT_VERSION 2>/dev/null || echo unknown) | sed 's/^/qt version: /'
  echo "effects modules present:"
  for d in /usr/lib64/qt6/qml /usr/lib/qt6/qml /opt/homebrew/share/qt/qml; do
    [[ -d "$d/QtQuick/Effects" ]] && echo "  QtQuick.Effects            -> $d"
    [[ -d "$d/Qt5Compat/GraphicalEffects" ]] && echo "  Qt5Compat.GraphicalEffects -> $d"
  done
  echo "backend forced by env: QT_QUICK_BACKEND='${QT_QUICK_BACKEND:-}' QSG_RHI_BACKEND='${QSG_RHI_BACKEND:-}' QSG_RENDER_LOOP='${QSG_RENDER_LOOP:-}'"
  echo
  echo "---- app log (QSG_INFO + scenegraph/rhi categories) ----"
} > "${LOG}"

echo "[qt-diag] log -> ${LOG}"
echo "[qt-diag] drive the app as described in this script's header, then close it normally."

# The two lines that answer the question are 'Creating QRhi with backend X' and
# 'Prefer software device: N'. The rhi/scenegraph categories carry them.
QSG_INFO=1 \
QT_LOGGING_RULES="qt.scenegraph.general=true;qt.rhi.general=true" \
"./${BIN}" >> "${LOG}" 2>&1 &
app_pid=$!

# CPU sampler. ps reports a per-process percentage on both platforms; on macOS
# it is a share of ONE core (so >100 is possible), same as Linux.
samples="$(mktemp)"
(
  while kill -0 "${app_pid}" 2>/dev/null; do
    cpu=$(ps -o %cpu= -p "${app_pid}" 2>/dev/null | tr -d ' ')
    rss=$(ps -o rss= -p "${app_pid}" 2>/dev/null | tr -d ' ')
    [[ -n "${cpu}" ]] && printf '%s %s %s\n' "$(date '+%H:%M:%S')" "${cpu}" "${rss:-0}" >> "${samples}"
    sleep "${INTERVAL}"
  done
) &
sampler=$!

trap 'kill "${app_pid}" 2>/dev/null || true' INT TERM
wait "${app_pid}" 2>/dev/null
kill "${sampler}" 2>/dev/null || true
wait "${sampler}" 2>/dev/null || true

{
  echo
  echo "---- cpu samples (time, %cpu of one core, rss KB) ----"
  cat "${samples}" 2>/dev/null
  echo
  echo "---- cpu summary ----"
  awk '{n++; s+=$2; if($2>mx)mx=$2; if(n==1||$2<mn)mn=$2}
       END{ if(n) printf "samples=%d  min=%.1f%%  avg=%.1f%%  max=%.1f%%\n", n, mn, s/n, mx;
            else print "(no samples — the process exited immediately)" }' "${samples}" 2>/dev/null
  echo
  echo "---- what to look for ----"
  echo "  * 'Creating QRhi with backend <X>'  -> OpenGL/Metal/Vulkan = GPU; 'software' = not."
  echo "  * 'Prefer software device: 1'       -> something asked for software explicitly."
  echo "  * 'Animation Driver: using vsync'   -> ms per frame; 5.56 = 180Hz, 10.0 = 100Hz."
  echo "  * a max% that tracks the NPB Large window is the FFT spectrum band."
} >> "${LOG}"
rm -f "${samples}"

echo
echo "[qt-diag] done. Backend line:"
grep -aE 'Creating QRhi|Prefer software|using vsync' "${LOG}" | sed 's/^/    /' || echo "    (none found — did the window open?)"
echo "[qt-diag] full log: ${LOG}"
