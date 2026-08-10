"""Dressing room: compose a Trove character out of the game's own appearance parts.

The Mods Hub assembles ONE creature - a mod's parts, or a native creature, onto the
skeleton the game's prefab binds them to. A player character isn't one prefab: its body
comes from a costume, its head and face from equipment styles, its weapon from another,
and only the CLASS prefab says which sockets it has and which weapon families fit them.
This package reads all of that out of the archive and hands the assembler an explicit
part list. See ``sockets`` for the wire formats, ``catalogue`` for the option lists and
``service`` for the assembled model.
"""
