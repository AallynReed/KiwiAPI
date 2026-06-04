#!/usr/bin/env bash
#
# deploy.sh - rebuild and (re)start the Kiwi API stack with the current code.
#
# Usage:
#   ./deploy.sh             build changed images, recreate containers, show status
#   ./deploy.sh --logs      ... then follow logs
#   ./deploy.sh --no-build  just recreate containers (skip image rebuild)
#   ./deploy.sh --pull      also pull newer base images (mongo, nginx) first
#   ./deploy.sh --prune     remove dangling images afterwards
#
# Env: SKIP_GIT=1 to skip the automatic 'git pull'.
#
set -euo pipefail

# Always run from the directory this script lives in.
cd "$(dirname "$(readlink -f "$0")")"

BUILD=1; FOLLOW=0; PULL=0; PRUNE=0
for arg in "$@"; do
  case "$arg" in
    --no-build) BUILD=0 ;;
    --logs)     FOLLOW=1 ;;
    --pull)     PULL=1 ;;
    --prune)    PRUNE=1 ;;
    -h|--help)  grep '^#' "$0" | sed 's/^#\{1,\} \{0,1\}//'; exit 0 ;;
    *) echo "Unknown option: $arg (try --help)" >&2; exit 2 ;;
  esac
done

# Pick the available compose command.
if docker compose version >/dev/null 2>&1; then
  COMPOSE="docker compose"
elif command -v docker-compose >/dev/null 2>&1; then
  COMPOSE="docker-compose"
else
  echo "ERROR: Docker Compose not found. Is Docker installed and are you in the 'docker' group?" >&2
  exit 1
fi

if [ ! -f .env ]; then
  echo "WARNING: no .env found - using compose defaults (insecure SECRET_KEY, no admin, no email)." >&2
fi

# Update code from git if this is a checkout (skip with SKIP_GIT=1).
if [ -d .git ] && [ "${SKIP_GIT:-0}" != "1" ]; then
  echo ">> git pull"
  git pull --ff-only || echo "   (git pull skipped/failed; continuing with local code)"
fi

if [ "$PULL" -eq 1 ]; then
  echo ">> pulling base images"
  $COMPOSE pull --ignore-pull-failures mongo docs || true
fi

echo ">> (re)creating the stack"
if [ "$BUILD" -eq 1 ]; then
  $COMPOSE up -d --build --remove-orphans
else
  $COMPOSE up -d --remove-orphans
fi

if [ "$PRUNE" -eq 1 ]; then
  echo ">> pruning dangling images"
  docker image prune -f >/dev/null || true
fi

echo
$COMPOSE ps
echo
echo "Done. api -> 127.0.0.1:15546 (api/dev hostnames)   docs -> 127.0.0.1:25468"

if [ "$FOLLOW" -eq 1 ]; then
  echo ">> following logs (Ctrl-C to stop)"
  $COMPOSE logs -f --tail=50
fi
