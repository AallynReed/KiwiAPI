#!/usr/bin/env bash
# Write parity on Zakros. Run from /opt/trove after `docker compose build api`.
# Only the kp-* containers and kiwi-parity* networks below are created or
# removed: never a trove_* container, the trove_default network or a production
# data directory (app, .gamedata and site are mounted read-only).
#
# Single sandbox: throwaway Mongo/Redis/Postgres shared by a sandbox api-py and
# kiwi-server, for writes whose replies are all that matters.
#
#   native/port/parity/sandbox.sh up
#   native/port/parity/sandbox.sh seed native/port/parity/leaf.seed.js   (mongosh script)
#   native/port/parity/sandbox.sh run native/port/parity/leaf.writes.json [...]
#   native/port/parity/sandbox.sh down
#
# Twin sandbox: two isolated stacks, each on its own internal network (no way
# out) with the same env and sandbox secrets, every loop named in RUST_OWNS.
# A = mongo:7 + redis:7-alpine + postgres:17 + api-py (pure Python); B = the
# same stores + api-py + kiwi-server in front. The runner kp-twin sits on both
# networks with this folder mounted read-only: it sends every request to A,
# then B, and compares what the stores hold afterwards. AREA is a spec
# basename (leaf) or a spec path under this folder.
#
#   native/port/parity/sandbox.sh twin-up [AREA]      # fresh stacks (replaces any old twin)
#   native/port/parity/sandbox.sh twin-seed AREA      # AREA.seed.js/.seed.sql, restart, tokens
#   native/port/parity/sandbox.sh twin-run AREA...    # AREA.writes.json (or a spec path)
#   native/port/parity/sandbox.sh twin-state AREA     # AREA.state.json: diff the stores
#   native/port/parity/sandbox.sh twin-down
#   native/port/parity/sandbox.sh twin AREA...        # all of it per area on fresh stacks
#
# twin-seed runs AREA.seed.js in A's Mongo, then copies A's whole kiwi database
# into B (mongodump | mongorestore), so the admin user, the parity tokens and
# Python's first-boot documents are byte-identical in both; AREA.seed.sql runs
# in both Postgres (keep it deterministic). The apps restart afterwards, so no
# cache predates the seed. Tokens stay in the runner (/tmp/a.env, /tmp/b.env).
# Both stacks see the client CLIENT_IP (default 203.0.113.7) and Host
# kiwi-parity unless a request sets its own.
#
# AREA.loops (optional, comma list) names loops the area moves: A's Python runs
# them, B's kiwi-server owns them, B's Python still runs none.
#
# IMG (default trove-app:latest) runs every container; RS_IMG overrides it for
# B's kiwi-server alone, so a branch build is tested without retagging the
# production image. Wait budget per service: WAIT seconds (default 180).
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

# ------------------------------------------------------------------ twin

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
ROOT=$(cd "$HERE/../../.." && pwd -P)
RS_IMG=${RS_IMG:-$IMG}
CLIENT_IP=${CLIENT_IP:-203.0.113.7}
RUNNER=kp-twin
TWIN=(kp-twin kp-b-rs kp-b-py kp-a-py kp-b-pg kp-b-redis kp-b-mongo kp-a-pg kp-a-redis kp-a-mongo)
# Same hostnames as production compose, resolved inside each stack's own network.
TWIN_ENV=(
  -e MONGO_URI=mongodb://mongo:27017/kiwi -e MONGO_DB=kiwi
  -e REDIS_URL=redis://redis:6379/0
  -e POSTGRES_DSN=postgresql://kiwi:kiwi@postgres:5432/kiwi_lb
  -e SECRET_KEY=parity-sandbox-secret-not-for-production
  -e INTERNAL_KEY=parity-sandbox-internal-key
  -e ADMIN_EMAIL=parity@aallyn.net -e ADMIN_PASSWORD=Parity-sandbox-pass-1
  -e GAMEDATA_DIR=/data/gamedata
)
TWIN_MOUNTS=(-v "$ROOT/app:/app/app:ro" -v "$ROOT/.gamedata:/data/gamedata:ro" -v "$ROOT/site:/app/site:ro")
PLACEHOLDER='db.market_interest_items.insertOne({_id: ObjectId("000000000000000000000001"), name: "Parity Placeholder", added_by: null, added_at: ISODate("2026-01-01T00:00:00Z")})'

