"""Giveaways feature.

A small prize-draw system:
  - a "vault" of redeemable prize codes (``PrizeCode``),
  - scheduled ``Giveaway`` windows that reserve one vault code each,
  - one entry per signed-in site user (``GiveawayEntry``),
  - a background worker that auto-draws a random winner at the end date and
    emails them the code.

Endpoints:
  - ``/v1/giveaways/*``  - signed-in site users (enter, my entries)
  - ``/admin/giveaways`` + ``/admin/vault`` - master-only management
  - public read goes through ``/site/giveaways/*`` (see app/site/router.py)
"""
