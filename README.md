# Kiwi API

**Kiwi 1.0 is a developer-API platform.** Developers sign up in a browser portal,
mint scoped, IP-restricted, rate-limited **API tokens**, and (once you add them)
authenticate data endpoints with those tokens. A master/admin oversees every
account, token, and request.

> **This 1.0 release ships the whole platform but _no data endpoints yet_.**
> Accounts, tokens, scopes, rate limiting, usage metrics, email, admin, and
> observability are all here and battle-tested — the product `/v1/*` surface is
> built on top of this foundation. Until then there's nothing under `/v1/*` to call.

Fully dockerized; Mongo and Redis persist to **local folders** via bind mounts
(no Docker named volumes). Includes a developer-portal SPA and a static docs site.

## Stack

- **Python 3.13** _(Beanie 2.1 does not yet support 3.14)_ · FastAPI + Uvicorn
- **Beanie 2.x** ODM on PyMongo's native async client · **MongoDB 7**
- **Redis 7** — sliding-window rate limiting, login lockout, OAuth state, write coalescing, idempotency
- Argon2 password hashing · PyJWT sessions + refresh rotation · hashed API tokens
- Captcha: Cloudflare Turnstile (default) or hCaptcha · Email via Postfix (Jinja2 + durable outbox)
- `cryptography` (GitHub secret-scanning signature verification)

## What's in the platform

- **Accounts** — signup (captcha + anti-spam + disposable-email block + HIBP breach check), email
  verification, password reset, change email/password, profile edit, GDPR export + account deletion.
- **Sessions** — short-lived access JWT + rotating single-use refresh tokens; `token_version` for
  instant global invalidation; view/revoke active sessions; "log out everywhere".
- **GitHub OAuth** — optional "Sign in with GitHub" (verified-email linking, one-time code exchange).
- **API tokens** — `kiwi_<body>_<checksum>` format (self-validating + secret-scanning friendly),
  Discord-style bitmask scopes, IP allowlist (exact + CIDR), expiry, rotation, revoke-with-reason,
  3/day creation cap, 6-month inactivity auto-revoke, expiry-warning emails.
- **Secret scanning** — `POST /secret-scanning/github` (ECDSA-verified) auto-revokes leaked tokens.
- **Rate limiting** — Redis sliding-window (Mongo fallback), per-endpoint overrides, `X-RateLimit-*`
  + `Retry-After` headers, daily admin digest of 429s.
- **Usage metrics** — every token-authenticated request recorded (buffered), TTL-expired; per-user,
  per-token, and global activity aggregations; cursor-paginated raw event feed.
- **Admin** — superuser-only (`/admin/*`), enforced server-side: users, tokens, usage, revoke-any.
- **Platform plumbing for new endpoints** — cursor pagination + standard `Page` envelope,
  `Idempotency-Key` replay safety, request-id correlation (`X-Request-ID`) + structured logs,
  consistent error envelope.

## Hosts

| Public domain             | Local target        | What it is                                              |
|---------------------------|---------------------|--------------------------------------------------------|
| `https://api.aallyn.net`  | `127.0.0.1:15546`   | Production API: `/v1/*` (none yet) + `/health`          |
| `https://dev.aallyn.net`  | `127.0.0.1:25470`   | **Developer portal** SPA (login, tokens, activity, account, admin) |
| `https://docs.aallyn.net` | `127.0.0.1:25468`   | Static documentation site                              |

The portal (`dev.aallyn.net`) is a no-build vanilla-JS SPA that calls the API
cross-origin (CORS-enabled). Programs authenticate the API with an **API token
only** — there is no programmatic login; humans use the portal.

## Layout

Feature modules grouped by endpoint path (vertical slices); cross-cutting
infrastructure in `app/core/`.

```
app/
├── main.py             # app assembly: routers, middleware order, lifespan
├── core/               # config, database, security, errors, redis, mailer,
│                       #   ratelimit, limits, pagination, idempotency,
│                       #   observability (request-id), scopes, maintenance, …
├── auth/               # /auth/*  signup, login, sessions, oauth, account — owns User, Session
├── tokens/             # /tokens/*  mint/list/edit/rotate/revoke — owns ApiToken
├── usage/              # UsageEvent model + buffered recorder + aggregations
├── admin/              # /admin/*  superuser metrics + revoke + events feed
└── scanning/           # /secret-scanning/github  partner webhook
portal/                 # developer-portal SPA (nginx + static app.js/styles.css)
docs/                   # static docs site (guide + Redoc reference + llms.txt)
scripts/                # backup-mongo.sh / restore-mongo.sh
tests/                  # unit (no deps) + integration (testcontainers Mongo+Redis)
```

