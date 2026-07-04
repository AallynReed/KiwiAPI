"""Outbound webhooks: POST a rendered Discord embed to a user-registered Discord
webhook URL when one of the subscribed live events fires.

A push alternative to the SSE stream (``/v1/events/stream``) for users who just
want a message in their Discord. Only three event types are deliverable -
``challenge``, ``mod_release`` and ``game_update`` - and only **Discord** webhook
URLs are accepted, so there is no arbitrary-host outbound (no SSRF surface).

Owned by ``SiteUser`` and managed in the User Dashboard.
"""
