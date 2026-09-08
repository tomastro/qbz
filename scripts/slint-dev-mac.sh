#!/usr/bin/env bash
# QBZ Slint — macOS dev build/run (Apple Silicon & Intel).
#
# macOS counterpart of slint-dev.sh / slint-dev-fast.sh. Same intent (build +
# run qbz-slint with the nightly PARALLEL FRONTEND so the giant Slint-generated
# module compiles across cores), but with the three Linux-only assumptions of
# those scripts removed because they break on Mach-O / Apple toolchains:
#
#   1. NO `-fuse-ld=mold`. mold only emits ELF; on macOS clang rejects it with
#      "invalid linker name in argument '-fuse-ld=mold'" and the very first
#      link (libc's build script) fails. Apple's default linker (ld-prime) is
#      already fast, so we simply don't override the linker.
#
#   2. NO x86 target-features anywhere (#549): the aes crate runtime-dispatches
#      to AES-NI at identical speed, and compile-time features SIGILL CPUs
#      without them (pre-2010 Intel Macs). aarch64 gets its AES from the armv8
#      crypto extensions automatically.
#
#   3. The nightly parallel frontend (`-Z threads`) is kept — it works on macOS
#      and only cuts COMPILE time; the produced binary is identical to a plain
#      `cargo build`/`cargo build --release`.
#
# By default this builds in RELEASE (release runtime perf is the whole reason we
# moved from Tauri to Slint — always measure at release). Pass --fast for a quick
# DEBUG build (opt-level 0, no debuginfo) for purely visual/layout iteration;
# DO NOT judge performance from a --fast build.
#
# Debug and release artifacts live in target/debug and target/release, so the
# two modes don't invalidate each other's cache.
#
# Usage: ./scripts/slint-dev-mac.sh [--fast] [extra cargo args]
#        THREADS=8 ./scripts/slint-dev-mac.sh           # override frontend threads
#        ./scripts/slint-dev-mac.sh --fast -- --some-app-arg
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/.."

FAST=false
if [[ "${1:-}" == "--fast" ]]; then
  FAST=true
  shift
fi

# Parallel rustc frontend across cores. Defaults to the machine's logical CPU
# count; override with THREADS=N. This only speeds compilation.
THREADS="${THREADS:-$(sysctl -n hw.ncpu)}"

# No x86 target-features (#549): the aes crate runtime-dispatches to AES-NI
# at identical speed, and compile-time features SIGILL pre-2010 Intel Macs.
export RUSTFLAGS="-Z threads=${THREADS}"

if [[ "$FAST" == true ]]; then
  # DEBUG: opt-level 0 skips the heavy LLVM passes; -C debuginfo=0 means less to
  # generate and link. Unoptimised — never trust runtime behaviour from this.
  export RUSTFLAGS="${RUSTFLAGS} -C debuginfo=0"
  exec cargo +nightly run --manifest-path crates/Cargo.toml -p qbz "$@"
else
  exec cargo +nightly run --release --manifest-path crates/Cargo.toml -p qbz "$@"
fi
