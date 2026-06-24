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
- **Redis 7** - sliding-window rate limiting, login lockout, OAuth state, write coalescing, idempotency
- Argon2 password hashing · PyJWT sessions + refresh rotation · hashed API tokens
- Captcha: Cloudflare Turnstile (default) or hCaptcha · Email via Postfix (Jinja2 + durable outbox)
- `cryptography` (GitHub secret-scanning signature verification)

## What's in the platform

- **Accounts** - signup (captcha + anti-spam + disposable-email block + HIBP breach check), email
  verification, password reset, change email/password, profile edit, GDPR export + account deletion.
- **Sessions** - short-lived access JWT + rotating single-use refresh tokens; `token_version` for
  instant global invalidation; view/revoke active sessions; "log out everywhere".
- **GitHub OAuth** - optional "Sign in with GitHub" (verified-email linking, one-time code exchange).
- **API tokens** - `kiwi_<body>_<checksum>` format (self-validating + secret-scanning friendly),
  Discord-style bitmask scopes, IP allowlist (exact + CIDR), expiry, rotation, revoke-with-reason,
  3/day creation cap, 6-month inactivity auto-revoke, expiry-warning emails.
- **Secret scanning** - `POST /secret-scanning/github` (ECDSA-verified) auto-revokes leaked tokens.
- **Rate limiting** - Redis sliding-window (Mongo fallback), per-endpoint overrides, `X-RateLimit-*`
  + `Retry-After` headers, daily admin digest of 429s.
- **Usage metrics** - every token-authenticated request recorded (buffered), TTL-expired; per-user,
  per-token, and global activity aggregations; cursor-paginated raw event feed.
- **Site analytics** - showcase-site page views + cookieless unique visitors (daily-rotating IP+UA
  salted hash, no cookie/PII stored), rolled up per page in the dev-portal "Site Analytics" admin
  tab (`GET /admin/pageviews`); buffered recorder, TTL-expired like usage metrics.
- **Admin** - superuser-only (`/admin/*`), enforced server-side: users, tokens, usage, site analytics, revoke-any.
- **Platform plumbing for new endpoints** - cursor pagination + standard `Page` envelope,
  `Idempotency-Key` replay safety, request-id correlation (`X-Request-ID`) + structured logs,
  consistent error envelope. A 404 from a **browser page** request (GET, `Accept: text/html`, non-API path)
  returns a themed HTML 404 page; data/API 404s stay JSON (`app/core/errors.py`).

## Data endpoints (`/v1` - API token)

