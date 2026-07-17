"""In-game Kiwi Store catalog (scope: store:read).

The StoreLog tmod (TroveScraper/StoreScraper) replaces ``ui/kiwistore.swf``
with a scraper that walks every store tab, records every product the engine
pushes (categories, prices in TWC/TWP, real-money price strings, promos,
limited-time deal countdowns, class tiles, texture layers, lootbox
probability text) and dumps it all into the mod cfg. The uploader POSTs that
cfg to ``POST /v1/store/insert`` (master token); the read endpoints serve the
current catalog + per-product price history.
"""
