"""Trove game-file version archiver (scope: updates:read).

Probes Trion's update CDN, downloads only changed files, extracts the changed
logical files out of Trove's `.tfa`/`.tfi` archives, and stores them once in a
content-addressed blob store so the full version history is walkable with minimal
space. Live and PTS are tracked as separate timelines over a shared blob store.
"""
