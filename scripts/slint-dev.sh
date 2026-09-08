#!/usr/bin/env bash
# QBZ Slint — standard dev build/run.
#
# Builds + runs qbz-slint in RELEASE mode (release runtime performance is
# the whole reason we moved from Tauri to Slint — we always test at release
# perf so a regression never hides behind "it's just dev mode"), but uses
# the nightly rustc PARALLEL FRONTEND (-Z threads) so the serial compile of
# the giant Slint-generated module is split across cores. The produced
# binary is the SAME optimized release binary as `cargo build --release`;
# only compile time improves. We deliberately do NOT use cranelift — it
# would lower runtime performance.
#
# No x86 target-features here or in .cargo/config.toml (#549): the aes crate
# runtime-dispatches to AES-NI at identical speed, and compile-time features
# SIGILL older CPUs.
#
# Usage: ./scripts/slint-dev.sh [extra cargo args]
#        THREADS=16 ./scripts/slint-dev.sh   # override frontend threads
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/.."
# `mold` (faster linker) + more frontend threads only cut COMPILE time — they
# don't change the optimised binary, so release perf measurements stay valid.
export RUSTFLAGS="-C link-arg=-fuse-ld=mold -Z threads=${THREADS:-16}"
exec cargo +nightly run --release --manifest-path crates/Cargo.toml -p qbz "$@"
