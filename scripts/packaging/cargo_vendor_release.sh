#!/usr/bin/env bash
# Build the Cargo vendor archive consumed by the source AUR and Gentoo
# packages. Cargo needs metadata for target-conditional packages while
# resolving the locked graph, so this must include every registry entry in
# Cargo.lock even though Linux compiles only a subset. The expanded tree is
# large, but deterministic xz compression keeps the release asset reasonable.

set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "usage: $0 VERSION OUTPUT_DIR" >&2
    exit 2
fi

version="$1"
output_dir="$2"
manifest="crates/Cargo.toml"

if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]]; then
    echo "invalid version: $version" >&2
    exit 2
fi

mkdir -p "$output_dir"
output_dir="$(cd "$output_dir" && pwd)"
work_dir="$(mktemp -d)"
trap 'rm -rf "$work_dir"' EXIT

vendor_dir="$work_dir/vendor"
available="$work_dir/available.txt"
archive="$output_dir/qbz-${version}-cargo-vendor.tar.xz"

cargo vendor --quiet --locked --versioned-dirs \
    --manifest-path "$manifest" "$vendor_dir"

find "$vendor_dir" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' \
    | sort > "$available"

if [[ ! -s "$available" ]]; then
    echo "vendor set is empty" >&2
    exit 1
fi

# Stable ownership, ordering and mtimes make reruns byte-for-byte reproducible
# for the same lockfile and source epoch. The transform gives both package
# managers one predictable directory after extraction.
source_date_epoch="${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}"
tar -C "$vendor_dir" \
    --sort=name \
    --mtime="@${source_date_epoch}" \
    --owner=0 --group=0 --numeric-owner \
    --transform="s,^,qbz-${version}-cargo-vendor/," \
    -cJf "$archive" \
    -T "$available"

echo "Created $archive ($(wc -l < "$available") crates)"
sha256sum "$archive"