## Adding the real endpoints

The platform is built so a new endpoint is a small, consistent addition:

1. Create `app/<feature>/` with `router.py` (+ `models.py` if it stores data).
2. Auth + scope: `Depends(require_scope("<resource>:<action>"))`. **Append** the new scope to
   `app/core/scopes.py` (bits are permanent — never renumber/reuse).
3. Lists: `Depends(list_params)` + `paginate_newest_first(...)` → return `Page{items, next_cursor, has_more}`.
4. Writes return `201`/`200`/`204`; any write honours an `Idempotency-Key` header automatically.
5. Register the router in `app/main.py` and any new `Document` in `app/core/database.py`.
6. Errors: `raise APIError(status, ErrorCode.x, "msg")` — never a bare `HTTPException`.

## Run with Docker

```bash
cp .env.example .env     # then set SECRET_KEY, MONGO_*, REDIS_PASSWORD (+ captcha/SMTP)
docker compose up -d --build
curl http://127.0.0.1:15546/health   # {"status":"ok"}
```

Mongo lives in `./data/mongo`, both bound from the host. **Whenever a model's
indexes change, wipe the data dir** (no migration code is carried): `docker
compose down && rm -rf data/mongo && docker compose up -d --build`.

### Key configuration (`.env` — see `.env.example` for the full list)

| Variable | Required | Notes |
|---|---|---|
| `SECRET_KEY` | **yes** | JWT signing key — `python -c "import secrets;print(secrets.token_urlsafe(48))"` |
| `MONGO_ROOT_PASSWORD` / `MONGO_APP_PASSWORD` | **yes** | Mongo root + least-privilege app user |
| `REDIS_PASSWORD` | **yes** | Redis auth |
| `CAPTCHA_SECRET` / `CAPTCHA_SITEKEY` | prod | Captcha is enforced only when **both** are set |
| `ADMIN_EMAIL` / `ADMIN_PASSWORD` | no | Bootstraps/promotes the master superuser on startup |
| `SMTP_HOST` (+ `SMTP_*`, `MAIL_FROM`) | prod | Postfix relay; if unset, email is logged instead of sent |
| `GITHUB_CLIENT_ID` / `GITHUB_CLIENT_SECRET` | no | Enables "Sign in with GitHub" |

### Behind the reverse proxy

All services bind to `127.0.0.1`; the proxy enforces the api/dev split (only
`/v1/*` + `/health` are routed on `api.aallyn.net`). Uvicorn runs with
`--proxy-headers` so client IPs (used for rate limiting) reflect the real client.

```nginx
server {  # production API — only /v1 + /health
  server_name api.aallyn.net;
  location /v1/      { proxy_pass http://127.0.0.1:15546; }
  location = /health { proxy_pass http://127.0.0.1:15546; }
  location /         { return 404; }
}
server { server_name dev.aallyn.net;  location / { proxy_pass http://127.0.0.1:25470; } }
server { server_name docs.aallyn.net; location / { proxy_pass http://127.0.0.1:25468; } }
```

## Errors & rate limits

Every error uses one envelope (branch on `code`, not `message`); each carries a
`request_id` (also `X-Request-ID`):

```json
{ "error": { "code": "rate_limited", "message": "…", "details": null, "request_id": "req_…" } }
```

Default limits: signup 5/h/IP · login 10/5min/IP · API 120/min/token · token
creation 3/day. Responses carry `X-RateLimit-Limit/Remaining/Reset`; a `429` adds
`Retry-After`.

## Development

```bash
python -m venv .venv && . .venv/bin/activate     # 3.11–3.13
pip install -r requirements-dev.txt
ruff check app tests          # lint
pyright                        # types (advisory)
pytest tests/unit             # unit tests (no services needed)
pytest tests/integration -m integration   # needs Docker (testcontainers spin up Mongo + Redis)
```

CI (`.github/workflows/ci.yml`): ruff · pyright (advisory) · unit · integration · pip-audit · docker build.

## Backups

`scripts/backup-mongo.sh` (gzip `mongodump`, prune, creds from the container env)
and `scripts/restore-mongo.sh` (`--drop` restore). Schedule the backup via cron and
add an **off-box** copy (a local-only backup won't survive disk loss).

## Documentation

`./docs` is a static site (getting-started guide, a Redoc API reference rendered
from the live OpenAPI spec, and an `llms.txt` reference for AI assistants).
