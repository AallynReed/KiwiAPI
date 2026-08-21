# Kiwi API

A developer-API platform with live **Trove** game data, plus the website and backend
behind [Better Trove Tools](https://trove.aallyn.net).

Developers sign up in a browser portal, mint scoped, rate-limited API tokens, and call
read-only data endpoints with them. Underneath is a reusable platform — accounts, tokens,
rate limiting, usage metrics, email, and admin — with the Trove data surfaces layered on top.

Fully dockerized. **MongoDB** + **Redis** + **PostgreSQL**, all persisted to local folders.

---

## Links

| | |
|---|---|
| **API** | `https://api.aallyn.net` — `/v1/*` + `/health` |
| **Developer portal** | `https://dev.aallyn.net` — sign up, mint tokens, view usage |
| **API reference** | `https://docs.aallyn.net` — interactive docs · `GET /openapi.json` is always current · `/llms.txt` for AI tools |
| **Showcase site** | `https://trove.aallyn.net` — the Better Trove Tools website |

## Stack

Python 3.13 · FastAPI + Uvicorn · Beanie/PyMongo on MongoDB 7 · Redis 7 · PostgreSQL
(leaderboards, market, codexes) · Argon2 + JWT auth · hashed API tokens.

## Platform features

- **Accounts** — signup with captcha + anti-spam + breach check, email verification,
  password reset, GDPR export and account deletion.
- **Sessions** — short-lived access tokens with rotating refresh tokens and instant
  global revocation ("log out everywhere").
- **API tokens** — scoped (Discord-style bitmask), IP-allowlisted, expiring, rotatable.
  Self-validating `kiwi_<body>_<checksum>` format, and leaked tokens are auto-revoked
  via GitHub secret scanning.
- **Rate limiting** — Redis sliding-window with per-endpoint overrides and standard
  `X-RateLimit-*` / `Retry-After` headers.
- **Usage & site analytics** — every authenticated request is recorded and aggregated;
  cookieless page-view counts for the showcase site.
- **Admin** — superuser-only panel for users, tokens, usage, and feature toggles.
- Shared plumbing for new endpoints: cursor pagination, idempotency keys, request-id
  correlation, and one consistent error envelope.

## Data API (`/v1`)

Live Trove game data — all `GET`, read-only, with real-UTC unix timestamps. Most categories
are public (callable with **no token** at a stricter per-IP limit); a matching scope unlocks
the full per-token limit. See the [API reference](https://docs.aallyn.net) for every endpoint.

| Category | What it covers |
|---|---|
| `rotations` | server time, daily/weekly buffs, merchant timers, biomes, chaos chest, hourly challenge, calendar |
| `feeds` | Trove news, Twitch/YouTube/Bilibili, Trovesaurus events |
| `stats` · `gems` | stat tables and a stateless gem simulator / build optimizer |
| `leaderboards` · `activity` | hourly in-game boards (with cheat detection) and player/class activity estimates |
| `market` | in-game marketplace listings and price history |
| `store` | the in-game cash-shop catalog — products, prices, sales, deals, lootbox odds |
| `codexes` | decoded game catalogs — 18 collectible types, plus the relationships between them (what crafts what, what unlocks what), badge requirements and progression trees |
| `updates` | archived game-update files, browsable and diffable per patch |
| `mods` · `modpacks` | `.tmod` tooling plus a hub for sharing and bundling mods |
| `embed` | short-lived preview tokens for the embeddable 3D/VFX viewer other sites can iframe |
| `events` | a live SSE stream so clients stop polling on the hour |
| `ocr` | reads a character stat sheet from a screenshot (self-hosted, no LLM) |
| `btt` · `misc` | Better Trove Tools release feeds, time tools, server status, supporters |

## Showcase site

The API container also serves the Better Trove Tools website out of `site/` — the landing
page, user manual, command reference, and live pages for leaderboards, player/class activity,
codexes, the mods hub, modpacks, the marketplace, game updates, server status, and server time.
Browsing is public; developing and publishing mods is a Discord-login action.

Its 3D model and particle-effect viewers are also **embeddable**: another site can iframe
`trove.aallyn.net/embed/viewer` to preview a mod it hosts, a mod on the hub, a dressed character, or
anything in the live game — a creature by its own prefab name, or a single blueprint filename, which
resolves into the whole assembled creature.
Framing is allowed per-origin (`embed.allowed_origins`), and a mod a partner uploads to preview is
held in Redis for minutes, never written to disk. Partner guide:
[docs/embed.html](docs/embed.html); code in `app/embed/`.

## Run with Docker

```bash
cp .env.example .env     # then set SECRET_KEY, MONGO_*, REDIS_PASSWORD (+ captcha/SMTP)
docker compose up -d --build
curl http://127.0.0.1:15546/health   # {"status":"ok"}
```

Data lives under `./data` (bind-mounted). There's no migration code: **when a model's
indexes change, wipe the data dir** and rebuild.

### Key configuration

See `.env.example` for the full list.

| Variable | Required | Notes |
|---|---|---|
| `SECRET_KEY` | **yes** | JWT signing key — `python -c "import secrets;print(secrets.token_urlsafe(48))"` |
| `MONGO_ROOT_PASSWORD` / `MONGO_APP_PASSWORD` | **yes** | Mongo root + least-privilege app user |
| `REDIS_PASSWORD` | **yes** | Redis auth |
| `CAPTCHA_SECRET` / `CAPTCHA_SITEKEY` | prod | Captcha is enforced only when **both** are set |
| `ADMIN_EMAIL` / `ADMIN_PASSWORD` | no | Bootstraps the master superuser on startup |
| `SMTP_HOST` (+ `SMTP_*`, `MAIL_FROM`) | prod | Mail relay; if unset, email is logged instead of sent |
| `GITHUB_CLIENT_ID` / `GITHUB_CLIENT_SECRET` | no | Enables "Sign in with GitHub" |

### Behind a reverse proxy

All services bind to `127.0.0.1`; the proxy routes `api.aallyn.net` → `:15546` (only `/v1/*`
+ `/health`), `dev.aallyn.net` → `:25470`, `docs.aallyn.net` → `:25468`, and
`trove.aallyn.net` → `:15546` (all paths). Run Uvicorn with `--proxy-headers` so client IPs
are correct for rate limiting. Bodies are capped at 8 MB, except the `.tmod` tools and bot
cfg ingests (`/v1/mods/*`, `/v1/leaderboards/insert`, `/v1/market/insert`) at 20 MB, and the
Sound Studio + Mod Workshop builds (`/site/sound-studio/build`, `/site/mod-workshop/*`,
which upload a whole bank or a whole mod at once) at 32 MB, and the Blueprint Editor
(`/site/blueprint-editor/*`, whose save carries a per-voxel edit list and whose `.qb`
import carries four voxel grids) at 64 MB, and the Unlock Debug patcher
(`/site/unlock-debug`, which uploads a whole game executable so seven bytes of it can
change) at 160 MB — allow at least as much at the proxy.

## Project layout

Feature modules grouped by endpoint path; cross-cutting infrastructure in `app/core/`.

```
app/
├── main.py     # app assembly: routers, middleware, lifespan
├── core/       # config, db, security, errors, redis, mailer, ratelimit, pagination, …
├── auth/       # signup, login, sessions, oauth, account
├── tokens/     # mint/list/edit/rotate/revoke API tokens
├── trove/      # the Trove data surfaces (rotations, leaderboards, mods, codexes, …)
├── site_auth/  # Discord login for the showcase site (mods/modpacks)
├── admin/      # superuser metrics, toggles, moderation
├── embed/      # the 3D/VFX viewers packaged for other sites' iframes
└── …           # usage, pageviews, events, scanning
site/           # showcase website (templates + static)
                #   server-rendered first paint: app/site/ssr.py
portal/         # developer-portal SPA
docs/           # static docs (guide + Redoc reference + llms.txt)
```

## Adding an endpoint

The platform is built so a new endpoint is a small, consistent addition:

1. Create `app/<feature>/` with `router.py` (+ `models.py` if it stores data).
2. Gate it with `Depends(require_scope("<resource>:<action>"))` and **append** the new scope
   to `app/core/scopes.py` (bits are permanent — never renumber or reuse).
3. Use `paginate_newest_first(...)` for lists; writes honour `Idempotency-Key` automatically.
4. Raise `APIError(status, ErrorCode.x, "msg")` — never a bare `HTTPException`.
5. Register the router in `app/main.py` and any new `Document` in `app/core/database.py`.

## Errors & rate limits

Every error uses one envelope (branch on `code`, not `message`); each carries a `request_id`:

```json
{ "error": { "code": "rate_limited", "message": "…", "details": null, "request_id": "req_…" } }
```

Default limits: signup 5/h/IP · login 10/5min/IP · API 120/min/token · token creation 3/day.

## Development

```bash
python -m venv .venv && . .venv/bin/activate     # 3.11–3.13
pip install -r requirements-dev.txt
ruff check app tests          # lint
pytest tests/unit             # unit tests (no services needed)
pytest tests/integration -m integration   # needs Docker (testcontainers spin up Mongo + Redis)
```

## Backups

`scripts/backup-mongo.sh` (gzip `mongodump`) and `scripts/restore-mongo.sh` (`--drop`
restore). Schedule the backup via cron and keep an **off-box** copy.
