"""Tome payout valuations (drives the /tomes page).

A tome is filled by completing dungeons and pays out a fixed bundle of items.
Regular tomes are repeatable without limit; legendary tomes are one of each per
week, resetting Monday 00:00 UTC-11.

The payout table is static game data (``gamedata/tomes.json``); the value of a
payout is not, so it is joined against live marketplace medians at read time.
"""
