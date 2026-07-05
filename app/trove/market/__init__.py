"""Trove marketplace listings.

Ingested from a bot that scrapes the in-game marketplace (the GrainusMod cfg
file) every hour and POSTs the raw cfg text to ``/v1/market/insert``. That
endpoint is master-only (superuser API token). Read endpoints are gated by
the ``market:read`` scope.

The interest-items allow-list (``gamedata/market_items.json``) is the item names
the bot scans for, exposed via ``GET /v1/market/interest_items`` so the bot can
refresh it without a redeploy.

Storage: listings live in **Postgres** (``market_listing``, keyed by the in-game
UUID; ``pg_schema`` + ``pg_store``) - moved off Mongo for the same volume reasons
leaderboards were. ``last_seen`` is bumped on every re-scrape (UPSERT); ``created_at``
is decoded from the UUID timestamp on first sighting. The interest allow-list
(``MarketInterestItem``) stays in Mongo.
"""
