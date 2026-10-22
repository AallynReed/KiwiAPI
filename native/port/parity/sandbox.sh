#!/usr/bin/env bash
# Write parity on Zakros: throwaway Mongo/Redis/Postgres with a sandbox api-py
# and kiwi-server in front of them. No background loops, no production secrets
# or data. Run from /opt/trove after `docker compose build api`.
#
#   native/port/parity/sandbox.sh up
#   native/port/parity/sandbox.sh seed native/port/parity/leaf.seed.js   (mongosh script)
#   native/port/parity/sandbox.sh run native/port/parity/leaf.writes.json [...]
#   native/port/parity/sandbox.sh down
set -euo pipefail

NET=kiwi-parity
IMG=${IMG:-trove-app:latest}
LOOPS=usage,email_outbox,news,feeds,trove_events,mod_stats,steam_workshop,chaos,status_prober,delves,btt_releases,update_archiver,leaderboards_warmer,giveaways,event_listener,event_watch,event_scheduler,webhooks,dm_subs,maintenance
ENV=(
  -e MONGO_URI=mongodb://kp-mongo:27017/kiwi -e MONGO_DB=kiwi
  -e REDIS_URL=redis://kp-redis:6379/0
  -e POSTGRES_DSN=postgresql://kiwi:kiwi@kp-pg:5432/kiwi_lb
  -e SECRET_KEY=parity-sandbox-secret-not-for-production
  -e ADMIN_EMAIL=parity@aallyn.net -e ADMIN_PASSWORD=Parity-sandbox-pass-1
  -e GAMEDATA_DIR=/data/gamedata
)

wait_http() {
  for _ in $(seq 1 90); do
    docker exec kp-rs python -c "import urllib.request,sys;urllib.request.urlopen(sys.argv[1],timeout=2)" "$1" >/dev/null 2>&1 && return 0
    sleep 1
  done
  echo "timed out waiting for $1" >&2
  return 1
}

up() {
  docker network create "$NET" >/dev/null 2>&1 || true
  docker run -d --rm --name kp-mongo --network "$NET" mongo:7 >/dev/null
  docker run -d --rm --name kp-redis --network "$NET" redis:7-alpine >/dev/null
  docker run -d --rm --name kp-pg --network "$NET" -e POSTGRES_USER=kiwi -e POSTGRES_PASSWORD=kiwi \
    -e POSTGRES_DB=kiwi_lb postgres:17 >/dev/null
  sleep 5
  # Python's interest-item seed has a duplicate name and crashes an empty database.
  docker exec kp-mongo mongosh --quiet kiwi --eval 'db.market_interest_items.insertOne({name: "Parity Placeholder", added_by: null, added_at: new Date()})' >/dev/null
  # RUST_OWNS names every loop, so the sandbox Python starts none of them.
  docker run -d --rm --name kp-py --network "$NET" "${ENV[@]}" -e RUST_OWNS="$LOOPS" \
    -v "$PWD/app:/app/app:ro" -v "$PWD/.gamedata:/data/gamedata:ro" -v "$PWD/site:/app/site:ro" \
    "$IMG" uvicorn app.main:app --host 0.0.0.0 --port 8000 --proxy-headers --forwarded-allow-ips '*' >/dev/null
  docker run -d --rm --name kp-rs --network "$NET" "${ENV[@]}" -e RUST_OWNS= \
    -e PY_UPSTREAM=http://kp-py:8000 -e KIWI_BIND=0.0.0.0:8000 \
    -v "$PWD/app:/app/app:ro" -v "$PWD/.gamedata:/data/gamedata:ro" -v "$PWD/site:/app/site:ro" \
    "$IMG" kiwi-server >/dev/null
  wait_http http://kp-py:8000/health
  wait_http http://127.0.0.1:8000/health
  docker cp native/port/parity/parity_tokens.py kp-py:/tmp/parity_tokens.py
  docker exec -e PYTHONPATH=/app kp-py python /tmp/parity_tokens.py mint > /tmp/kp_env.sh
  echo "sandbox up; tokens in /tmp/kp_env.sh"
}

seed() {
  for f in "$@"; do
    docker cp "$f" kp-mongo:/tmp/seed.js
    docker exec kp-mongo mongosh --quiet kiwi /tmp/seed.js
  done
}

run() {
  docker exec kp-rs mkdir -p /tmp/p
  docker cp native/port/parity/run.py kp-rs:/tmp/p/run.py
  set -a; . /tmp/kp_env.sh; set +a
  local rc=0
  for spec in "$@"; do
    docker cp "$spec" kp-rs:/tmp/p/spec.json
    echo "######## $spec"
    docker exec -e KIWI_PARITY_TOKEN -e KIWI_PARITY_TOKEN_NO_GEMS -e KIWI_PARITY_JWT kp-rs \
      python /tmp/p/run.py --allow-writes /tmp/p/spec.json http://kp-py:8000 http://127.0.0.1:8000 || rc=1
  done
  return $rc
}

down() {
  docker rm -f kp-rs kp-py kp-pg kp-redis kp-mongo >/dev/null 2>&1 || true
  docker network rm "$NET" >/dev/null 2>&1 || true
  rm -f /tmp/kp_env.sh
}

cmd=${1:-}; shift || true
case "$cmd" in
  up) up ;;
  seed) seed "$@" ;;
  run) run "$@" ;;
  down) down ;;
  *) echo "usage: $0 up|seed FILE...|run SPEC...|down" >&2; exit 2 ;;
esac
