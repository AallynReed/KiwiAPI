"""Codexes: structured game data parsed from `prefabs/*.binfab` in the archive.

Eight typed datasets (allies, mounts, dragons, mementos, recipes, items, fish,
badges) extracted from Trove's `.binfab` prefab files (a protobuf-like wire
format), with names/descriptions resolved via the `languages/` locale tables.
The indexer runs after each archive sync, driven by what actually changed, and
stores entries in Mongo for fast serving under the `codexes:read` scope.
"""
