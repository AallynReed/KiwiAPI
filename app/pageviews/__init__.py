"""Cookieless page-view / unique-visitor analytics for the showcase site.

Mirrors ``app/usage/`` (buffered recorder + Beanie document + Mongo aggregation) but
tracks public site PAGE loads, not authenticated API requests. Unique visitors come
from a daily-rotating salted hash of client IP + User-Agent (no cookie, no raw IP
stored). Surfaced master-only in the dev portal's "Site Analytics" tab.
"""
