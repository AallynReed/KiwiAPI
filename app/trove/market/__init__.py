"""Trove marketplace listings.

Ingested from a bot that scrapes the in-game marketplace (the GrainusMod cfg
file) every hour and POSTs the raw cfg text to ``/v1/market/insert``. That
endpoint is master-only (superuser API token). Read endpoints are gated by
the ``market:read`` scope.

The interest-items list (``gamedata/market_items.json``) is the small allow-list
of item names the bot scans for. The list is also exposed via
``GET /v1/market/interest_items`` so the bot can refresh it without redeploying.

Storage shape (see ``models``):
- ``MarketListing`` - one document per in-game listing (its UUID v1 is the _id).
  ``last_seen`` is bumped every time the bot re-scrapes the same listing;
  ``created_at`` is decoded from the UUID's timestamp the first time we see it.
"""
