"""Self-hosted OCR extraction of Trove character-sheet stats.

Engine-agnostic and split so the accuracy-critical part (parse.py) is pure and
unit-testable without any OCR dependency.

The moddable in-game UI varies wildly (themes, fonts, columns, language), but the
SEMANTICS don't: every panel is `(number, known-stat-label)` pairs from a fixed
vocabulary. So we fuzzy-match labels against that closed set (a garbled or
translated label still snaps to the right stat) and sanity-check each value
against the stat's expected type + range. No external services.
"""
