"""Site-side accounts - public-facing user system at trove.aallyn.net.

Parallel to ``app/auth/`` (the dev portal) but shares NO data. ``app/auth/``
owns ``users``/``sessions`` at ``/auth/*`` and gates ``/v1/*`` via scope;
this package owns ``site_users``/``site_sessions`` at ``/v1/site-auth/*`` and
``/site/auth/*``, where the JWT subject identifies a player. Kept separate so
a public signup can never leak into the API surface that runs the leaderboards
bot and master ingest.
"""
