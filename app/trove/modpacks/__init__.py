"""Modpacks - user-curated bundles of Mods Hub mods.

A *modpack* groups several published mods so a player can grab them all at once.
It is a thin layer over the Mods Hub: it stores no mod content of its own, only
*references* to mods (which mod + which variant/branch was picked) and, optionally,
a pinned version per mod. By default each entry tracks the latest published build
of its picked variant; the maker can lock any entry to a specific version.

Modpacks have NO releases (nothing is compiled/published per-version) but DO have
*variants* - named spin-offs, each its own list of mods (e.g. "full" vs "lite").

Artifacts are built on the fly at download time, always resolving unlocked entries
to the current latest build:
  - website downloads a ``.zip`` (each mod's ``.tmod`` + a ``modpack.json`` manifest);
  - the API serves a ``.tpack`` - the same container format as a ``.tmod`` (see
    ``app/trove/tmod.build_tpack``) packing each mod's ``.tmod`` plus the manifest.

Storage reuses the Mods Hub: images live in the shared content-addressed store and
are served via ``/site/mods/image/<sha>``; metadata is one Mongo document per pack.
Browsing + downloading is public (tokenless ``mods:read``); creating/editing requires
a signed-in *site* user (Discord login). Gated by the same master toggle as the hub.
"""
