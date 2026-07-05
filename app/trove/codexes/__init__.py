"""Codexes: structured game data parsed from `prefabs/*.binfab` in the archive.

Typed datasets (allies, mounts, dragons, mementos, recipes, items, fish, badges,
styles) extracted from Trove's `.binfab` prefab files (a protobuf-like wire
format), with names/descriptions resolved via the `languages/` locale tables.
The indexer runs after each archive sync, driven by what actually changed, and
stores entries in Postgres (`codex_entry`) for serving under the `codexes:read`
scope.
"""
