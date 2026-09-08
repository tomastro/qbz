#!/usr/bin/env bash
set -euo pipefail

qt_root=${1:?usage: qmllint_gate.sh QT_ROOT_DIR}
qml_root="crates/qbz-qt/qml"
native_qmldir="scripts/qml-audits/qbz-native-types/qmldir"

lint="$qt_root/bin/qmllint"
if [[ ! -x "$lint" && -x "$lint.exe" ]]; then
  lint="$lint.exe"
fi
if [[ ! -x "$lint" ]]; then
  echo "::error::qmllint not found under $qt_root/bin"
  exit 1
fi

if command -v python3 >/dev/null 2>&1; then
  python_cmd=python3
else
  python_cmd=python
fi
"$python_cmd" scripts/qml-audits/qml_native_types_audit.py

fail=0
while IFS= read -r -d '' file; do
  out=$("$lint" -I "$qt_root/qml" -I "$qml_root" \
    -i "$native_qmldir" "$file" 2>&1 || true)
  # Limit the gate to unresolved types/properties. Hand-written bridge
  # singletons have no generated qmltypes metadata yet, so their member
  # findings remain filtered by bridge name.
  hits=$(printf '%s\n' "$out" \
    | grep -E 'was not found|Could not find property' \
    | grep -vE 'Qbz(Shell|Session|Bridge|Player|Mini|About|Local|Home|Tray)\b' || true)
  if [[ -n "$hits" ]]; then
    echo "::group::$file"
    printf '%s\n' "$hits"
    echo "::endgroup::"
    fail=1
  fi
done < <(find "$qml_root" -name '*.qml' -print0)

if [[ "$fail" -ne 0 ]]; then
  echo "::error::qmllint found unresolved types or properties"
  exit 1
fi
echo "qmllint clean"
