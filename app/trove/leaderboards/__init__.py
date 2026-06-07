"""Trove in-game leaderboards.

Ingested from a bot that periodically reads the game's exposed LeaderBot.cfg
file (one .cfg dump per scrape) and POSTs the raw text to ``/v1/leaderboards/insert``.
That endpoint is master-only (superuser API token). Read endpoints are gated by
the ``leaderboards:read`` scope.

Storage shape (see ``models``):
- ``Leaderboard``       — one document per board (uuid is the stable id)
- ``LeaderboardEntry``  — one document per (board, timestamp, rank) row

Entries older than the retention window are pruned at the end of each insert
(no separate archive collection — keeps things simple for v1).
"""
