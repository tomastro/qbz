#!/usr/bin/env bash
set -euo pipefail

GLOBAL_DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/qbz"
LAST_USER_ID_FILE="${GLOBAL_DATA_DIR}/last_user_id"

USER_ID="${1:-}"
if [[ -z "${USER_ID}" ]]; then
  if [[ ! -f "${LAST_USER_ID_FILE}" ]]; then
    echo "[qbz] No last_user_id marker found at ${LAST_USER_ID_FILE}"
    echo "[qbz] Pass a user id explicitly: bash ./scripts/dev-reset-last-view.sh <user_id>"
    exit 1
  fi
  USER_ID="$(tr -d '[:space:]' < "${LAST_USER_ID_FILE}")"
fi

if [[ -z "${USER_ID}" ]]; then
  echo "[qbz] Could not determine the user id to reset."
  exit 1
fi

DB_PATH="${GLOBAL_DATA_DIR}/users/${USER_ID}/session.db"
if [[ ! -f "${DB_PATH}" ]]; then
  echo "[qbz] Session database not found: ${DB_PATH}"
  exit 1
fi

echo "[qbz] Resetting persisted last_view for user ${USER_ID}"
echo "[qbz] Database: ${DB_PATH}"

before_state="$(sqlite3 "${DB_PATH}" "SELECT last_view || '|' || COALESCE(view_context_id, '') || '|' || COALESCE(view_context_type, '') FROM player_state WHERE id = 1;")"
echo "[qbz] Before: ${before_state:-<missing>}"

sqlite3 "${DB_PATH}" <<'SQL'
UPDATE player_state
SET last_view = 'home',
    view_context_id = NULL,
    view_context_type = NULL
WHERE id = 1;
SQL

after_state="$(sqlite3 "${DB_PATH}" "SELECT last_view || '|' || COALESCE(view_context_id, '') || '|' || COALESCE(view_context_type, '') FROM player_state WHERE id = 1;")"
echo "[qbz] After: ${after_state:-<missing>}"
