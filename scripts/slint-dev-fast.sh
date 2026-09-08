#!/usr/bin/env bash
# QBZ Slint — FAST dev build/run (DEBUG, *not* release).
#
# ADDITION to ./slint-dev.sh, NOT a replacement. Use this for quick, purely
# visual / layout iteration where runtime performance does NOT matter — it is
# dramatically faster to compile than the release build because:
#   - debug profile (opt-level 0) → the giant Slint-generated module skips the
#     heavy LLVM optimisation passes that dominate the release build,
#   - the `mold` linker (much faster than the default `ld`),
#   - NO debuginfo (`-C debuginfo=0`) → less to generate AND less to link,
#   - the nightly PARALLEL FRONTEND (`-Z threads`) across cores.
# Debug builds live in target/debug, separate from the release target/release,
# so the two scripts don't invalidate each other's cache.
#
# DO NOT use this to judge performance — the binary is unoptimised. Always do a
# final pass with ./slint-dev.sh (release) before trusting runtime behaviour.
#
# Usage: ./scripts/slint-dev-fast.sh [extra cargo args]
#        THREADS=22 ./scripts/slint-dev-fast.sh   # override frontend threads
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/.."
export RUSTFLAGS="-C link-arg=-fuse-ld=mold -C debuginfo=0 -Z threads=${THREADS:-16}"
exec cargo +nightly run --manifest-path crates/Cargo.toml -p qbz "$@"
