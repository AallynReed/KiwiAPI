#!/usr/bin/env bash
#
# deploy.sh - rebuild and (re)start the Kiwi API stack with the current code.
#
# Usage:
#   ./deploy.sh                   build changed images, recreate containers, show status
#   ./deploy.sh --force-recreate  recreate ALL containers even if unchanged (picks up .env edits)
#   ./deploy.sh --logs            ... then follow logs
#   ./deploy.sh --no-build        just recreate containers (skip image rebuild)
#   ./deploy.sh --no-minify       skip the automatic CSS/JS minify step
#   ./deploy.sh --pull            also pull newer base images (mongo, nginx) first
#   ./deploy.sh --prune           remove dangling images afterwards
#
# Static assets are minified automatically (site/static/*.css|js -> *.min.*) using a
# small tools venv at ~/.cache/trove-tools-venv (override with TOOLS_VENV=...), created
# once. Env: SKIP_GIT=1 to skip the automatic 'git pull'.
#
set -euo pipefail

# Always run from the directory this script lives in.
cd "$(dirname "$(readlink -f "$0")")"

BUILD=1; FOLLOW=0; PULL=0; PRUNE=0; FORCE=0; MINIFY=1
for arg in "$@"; do
  case "$arg" in
    --no-build)       BUILD=0 ;;
    --logs)           FOLLOW=1 ;;
    --pull)           PULL=1 ;;
    --prune)          PRUNE=1 ;;
    --force-recreate) FORCE=1 ;;
    --no-minify)      MINIFY=0 ;;
    -h|--help)        grep '^#' "$0" | sed 's/^#\{1,\} \{0,1\}//'; exit 0 ;;
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

# NOTE: deploy is Syncthing-based - the source is already synced to this host, so
# we build straight from the local tree and do NOT touch git. (Set FETCH_GIT=1 to
# opt back into a 'git pull' if you ever run this on a plain git checkout.)
if [ -d .git ] && [ "${FETCH_GIT:-0}" = "1" ]; then
  git config --global --add safe.directory "$(pwd)" 2>/dev/null || true
  echo ">> git pull (FETCH_GIT=1)"
  git pull --ff-only || echo "   (git pull skipped/failed; continuing with local code)"
fi

# Regenerate minified CSS/JS from source so nobody has to hand-minify. The tools
# (csscompressor + rjsmin) live in a tiny venv kept OUTSIDE the repo, so it's never
# synced or committed; it's created once and reused. Non-fatal - a failure just
# leaves the existing *.min.* files in place. Skip with --no-minify.
if [ "$MINIFY" -eq 1 ] && [ -f scripts/minify_static.py ]; then
  TOOLS_VENV="${TOOLS_VENV:-$HOME/.cache/trove-tools-venv}"
  # The venv counts as usable only if it can actually IMPORT the tools. A
  # half-built venv (python present but python3-venv/ensurepip was missing, so
  # no packages got installed) passed the old "is there a python?" check and then
  # failed every minify - this checks the real thing and self-heals a broken one.
  venv_ok() { "$TOOLS_VENV/bin/python" -c "import csscompressor, rjsmin" >/dev/null 2>&1; }
  if ! venv_ok; then
    echo ">> setting up minify tools (one-time, $TOOLS_VENV)"
    rm -rf "$TOOLS_VENV"
    if ! ( python3 -m venv "$TOOLS_VENV" 2>/dev/null \
           && "$TOOLS_VENV/bin/pip" install --quiet --disable-pip-version-check rjsmin csscompressor 2>/dev/null ); then
      rm -rf "$TOOLS_VENV"   # don't leave a broken venv to poison the next run
      pyver="$(python3 -c 'import sys;print("%d.%d"%sys.version_info[:2])' 2>/dev/null || echo 3)"
      echo "   (couldn't build the minify venv - run:  apt install -y python${pyver}-venv  then re-deploy."
      echo "    Skipping minify; keeping the existing .min files.)"
    fi
  fi
  if venv_ok; then
    echo ">> minifying static assets"
    "$TOOLS_VENV/bin/python" scripts/minify_static.py \
      || echo "   (minify failed; keeping existing .min files)"
    # Deploy may run as root; keep the generated files owned by the source tree's
    # owner so a later Syncthing push / edit can overwrite them without a clash.
    chown --reference=site/static site/static/*.min.* 2>/dev/null || true
  fi
fi

# nginx serves the docs site + dev portal straight off these bind-mounted trees
# (worker uid 101). A git pull / Syncthing write under a tight umask or an
# inherited ACL can leave fresh files unreadable to nginx -> 403 across the whole
# static site. They're public, so normalise to world-readable on every deploy.
echo ">> normalising static perms (docs + portal)"
chmod -R a+rX portal/html docs 2>/dev/null || true

if [ "$PULL" -eq 1 ]; then
  echo ">> pulling base images"
  $COMPOSE pull --ignore-pull-failures mongo docs || true
fi

echo ">> (re)creating the stack"
UP_ARGS="up -d --remove-orphans"
if [ "$BUILD" -eq 1 ]; then UP_ARGS="$UP_ARGS --build"; fi
if [ "$FORCE" -eq 1 ]; then UP_ARGS="$UP_ARGS --force-recreate"; fi
# shellcheck disable=SC2086  # intentional word-split of the flag list
$COMPOSE $UP_ARGS

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