Live Trove game data, ported from BetterTroveTools, **grouped by function** (most of the API is
Trove, so it's organized by what the data is, not the game). All `GET`, read-only; timestamps are
real-UTC unix seconds. Full schema in `/openapi.json`.

> **`rotations`, `feeds`, `codexes` and `btt` are public** - callable with **no token** at a stricter
> per-IP rate limit (30 req/min/IP by default). Send a token carrying the scope to get the full per-token
> limit (120 req/min). **`codexes` gets 5× both budgets** (150 req/min/IP anonymous, 600 req/min/token)
> since it's lightweight reference data. A revoked/malformed token still 401s; a valid token lacking the
> scope falls back to the anonymous per-IP budget. Every other category still requires a token with the
> matching scope.

**`rotations` category - scope `rotations:read`** (public - token optional, see above)

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
| `POST /v1/rotations/chaos-chest/insert` | **master-only** body `{name}` - bot ingest, server anchors to current Tue-11:00-UTC week |
| `/v1/rotations/challenge/current` | hourly challenge active right now (or last window during a gap); cadence drops to half-hourly on trove Fridays |
| `/v1/rotations/challenge/history?limit=&offset=` | past challenge captures, newest window first |
| `POST /v1/rotations/challenge/insert` | **master-only** body `{name}` - bot ingest, server anchors to the active 20-min window |
| `/v1/rotations/calendar` | yearly calendar: all recurring rotations (buffs, merchants, gardening, biomes) as one ±365-day timeline |
| `/v1/rotations/delves?week=` | a week's delve rotation - floor records relayed from a community delve source (default current week; `/delves/weeks` lists available weeks) |
| `/v1/rotations/biomes` | 3-hour adventure biome rotation (current + upcoming) |
| `/v1/rotations/wild-mana` | weekly Wild Mana biome rotation |
| `/v1/rotations/stampy` | weekly Stampy event biome (48h) |

**`feeds` category - scope `feeds:read`** (public - token optional; fetched/relayed from upstream + cached in Mongo)

| Endpoint | Returns |
|---|---|
| `/v1/feeds/news?limit=` | latest Trove news relayed from `trovegame.com/feed` (small live view; full archive at `/v1/misc/news-history`) |
| `/v1/feeds/twitch` | live Trove Twitch streams |
| `/v1/feeds/youtube` | recent Trove YouTube videos |
| `/v1/feeds/bilibili` | recent Trove Bilibili videos |
| `/v1/feeds/events` | ongoing Trovesaurus events (filter `?category=`) |
| `/v1/feeds/events/categories` | distinct event categories (discovered dynamically) |
| `/v1/feeds/events/upcoming` · `/history` | events not yet started / already ended |

(Twitch/YouTube/Bilibili are fetched **at source** - Twitch Helix via a client-credentials app token,
the YouTube Data API, and a Bilibili search-page scrape - then cached in `FeedCache`. Set
`TWITCH_CLIENT_ID`/`TWITCH_CLIENT_SECRET`/`YT_API_KEY` in the env; the filter knobs (search query,
excluded channels/titles, per-channel cap, cutoff, count) are admin runtime_config tunables under
`community_feeds`.) Events are relayed from `trovesaurus.com/calendar/feed` and
**stored** (kept after they leave the upstream feed) so history/upcoming work; `status` is
`upcoming`/`ongoing`/`ended`, categories are free-form and discovered via a distinct query.

**`stats` category - scope `stats:read`** (raw game data, transmitted as-is - the tables do no calculation)

| Endpoint | Returns |
|---|---|
| `/v1/stats/power-rank` | Power Rank stat table - each source and the PR it contributes |
| `/v1/stats/magic-find` | Magic Find stat table |
| `/v1/stats/light` | Light stat table (`step` / `permanent` per source) |
| `/v1/stats/classes` | all 18 classes as full objects, keyed by `tech_name` |
| `/v1/stats/classes/{tech_name}` | one class by its `tech_name` token (e.g. `knight`) |
| `POST /v1/stats/coefficient` | (**tokenless**) stateless calculator → `{ coefficient, damage_used, formula }`. Body `{ physical_damage?, magic_damage?, critical_damage }`. Computes the in-game **Coefficient** = `floor(max(physical, magic) damage * (1 + critical_damage/100))` (the higher damage stat is used). Needs `critical_damage` + ≥1 damage stat (else 400). Shares one formula with the OCR Coefficient derivation |

Each class carries a stable `tech_name` token (the canonical id - display `name` differs, e.g.
`adventurer` → "Boomeranger"); store it and look the class up by that token later. The Coefficient
calculator and OCR derivation call the same `app.trove.stats.compute_coefficient`, so they can't drift.

**`gems` category - scope `gems:read`** (stateless calculators - gem objects round-trip through the client)

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

**`misc` category - scope `misc:read`**

| Endpoint | Returns |
|---|---|
| `GET /v1/misc/news-history?limit=&offset=` | the full Trove news archive (never pruned), newest first, paginated |
| `GET /v1/misc/software` | (**tokenless**) third-party Trove modding software, grouped by category |
| `GET /v1/misc/timezones` | (**tokenless**) timezones supported by the converter and clocks |
| `GET /v1/misc/time/now` | (**tokenless**) current time across every zone, incl. Trove server (reset) time |
| `POST /v1/misc/time/convert` | (**tokenless**) convert a time + zone (or a unix) → every zone + Discord timestamp codes |
| `GET /v1/misc/trove-status` | (**tokenless**) live Trove server status from a 60s background prober. `overall` rolls up the Live regions EU+US (`online`/`maintenance`/`down`/`unknown`). `auth` = HTTPS liveness of `auth.trionworlds.com`; `environments.{eu,us,pts}` each carry a `game` probe of the glsserver port (6560). The probe is **deep**: a region in maintenance still completes the TCP handshake (and answers the glsserver hello) before dropping the connection, so connect-only would read a false `online`; instead it replays the captured glsserver hello and counts the region online only if the server **holds the connection open**, flagging `maintenance` when the server drops right after the hello (or refuses/times out). Live-tunable + degrades to connect-only on any anomaly (`trove_status_game_deep_probe`) |
| `GET /v1/misc/trove-status/history?env=&days=` | (**tokenless**) status timeline for one environment (`eu`/`us`/`pts`, default eu; days 1–90) - `segments` (continuous status periods, open one has `ended_at=null`), `outages`, and an `uptime` fraction. Backs the `/status` page downtime history |
| `GET /v1/misc/supporters` | (**tokenless**) the project's supporters credits list `{supporters:[name], count}`, in display order. Same list shown on `/support`; admin-managed via `/admin/supporters` (master panel) |

Trove "server"/reset time is a fixed **UTC−11**. The converter takes a naive `datetime` interpreted in
the given `timezone` (`trove` / `UTC` / any IANA id) or an absolute `unix`, and returns the instant in
every zone plus Discord `<t:unix:style>` codes.

**`btt` category - scope `btt:read`** (public - token optional; drives BetterTroveTools' in-app update checks)

| Endpoint | Returns |
|---|---|
| `GET /v1/btt/releases?channel=&limit=&offset=` | BetterTroveTools GitHub releases, newest first; optional `?channel=release\|beta` filter |
| `GET /v1/btt/latest?channel=` | latest BTT version **per platform** (windows/linux/android) on a channel; each platform walks back independently until a release ships an asset for it |
| `GET /v1/btt/latest/{platform}?channel=` | latest BTT version for a single platform |
| `GET /v1/btt/check?installed=&platform=&channel=` | **"is there an update?"** - server-side version compare; returns `{ update_available, comparable, latest }` so the client just reads a bool |
| `GET /v1/btt/changelog?limit_groups=&commits_per_group=` | commits grouped by tag (mirrors BTT's "Show changelog" button), newest first, `"Unreleased"` group leads when there are post-tag commits |

**`leaderboards` category - scope `leaderboards:read`** (read side; `POST /insert` is **master-only**, requires a superuser API token)

| Endpoint | Returns |
|---|---|
| `GET /v1/leaderboards/timestamps?limit=60` | recent dump anchors (unix seconds at 11:00 UTC), newest first |
| `GET /v1/leaderboards?created_at=` | boards present at that anchor; each carries `contest_type` for THIS anchor + `reset_kind` / `player_board` flags |
| `GET /v1/leaderboards/{uuid}` | one board's metadata + full `contests` list |
| `GET /v1/leaderboards/{uuid}/entries?created_at=&limit=&offset=` | top-N entries for one board at one anchor, ranked, with day-over-day `rank_delta`/`score_delta` (when the board didn't reset since the prior snapshot) |
| `GET /v1/leaderboards/players/{name}/history?uuid=&limit=` | recent appearances of one player across boards (case-insensitive on `name`) |
| `GET /v1/leaderboards/players/{name}/profile` | (**tokenless**) public profile aggregate: recent appearances (board names + day-over-day deltas), a summary (boards, best rank, last seen), and a `verified` flag (true when a site account claimed + was master-approved for this name). Powers the `/player/<name>` page |
| `GET /v1/leaderboards/{uuid}/health` | board health: roster **turnover** + day-over-day **score inflation** (when comparable - no reset crossed) + **competitiveness** (top-N score concentration: leader share, #1/last ratio, Gini), latest snapshot vs the previous trove-day |
| `GET /v1/leaderboards/cheaters` | **tokenless** anti-cheat flagging. Per-player: MAD-Z + rank-gap + velocity (per-evidence + per-player confidence). Plus group-shaped **clusters** (separate array, each tagged `method`): **co-movement** (primary, name-agnostic — accounts whose hourly gains move in lockstep across the week), **name_stem** (similar names at near-identical scores), or **both**. Cached 30 min; co-movement throttled + off-thread; pre-warmed at boot |
| `POST /v1/leaderboards/insert?timestamp=&backfill=&sync=&warm=` | **master-only ingest**: multipart `file` with the raw `LeaderBot.cfg` text. Default returns **202** and parses/persists in the **background** (so a multi-second insert can't time out the bot); counts/errors land in the ingest log. The parsed snapshot is bulk-loaded into **Postgres** via `COPY` → staging temp table → player-upsert → `INSERT…SELECT` (sub-second for ~720k rows); the row lands in the anchor's partition. Idempotent for a given anchor (delete-by-anchor then reload). `backfill=true` lifts the 14-day anchor limit (bulk re-seed from saved `<unix>.cfg` files). `sync=true` processes **inline → 200 with real counts** (backpressure: one dump in memory at a time). `warm=false` defers cache-warming. Subject to the **ingest cooldown** when called with an API token |
| `POST /v1/leaderboards/reset?drop_boards=` | **master-only, destructive**: `TRUNCATE … RESTART IDENTITY` the Postgres entry/player/activity tables (not row-by-row), and drop every cheater/activity/Redis cache + the ready pointer — a clean slate before a full re-ingest. Board metadata (incl. admin reset-cadence overrides) is kept unless `drop_boards=true`. Returns the deletion tallies; logged at WARNING |
| `POST /v1/leaderboards/warm` | **master-only**: wake the cache warmer to recompute the latest anchor's cheaters + activity + page snapshots. Call once after a bulk back-fill (which uploads with `warm=false`) |
| `POST /v1/leaderboards/cheaters/recompute` | **master-only**: drop the cached cheater results (in-process + Redis snapshots) and recompute the latest anchor from scratch — independent of the activity/class-activity histories, so a cheater-config tweak is re-evaluated without rerunning the long backfills. Inline; returns `{anchor, total_flagged, boards_analyzed}` |
| `POST /v1/leaderboards/views/recompute` | **master-only**: drop the page-view caches (anchor list, board lists, entry pages, board-history charts, ready pointer) and re-warm the latest captures + every board's default chart — independent of the cheaters/activity data. Inline; returns `{keys_cleared, board_charts_warmed, anchor}` |
| `POST /v1/leaderboards/reingest-backlog?clear_first=` | **master-only**: replay the **server-side backlog** — every dump the API received is auto-gzip-saved to `{backlog_dir}/leaderboards/<anchor>.cfg.gz`, so this re-ingests the whole history with **no upload**, server-paced one dump at a time, heavy compute run **once at the end**. `clear_first=true` resets first. Drop `<unix>.cfg` files into the folder to seed it by hand. Poll `/reingest-status` for progress |
| `GET /v1/leaderboards/reingest-status` | **master-only**: live re-ingest progress `{running, total, done, ok, failed, last_anchor, phase, errors[], backlog_files}` for the admin poll |

The bot dumps the game's `LeaderBot.cfg` hourly and POSTs the file. **Full history is preserved** in a
dedicated **PostgreSQL** database (separate from the app's Mongo): the `entry` table is RANGE-partitioned
by anchor (one partition per trove-day, 11:00-UTC boundary), so old data is just old partitions — no
hot/cold collection split. Partitioned-parent indexes `(board_uuid, anchor, rank)` and `(player_id, anchor)`
serve board-at-time and player-over-time reads; player names are normalised into a `player` dimension table.
`leaderboards_hot_retention_days` (default **3 days**; runtime-tunable from the master admin panel) now just
controls how many recent days the warmer pre-warms and the page surfaces.

**Archive rate limit** - queries with `?created_at=` older than `leaderboards_archive_query_threshold_days`
(default **3** - same window as the hot/warm window by convention, so "old/cold lookup" and "pays archive
rate limit" line up; runtime-tunable from the master admin panel) pay a SECOND, tighter per-token bucket
(default 10 req/min) on top of the standard per-token cap. The bucket's state is surfaced via
`X-RateLimit-Archive-Limit` / `X-RateLimit-Archive-Remaining` / `X-RateLimit-Archive-Reset` response headers
so clients can self-throttle. Recent queries (≤ threshold) cost only the standard cap.

**`activity` category - scope `activity:read`** (public - token optional; the reads are free/tokenless. Derived from the leaderboard captures; the showcase `/activity` page is the main consumer)

| Endpoint | Returns |
|---|---|
| `GET /v1/activity/current` | (**tokenless**) lower-bound active-player count in the most recent capture window + distinct `estimate_24h` / `estimate_7d` rollups. "Active" = a player whose score **rose** on some board vs the previous capture, or who newly appears with a non-zero score (a reset drop isn't a rise). Each capture's active set is **materialized** (`activity_active` table) once, so the 24h/7d figures are an indexed `COUNT(DISTINCT)` UNION of the windows in range - the true distinct union, monotonic (`7d ⊇ 24h ⊇ 1h`), not a re-scan. `estimate` null until two captures exist |
| `GET /v1/activity/history?days=` | (**tokenless**) time-series of activity estimates, one point per capture pair, with `estimate_per_hour` normalisation so missed-capture gaps don't spike the line |
| `GET /v1/activity/series?period=` | (**tokenless**) bucketed activity-level series for one period (`1d`/`7d`/`1m`/`3m`/`6m`/`1y`/`all`) with `peak`/`average`/`latest` - backs the `/activity` page charts |
| `POST /v1/activity/backfill?total_days=&chunk_days=&force=&reset=` | **master-only**: seeds the `/series` history from all stored Postgres captures so the multi-period charts have data before the hourly forward-fill. `total_days=0` rebuilds **ALL** stored history (no lower bound); else the trailing N days. **202** + runs in the background, streaming one capture at a time (memory-safe regardless of range); coverage capped by stored history depth. The gap cutoff (windows that span a missed capture, skipped from the per-hour series) is derived from the **actual median capture cadence**, not a hardcoded 1h, so a non-hourly/jittery archive isn't dropped. **`reset=true` is destructive** - wipes the `activity_estimate` table and recomputes from scratch (implies `force`), to flush miscalculations from older runs |

**`class-activity` category - scope `activity:read`** (public - token optional. Per-Trove-class activity from the **Effort `4000+i`** leaderboards, `class_index = uuid % 1000`; the showcase `/class-activity` page is the main consumer. Paragon `5000+i` is **excluded as ambiguous** — neither counted nor filtered on). **Two views in every payload:** RAW counts everyone; CLEAN (the page default, "established", `*_clean` fields) keeps only players who clear BOTH per-class floors (snapshot at the window end): Power Rank (`1000+i`) ≥ `class_activity_power_rank_threshold` (default **25000**) and Effort (`4000+i`) ≥ `class_activity_effort_threshold` (default **50**) — both master-tunable (set either to 0 to drop that gate) — filtering new characters + alts. `clean` is `null` where unmeasurable (no Power Rank snapshot)

| Endpoint | Returns |
|---|---|
| `GET /v1/class-activity/current` | (**tokenless**) **direct headcount of the latest snapshot** (NOT the activity pipeline — no score-rose step): players **present** on a class's Effort board at the newest capture (Paragon excluded as ambiguous). Both views: `active_players`/`share`/`total_active` (raw) + `active_players_clean`/`share_clean`/`total_active_clean` (established: clears both floors) + `power_rank_threshold`/`effort_threshold`. Each class also carries `effort_added`/`effort_added_clean` (+ totals `total_effort_added`/`total_effort_added_clean`) — Effort added to that board in the **latest hour** (this capture vs the previous; Σ positive per-player gains, per view; `null` when no previous capture or the pair crosses the weekly reset). `share` sums to 1 but counts a multi-class player in each class (share-of-players, not distinct). `window_start`=`window_end`=snapshot anchor, `duration_hours` null. (Powers the donut; the time-series below stays activity-based.) |
| `GET /v1/class-activity/series?period=` | (**tokenless**) bucketed per-class series for one period (`1d`…`all`) - a shared `buckets` x-axis + per class `values[]` (raw) and `values_clean[]` (established) aligned to it (null where that view had no measurable window, e.g. across the weekly reset) + `power_rank_threshold`/`effort_threshold`. Backs the `/class-activity` multi-line chart + its Established/All toggle |
| `POST /v1/class-activity/backfill?total_days=&force=&reset=` | **master-only**: seeds the per-class history (raw + clean) from the stored captures (same memory-safe streaming as `/v1/activity/backfill`; the 18 Effort boards + 18 Power Rank boards load per anchor). `total_days=0` = all history; `reset=true` wipes the `class_activity_estimate` table first. **202** + background. Run after changing either clean-view threshold to apply it to history (the warmer applies it to the latest window automatically) |

**`giveaways` category - scope `giveaways:read`** (public - token optional; the showcase `/giveaways` page is the main consumer. Entering is a signed-in site-user action, not part of the public API)

| Endpoint | Returns |
|---|---|
| `GET /v1/giveaways/ongoing` | (**tokenless**) currently-open giveaways (accepting entries), soonest-ending first: `[{ id, title, description, prize_name, status, starts_at, ends_at, entry_count, winner_username }]`. `winner_username` is null until drawn; the prize code is never exposed. Compute odds from `entry_count`. Cached 30s |
| `GET /v1/giveaways/upcoming` | (**tokenless**) scheduled giveaways not yet open, soonest-starting first (same shape; `status="scheduled"`, `entry_count` 0 until it opens). Cached 30s |
| `GET /v1/giveaways/ended?days=` | (**tokenless**) giveaways ended in the last `days` days (default 7, max 30), most-recently-ended first (same shape; `status` `"drawn"`/`"closed"`, cancelled excluded). Cached 30s |

**`events` category - scope `events:read`** (public - token optional). A **push** stream so consumers stop polling rotations at the top of the hour.

| Endpoint | Returns |
|---|---|
| `GET /v1/events/stream` | (**tokenless**) a long-lived `text/event-stream` (SSE). On connect: a snapshot of the current state of every registered source; then one message per change the instant it happens. Each message is `event: <type>` + JSON `data: {type, data, ts}`. `type` ∈ `challenge`/`chaos`/`corruxion`/`fluxion`/`longshade`/`wild_mana`/`stampy`/`daily_bonuses`/`activity`/`server_status`/`trove_news`/`giveaways`/`game_update` (a new live build was mirrored). `: ping` keep-alive ~20s. Fans out across uvicorn workers via Redis pub/sub; exactly-once via a SET-GET dedup guard |

**`market` category - scope `market:read`** (read side; `POST /insert` is **master-only**)

| Endpoint | Returns |
|---|---|
| `GET /v1/market/listings?name=&price_min=&price_max=&last_seen_after=&hide_expired=true&sort=&limit=&offset=` | paginated marketplace listings (default sort newest-`last_seen` first; `hide_expired` filters past 7d / stale 3h+) |
| `GET /v1/market/items` | item names that currently have a stored listing (sorted) |
| `GET /v1/misc/interest-items` | (lives under misc, **tokenless**) the full allow-list of items the bot tracks; admin-managed via the master panel at `/admin/market/interest-items` |
| `GET /v1/market/items/{name}/summary` | min/max/avg/median price-each + listing count for one item |
| `POST /v1/market/insert?timestamp=` | **master-only ingest**: multipart `file` with the raw `GrainusMod.cfg` text. Listings upserted by UUID - re-scrapes bump `last_seen`, never duplicate. Subject to the **ingest cooldown** (see below) when called with an API token |

Bot scrapes the in-game marketplace hourly. Listings live in **Postgres** (`market_listing` table, alongside
leaderboards); each listing's UUID v1 is the primary key; `created_at` is decoded from the UUID's timestamp
(so it matches when the player posted in-game); `last_seen` is bumped on every re-scrape (UPSERT). Only items
on `gamedata/market_items.json` are persisted; the rest are dropped at ingest. (The interest allow-list itself
stays in Mongo.)

**Archive rate limit** - passing `hide_expired=false` on `/listings` (i.e. asking for the historical
tail past the 7-day in-game lifetime) pays a SECOND, tighter per-token bucket (default 10 req/min) on
top of the standard per-token cap. Same `X-RateLimit-Archive-*` headers as the leaderboards archive
throttle. Market doesn't use a day-count threshold because the 7-day listing lifetime already defines
fresh-vs-historical; the limit fires on the opt-in flag instead.

**Ingest cooldown (all `POST /insert` endpoints)** - backstop against a misbehaving bot
resubmitting the same dump every few seconds. Per-token, per-endpoint bucket (default
**1 submit per 5 min** - runtime-tunable as `ingest_cooldown_max` / `ingest_cooldown_window_seconds`
in the master admin panel). Bucketed independently per endpoint - a leaderboards push doesn't share
a budget with a market push. Returns 429 with `Retry-After` once exhausted. **API-token auth only**:
session-JWT calls from the portal "Manual cfg ingest" card bypass this cap, so the master can replay
captured cfgs / back-fills without waiting out the window. Bots should honor `Retry-After` and
suppress duplicate-anchor submissions client-side.

**`ocr` category - scope `ocr:read`** (token required - the OCR model is CPU-heavy, so it isn't a tokenless freebie)

| Endpoint | Returns |
|---|---|
| `POST /v1/ocr/character` | reads the in-game character stat sheet from a screenshot (multipart `file`; PNG/JPEG/WebP/GIF/BMP, ≤12 MB) → `{ stats: { <key>: { value, unit, raw, confidence, in_range, type_match, derived } }, matched, total_known, lines }`. `stats` keyed by canonical key (`physical_damage`, `critical_hit`, `power_rank`, …); `value` int for counts / float for percents. `derived:true` = computed, not read. Returns **503** if the OCR engine isn't installed, **400** on an unreadable image |

Self-hosted OCR (**RapidOCR / ONNX - no external service, no LLM**). The moddable UI varies wildly
(themes, fonts, columns, language), so raw OCR text isn't trusted: every recognized label is fuzzy-matched
against a CLOSED multilingual vocabulary (English + French — `app/trove/gamedata/character_stats.json`) so a
garbled or translated label still snaps to the right stat, and every value is sanity-checked against the
stat's expected type (int vs percent) + plausible range. `in_range`/`type_match` flag a value that parsed
but looks implausible (don't trust a stat with either `false`); `confidence` folds match-quality with those
checks. Labels whose value renders on a separate line (Power Rank in the equipment view, a detached
Coefficient) are paired across lines within a small window. **Coefficient** is special: it's often not shown,
so it's derived from the game's own formula — `floor(max(physical, magic) damage * (1 + critical_damage/100))` —
and `derived:true` marks a computed value; a shown Coefficient that agrees is a confirmed read, one that
contradicts the formula is surfaced but flagged low-confidence (a cheap consistency check on the damage reads).
The accuracy core (`app/trove/ocr/parse.py` + `vocabulary.py`) is engine-agnostic and unit-tested
without any OCR dependency — the engine (`engine.py`) just turns pixels into the text lines it consumes, and
is swappable (Tesseract/EasyOCR) behind the same interface. The image is processed in memory, never stored.
The engine import is guarded, so the app still boots without the optional `rapidocr-onnxruntime` dependency
(the endpoint then returns 503); the Docker image installs it plus the `libgl1`/`libglib2.0-0` runtime libs.

## BetterTroveTools showcase site (`trove.aallyn.net`)

The api container ALSO serves the BTT marketing/manual site out of `site/`
(templates + static + ~20 MB of screenshots). Routes:

| Path | Returns |
|---|---|
| `GET /` | the BTT landing page (index.html) |
| `GET /documentation` | the user manual |
| `GET /commands` | searchable in-game slash-command reference |
| `GET /leaderboards` | hourly in-game leaderboard browser (charts, cheaters) |
| `GET /activity` | Player Activity - live active-player estimate + multi-period trend charts (1D…all-time) |
| `GET /player/<name>` | public player profile - leaderboard appearances + verified-claim badge (consumes `/site/leaderboards/players/<name>/profile`) |
| `GET /updates` | per-server (Live US / PTS) game-update file explorer + version diff + in-browser preview of small text files |
| `GET /support` | "support the project" landing for the navbar heart icon |
| `GET /status` | Trove server-status page - live EU/US/PTS state + downtime-history timeline |
| `GET /mods` · `/mods/{handle}/{slug}` | Mods Hub - browse/download shared mods (public) + the signed-in studio to develop, version & publish them |
| `GET /static/*` | site assets (bind-mounted from `site/static/`) |
| `GET /site/*` | page-side JSON proxies (leaderboards, updates) - same-origin, no token |
| `GET /api-info` | the old developer-card landing (lives here so `/` is free for the site) |

Point your reverse proxy: `trove.aallyn.net` → the api container's `:15546`,
forward all paths. `api.aallyn.net` keeps its existing filter to `/v1/*` +
`/health`. The site's CSP is broader than the API's (loads FontAwesome + Google
Fonts from CDN, calls `api.aallyn.net` for release data) - middleware picks
the right CSP per path.

> **Removed 2026-06:** `/unlock_debug` and `/unlock_fps` byte-patcher routes
> were deleted after Trion shipped anti-cheat. Any binary tampering is now
> grounds for a ban; the tools shouldn't exist anymore.

A background relayer polls the configured GitHub repo every 30 min and stores releases in Mongo, so the
endpoints serve from cache. Channels are detected from GitHub's `prerelease` flag (`release`/`beta`).
Platform assets are detected by file extension - `.msi`/`.exe` for windows (msi prioritized), `.AppImage`/
`.deb`/`.rpm`/`.tar.gz` for linux, `.apk` for android.

**`mods` category - scope `mods:read`** (stateless `.tmod` tooling - 20 MB body cap on `/v1/mods/*`)

| Endpoint | Returns |
|---|---|
| `POST /v1/mods/read` | decompile a `.tmod` (POST raw bytes) → header properties + file table; `?metadata_only=` omits contents |
| `POST /v1/mods/build` | build a `.tmod` from header fields + files (base64) → raw bytes |

The `.tmod` binary format (little-endian header + LEB128 + zlib file stream + Trove's FNV-1a-variant
checksum) is ported in pure Python (`app/trove/tmod.py`) - no native lib. `build` stamps the `modLoader`
header `KiwiAPI` (where BetterTroveTools uses `BTT`); nothing is stored - built in memory and discarded
after sending.

**Mods Hub** (the stored, git-like mod-sharing hub - `app/trove/mods_hub`)

Distinct from the stateless tools above: the Mods Hub *stores* mods. A *project* owns *releases* plus
banner/preview images, **forking** (copy + credit), **inspired-by attribution** (pointer + credit) and
**stars** (a logged-in user favourites a mod - `ModStar` + a denormalized `star_count`, with a "Most starred"
sort and a Starred list on the dashboard) - the original work is always surfaced on derivatives. Each project
has a **mode**:
- **files** - full versioned workflow (git commits/branches/clone) + releases. A release is compiled
  server-side from a commit into a chosen **format** - `.tmod` (via `build_tmod`) or `.zip` - or an uploaded
  prebuilt build. When compiling a `.tmod`, the modder picks one of the project's **preview images** (or uploads
  a new one) to embed as `ui/<slug>.<ext>` with a `previewPath` header property (the Trove convention - the
  image lives only in the artifact, never committed to the repo), and can set the **author(s)** stamped into the
  `.tmod` (`author` - comma-separated for several, default the owner's name). Can set **`source_visibility=private`**
  to keep the files/git owner-only and expose just the releases (an internal-tool mode).
- **releases** - releases-only: upload already-built `.tmod`/`.zip` files; no file history, git or files view.
  No preview embedding (the build is already compiled) - previews still enrich the project page.

**Trove file placement** (`app/trove/mods_hub/trove_layout.py`, ported from BetterTroveTools): only files inside
one of Trove's 11 override folders (`blueprints/`, `ui/`, `prefabs/`, `models/`, `textures/`, …) are compiled -
root files and non-Trove folders (`bin/` etc.) and non-content extensions are ignored. The project page shows a
warning panel for skipped files, and for **misplaced** files (a file whose name matches a real game file but
sits at the wrong path, looked up in the updates-archive game index) offers a one-click **Fix file placement**
that commits the moves to the game's paths. The game-index check degrades to unavailable if the updates archive
isn't running; the compile filter always applies.

**Branches are variants.** Each release records its `branch`; the page groups releases per branch (latest of
each surfaced, older collapsible) and the owner can hide chosen variants from the public
(`hidden_release_branches`) and **set the order the variants display in** (`branch_order` - up/down controls on
each variant head; branches not listed fall to the end alphabetically). **Each branch has its own version
timeline** - release tags are unique per
`(project, branch)`, so branch X and branch Y can both ship a `v1.0` (the unique index is
`project_id+branch+tag`; the old project-wide `project_id+tag` index is dropped on startup). **Forking** copies
the source, so it's allowed only when the source is visible to the forker - a **source-locked** mod (private
source / releases-only) can't be forked, only credited as **inspiration** (`fork_project` raises 403; the UI
swaps the Fork button for "Use as inspiration").

**Addressing (`<handle>/<slug>`).** A mod is `/mods/<owner_handle>/<slug>` everywhere (page, `/v1/mods/*`,
`/site/mods/*`, git). The **handle** is the owner's canonical `SiteUser.username` (denormalized to
`ModProject.owner_handle`, resynced on owner writes + when they open My Mods), so **slugs only have to be unique
per owner** - two modders can both own a `my-mod`. The unique index moved `slug` → `(owner_id, slug)` (old global
`slug_1` dropped on startup; `backfill_owner_handles()` sets the handle on pre-existing mods). Renaming Discord
changes a modder's mod URLs + git remotes (the create form warns about this). Master actions address by **project
id** (not slug). `get_project(handle, slug)` resolves the handle → owner → `(owner_id, slug)`.

**Download filename = the `.tmod`'s internal `title`.** Trove identifies a mod in-game by the `title` property
baked into the `.tmod`, so `release_download_filename()` serves the artifact as `<internal title>.tmod` (not
`slug-tag.tmod`) - a mismatch breaks the mod. Applied at download time (fixes old releases too) and stored on new
releases; zips keep their `slug-tag.zip` name.

**Owner links.** A project carries up to three kinds of owner-provided links shown as buttons under the title:
a **Discord invite** (`discord_url`), a **website** (`website_url`) and up to **5 donation links**
(`donation_urls` - the page auto-detects Ko-fi/Patreon/PayPal/Buy-me-a-coffee/GitHub-Sponsors for the icon +
label). All are validated server-side (`http(s)` only, ≤300 chars) and editable from **Edit details**.

**Project page (GitHub-style).** Wide two-column layout: file browser + rendered `README.md` on the left, an
**About → Releases → commit history → clone** sidebar on the right. Owner actions are inline rather than a
toolbar - **Edit details** + a **gear** sit beside the title, **Commit files** / **New release** / **Add
previews** are right-aligned in their section headers, the **banner is clickable** to add/change it, and a new
**branch** is created from the branch dropdown. The gear opens a **Settings** modal holding the structural
options (`mode`, `source_visibility`) and a **Danger zone** with the **Delete** button (kept off the main page
so deletion isn't a one-click slip).

**Markdown rendering** (`mods_project.js`). READMEs/descriptions render GitHub-flavored markdown **plus a safe
subset of raw HTML** (shields-style badges, `<div align>`, `<br>`, tables, blockquotes, …): markdown → HTML with
raw HTML passed through, then the whole output runs through a **DOM allowlist sanitizer** (`sanitizeHTML` -
tag/attribute allowlist, `javascript:`/`data:`/`on*` stripped, links forced `rel="noopener nofollow ugc"`), so
author content renders richly with no XSS surface. In **releases-only** mode there's no `README.md`, so the
owner edits a saved **`readme_text`** (rendered as the main content; **ignored** once the mod switches to files
mode, which renders the repo's `README.md` instead). **Warnings** (`warnings`, edited under the description) show
as highlighted yellow blocks below the description - **one block per `<br>`**.

**Modder profiles** (`/mods/<handle>`). A profile **only exists once the modder has ≥1 public, non-taken-down
mod** (nothing to showcase otherwise) — `profile_view` returns `None` and the page 404s until then. Each modder
gets a customizable home page (`ModProfile`, lazily created on first edit): **avatar** (custom upload, else their Discord avatar), **banner**, display name, tagline, a
markdown **README**, **socials** (Discord / website / up to 5 donation links), and a grid of their mods (owner
sees drafts; others see public only). It's a **two-column** layout: README + the mods grid on the left, a
sidebar on the right with an **About** section (socials + meta) and a single **Highlighted** mod. The owner sets
the **mod order** (`mod_order` - up/down on each card) and **highlights one** (`featured_slug` - a pin on each
card → shown in the sidebar). Edited via `/v1/mods/hub/me/profile` (+ `/avatar`, `/banner` uploads), read via
`/v1/mods/hub/profile/{handle}` + the `/site/mods/profile/{handle}` proxy. The mod page's author name and the
dashboard both link here, and the page emits per-modder OG/Twitter tags. The markdown renderer + DOM-allowlist
sanitizer were extracted into a shared `md_render.js` (`window.BTTMarkdown`) so the mod + profile pages share
**one** copy of the XSS-safe sanitizer. The renderer matches GitHub READMEs: single newlines are **soft breaks**
(badge rows flow inline), and the site CSP's `img-src` allows **any https image** so shields-style badges +
screenshots render.

**Link unfurls.** The `/mods/{handle}/{slug}` page is client-rendered, but the route fetches the mod
(anonymously) to emit **per-mod Open Graph + Twitter-card** tags - real title, summary and **banner image** -
so a shared link unfurls properly in Discord/Twitter/etc. Drafts / private / not-found fall back to generic tags
(nothing private leaks into an embed); the page still renders for the owner.

The website-internal hub surface — the `/v1/mods/hub/*` reads, the site-auth-gated write API, and the
same-origin `/site/mods/*` proxies — is **deliberately kept OUT of the public OpenAPI reference**
(`include_in_schema=False`); the showcase `/mods` + `/mods/{handle}/{slug}` pages drive it. Browse + download are public;
developing/submitting is a signed-in **site-user** action (Discord login on the User Dashboard - *not* the dev
portal).

**Documented public catalog API** (`mods_public_router`, in the OpenAPI reference, tokenless `mods:read`) is the
app-facing surface — it returns **absolute** image / download / page URLs so external apps can consume it
directly:
- `GET /v1/mods` — list/browse published mods (filter by `q`/`tag`/`author`, `sort` ∈ recent|popular|downloads|stars|new|title).
- `GET /v1/mods/{handle}/{slug}` — full metadata + published releases for one mod.
- `GET /v1/mods/popular` — the **25 most popular** mods by a 0.0-1.0 `popularity_score`.
- `POST /v1/mods/lookup` — resolve mod + release metadata from **one or many artifact hashes** (sha256 hex);
  returns `results` keyed by hash plus an `unknown` list (an app can identify installed `.tmod` files).
- `GET /v1/mods/categories` — the fixed category vocabulary + each category's bit value.

**Categories** (`app/trove/mod_categories.py`). A fixed, append-only vocabulary (Allies, Banners, Boats and
Sails, … Radar — 19 categories, each owning one bit). A mod's categories are stored **twice**: the natural Trove
way as plain strings in the `tags` property, and as a compact integer **bitmask** in a `flags` header property
(`flags=160` ⇒ Dragons|GUI), so a consumer recovers the exact set from one number instead of string-matching.
`build_tmod` auto-derives `flags` from any category tags; `read_tmod` decodes `flags` back into `categories`.
The public DTO exposes both `categories` (labels) and `flags` (number); the project page edits them as toggle
chips merged into the tags. The browse page's **tag filter** is fed by `service.tag_facets()`
(`GET /v1/mods/hub/tags` + the `/site/mods/tags` proxy) — per-tag **counts** across the public catalog, the fixed
categories shown first, then custom tags by descending count.

**Hash ownership.** Every release stores the artifact's **sha256 content hash** (`ModRelease.tmod_sha`) with a
denormalized `owner_id`; a hash belongs to whoever first published it. Releasing an artifact whose exact hash is
already owned by a **different** creator is rejected (409) — anti-reupload. The same owner may reuse their own
artifact across branches/projects.

**Popularity.** Each download logs a `ModDownloadEvent` (TTL-pruned to ~8 days). A periodically-refreshed
(lazy, ≤10 min, no background job) recompute writes a per-project `downloads_7d` and a normalized 0.0-1.0
`popularity_score` (a log-damped blend weighted toward the last 7 days of downloads, with stars + lifetime
downloads as a floor, scaled so the top mod ≈ 1.0). `sort=popular` and `/v1/mods/popular` read the snapshot.

**git as the source of truth.** Each project is a real bare git repo (pure-Python **dulwich**, no `git` binary)
under `mods_store_dir` (`/data/mods`, bind-mounted). File content + history live in git - a web "Commit files"
and a `git push` land in the **same** history; commits/branches/trees are read live from git (a push needs no
DB sync). Releases compile a `.tmod` from a git tree; only release artifacts + images stay in the CAS.

An **authenticated git smart-HTTP server** (`app/trove/mods_hub/git_http.py`) serves
`git clone/pull/push https://api.aallyn.net/git/mods/<handle>/<slug>.git`. Auth is HTTP Basic with a **git access token**
as the password (site login is Discord-only, no password) - users mint tokens from the studio's "Access tokens"
panel (`/v1/mods/hub/me/git-tokens`). Public/unlisted repos clone anonymously; drafts require the owner; push
requires the owner. Body cap on `/git/*` is `mods_git_max_body_bytes` (100 MB).

**Stray (imported, unclaimed) mods** (`app/trove/mods_hub/strayimport.py`). A *stray* mod is a `ModProject` with
`owner_id=None` + `is_stray=True`, mirrored from an external mod catalog and not yet attributed to a site user.
They carry their original `author` + a single mirrored release, live at the reserved handle **`/mods/stray/<slug>`**
(`STRAY_HANDLE`; `get_project` special-cases it), and appear in the normal public catalog once approved.
**The external origin is an internal implementation detail - it is never named anywhere a user or admin can see
it** (no UI text, no API field, no source link). The import flow:
- **Bulk import** (admin) creates mods **approved/visible** and **mirrors every file** into the shared CAS; a
  later **resync** refreshes download counts + re-mirrors changed files and adds newly-found mods as **pending**
  (hidden) for **per-mod admin approval**. Throttled, resumable background job; idempotent by `(source, source_id)`;
  progress in `StrayImportState`. Driven from the dev-portal "Mods hub" tab.
- **Author** comes from the **`.tmod` header's `author`** property (multiple, comma-separated, supported); only
  `.zip` mods (no header) fall back to the source-listed author. **Download counts carry over**.
- **Claim → handover**: a signed-in user hits "This is my mod" (`POST …/projects/stray/<slug>/claim` →
  `ModClaimRequest`); a master approves in the admin panel, which **hands the mod over** (`handover_stray`: assigns
  the owner, clears `is_stray`, re-homes the slug to `/mods/<username>/<slug>`) - it's now an ordinary mod.
- **Public copy is deliberately ambiguous**: the badge says **"Stray"**, the notice says the mod was "uploaded via
  contributions", and the origin/source is **never exposed publicly** (`source`/`source_url` are stripped from the
  public `project_card` + `public_mod_dto`; the mod page shows no source link). The real source is kept **admin-
  only** (on `_stray_card`) for attribution + verifying claims. Models: `ModClaimRequest`, `StrayImportState`;
  `ModProject.owner_id` + `ModImageAsset.owner_id` are optional (None for stray).

**Master oversight** lives in the dev-portal master panel ("Mods hub" tab → `/admin/mods/*`): list **every**
modder's project (drafts included), take down / restore a reported project, or force-delete any project. The same
tab drives the **stray** import (`/admin/mods/stray/import`), the **pending approval queue**
(`/admin/mods/stray` + `…/approve`/`…/reject`), and **mod claims** (`/admin/mods/claims` + `…/approve`/`…/reject`).

**Hiding the whole feature.** The Mods Hub (and the Market) each have a master **feature toggle** -
`feature_mods_hub_enabled` / `feature_market_enabled`, flippable from the dev-portal **Configuration** tab
("features" category), no restart. OFF hides the navbar link + dashboard tab, 404s the pages and every endpoint
(`/v1/…`, `/site/…`, and `/git/mods/*`), and leaves all stored data intact for when it's switched back ON. The
gate is one runtime-config bool read via `app/core/features.py` (router-level dependency on the API side, a site-
router dependency + Jinja context flag on the site side).

**Modpacks** (user-curated bundles of hub mods - `app/trove/modpacks`)

A *modpack* groups several published Mods Hub mods so a player can grab them all at once. It is a thin layer
**over** the hub - it stores no mod content of its own, only **references** to mods - and rides the same master
toggle (`feature_mods_hub_enabled`; meaningless without the hub). Pages live at `/modpacks` (browse) +
`/modpacks/<handle>/<slug>` (one pack); the write API is `/v1/modpacks/hub/*` (site-login) with same-origin
`/site/modpacks/*` proxies; banners/images **reuse the hub's CAS + `ModImageAsset`** (served via
`/site/mods/image/<sha>` - one store). All hidden from the OpenAPI reference, same as the hub.

- **No releases, but variants.** A pack has no per-version releases; instead it has **variants** - named
  spin-offs (e.g. "Full" vs "Lite"), each its own ordered list of mod **entries**. One is the **default**
  (`default_variant`); display order is the variant list order. A new variant can **copy** another's mods.
- **Entries are references with a picked mod-variant + optional version lock.** Each `ModpackEntry` names a mod
  by its **stable `project_id`** (survives the mod being renamed/re-handled; `handle`/`slug`/`title` are
  denormalized + resynced on resolve), the **mod variant** (Mods Hub branch) to pull from, and a **version
  lock**: **OFF by default** - the entry tracks the **latest published `.tmod`** of that branch - or **ON**,
  pinning to a specific release tag (`locked_tag`) that never auto-updates even when the mod ships newer builds.
  The pack page lists **all mods + variants + the version each resolves to** (latest → `vX`, or 🔒 `vX` when
  locked), flagging any entry whose build is currently unavailable (mod removed/private, no published build).
- **Artifacts are built on the fly at download time** (so unlocked entries always reflect the current latest):
  the website downloads a **`.zip`** (each mod's `.tmod` under `mods/` + a `modpack.json` manifest), the API a
  **`.tpack`** - the **same container format as a `.tmod`** (`tmod.build_tpack`; same `modLoader` marker) packing
  each mod's `.tmod` as a "file", with the mod manifest carried in the header `manifest` property. Trove has no
  native `.tpack`; it's our own format, structurally identical to a `.tmod` so the existing reader round-trips it
  - a consumer tells a pack from a mod by the `.tpack` extension + the `manifest`/`packVersion` header properties.
  In **both** formats every packed `.tmod` keeps its exact **`<internal title>.tmod`** filename (via
  `release_download_filename`, case preserved - `build_tpack` passes `lowercase_paths=False`): **Trove validates a
  mod's filename against the `title` baked into its header and rejects a mismatch**, so the name is never invented.
- **Editing.** Owner-only inline editor on the pack page: edit details (title/summary/markdown description/
  **warnings**/tags/visibility/links), upload a **banner**, manage variants (add/rename/delete/make-default), and
  add/remove/reorder mods + set each one's branch + version lock. A variant's mod list is saved by **PUTting the
  whole ordered list** (`PUT …/variants/{name}/entries`) - add/remove/reorder/lock in one write.
- **Downloads + likes.** Each download bumps `download_count`; a signed-in user can **like** a pack (`ModpackStar`
  + denormalized `star_count`, with a "Most liked" sort) - both surfaced on cards and the pack header. The included-
  mods list shows each mod's **author** linked to their modder page (`/mods/<handle>`), and a mod's own page lists
  the **modpacks that include it** (a reverse link queried by the embedded `variants.entries.project_id`).
- **Documented app-facing catalog API** (`modpacks_public_router`, in the OpenAPI reference, tokenless `mods:read`),
  mirroring `/v1/mods/*` - returns **absolute** image / page / download URLs so external apps (e.g. Better Trove
  Tools) consume it directly: `GET /v1/modpacks` (browse cards; `q`/`tag`/`author`/`sort`∈recent|downloads|stars|new|title),
  `GET /v1/modpacks/{handle}/{slug}` (full detail - every variant + the mods it bundles, each with `author`/`author_url`
  + the version each resolves to + per-variant `download_url`/`zip_url`), `GET /v1/modpacks/{handle}/{slug}/download?variant=&format=tpack|zip`,
  and `GET /v1/modpacks/for-mod/{handle}/{slug}` (the reverse link - packs including a given mod). The website-internal
  `/v1/modpacks/hub/*` + `/site/modpacks/*` surface (incl. the `…/star` like endpoints) stays out of the reference.

**`updates` category - scope `updates:read`** (browse the archived game files - latest version)

| Endpoint | Returns |
|---|---|
| `GET /v1/updates/branches` | tracked branches (`live-us`, `pts`) with current version + file count |
| `GET /v1/updates/{branch}/versions` | captured version history, newest first |
| `GET /v1/updates/{branch}/changes?version=&ordinal=&type=` | per-file diff a version introduced (added/modified/removed paths); latest if unpinned |
| `GET /v1/updates/{branch}/tree?prefix=` | one directory level (ls-style); empty prefix = root |
| `GET /v1/updates/{branch}/file?path=` | (**tokenless**) a single file's bytes, streamed from the blob store (`/file/meta` for hash+size) |
| `GET /v1/updates/{branch}/file/view?path=` | preview payload for the in-browser viewer: UTF-8 `text` when the file is small (≤512 KB) + text-like, else `viewable:false` with a `reason` (`too_large`/`binary`/`missing`) so the client falls back to the raw `/file` download |

Kiwi mirrors Trove's update CDN into a content-addressed, deduped store (see "Game-file archive" below);
these endpoints serve the latest captured version. Loose files and TFA-extracted files are browsed
identically. Historical-version querying is the next layer.

**`codexes` category - scope `codexes:read`** (public - token optional; structured game data parsed from the archive. The showcase `/codexes` page is the main consumer, reading these via same-origin `/site/codexes/*` proxies)

| Endpoint | Returns |
|---|---|
| `GET /v1/codexes/types` | the codex types present for a branch, each with its entry count |
| `GET /v1/codexes/search?q=&type=&category=&tradable=&sort=` | cross-type search/filter (the unified search surface); each result carries its `type` |
| `GET /v1/codexes/{type}?search=&category=&tradable=&sort=&limit=&offset=` | entries of one type - filterable, sortable, paginated |
| `GET /v1/codexes/{type}/categories` | distinct categories (+ counts) in a type, for filter dropdowns |
| `GET /v1/codexes/{type}/entry?path=` | a single entry by its source prefab path |

Eight typed datasets - `ally`, `mount`, `dragon`, `memento`, `recipe`, `item`, `fish`, `badge` - parsed
from Trove's `prefabs/*.binfab` files (a protobuf-like wire format) with names/descriptions resolved via
the `languages/` locale tables. Parsed rows live in **Postgres** (`codex_entry` table, alongside
leaderboards/market), keyed by `(branch, path)` - the two modes (`live-us` + `pts`) are just rows with a
different `branch`, and `content_sha256` ties each row back to the exact source binfab. The indexer runs
after each archive sync: a full build the first time (or after switching to Postgres), then only the
changed prefabs (driven by the version delta), so a routine patch never re-parses the rest of the game.
The table is disposable - rebuildable from the archive at any time. All endpoints default to the `live-us`
branch (`?branch=pts` for PTS). Each entry carries identity (name, category, description, tradability)
plus decoded collectible bonuses: `mastery` (normal, from `meta/multipliers.binfab`), `mastery_geode`
(geode-mode, from `meta/geode_multipliers.binfab`), `power_rank`, and a `data` JSONB object of numeric
stat bonuses, visible/hidden ability refs, **recipe** structure (output + ingredients + requirements), and
(for geode companions) per-level upgrade-tree bonuses. The `$…` localization keys those carry (stat/slot
names, ability descriptions) and the referenced recipe item names are resolved at index time against the
archived `languages/` locale tables + item prefabs - so the served strings are the real in-game text.

More are added following the conventions in "Adding the real endpoints" below.

## Hosts

| Public domain             | Local target        | What it is                                              |
|---------------------------|---------------------|--------------------------------------------------------|
| `https://api.aallyn.net`  | `127.0.0.1:15546`   | Production API: `/v1/*` (e.g. `/v1/rotations/*`) + `/health` |
| `https://dev.aallyn.net`  | `127.0.0.1:25470`   | **Developer portal** SPA (login, tokens, activity, account, admin) |
| `https://docs.aallyn.net` | `127.0.0.1:25468`   | Static documentation site                              |

The portal (`dev.aallyn.net`) is a no-build vanilla-JS SPA that calls the API
cross-origin (CORS-enabled). Programs authenticate the API with an **API token
only** - there is no programmatic login; humans use the portal.

## Layout

Feature modules grouped by endpoint path (vertical slices); cross-cutting
infrastructure in `app/core/`.

```
app/
├── main.py             # app assembly: routers, middleware order, lifespan
├── core/               # config, database, security, errors, redis, mailer,
│                       #   ratelimit, limits, pagination, idempotency,
│                       #   observability (request-id), scopes, maintenance, …
├── auth/               # /auth/*  signup, login, sessions, oauth, account - owns User, Session
├── tokens/             # /tokens/*  mint/list/edit/rotate/revoke - owns ApiToken
├── usage/              # UsageEvent model + buffered recorder + aggregations
├── pageviews/          # PageView model + buffered recorder + site page-view analytics
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
   `app/core/scopes.py` (bits are permanent - never renumber/reuse).
3. Lists: `Depends(list_params)` + `paginate_newest_first(...)` → return `Page{items, next_cursor, has_more}`.
4. Writes return `201`/`200`/`204`; any write honours an `Idempotency-Key` header automatically.
5. Register the router in `app/main.py` and any new `Document` in `app/core/database.py`.
6. Errors: `raise APIError(status, ErrorCode.x, "msg")` - never a bare `HTTPException`.

## Run with Docker

```bash
cp .env.example .env     # then set SECRET_KEY, MONGO_*, REDIS_PASSWORD (+ captcha/SMTP)
docker compose up -d --build
curl http://127.0.0.1:15546/health   # {"status":"ok"}
```

Mongo lives in `./data/mongo`, both bound from the host. **Whenever a model's
indexes change, wipe the data dir** (no migration code is carried): `docker
compose down && rm -rf data/mongo && docker compose up -d --build`.

### Key configuration (`.env` - see `.env.example` for the full list)

| Variable | Required | Notes |
|---|---|---|
| `SECRET_KEY` | **yes** | JWT signing key - `python -c "import secrets;print(secrets.token_urlsafe(48))"` |
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
server {  # production API - only /v1 + /health
  server_name api.aallyn.net;
  client_max_body_size 8m;                 # default cap (matches the app)
  location /v1/      { proxy_pass http://127.0.0.1:15546; }
  location /v1/mods/ { client_max_body_size 20m; proxy_pass http://127.0.0.1:15546; }  # .tmod tools
  location = /v1/leaderboards/insert { client_max_body_size 20m; proxy_pass http://127.0.0.1:15546; }  # bot cfg dump (~16 MB at ~20k entries/board)
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
