#!/usr/bin/env bash
#
# Restore a Kiwi Mongo backup produced by backup-mongo.sh.
#
#   ./scripts/restore-mongo.sh backups/kiwi-kiwi-YYYYmmdd-HHMMSS.archive.gz
#
# WARNING: --drop replaces the current database with the archive's contents.
# Set FORCE=1 to skip the confirmation prompt (for automated DR drills).

set -euo pipefail

ARCHIVE="${1:?usage: restore-mongo.sh <archive.gz>}"
[[ -s "$ARCHIVE" ]] || { echo "archive not found or empty: $ARCHIVE" >&2; exit 1; }

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$PROJECT_DIR"

if [[ "${FORCE:-0}" != "1" ]]; then
  read -r -p "DROP and replace the live database from '$ARCHIVE'? Type 'yes': " ans
  [[ "$ans" == "yes" ]] || { echo "aborted"; exit 1; }
fi

echo "[$(date -Is)] restoring from $ARCHIVE"
docker compose exec -T mongo sh -c \
  'mongorestore --username "$MONGO_INITDB_ROOT_USERNAME" --password "$MONGO_INITDB_ROOT_PASSWORD" --authenticationDatabase admin --drop --archive --gzip' \
  < "$ARCHIVE"
echo "[$(date -Is)] restore complete"
