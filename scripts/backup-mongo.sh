#!/usr/bin/env bash
#
# Mongo backup for Kiwi API. Run from anywhere — it cd's to the project root
# (the directory containing docker-compose.yml). Dumps the app database to a
# gzipped archive and prunes old local backups.
#
# Credentials are read from the *running mongo container's* environment, so this
# script never parses .env on the host (avoids quoting/`$` pitfalls).
#
# Schedule daily via cron, e.g.:
#   0 3 * * *  cd /opt/trove && ./scripts/backup-mongo.sh >> ./backups/backup.log 2>&1
#
# ── OFF-BOX (do not skip) ───────────────────────────────────────────────────
# This writes backups locally only. A local backup does NOT survive a disk loss.
# Add one off-box copy after the dump (uncomment + adapt the TODO at the bottom):
#   rsync -a "$OUT" backups@otherhost:/srv/kiwi-backups/      # another machine
#   restic backup "$OUT"                                      # or rclone -> object storage
# ────────────────────────────────────────────────────────────────────────────

set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$PROJECT_DIR"

DB="${MONGO_DB:-kiwi}"
RETENTION_DAYS="${BACKUP_RETENTION_DAYS:-14}"
BACKUP_DIR="${BACKUP_DIR:-$PROJECT_DIR/backups}"

mkdir -p "$BACKUP_DIR"
STAMP="$(date +%Y%m%d-%H%M%S)"
OUT="$BACKUP_DIR/kiwi-$DB-$STAMP.archive.gz"

echo "[$(date -Is)] dumping '$DB' -> $OUT"
# -T: no pseudo-TTY, so the gzip archive streamed to stdout stays uncorrupted.
# Creds come from the container's own MONGO_INITDB_* env (single-quoted so the
# host shell doesn't expand them).
docker compose exec -T mongo sh -c \
  'mongodump --username "$MONGO_INITDB_ROOT_USERNAME" --password "$MONGO_INITDB_ROOT_PASSWORD" --authenticationDatabase admin --db "${MONGO_INITDB_DATABASE:-kiwi}" --archive --gzip' \
  > "$OUT"

# Refuse to keep an empty/truncated dump (e.g. if the container was down).
if [[ ! -s "$OUT" ]]; then
  echo "[$(date -Is)] ERROR: backup is empty — removing and failing" >&2
  rm -f "$OUT"
  exit 1
fi

echo "[$(date -Is)] ok: $(du -h "$OUT" | cut -f1)"

# Prune local backups older than the retention window.
find "$BACKUP_DIR" -name 'kiwi-*.archive.gz' -mtime +"$RETENTION_DAYS" -print -delete

# TODO(off-box): copy "$OUT" off this machine here. See the header for examples.
echo "[$(date -Is)] done"
