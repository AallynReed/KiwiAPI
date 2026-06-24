"""Mods Hub - a git-like system for sharing & developing Trove mods.

A *project* is the repo: it owns *branches*, each branch points at a *commit*,
and a commit is a full snapshot of the files that compose the mod (paths ->
content hashes). A *release* is a published build: either compiled server-side
from a commit's file tree (reusing ``app/trove/tmod.build_tmod``) or an uploaded
finalized ``.tmod``.

Storage split, mirroring ``app/trove/updates``:
  - bytes (file blobs, compiled .tmod, banner/preview images) live in a
    filesystem content-addressed store (``store.py`` -> ``ContentStore``);
  - metadata (projects/branches/commits/releases/images/reports) lives in
    MongoDB (``models.py``).

Browse + download is fully public (tokenless, ``mods:read``); developing /
versioning / submitting requires a signed-in *site* user (Discord login, the
User Dashboard - NOT the dev portal).
"""
