#!/usr/bin/env bash
# QBZ Slint — build with cargo, then run the BINARY DIRECTLY.
#
# Why this exists (vs slint-dev.sh which does `cargo run`): `cargo run` launches
# the app as a cargo-managed run target — the process inherits CARGO_* env and a
# cargo launch context, which KDE plasma-systemmonitor surfaces by labelling the
# RUNNING APP as "cargo" instead of "qbz-slint" (the kernel comm is still
# qbz-slint; it's only the monitor's display name). Running the prebuilt binary
# directly (no cargo wrapper) makes process monitors show it as `qbz-slint`,
# cleanly separate from the cargo/rustc BUILD processes.
#
# ─── THE MEMORY WALL ────────────────────────────────────────────────────────
# A single RELEASE rustc for qbz-slint can hit ~20-24 GB. This box has 30 GB,
# no hibernation, and HARD-FREEZES (power-cycle, lost work) on OOM/swap-thrash.
# The old fixed `-Z threads=16` + opt-level=3 build SIGTERM'd at codegen whenever
# the desktop held ~8-11 GB. To "skip the wall" this script:
#   (a) SCALES rustc frontend threads + codegen-units + opt-level to the RAM that
#       is actually free, so the compile FITS instead of being OOM-killed, and
#   (b) runs the build under the `cargo-capped` cgroup so even a runaway dies
#       cleanly (build killed) instead of freezing the whole box.
#
# Tiers (auto, from `MemAvailable`); override any knob via env:
#   >= 26 GB free  → FAST  : threads=16 cgu=16  opt=3  (identical to slint-dev,
#                            uncapped) — best with the desktop closed / on a TTY.
#   14-26 GB free  → SAFE  : threads=2  cgu=256 opt=3  (fits a normal desktop).
#   <  14 GB free  → MIN   : threads=1  cgu=256 opt=2  (slow but never freezes).
# NOTE: the SAFE/MIN tiers change codegen-units/opt-level, so the produced binary
# is functionally identical but not byte-identical to the FAST/distribution build,
# and switching tiers (or any RUSTFLAGS/profile knob) forces a one-time rebuild.
#
# ─── VISUAL PROGRESS ────────────────────────────────────────────────────────
# Prints a start banner (wall-clock + tier), a live ⏱ ticker every 15s while the
# long codegen phase is otherwise silent (elapsed / ETA / percent), and a final
# banner with total build time. The ETA is learned: each successful build records
# its duration per tier under ${XDG_CACHE_HOME:-~/.cache}/qbz-slint/, and the next
# run of the same tier uses it as the estimate (no estimate on the first ever run
# of a tier, or right after `cargo clean`). NO_TICKER=1 silences the live ticker.
#
# Usage: ./scripts/slint-run.sh [extra app args]
#   FAST=1                        ./scripts/slint-run.sh   # force the fast build
#   THREADS=4 CODEGEN_UNITS=128 OPT=3 ./scripts/slint-run.sh   # manual override
#   CAPPED=0                      ./scripts/slint-run.sh   # disable the cgroup cap
#   NORUN=1                       ./scripts/slint-run.sh   # build only, don't exec
#   NO_TICKER=1                   ./scripts/slint-run.sh   # no live progress ticker
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/.."

# --- Pretty helpers ----------------------------------------------------------
if [[ -t 2 ]]; then C_DIM=$'\033[2m'; C_BOLD=$'\033[1m'; C_GRN=$'\033[32m'; C_RST=$'\033[0m'
else C_DIM=""; C_BOLD=""; C_GRN=""; C_RST=""; fi
fmt_dur() { local s=$1; printf '%dm %02ds' $(( s / 60 )) $(( s % 60 )); }

avail_mb=$(free -m | awk '/^Mem:/ {print $7}')

# --- Pick build settings from available RAM (any knob overridable via env) ----
if [[ "${FAST:-0}" == 1 ]] || (( avail_mb >= 26000 )); then
  TIER=FAST;  THREADS="${THREADS:-16}"; CODEGEN_UNITS="${CODEGEN_UNITS:-16}";  OPT="${OPT:-3}"
  CAPPED="${CAPPED:-0}"          # ample RAM → earlyoom is the net, no cgroup cap
elif (( avail_mb >= 14000 )); then
  TIER=SAFE;  THREADS="${THREADS:-2}";  CODEGEN_UNITS="${CODEGEN_UNITS:-256}"; OPT="${OPT:-3}"
  CAPPED="${CAPPED:-1}"
else
  TIER=MIN;   THREADS="${THREADS:-1}";  CODEGEN_UNITS="${CODEGEN_UNITS:-256}"; OPT="${OPT:-2}"
  CAPPED="${CAPPED:-1}"
  echo "[slint-run] WARNING: only ${avail_mb} MB free — lowest-memory tier (slow). Close apps / drop to a TTY for a faster build." >&2
fi

