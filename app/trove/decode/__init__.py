"""Game-data decoders: rebuild app/trove/gamedata/*.json from the game files.

Each decoder module exposes ``TITLE``, ``OUTPUT`` (file name), ``PREFIXES`` (the
game paths it reads), ``INDENT``/``FINAL_NEWLINE`` (repo file format), ``build(tree)`` and
``count(data)``. ``runner`` rebuilds them after every patch; ``python -m
app.trove.decode`` rebuilds the repo copies from a local client.

Kept import-free: readers only need ``store``.
"""
