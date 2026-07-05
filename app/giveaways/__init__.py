"""Prize-draw system: a vault of redeemable codes, scheduled giveaway windows, and a worker that auto-draws a winner and emails the code.

Endpoints:
  - ``/v1/giveaways/*``  - signed-in site users (enter, my entries)
  - ``/admin/giveaways`` + ``/admin/vault`` - master-only management
  - public read goes through ``/site/giveaways/*`` (see app/site/router.py)
"""
