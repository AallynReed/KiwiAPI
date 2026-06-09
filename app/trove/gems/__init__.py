"""Trove gem features: simulator (generate + augment + actions), evaluator, and builds.

Ported faithfully from BetterTroveTools (`models/trove/gem_*.py`,
`backend/gems_and_builds/`, `utils/gem_engine.py`). Pure compute + RNG - nothing
is persisted server-side; gem objects round-trip through the client.
"""