# No x86 target-features here or in .cargo/config.toml (#549): the aes crate
# runtime-dispatches to AES-NI at identical speed, and compile-time features
# SIGILL older CPUs.
export RUSTFLAGS="-C link-arg=-fuse-ld=mold -Z threads=${THREADS}"
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS="${CODEGEN_UNITS}"
export CARGO_PROFILE_RELEASE_OPT_LEVEL="${OPT}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"   # one rustc at a time (memory)

# --- Learned ETA: last successful build duration for THIS tier ---------------
eta_dir="${XDG_CACHE_HOME:-$HOME/.cache}/qbz-slint"
eta_file="${eta_dir}/last-build-${TIER}.secs"
eta_secs=0
[[ -r "${eta_file}" ]] && eta_secs=$(cat "${eta_file}" 2>/dev/null || echo 0)
[[ "${eta_secs}" =~ ^[0-9]+$ ]] || eta_secs=0

echo "[slint-run] tier=${TIER} avail=${avail_mb}MB → threads=${THREADS} codegen-units=${CODEGEN_UNITS} opt-level=${OPT} capped=${CAPPED}"

# --- Start banner ------------------------------------------------------------
build_start=$(date +%s)
if (( eta_secs > 0 )); then eta_txt="~$(fmt_dur "${eta_secs}") (last ${TIER})"; else eta_txt="unknown (first ${TIER} build)"; fi
printf '%s[slint-run] ▶ build started %s  ·  ETA %s%s\n' \
  "${C_BOLD}" "$(date '+%H:%M:%S')" "${eta_txt}" "${C_RST}"

# --- Live ticker: elapsed / ETA / percent, every 15s while codegen is silent -
tick_pid=""
if [[ "${NO_TICKER:-0}" != 1 ]] && [[ -t 2 ]]; then
  (
    while true; do
      sleep 15
      now=$(date +%s); el=$(( now - build_start ))
      if (( eta_secs > 0 )); then
        pct=$(( el * 100 / eta_secs )); (( pct > 99 )) && pct=99
        printf '%s[slint-run] ⏱  %s elapsed · ETA ~%s · ~%d%%%s\n' \
          "${C_DIM}" "$(fmt_dur "${el}")" "$(fmt_dur "${eta_secs}")" "${pct}" "${C_RST}" >&2
      else
        printf '%s[slint-run] ⏱  %s elapsed%s\n' "${C_DIM}" "$(fmt_dur "${el}")" "${C_RST}" >&2
      fi
    done
  ) &
  tick_pid=$!
  # Safety net: if the build aborts (set -e), don't leak the ticker.
  trap '[[ -n "${tick_pid}" ]] && kill "${tick_pid}" 2>/dev/null || true' EXIT
fi

# --- The build ---------------------------------------------------------------
if [[ "${CAPPED}" == 1 ]] && command -v cargo-capped >/dev/null 2>&1; then
  # Cap the cgroup so the GLOBAL MemAvailable floor never reaches earlyoom's
  # 10% trigger (~3.2 GB on this box). Empirically (2026-07-04, ftrace-caught):
  # a 3.5 GB margin is NOT enough — the desktop grows a few GB during an
  # hour-long build, avail cratered to 3.1 GB and earlyoom SIGTERM'd rustc
  # while the cgroup sat comfortably under its cap. memory.high is the anchor
  # (throttle-to-swap early); leave ~9 GB of global headroom above the trigger.
  high=$(( avail_mb - 9000 )); (( high > 24000 )) && high=24000; (( high < 8000 )) && high=8000
  export BUILD_MEM_HIGH="${high}M"
  export BUILD_MEM_MAX="$(( high + 2000 ))M"
  echo "[slint-run] cgroup cap: high=${BUILD_MEM_HIGH} max=${BUILD_MEM_MAX}"
  cargo-capped cargo +nightly build --release --manifest-path crates/Cargo.toml -p qbz
else
  cargo +nightly build --release --manifest-path crates/Cargo.toml -p qbz
fi

# --- Stop the ticker, record the duration, print the final banner ------------
[[ -n "${tick_pid}" ]] && { kill "${tick_pid}" 2>/dev/null || true; wait "${tick_pid}" 2>/dev/null || true; }
trap - EXIT
build_secs=$(( $(date +%s) - build_start ))
mkdir -p "${eta_dir}" 2>/dev/null && printf '%s\n' "${build_secs}" > "${eta_file}" 2>/dev/null || true
printf '%s[slint-run] ✔ build finished %s  ·  took %s  (tier %s)%s\n' \
  "${C_BOLD}${C_GRN}" "$(date '+%H:%M:%S')" "$(fmt_dur "${build_secs}")" "${TIER}" "${C_RST}"

[[ "${NORUN:-0}" == 1 ]] && { echo "[slint-run] build done (NORUN set)."; exit 0; }

# exec the binary directly — no `cargo run`, so no CARGO_* env / cargo context,
# so the monitor shows `qbz-slint`.
exec crates/target/release/qbz "$@"
