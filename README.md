# Kiwi API

**Kiwi 1.0 is a developer-API platform.** Developers sign up in a browser portal,
mint scoped, IP-restricted, rate-limited **API tokens**, and authenticate data
endpoints with those tokens. A master/admin oversees every account, token, and
request.

> Built as a reusable platform (accounts, tokens, scopes, rate limiting, usage
> metrics, email, admin, observability) with product endpoints added on top. The
> first data surface is live: **Trove game data** under `/v1/rotations/*` (server time,
> bonuses, merchant timers, and a news relay). `GET /openapi.json` is always current.

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

## Data endpoints (`/v1` — API token)

Live Trove game data, ported from BetterTroveTools, **grouped by function** (most of the API is
Trove, so it's organized by what the data is, not the game). All `GET`, read-only; timestamps are
real-UTC unix seconds. Full schema in `/openapi.json`.

> **`rotations`, `feeds`, `codexes` and `btt` are public** — callable with **no token** at a stricter
> per-IP rate limit (30 req/min/IP by default). Send a token carrying the scope to get the full per-token
> limit (120 req/min). **`codexes` gets 5× both budgets** (150 req/min/IP anonymous, 600 req/min/token)
> since it's lightweight reference data. A revoked/malformed token still 401s; a valid token lacking the
> scope falls back to the anonymous per-IP budget. Every other category still requires a token with the
> matching scope.

**`rotations` category — scope `rotations:read`** (public — token optional, see above)

| Endpoint | Returns |
|---|---|
| `/v1/rotations/server-time` | server time, in-game day, next daily + weekly resets |
| `/v1/rotations/daily-buffs` | today's daily buff + full Mon→Sun rotation |
| `/v1/rotations/weekly-buffs` | this week's weekly buff + 4-week rotation |
| `/v1/rotations/corruxion` | Corruxion merchant: live timer + upcoming schedule |
| `/v1/rotations/fluxion` | Fluxion merchant: voting/selling timer + schedule |
| `/v1/rotations/gardening` | 2-day / 3-day plant harvest windows |
| `/v1/rotations/chaos-chest` | weekly Chaos Chest: featured item (bot-captured ▸ falls back to Trovesaurus relay) + window + countdown |
| `/v1/rotations/chaos-chest/history?limit=&offset=` | past chaos-chest captures, newest week first |
| `POST /v1/rotations/chaos-chest/insert` | **master-only** body `{name}` — bot ingest, server anchors to current Tue-11:00-UTC week |
| `/v1/rotations/challenge/current` | hourly challenge active right now (or last window during a gap); cadence drops to half-hourly on trove Fridays |
| `/v1/rotations/challenge/history?limit=&offset=` | past challenge captures, newest window first |
| `POST /v1/rotations/challenge/insert` | **master-only** body `{name}` — bot ingest, server anchors to the active 20-min window |
| `/v1/rotations/calendar` | yearly calendar: all recurring rotations (buffs, merchants, gardening, biomes) as one ±365-day timeline |
| `/v1/rotations/delves?week=` | a week's delve rotation — floor records relayed from a community delve source (default current week; `/delves/weeks` lists available weeks) |
| `/v1/rotations/biomes` | 3-hour adventure biome rotation (current + upcoming) |
| `/v1/rotations/wild-mana` | weekly Wild Mana biome rotation |
| `/v1/rotations/stampy` | weekly Stampy event biome (48h) |

**`feeds` category — scope `feeds:read`** (public — token optional; fetched/relayed from upstream + cached in Mongo)

| Endpoint | Returns |
|---|---|
| `/v1/feeds/news?limit=` | latest Trove news relayed from `trovegame.com/feed` (small live view; full archive at `/v1/misc/news-history`) |
| `/v1/feeds/twitch` | live Trove Twitch streams |
| `/v1/feeds/youtube` | recent Trove YouTube videos |
| `/v1/feeds/bilibili` | recent Trove Bilibili videos |
| `/v1/feeds/events` | ongoing Trovesaurus events (filter `?category=`) |
| `/v1/feeds/events/categories` | distinct event categories (discovered dynamically) |
| `/v1/feeds/events/upcoming` · `/history` | events not yet started / already ended |

(Twitch/YouTube/Bilibili are fetched **at source** — Twitch Helix via a client-credentials app token,
the YouTube Data API, and a Bilibili search-page scrape — then cached in `FeedCache`. Set
`TWITCH_CLIENT_ID`/`TWITCH_CLIENT_SECRET`/`YT_API_KEY` in the env; the filter knobs (search query,
excluded channels/titles, per-channel cap, cutoff, count) are admin runtime_config tunables under
`community_feeds`.) Events are relayed from `trovesaurus.com/calendar/feed` and
**stored** (kept after they leave the upstream feed) so history/upcoming work; `status` is
`upcoming`/`ongoing`/`ended`, categories are free-form and discovered via a distinct query.

**`stats` category — scope `stats:read`** (raw game data, transmitted as-is — no calculation)

| Endpoint | Returns |
|---|---|
| `/v1/stats/power-rank` | Power Rank stat table — each source and the PR it contributes |
| `/v1/stats/magic-find` | Magic Find stat table |
| `/v1/stats/light` | Light stat table (`step` / `permanent` per source) |
| `/v1/stats/classes` | all 18 classes as full objects, keyed by `tech_name` |
| `/v1/stats/classes/{tech_name}` | one class by its `tech_name` token (e.g. `knight`) |

Each class carries a stable `tech_name` token (the canonical id — display `name` differs, e.g.
`adventurer` → "Boomeranger"); store it and look the class up by that token later.

**`gems` category — scope `gems:read`** (stateless calculators — gem objects round-trip through the client)

| Endpoint | Returns |
|---|---|
| `GET /v1/gems/lookups` | every valid gem field value (tiers, types, elements, stats, augments, abilities) |
| `POST /v1/gems/generate` | roll a gem (omit any field for random) → a gem object |
| `POST /v1/gems/augment` | apply a focus augment to a stat by position → updated gem |
| `POST /v1/gems/spark` · `/flare` | reroll a stat type / move a proc, by stat position → updated gem |
| `POST /v1/gems/level-up` · `/set-level` | raise or set a gem's level → updated gem |
| `POST /v1/gems/evaluate` | score a typed-in gem: quality %, Power Rank, cost to perfect |
| `GET /v1/gems/stat-range` | plausible (min, max) value a stat can roll at |
| `GET /v1/gems/builds/options` | valid build-config field values (classes, allies, foods, flags) |
| `POST /v1/gems/builds/calculate` | top gem proc layouts for a build, ranked by damage coefficient |

The simulator is stateless: `generate` returns a gem object, the client holds it and POSTs it back to
an action endpoint with a `stat_position` (0/1/2) to mutate it. Nothing gem-related is stored server-side.

**`misc` category — scope `misc:read`**

| Endpoint | Returns |
|---|---|
| `GET /v1/misc/news-history?limit=&offset=` | the full Trove news archive (never pruned), newest first, paginated |
| `GET /v1/misc/software` | third-party Trove modding software, grouped by category |
| `GET /v1/misc/timezones` | timezones supported by the converter and clocks |
| `GET /v1/misc/time/now` | current time across every zone, incl. Trove server (reset) time |
| `POST /v1/misc/time/convert` | convert a time + zone (or a unix) → every zone + Discord timestamp codes |
| `GET /v1/misc/activity` | (**tokenless**) lower-bound count of active players in the most recent capture window; mirror of `/v1/leaderboards/activity` |
| `GET /v1/misc/activity/history?days=` | (**tokenless**) time-series of activity estimates with per-hour normalisation; mirror of `/v1/leaderboards/activity/history` |
| `GET /v1/misc/trove-status` | (**tokenless**) live Trove server status from a 60s background prober. `overall` rolls up the Live regions EU+US (`online`/`maintenance`/`down`/`unknown`). `auth` = HTTPS liveness of `auth.trionworlds.com`; `environments.{eu,us,pts}` each carry a `game` TCP probe of the glsserver port (6560). Auth-up + game-refused = `maintenance` |
| `GET /v1/misc/trove-status/history?env=&days=` | (**tokenless**) status timeline for one environment (`eu`/`us`/`pts`, default eu; days 1–90) — `segments` (continuous status periods, open one has `ended_at=null`), `outages`, and an `uptime` fraction. Backs the `/status` page downtime history |

Trove "server"/reset time is a fixed **UTC−11**. The converter takes a naive `datetime` interpreted in
the given `timezone` (`trove` / `UTC` / any IANA id) or an absolute `unix`, and returns the instant in
every zone plus Discord `<t:unix:style>` codes.

**`btt` category — scope `btt:read`** (public — token optional; drives BetterTroveTools' in-app update checks)

| Endpoint | Returns |
|---|---|
| `GET /v1/btt/releases?channel=&limit=&offset=` | BetterTroveTools GitHub releases, newest first; optional `?channel=release\|beta` filter |
| `GET /v1/btt/latest?channel=` | latest BTT version **per platform** (windows/linux/android) on a channel; each platform walks back independently until a release ships an asset for it |
| `GET /v1/btt/latest/{platform}?channel=` | latest BTT version for a single platform |
| `GET /v1/btt/check?installed=&platform=&channel=` | **"is there an update?"** — server-side version compare; returns `{ update_available, comparable, latest }` so the client just reads a bool |
| `GET /v1/btt/changelog?limit_groups=&commits_per_group=` | commits grouped by tag (mirrors BTT's "Show changelog" button), newest first, `"Unreleased"` group leads when there are post-tag commits |

**`leaderboards` category — scope `leaderboards:read`** (read side; `POST /insert` is **master-only**, requires a superuser API token)

| Endpoint | Returns |
|---|---|
| `GET /v1/leaderboards/timestamps?limit=60` | recent dump anchors (unix seconds at 11:00 UTC), newest first |
| `GET /v1/leaderboards?created_at=` | boards present at that anchor; each carries `contest_type` for THIS anchor + `reset_kind` / `player_board` flags |
| `GET /v1/leaderboards/{uuid}` | one board's metadata + full `contests` list |
| `GET /v1/leaderboards/{uuid}/entries?created_at=&limit=&offset=` | top-N entries for one board at one anchor, ranked |
| `GET /v1/leaderboards/players/{name}/history?uuid=&limit=` | recent appearances of one player across boards (case-insensitive on `name`) |
| `GET /v1/leaderboards/cheaters` | **tokenless** statistical-outlier flagging: MAD-Z + rank-gap + velocity. Per-evidence + per-player confidence; cached 30 min; pre-warmed at boot |
| `POST /v1/leaderboards/insert?timestamp=` | **master-only ingest**: multipart `file` field with the raw `LeaderBot.cfg` text. Idempotent for a given anchor; `timestamp` is optional and only used for back-fills. Subject to the **ingest cooldown** (see below) when called with an API token |

The bot dumps the game's `LeaderBot.cfg` hourly and POSTs the file. **Full history is preserved**: entries
older than `leaderboards_hot_retention_days` (default **3 days**; runtime-tunable from the master admin
panel) are moved into a cold `leaderboard_entries_archive` collection at the tail of each insert. The read
endpoints route old anchors straight to the archive, so the hot collection stays small/fast while
historical queries still work (slower per-row but unaffected by the hot index footprint). `/timestamps`
unions both, deduped.

**Archive rate limit** — queries with `?created_at=` older than `leaderboards_archive_query_threshold_days`
(default **3** — same window as hot retention by convention, so "served from cold" and "pays archive
rate limit" line up; runtime-tunable from the master admin panel) pay a SECOND, tighter per-token bucket
(default 10 req/min) on top of the standard per-token cap. The bucket's state is surfaced via
`X-RateLimit-Archive-Limit` / `X-RateLimit-Archive-Remaining` / `X-RateLimit-Archive-Reset` response headers
so clients can self-throttle. Recent queries (≤ threshold) cost only the standard cap.

**`market` category — scope `market:read`** (read side; `POST /insert` is **master-only**)

| Endpoint | Returns |
|---|---|
| `GET /v1/market/listings?name=&price_min=&price_max=&last_seen_after=&hide_expired=true&sort=&limit=&offset=` | paginated marketplace listings (default sort newest-`last_seen` first; `hide_expired` filters past 7d / stale 3h+) |
| `GET /v1/market/items` | item names that currently have a stored listing (sorted) |
| `GET /v1/misc/interest-items` | (lives under misc, **tokenless**) the full allow-list of items the bot tracks; admin-managed via the master panel at `/admin/market/interest-items` |
| `GET /v1/market/items/{name}/summary` | min/max/avg/median price-each + listing count for one item |
| `POST /v1/market/insert?timestamp=` | **master-only ingest**: multipart `file` with the raw `GrainusMod.cfg` text. Listings upserted by UUID — re-scrapes bump `last_seen`, never duplicate. Subject to the **ingest cooldown** (see below) when called with an API token |

Bot scrapes the in-game marketplace hourly. Each listing's UUID v1 is the document `_id`; `created_at` is
decoded from the UUID's timestamp (so it matches when the player posted in-game); `last_seen` is bumped on
every re-scrape. Only items on `gamedata/market_items.json` are persisted; the rest are dropped at ingest.

**Archive rate limit** — passing `hide_expired=false` on `/listings` (i.e. asking for the historical
tail past the 7-day in-game lifetime) pays a SECOND, tighter per-token bucket (default 10 req/min) on
top of the standard per-token cap. Same `X-RateLimit-Archive-*` headers as the leaderboards archive
throttle. Market doesn't use a day-count threshold because the 7-day listing lifetime already defines
fresh-vs-historical; the limit fires on the opt-in flag instead.

**Ingest cooldown (all `POST /insert` endpoints)** — backstop against a misbehaving bot
resubmitting the same dump every few seconds. Per-token, per-endpoint bucket (default
**1 submit per 5 min** — runtime-tunable as `ingest_cooldown_max` / `ingest_cooldown_window_seconds`
in the master admin panel). Bucketed independently per endpoint — a leaderboards push doesn't share
a budget with a market push. Returns 429 with `Retry-After` once exhausted. **API-token auth only**:
session-JWT calls from the portal "Manual cfg ingest" card bypass this cap, so the master can replay
captured cfgs / back-fills without waiting out the window. Bots should honor `Retry-After` and
suppress duplicate-anchor submissions client-side.

## BetterTroveTools showcase site (`trove.aallyn.net`)

The api container ALSO serves the BTT marketing/manual site out of `site/`
(templates + static + ~20 MB of screenshots). Routes:

| Path | Returns |
|---|---|
| `GET /` | the BTT landing page (index.html) |
| `GET /documentation` | the user manual |
| `GET /commands` | searchable in-game slash-command reference |
| `GET /leaderboards` | hourly in-game leaderboard browser (charts, cheaters, activity) |
| `GET /updates` | per-server (Live US / PTS) game-update file explorer + version diff |
| `GET /support` | "support the project" landing for the navbar heart icon |
| `GET /status` | Trove server-status page — live EU/US/PTS state + downtime-history timeline |
| `GET /static/*` | site assets (bind-mounted from `site/static/`) |
| `GET /site/*` | page-side JSON proxies (leaderboards, updates) — same-origin, no token |
| `GET /api-info` | the old developer-card landing (lives here so `/` is free for the site) |

Point your reverse proxy: `trove.aallyn.net` → the api container's `:15546`,
forward all paths. `api.aallyn.net` keeps its existing filter to `/v1/*` +
`/health`. The site's CSP is broader than the API's (loads FontAwesome + Google
Fonts from CDN, calls `api.aallyn.net` for release data) — middleware picks
the right CSP per path.

> **Removed 2026-06:** `/unlock_debug` and `/unlock_fps` byte-patcher routes
> were deleted after Trion shipped anti-cheat. Any binary tampering is now
> grounds for a ban; the tools shouldn't exist anymore.

A background relayer polls the configured GitHub repo every 30 min and stores releases in Mongo, so the
endpoints serve from cache. Channels are detected from GitHub's `prerelease` flag (`release`/`beta`).
Platform assets are detected by file extension — `.msi`/`.exe` for windows (msi prioritized), `.AppImage`/
`.deb`/`.rpm`/`.tar.gz` for linux, `.apk` for android.

**`mods` category — scope `mods:read`** (stateless `.tmod` tooling — 20 MB body cap on `/v1/mods/*`)

| Endpoint | Returns |
|---|---|
| `POST /v1/mods/read` | decompile a `.tmod` (POST raw bytes) → header properties + file table; `?metadata_only=` omits contents |
| `POST /v1/mods/build` | build a `.tmod` from header fields + files (base64) → raw bytes |

The `.tmod` binary format (little-endian header + LEB128 + zlib file stream + Trove's FNV-1a-variant
checksum) is ported in pure Python (`app/trove/tmod.py`) — no native lib. `build` stamps the `modLoader`
header `KiwiAPI` (where BetterTroveTools uses `BTT`); nothing is stored — built in memory and discarded
after sending.

**`updates` category — scope `updates:read`** (browse the archived game files — latest version)

| Endpoint | Returns |
|---|---|
| `GET /v1/updates/branches` | tracked branches (`live-us`, `pts`) with current version + file count |
| `GET /v1/updates/{branch}/versions` | captured version history, newest first |
| `GET /v1/updates/{branch}/changes?version=&ordinal=&type=` | per-file diff a version introduced (added/modified/removed paths); latest if unpinned |
| `GET /v1/updates/{branch}/tree?prefix=` | one directory level (ls-style); empty prefix = root |
| `GET /v1/updates/{branch}/file?path=` | a single file's bytes, streamed from the blob store (`/file/meta` for hash+size) |

Kiwi mirrors Trove's update CDN into a content-addressed, deduped store (see "Game-file archive" below);
these endpoints serve the latest captured version. Loose files and TFA-extracted files are browsed
identically. Historical-version querying is the next layer.

**`codexes` category — scope `codexes:read`** (public — token optional; structured game data parsed from the archive)

| Endpoint | Returns |
|---|---|
| `GET /v1/codexes/types` | the codex types present for a branch, each with its entry count |
| `GET /v1/codexes/search?q=&type=&category=&tradable=&sort=` | cross-type search/filter (the unified search surface); each result carries its `type` |
| `GET /v1/codexes/{type}?search=&category=&tradable=&sort=&limit=&offset=` | entries of one type — filterable, sortable, paginated |
| `GET /v1/codexes/{type}/categories` | distinct categories (+ counts) in a type, for filter dropdowns |
| `GET /v1/codexes/{type}/entry?path=` | a single entry by its source prefab path |

Eight typed datasets — `ally`, `mount`, `dragon`, `memento`, `recipe`, `item`, `fish`, `badge` — parsed
from Trove's `prefabs/*.binfab` files (a protobuf-like wire format) with names/descriptions resolved via
the `languages/` locale tables. The indexer runs after each archive sync: a full build the first time,
then only the changed prefabs (driven by the version delta), so a routine patch never re-parses the rest
of the game. All endpoints default to the `live-us` branch (`?branch=pts` for PTS). Each entry carries
identity (name, category, description, tradability) plus `mastery` (collectible mastery, from
`meta/multipliers.binfab`); richer per-type fields (power rank, stats, models) fill the `data` object
incrementally.

More are added following the conventions in "Adding the real endpoints" below.

## Hosts

| Public domain             | Local target        | What it is                                              |
|---------------------------|---------------------|--------------------------------------------------------|
| `https://api.aallyn.net`  | `127.0.0.1:15546`   | Production API: `/v1/*` (e.g. `/v1/rotations/*`) + `/health` |
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
  client_max_body_size 8m;                 # default cap (matches the app)
  location /v1/      { proxy_pass http://127.0.0.1:15546; }
  location /v1/mods/ { client_max_body_size 20m; proxy_pass http://127.0.0.1:15546; }  # .tmod tools
  location = /v1/leaderboards/insert { client_max_body_size 20m; proxy_pass http://127.0.0.1:15546; }  # bot cfg dump
  location = /v1/market/insert       { client_max_body_size 20m; proxy_pass http://127.0.0.1:15546; }  # bot cfg dump
  location = /health { proxy_pass http://127.0.0.1:15546; }
  location /         { return 404; }
}
server { server_name dev.aallyn.net;  location / { proxy_pass http://127.0.0.1:25470; } }
server { server_name docs.aallyn.net; location / { proxy_pass http://127.0.0.1:25468; } }
```

The app caps request bodies at 8 MB, except `/v1/mods/*`, `/v1/leaderboards/insert`, and
`/v1/market/insert` at 20 MB (the `.tmod` tools and the bot's raw cfg dumps). The proxy must
allow at least as much on those paths or it rejects large uploads before they reach the app.

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
