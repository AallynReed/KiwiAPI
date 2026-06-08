#!/usr/bin/env bash
#
# Load a shifted MarketListing ndjson into the Mongo container.
#
# Run AFTER ``market_shift_demo_data.py`` produced the ndjson:
#
#   python scripts/market_shift_demo_data.py \
#       --input  /path/to/trove_api.MarketListing.json \
#       --output /path/to/trove_api.MarketListing.shifted.ndjson
#
#   ./scripts/market-load-demo.sh /path/to/trove_api.MarketListing.shifted.ndjson
#
# Pipes the file through ``mongoimport`` inside the compose container — no
# Python loader, no UUID decoding glue: mongoimport reads MongoDB Extended
# JSON natively and the ``{"$binary":{"base64":"...","subType":"04"}}``
# envelope decodes back into a BSON UUID without any custom code.
#
# Set FORCE=1 to skip the confirmation prompt.

set -euo pipefail

NDJSON="${1:?usage: market-load-demo.sh <shifted.ndjson>}"
[[ -s "$NDJSON" ]] || { echo "ndjson not found or empty: $NDJSON" >&2; exit 1; }

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$PROJECT_DIR"

if [[ "${FORCE:-0}" != "1" ]]; then
  read -r -p "DROP and replace market_listings with '$NDJSON'? Type 'yes': " ans
  [[ "$ans" == "yes" ]] || { echo "aborted"; exit 1; }
fi

echo "[$(date -Is)] importing $NDJSON → market_listings"
# -T disables TTY allocation so stdin pipes through. Auth runs against the
# admin DB; the root creds live in the container's env from compose.
# MONGO_INITDB_DATABASE is the DB name we initialise into and is what the
# app uses ($MONGO_DB) — we read it from inside the container so changes
# to the .env file flow through automatically.
docker compose exec -T mongo sh -c '
  mongoimport \
    --username "$MONGO_INITDB_ROOT_USERNAME" \
    --password "$MONGO_INITDB_ROOT_PASSWORD" \
    --authenticationDatabase admin \
    --db "$MONGO_INITDB_DATABASE" \
    --collection market_listings \
    --type json \
    --drop \
    --numInsertionWorkers 4 \
    --stopOnError
' < "$NDJSON"

echo "[$(date -Is)] import complete"
