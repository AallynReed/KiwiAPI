"""Discord DM subscriptions.

Users opt in (via the User Dashboard) to targeted direct-message alerts from the
Kiwi bot: hourly challenges (by type), the Corruxion / Fluxion merchants, game
updates, and a per-user market price watchlist. Event-driven alerts hook the same
exactly-once event bus the webhooks feature uses; the market watchlist is checked
on each market ingest. Delivery is REST-only (the API opens a DM channel via the
bot token), so this needs no gateway/bot-container change.
"""
