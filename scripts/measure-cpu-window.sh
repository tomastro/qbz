#!/usr/bin/env bash
# measure-cpu-window.sh — sample CPU% of QBZ + WebKit children over a window
#
# Usage:
#   ./scripts/measure-cpu-window.sh [label] [duration_seconds]
#
# Examples:
#   ./scripts/measure-cpu-window.sh blur-on        # 30s sample, labelled "blur-on"
#   ./scripts/measure-cpu-window.sh blur-off 60    # 60s sample, labelled "blur-off"
#   ./scripts/measure-cpu-window.sh                # auto-label with timestamp
#
# Reads /proc/PID/stat directly (no pidstat dependency). Samples per second.
# CPU% is normalized per-core (100% = one fully busy core), matching the
# convention used in `top` and in the bug report at issue #414/#415.
#
# Output:
#   - Live one-line update per second (qbz + webkit + total)
#   - At end: mean, p50, p95, max for the whole window
#   - CSV row appended to /tmp/qbz-cpu-measurements.csv for cross-run comparison
#
# To compare runs:
#   1. Pick a stable window state (e.g. 4K full-screen, home view, track playing)
#   2. Run script, do NOT touch the window during the sample
#   3. Change the variable (e.g. resize, toggle backdrop-filter), repeat
#   4. cat /tmp/qbz-cpu-measurements.csv

set -euo pipefail

LABEL="${1:-run-$(date +%H%M%S)}"
DURATION="${2:-30}"
LOG_CSV="/tmp/qbz-cpu-measurements.csv"
CLK_TCK="$(getconf CLK_TCK)"

if [[ ! "$DURATION" =~ ^[0-9]+$ ]] || (( DURATION < 5 )); then
  echo "error: duration must be a positive integer >= 5" >&2
  exit 2
fi

# Find QBZ main process — match both legacy binary name (qbz-nix, dev builds)
# and the production binary (qbz). Prefer the largest-RSS match to dodge
# wrapper shells.
find_qbz_pid() {
  local candidates pid best_rss=0 best_pid=""
  candidates="$(pgrep -f 'target/(debug|release)/qbz(-nix)?$|/usr/(local/)?bin/qbz$|^qbz$' 2>/dev/null || true)"
  if [[ -z "$candidates" ]]; then
    candidates="$(pgrep -x qbz 2>/dev/null || true)"
  fi
  if [[ -z "$candidates" ]]; then
    candidates="$(pgrep -f 'qbz-nix' 2>/dev/null || true)"
  fi
  for pid in $candidates; do
    [[ -r "/proc/$pid/status" ]] || continue
    local rss
    rss="$(awk '/^VmRSS:/ {print $2}' "/proc/$pid/status" 2>/dev/null || echo 0)"
    if (( rss > best_rss )); then
      best_rss=$rss
      best_pid=$pid
    fi
  done
  [[ -n "$best_pid" ]] && echo "$best_pid"
}

QBZ_PID="$(find_qbz_pid || true)"
if [[ -z "${QBZ_PID:-}" ]]; then
  echo "error: no qbz process found. Is the app running?" >&2
  exit 1
fi

# All WebKit child processes. WebKitWebProcess is the one that does CSS
# layout/paint, so its CPU% is the most relevant for backdrop-filter cost.
# WebKitNetworkProcess does I/O — usually low CPU.
find_webkit_pids() {
  pgrep -f 'WebKit(Web|Network|GPU)Process' 2>/dev/null || true
}

WEBKIT_PIDS=()
mapfile -t WEBKIT_PIDS < <(find_webkit_pids)

read_jiffies() {
  # Field 14 (utime) + field 15 (stime) of /proc/PID/stat. Pre-paren tokens
  # in field 2 (comm) can contain spaces, so split off comm carefully.
  local pid="$1" stat content rest
  [[ -r "/proc/$pid/stat" ]] || { echo 0; return; }
  content="$(cat "/proc/$pid/stat" 2>/dev/null || echo "")"
  [[ -z "$content" ]] && { echo 0; return; }
  rest="${content#* (}"
  rest="${rest#*) }"
  read -r _state _ppid _pgrp _sid _tty _tpgid _flags _minflt _cminflt _majflt _cmajflt utime stime _rest <<< "$rest"
  echo $(( utime + stime ))
}

read_window_size() {
  # Best-effort: read the persisted window size from the qbz log. Not used in
  # math, just printed as context in the CSV.
  local logf="$HOME/.local/share/qbz/qbz.log"
  if [[ -r "$logf" ]]; then
    grep -oE 'size=[0-9]+x[0-9]+' "$logf" 2>/dev/null | tail -1 | cut -d= -f2 || echo "unknown"
  else
    echo "unknown"
  fi
}

