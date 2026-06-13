"""Trove in-game leaderboards.

Ingested from a bot that periodically reads the game's exposed LeaderBot.cfg
file (one .cfg dump per scrape) and POSTs the raw text to ``/v1/leaderboards/insert``.
That endpoint is master-only (superuser API token). Read endpoints are gated by
the ``leaderboards:read`` scope.

Storage is PostgreSQL (see ``pg_schema`` / ``pg_store``), a dedicated DB separate
from the app's Mongo. The parser explodes each dump into relational rows:
- ``board`` / ``board_contest`` - per-board metadata + contest windows
- ``player``                    - name stored once (entries carry ``player_id``)
- ``entry``                     - one row per (board, anchor, rank), RANGE-partitioned
                                  by anchor (one partition per trove-day)
- ``activity_estimate``         - persisted active-player time-series points

``models`` keeps only the pure cadence/board-classification helpers (no documents).
"""
