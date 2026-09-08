#!/usr/bin/env bash
# scripts/cargo-test.sh — the EXACT local mirror of .github/workflows/test-crates.yml.
#
# Rule (owner, 2026-08-26): test-crates is a living regression gate. Every
# step it gains lands here in the same commit, and vice versa; the workflow
# calls THIS script so the two cannot diverge. Bounded: --no-fail-fast and
# the `test` job's 20-minute limit. CARGO_BUILD_JOBS is left to cargo (= all
# cores): the old "2" was a Slint-era memory tier, and measured cold on the
# 4-vCPU CI VM (2026-08-26) jobs=4 took 392 s vs 589 s at jobs=2 with a
# 2.7 GB peak — nothing in this graph needs the throttle.
#
# Default (job `test`, no Qt):
#   cargo test --workspace --exclude qbz-qt
#   + the listen-log suite check: `qbz-app::listen_log` must list >= 25 tests
#     and pass (schema migration, accumulator: paused/seek/stall deltas add
#     nothing, natural/skip/stop/shutdown closes, orphan close on reopen,
#     paused writes nothing, clear + VACUUM, is_play/is_skip bounds).
#   --workspace today = the 42 members of crates/Cargo.toml minus qbz-qt: the
#   audio/player/cache/DSD/disc/rip core, qbz-app/core/models/theme/i18n,
#   the Qobuz client and the source seam, Plex/Jellyfin/Subsonic + media
#   cache, local library + catalog, integrations/reco/lyrics/radio/mixtape/
#   playlist-import/media-controls/cast, credentials/secrets/offline-cache,
#   the four qconnect-* crates, the HiFi wizard core, and qbzd. Nothing here
#   needs Qt; wayland-sys enters the graph only through qbz-qt.
#   The Slint crates (qbz, qbz-ui, qbz-dac-wizard, qbz-slint-common) are gone
#   from the workspace: no exclusions for them, and never bring them back.
#
# QT=1 (jobs `shader-gate` + `qt-gate`, needs a Qt >= 6.8 install):
#   1. the five static QML audits (scripts/qml-audits)
#   2. the shader bake gate with the qsb on PATH (CI: the pinned aqt qsb)
#   3. the Slint-free dep-graph gate for qbz-qt
#   4. cargo test -p qbz-qt (debug)
#   5. offscreen boots of BOTH debug and release: zero QML complaints,
#      QbzCore initialized, and process still alive at the deadline.
#      Native Qt SDK content participates in the C++ dependency cache.
#   6. release xcb boot against a private silent D-Bus (requires Xvfb).
#   The shared runtime gate also executes Local Library QML logic and Kiosk artwork/navigation with Node.
#
# Usage:
#   ./scripts/cargo-test.sh                 # job `test`
#   ./scripts/cargo-test.sh -- --lib        # extra cargo test args
#   QT=1 ./scripts/cargo-test.sh            # the Qt gates too
#   QBZ_QT_QML_DIR=/path/to/qt/qml QT=1 ./scripts/cargo-test.sh   # point the audits at a Qt
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
say() { printf '[cargo-test] %s\n' "$*"; }

say "gate: native Qt SDK cache and crash-aware smoke regressions"
python3 scripts/test_qt_build_gates.py

say "gate: Windows app-local CRT import regressions"
python3 scripts/packaging/test_windows_crt.py

say "gate: packaged Qt process survival and smoke isolation"
python3 scripts/packaging/test_smoke_release.py

say "gate: all eight gettext catalogs"
for locale in en es de fr pt ru ja nl; do
  msgfmt --check --output-file=/dev/null "crates/qbz-i18n/translations/$locale/LC_MESSAGES/qbz-ui.po"
done

say "job test: cargo test --workspace --exclude qbz-qt (jobs=${CARGO_BUILD_JOBS:-all cores})"
cargo test \
  --manifest-path crates/Cargo.toml \
  --workspace \
  --exclude qbz-qt \
  --no-fail-fast \
  "$@"

say "gate: SACD physical-sector and scanner regressions present"
# Already executed by the workspace run: keep the container/seek equivalence
# and failed-scan preservation checks from silently disappearing.
n=$(cargo test --manifest-path crates/Cargo.toml -p qbz-disc --lib -- --list raw_sector_tests:: 2>/dev/null \
    | grep -c ': test$' || true)
(( n >= 9 )) || { echo "SACD physical-sector suite has $n tests (expected >= 9)"; exit 1; }
n=$(cargo test --manifest-path crates/Cargo.toml -p qbz-library --lib -- --list raw_sacd_scan_ 2>/dev/null \
    | grep -c ': test$' || true)
(( n >= 1 )) || { echo "SACD raw scanner suite has $n tests (expected >= 1)"; exit 1; }

say "gate: listen log suite present and green (qbz-app::listen_log)"
# The workspace run above already executes these; this step exists so a
# refactor that silently drops the module's tests (a renamed mod, a cfg gate)
# fails the gate instead of shrinking coverage without a trace. Cheap: the
# test binary is already built.
n=$(cargo test --manifest-path crates/Cargo.toml -p qbz-app --lib -- --list listen_log:: 2>/dev/null \
    | grep -c ': test$' || true)