printf '%s\n' "=== qbz CPU sampler ==="
printf 'label:     %s\n' "$LABEL"
printf 'qbz pid:   %s\n' "$QBZ_PID"
if (( ${#WEBKIT_PIDS[@]} > 0 )); then
  printf 'webkit:    %s\n' "${WEBKIT_PIDS[*]}"
else
  printf 'webkit:    (none found — is the GUI window open?)\n'
fi
printf 'duration:  %ss\n' "$DURATION"
printf 'window:    %s (persisted)\n' "$(read_window_size)"
printf 'log:       %s\n' "$LOG_CSV"
printf '\nSampling — leave the window UNTOUCHED for the full window.\n\n'

# Take initial snapshot
prev_qbz="$(read_jiffies "$QBZ_PID")"
declare -A prev_wk
for pid in "${WEBKIT_PIDS[@]:-}"; do
  [[ -z "$pid" ]] && continue
  prev_wk[$pid]="$(read_jiffies "$pid")"
done
prev_ts="$(date +%s.%N)"

samples_qbz=()
samples_wk_total=()
samples_total=()

printf '%4s  %8s  %8s  %8s\n' "t/s" "qbz%" "webkit%" "total%"
printf '%4s  %8s  %8s  %8s\n' "---" "----" "-------" "------"

for (( t=1; t<=DURATION; t++ )); do
  sleep 1
  now_ts="$(date +%s.%N)"
  dt="$(awk -v a="$now_ts" -v b="$prev_ts" 'BEGIN{printf "%.4f", a-b}')"
  prev_ts="$now_ts"

  cur_qbz="$(read_jiffies "$QBZ_PID")"
  d_qbz=$(( cur_qbz - prev_qbz ))
  prev_qbz="$cur_qbz"
  pct_qbz="$(awk -v j="$d_qbz" -v dt="$dt" -v hz="$CLK_TCK" 'BEGIN{ if (dt<=0) print 0; else printf "%.1f", (j/hz)/dt*100 }')"

  wk_total=0
  for pid in "${WEBKIT_PIDS[@]:-}"; do
    [[ -z "$pid" ]] && continue
    if [[ ! -r "/proc/$pid/stat" ]]; then
      continue
    fi
    cur="$(read_jiffies "$pid")"
    d=$(( cur - ${prev_wk[$pid]:-$cur} ))
    prev_wk[$pid]="$cur"
    (( d < 0 )) && d=0
    wk_total=$(( wk_total + d ))
  done
  pct_wk="$(awk -v j="$wk_total" -v dt="$dt" -v hz="$CLK_TCK" 'BEGIN{ if (dt<=0) print 0; else printf "%.1f", (j/hz)/dt*100 }')"

  pct_total="$(awk -v a="$pct_qbz" -v b="$pct_wk" 'BEGIN{ printf "%.1f", a+b }')"

  samples_qbz+=("$pct_qbz")
  samples_wk_total+=("$pct_wk")
  samples_total+=("$pct_total")

  printf '%4d  %8s  %8s  %8s\n' "$t" "$pct_qbz" "$pct_wk" "$pct_total"
done

# Aggregate
summarize() {
  local name="$1"
  shift
  local n="$#"
  local sorted
  sorted="$(printf '%s\n' "$@" | sort -n)"
  local mean p50 p95 max
  mean="$(printf '%s\n' "$@" | awk '{s+=$1} END{ if (NR>0) printf "%.1f", s/NR; else print 0 }')"
  p50="$(printf '%s\n' "$sorted" | awk -v n="$n" 'NR==int(n*0.50)+1 || (NR==1 && n==1) {print; exit}')"
  p95="$(printf '%s\n' "$sorted" | awk -v n="$n" 'NR==int(n*0.95)+1 || (NR==n) {print; exit}')"
  max="$(printf '%s\n' "$sorted" | tail -1)"
  printf '  %-10s mean=%5s%%  p50=%5s%%  p95=%5s%%  max=%5s%%\n' "$name" "$mean" "$p50" "$p95" "$max"
  echo "$mean,$p50,$p95,$max"
}

echo ""
echo "=== Summary ==="
summary_qbz="$(summarize "qbz"     "${samples_qbz[@]}"       | tail -1)"
summarize "qbz"     "${samples_qbz[@]}"       >/dev/null

read -r mean_qbz p50_qbz p95_qbz max_qbz <<< "$(echo "$summary_qbz" | tr ',' ' ')"

mean_wk=0 p50_wk=0 p95_wk=0 max_wk=0
if (( ${#samples_wk_total[@]} > 0 )); then
  summary_wk="$(summarize "webkit"  "${samples_wk_total[@]}"  | tail -1)"
  read -r mean_wk p50_wk p95_wk max_wk <<< "$(echo "$summary_wk" | tr ',' ' ')"
fi

summary_total="$(summarize "total"   "${samples_total[@]}"     | tail -1)"
read -r mean_total p50_total p95_total max_total <<< "$(echo "$summary_total" | tr ',' ' ')"

# Re-print summaries to stdout (the summarize calls above piped them out)
{
  printf '  %-10s mean=%5s%%  p50=%5s%%  p95=%5s%%  max=%5s%%\n' "qbz"    "$mean_qbz"    "$p50_qbz"    "$p95_qbz"    "$max_qbz"
  printf '  %-10s mean=%5s%%  p50=%5s%%  p95=%5s%%  max=%5s%%\n' "webkit" "$mean_wk"     "$p50_wk"     "$p95_wk"     "$max_wk"
  printf '  %-10s mean=%5s%%  p50=%5s%%  p95=%5s%%  max=%5s%%\n' "total"  "$mean_total"  "$p50_total"  "$p95_total"  "$max_total"
}

# CSV header if first run
if [[ ! -f "$LOG_CSV" ]]; then
  echo "timestamp,label,duration_s,window_size,qbz_mean,qbz_p50,qbz_p95,qbz_max,webkit_mean,webkit_p50,webkit_p95,webkit_max,total_mean,total_p50,total_p95,total_max" > "$LOG_CSV"
fi

WINDOW="$(read_window_size)"
echo "$(date -Iseconds),$LABEL,$DURATION,$WINDOW,$mean_qbz,$p50_qbz,$p95_qbz,$max_qbz,$mean_wk,$p50_wk,$p95_wk,$max_wk,$mean_total,$p50_total,$p95_total,$max_total" >> "$LOG_CSV"

echo ""
echo "Appended to $LOG_CSV"
echo "Compare runs:  cat $LOG_CSV | column -t -s,"
