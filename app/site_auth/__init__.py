"""Site-side accounts - public-facing user system at trove.aallyn.net.

Deliberately parallel to ``app/auth/`` (which powers the dev portal at
dev.aallyn.net). The two systems share NO data:

  - ``app/auth/`` owns the ``users`` + ``sessions`` collections, surfaces
    at ``/auth/*``, JWT subject + scope checks gate ``/v1/*`` API access.
    Audience: API consumers, bot operators, superusers.

  - ``app/site_auth/`` owns the ``site_users`` + ``site_sessions``
    collections, surfaces at ``/v1/site-auth/*`` and ``/site/auth/*``,
    JWT subject identifies a "player" rather than a developer. Audience:
    casual visitors to trove.aallyn.net - claim a Trove player name,
    save favourites, etc.

A person who happens to be both can keep two accounts. That's the price
of keeping a publicly-signed-up system from leaking into the API surface
that runs the leaderboards bot and the master ingest endpoints.
"""