retry() {
  local i
  for ((i = 0; i < ${WAIT:-180}; i++)); do
    "$@" >/dev/null 2>&1 && return 0
    sleep 1
  done
  echo "timed out: $*" >&2
  return 1
}

probe() {
  docker exec "$RUNNER" python -c 'import sys, urllib.request; urllib.request.urlopen(sys.argv[1], timeout=2)' "$1"
}

twin_wait() {
  retry probe http://kp-a-py:8000/health
  retry probe http://kp-b-py:8000/health
  retry probe http://kp-b-rs:8000/health
}

twin_stores() {
  local s=$1 net=kiwi-parity-$1
  docker network create --internal "$net" >/dev/null
  docker run -d --name "kp-$s-mongo" --network "$net" --network-alias mongo --memory 1g mongo:7 >/dev/null
  docker run -d --name "kp-$s-redis" --network "$net" --network-alias redis --memory 256m redis:7-alpine >/dev/null
  docker run -d --name "kp-$s-pg" --network "$net" --network-alias postgres --memory 1g --shm-size 256m \
    -e POSTGRES_USER=kiwi -e POSTGRES_PASSWORD=kiwi -e POSTGRES_DB=kiwi_lb -e POSTGRES_HOST_AUTH_METHOD=trust \
    postgres:17 >/dev/null
}

moved_loops() {
  local f
  f=$HERE/$(area "$1").loops
  if [ -f "$f" ]; then tr -d ' \r\n' < "$f"; fi
}

loops_without() {
  local out='' l all
  IFS=, read -ra all <<< "$LOOPS"
  for l in "${all[@]}"; do
    case ",$1," in *",$l,"*) ;; *) out=${out:+$out,}$l ;; esac
  done
  printf '%s\n' "$out"
}

twin_py() {
  docker run -d --name "kp-$1-py" --network "kiwi-parity-$1" --network-alias api-py --memory 2g \
    "${TWIN_ENV[@]}" -e RUST_OWNS="${2:-$LOOPS}" "${TWIN_MOUNTS[@]}" "$IMG" \
    uvicorn app.main:app --host 0.0.0.0 --port 8000 --proxy-headers --forwarded-allow-ips '*' \
    --timeout-keep-alive 120 --timeout-graceful-shutdown 3 >/dev/null
}

twin_up() {
  local moved=${1:-}
  twin_down
  twin_stores a
  twin_stores b
  docker run -d --name "$RUNNER" --network kiwi-parity-a --memory 512m -v "$HERE:/parity:ro" "$IMG" sleep infinity >/dev/null
  docker network connect kiwi-parity-b "$RUNNER"
  local s
  for s in a b; do
    retry docker exec "kp-$s-mongo" mongosh --quiet --eval 'db.adminCommand({ping: 1})'
    retry docker exec "kp-$s-redis" redis-cli ping
    retry docker exec "kp-$s-pg" pg_isready -h 127.0.0.1 -U kiwi -d kiwi_lb
    # Python's interest-item seed has a duplicate name and crashes an empty database.
    docker exec "kp-$s-mongo" mongosh --quiet kiwi --eval "$PLACEHOLDER" >/dev/null
  done
  twin_py a "$(loops_without "$moved")"
  twin_py b
  docker run -d --name kp-b-rs --network kiwi-parity-b --memory 1g "${TWIN_ENV[@]}" -e RUST_OWNS="$moved" \
    -e KIWI_BIND=0.0.0.0:8000 -e PY_UPSTREAM=http://api-py:8000 -e FORWARDED_ALLOW_IPS='*' \
    "${TWIN_MOUNTS[@]}" "$RS_IMG" kiwi-server >/dev/null
  twin_wait
  echo "twin up; next: $0 twin-seed AREA"
}

