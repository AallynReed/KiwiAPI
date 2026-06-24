"""Cookieless page-view / unique-visitor analytics for the showcase site.

Mirrors the ``app/usage/`` pattern (buffered recorder + Beanie document + Mongo
aggregation), but tracks public site PAGE loads rather than authenticated API
requests, and counts unique visitors via a daily-rotating salted hash of the
client IP + User-Agent (no cookie, no raw IP stored). Surfaced master-only in the
dev portal's "Site Analytics" admin tab.
"""
