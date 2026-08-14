#!/usr/bin/env bash
#
# deploy.sh - update, rebuild and (re)start the Kiwi API stack.
#
# /opt/trove is a git checkout, so this is the single update command: it pulls
# origin/main, re-minifies the static assets, rebuilds and recreates. A failed
# pull aborts rather than silently deploying the old tree.
#
# Usage:
#   ./deploy.sh                   pull, build changed images, recreate containers
#   ./deploy.sh --no-pull         build the tree as-is, skip the git pull
#   ./deploy.sh --force-recreate  recreate ALL containers even if unchanged (picks up .env edits)
#   ./deploy.sh --logs            ... then follow logs
#   ./deploy.sh --no-build        just recreate containers (skip image rebuild)
#   ./deploy.sh --no-minify       skip the automatic CSS/JS minify step
#   ./deploy.sh --pull            also pull newer base images (mongo, nginx) first
#   ./deploy.sh --no-prune        keep old images + build cache (skip the post-deploy cleanup)
#
# After a successful (re)create, the script cleans up by default: it removes
# DANGLING images (the old app image each rebuild leaves behind) and caps the
# BuildKit cache so it can't balloon. It deliberately does NOT run
# `docker system prune -a` - that would also delete images for any service not
# currently running (adminer/mailpit/etc.), forcing re-pulls. The cleanup runs
# only after the stack is up, so running images are always safe.
#
# Static assets are minified automatically (site/static/*.css|js -> *.min.*) using a
# small tools venv at ~/.cache/trove-tools-venv (override with TOOLS_VENV=...), created
# once. Env: SKIP_GIT=1 to skip the automatic 'git pull'.
#
# The static nginx containers (portal, docs) are ALWAYS force-recreated, so a
# replaced bind-mounted directory can never leave them serving a stale/empty
# mount (403).
#
set -euo pipefail

# Always run from the directory this script lives in.
cd "$(dirname "$(readlink -f "$0")")"

BUILD=1; FOLLOW=0; PULL=0; PRUNE=1; FORCE=0; MINIFY=1; PULLSRC=1
for arg in "$@"; do
  case "$arg" in
    --no-build)       BUILD=0 ;;
    --logs)           FOLLOW=1 ;;
    --pull)           PULL=1 ;;
    --prune)          PRUNE=1 ;;   # default; kept for back-compat
    --no-prune)       PRUNE=0 ;;
    --force-recreate) FORCE=1 ;;
    --no-minify)      MINIFY=0 ;;
    --no-pull)        PULLSRC=0 ;;   # build the tree as-is, skip 'git pull'
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

# /opt/trove is a git checkout, so a deploy IS an update: pull first, then build
# from what we just pulled. One command updates and restarts the stack.
#
# A failed pull is FATAL rather than a warning. Continuing on failure would build
# the old tree while printing "deploy complete", which looks identical to a
# successful update - the whole point of this step is that you can trust the
# running code matches origin/main. Skip deliberately with --no-pull, and set
# FETCH_GIT=0 to turn it off for a host that is not a checkout.
if [ -d .git ] && [ "$PULLSRC" -eq 1 ] && [ "${FETCH_GIT:-1}" = "1" ]; then
  git config --global --add safe.directory "$(pwd)" 2>/dev/null || true
  echo ">> git pull --ff-only"
  if ! git pull --ff-only; then
    echo "ERROR: git pull failed - refusing to deploy stale code." >&2
    echo "       Local commits or a dirty tree block a fast-forward; resolve on the" >&2
    echo "       server (git status), or re-run with --no-pull to build as-is." >&2
    exit 1
  fi
elif [ ! -d .git ]; then
  echo ">> not a git checkout - building from the local tree"
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
    # owner so a later edit can overwrite them without a permission clash.
    chown --reference=site/static site/static/*.min.* 2>/dev/null || true
  fi
fi

# The .min.* files are build output and are no longer tracked in git, so a fresh
# checkout has none until minify runs. Every template hardcodes /static/*.min.js
# with no unminified fallback, so shipping without them serves pages whose CSS and
# JS all 404. "minify failed, keep the existing files" only works when there ARE
# existing files - check, and refuse to deploy a stylesheet-less site.
if ! ls site/static/*.min.js >/dev/null 2>&1 || ! ls site/static/*.min.css >/dev/null 2>&1; then
  echo "ERROR: no site/static/*.min.js|css present and minify did not produce any." >&2
  echo "       Templates reference these directly - deploying now serves pages with" >&2
  echo "       no CSS or JS. Fix the minify venv (see above) or run:" >&2
  echo "         python scripts/minify_static.py" >&2
  exit 1
fi

# nginx serves the docs site + dev portal straight off these bind-mounted trees
# (worker uid 101). A deploy write under a tight umask or an inherited ACL can
# leave fresh files unreadable to nginx -> 403 across the whole static site.
# They're public, so normalise to world-readable on every deploy.
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

# portal + docs are long-lived static nginx containers that bind-mount host dirs
# (./portal/html, ./docs). If a deploy REPLACES one of those directories, it gets a
# new inode while the container keeps serving the OLD (now-empty) one -> nginx 403s
# the whole site (directory-index-forbidden) until it's recreated. A plain `up -d`
# won't fix it: their config is unchanged, so compose leaves them running on the
# stale mount. Force-recreate them every deploy so they always re-resolve the bind
# mount to the current tree - cheap (stock nginx:alpine, nothing to build). Skipped
# when --force-recreate already did it.
if [ "$FORCE" -ne 1 ]; then
  echo ">> refreshing static containers (portal + docs) so their bind mounts re-resolve"
  $COMPOSE up -d --no-deps --force-recreate portal docs
fi

if [ "$PRUNE" -eq 1 ]; then
  echo ">> cleaning up old images + build cache"
  # Dangling images = the previous app image, left untagged by each rebuild.
  # Safe: nothing references them once the new image is tagged + running.
  docker image prune -f >/dev/null 2>&1 || true
  # Cap the BuildKit cache so it can't balloon over time, but KEEP recent layers
  # so the next rebuild stays fast. --keep-storage was RENAMED to --max-used-space
  # in buildkit 0.32: the old flag still ran but pruned the cache to nothing, so
  # every deploy left the next one re-exporting the whole image (~60s of "exporting
  # layers" on a build with zero changes). Try the current flag first.
  docker builder prune -f --max-used-space=10GB >/dev/null 2>&1 \
    || docker builder prune -f --keep-storage=10GB >/dev/null 2>&1 || true
fi

echo
$COMPOSE ps
echo
echo "Done. api -> 127.0.0.1:15546 (api/dev hostnames)   docs -> 127.0.0.1:25468"

if [ "$FOLLOW" -eq 1 ]; then
  echo ">> following logs (Ctrl-C to stop)"
  $COMPOSE logs -f --tail=50
fi