area() {
  local b=${1##*/}
  printf '%s\n' "${b%%.*}"
}

runner_path() {
  local f
  f=$(realpath -e -- "$1") || return 1
  case $f in
    "$HERE"/*) printf '/parity/%s\n' "${f#"$HERE"/}" ;;
    *) echo "$1 is not under $HERE, the only folder the runner sees" >&2; return 1 ;;
  esac
}

twin_seed() {
  local a js sql
  a=$(area "$1")
  js=$HERE/$a.seed.js
  sql=$HERE/$a.seed.sql
  [ -f "$js" ] || [ -f "$sql" ] || echo "no $a.seed.js or $a.seed.sql: restarting and minting only"
  if [ -f "$js" ]; then
    docker cp "$js" kp-a-mongo:/tmp/seed.js
    docker exec kp-a-mongo mongosh --quiet kiwi /tmp/seed.js
  fi
  if [ -f "$sql" ]; then
    docker exec -i kp-a-pg psql -X -q -v ON_ERROR_STOP=1 -U kiwi -d kiwi_lb < "$sql"
    docker exec -i kp-b-pg psql -X -q -v ON_ERROR_STOP=1 -U kiwi -d kiwi_lb < "$sql"
  fi
  docker cp "$HERE/parity_tokens.py" kp-a-py:/tmp/parity_tokens.py
  docker exec -e PYTHONPATH=/app kp-a-py python /tmp/parity_tokens.py mint \
    | docker exec -i "$RUNNER" sh -c 'cat > /tmp/a.env && cp /tmp/a.env /tmp/b.env'
  docker exec kp-b-mongo mongosh --quiet kiwi --eval 'db.dropDatabase()' >/dev/null
  docker exec kp-a-mongo mongodump --quiet --archive --db=kiwi \
    | docker exec -i kp-b-mongo mongorestore --quiet --archive --drop
  docker restart kp-a-py kp-b-py kp-b-rs >/dev/null
  twin_wait
  echo "seeded $a; tokens: docker exec $RUNNER cat /tmp/a.env"
}

twin_run() {
  local rc=0 spec f
  docker exec "$RUNNER" test -s /tmp/a.env || { echo "no tokens: run twin-seed first" >&2; return 2; }
  for spec in "$@"; do
    case $spec in */* | *.json) f=$spec ;; *) f=$HERE/$spec.writes.json ;; esac
    f=$(runner_path "$f") || { rc=1; continue; }
    echo "######## $f"
    docker exec "$RUNNER" python "/parity/run.py" --allow-writes --env-a /tmp/a.env --env-b /tmp/b.env \
      --header "Host: kiwi-parity" --header "X-Forwarded-For: $CLIENT_IP" \
      "$f" http://kp-a-py:8000 http://kp-b-rs:8000 || rc=1
  done
  return $rc
}

twin_state() {
  local f
  f=$(runner_path "$HERE/$(area "$1").state.json") || return 2
  echo "######## $f"
  docker exec "$RUNNER" python /parity/statediff.py "$f" \
    --a mongodb://kp-a-mongo:27017/kiwi postgresql://kiwi@kp-a-pg:5432/kiwi_lb redis://kp-a-redis:6379/0 \
    --b mongodb://kp-b-mongo:27017/kiwi postgresql://kiwi@kp-b-pg:5432/kiwi_lb redis://kp-b-redis:6379/0
}

twin_down() {
  docker rm -fv "${TWIN[@]}" >/dev/null 2>&1 || true
  docker network rm kiwi-parity-a kiwi-parity-b >/dev/null 2>&1 || true
}

twin_all() {
  local rc=0 a
  for a in "$@"; do
    a=$(area "$a")
    echo "======== $a"
    twin_up "$(moved_loops "$a")"
    twin_seed "$a"
    twin_run "$a" || rc=1
    if [ -f "$HERE/$a.state.json" ]; then twin_state "$a" || rc=1; fi
  done
  if [ "$rc" = 0 ]; then twin_down; else echo "differences found; the last twin is still up (twin-down when done)"; fi
  return $rc
}

cmd=${1:-}; shift || true
case "$cmd" in
  up) up ;;
  seed) seed "$@" ;;
  run) run "$@" ;;
  down) down ;;
  twin-up) if [ $# -ge 1 ]; then twin_up "$(moved_loops "$1")"; else twin_up; fi ;;
  twin-seed) [ $# = 1 ] || { echo "usage: $0 twin-seed AREA" >&2; exit 2; }; twin_seed "$1" ;;
  twin-run) [ $# -ge 1 ] || { echo "usage: $0 twin-run AREA|SPEC..." >&2; exit 2; }; twin_run "$@" ;;
  twin-state) [ $# = 1 ] || { echo "usage: $0 twin-state AREA" >&2; exit 2; }; twin_state "$1" ;;
  twin-down) twin_down ;;
  twin) [ $# -ge 1 ] || { echo "usage: $0 twin AREA..." >&2; exit 2; }; twin_all "$@" ;;
  *) echo "usage: $0 up|seed FILE...|run SPEC...|down|twin-up|twin-seed AREA|twin-run AREA...|twin-state AREA|twin-down|twin AREA..." >&2; exit 2 ;;
esac