(( n >= 25 )) || { echo "listen_log suite has $n tests (expected >= 25)"; exit 1; }
cargo test --manifest-path crates/Cargo.toml -p qbz-app --lib -- listen_log::

say "gate: account-migration and portable-blacklist suites present and green"
# Same shape as the listen-log check: the workspace run already executed
# these; the count keeps a renamed module or a dropped cfg from shrinking the
# coverage silently. qbz-account-migration is the crate behind Settings >
# Import / Export (snapshot, plan, apply, ledger, local-profile copy);
# blacklist_portable is the JSON one user hands to another.
n=$(cargo test --manifest-path crates/Cargo.toml -p qbz-account-migration --lib -- --list 2>/dev/null \
    | grep -c ': test$' || true)
(( n >= 12 )) || { echo "qbz-account-migration has $n tests (expected >= 12)"; exit 1; }
n=$(cargo test --manifest-path crates/Cargo.toml -p qbz-app --lib -- --list blacklist_portable:: 2>/dev/null \
    | grep -c ': test$' || true)
(( n >= 3 )) || { echo "blacklist_portable suite has $n tests (expected >= 3)"; exit 1; }
cargo test --manifest-path crates/Cargo.toml -p qbz-account-migration --lib
cargo test --manifest-path crates/Cargo.toml -p qbz-app --lib -- blacklist_portable::

say "gate: QConnect controller smoke and consecutive-log regressions present and green"
# Do not silently lose the iOS handoff regression coverage: read queries must
# not feed a resync loop or starve controls, while real queue writes serialize.
# Wire-to-controller tests also cover proto3 item zero, takeback, seek and mute;
# manual skip fixtures cover shuffle/loop/position/autoplay boundaries.
n=$(cargo test --manifest-path crates/Cargo.toml -p qconnect-app --lib -- --list controller_smoke:: 2>/dev/null \
    | grep -c ': test$' || true)
(( n >= 13 )) || { echo "QConnect controller smoke suite has $n tests (expected >= 13)"; exit 1; }
n=$(cargo test --manifest-path crates/Cargo.toml -p qconnect-app --lib -- --list controller_takeover:: 2>/dev/null \
    | grep -c ': test$' || true)
(( n >= 5 )) || { echo "QConnect takeover suite has $n tests (expected >= 5)"; exit 1; }
n=$(cargo test --manifest-path crates/Cargo.toml -p qconnect-app --lib -- --list queue_resolution::tests::manual_skip 2>/dev/null \
    | grep -c ': test$' || true)
(( n >= 6 )) || { echo "QConnect manual skip suite has $n tests (expected >= 6)"; exit 1; }
n=$(cargo test --manifest-path crates/Cargo.toml -p qconnect-protocol --lib -- --list decoder::tests::controller_ 2>/dev/null \
    | grep -c ': test$' || true)
(( n >= 3 )) || { echo "QConnect controller wire suite has $n tests (expected >= 3)"; exit 1; }
n=$(cargo test --manifest-path crates/Cargo.toml -p qbz-log --lib -- --list repeat::tests:: 2>/dev/null \
    | grep -c ': test$' || true)
(( n >= 8 )) || { echo "consecutive log suite has $n tests (expected >= 8)"; exit 1; }
cargo test --manifest-path crates/Cargo.toml -p qconnect-app --lib -- controller_smoke::
cargo test --manifest-path crates/Cargo.toml -p qconnect-app --lib -- controller_takeover::
cargo test --manifest-path crates/Cargo.toml -p qconnect-app --lib -- queue_resolution::tests::manual_skip
cargo test --manifest-path crates/Cargo.toml -p qconnect-protocol --lib -- decoder::tests::controller_
cargo test --manifest-path crates/Cargo.toml -p qbz-log --lib -- repeat::tests::

say "gate: qbzd resolves no Slint crate"
hits=$(cargo tree --manifest-path crates/Cargo.toml -p qbzd -e normal \
       | grep -E '\b(slint|qbz-ui|qbz-slint-common|qbz-dac-wizard) v' || true)
[[ -z "$hits" ]] || { echo "qbzd graph resolves Slint crates:"; echo "$hits"; exit 1; }

[[ "${QT:-0}" == 1 ]] || { say "done (set QT=1 for the Qt gates)"; exit 0; }

say "qt gate 1/5: QML audits"
for a in qml_resolution_audit qml_singleton_xref qml_eager_tab_audit qml_module_registration_audit qml_icon_bake_audit qml_on_prefixed_property_audit; do
  python3 "scripts/qml-audits/$a.py" "$ROOT/crates/qbz-qt"
done

say "qt gate 2/5: shader bake gate"
scripts/qml-audits/shader_bake_gate.sh "${QSB:-$(command -v qsb || echo /usr/lib64/qt6/bin/qsb)}"

say "qt gate 3/5: qbz-qt resolves no Slint crate"
hits=$(cargo tree --manifest-path crates/Cargo.toml -p qbz-qt -e normal \
       | grep -E '\b(slint|qbz-ui|qbz-slint-common|qbz-dac-wizard) v' || true)
[[ -z "$hits" ]] || { echo "qbz-qt graph resolves Slint crates:"; echo "$hits"; exit 1; }

say "qt gates 4/5 + 5/5: tests and debug/release startup liveness"
bash scripts/qt-runtime-gate.sh
say "done"
