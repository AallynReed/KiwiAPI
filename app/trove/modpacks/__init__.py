"""Modpacks - user-curated bundles of Mods Hub mods.

A modpack stores no mod content, only *references* to hub mods (which mod + which
variant/branch) and an optional per-mod version lock; unlocked entries track the
latest published build, resolved at download time. Packs have no releases but do
have *variants* (named spin-offs, each its own mod list). Artifacts are built on
the fly: a ``.zip`` for the website, a ``.tpack`` (``.tmod``-format container) for
the API. Reuses the Mods Hub CAS for images; gated by the same master toggle.
"""
