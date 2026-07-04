"""User-designed images: a freeform, server-rendered (Pillow) image studio.

A ``ImageDesign`` is a canvas (size + background) plus ordered **layers** (text /
rectangle / image). Text layers may contain ``{variable}`` placeholders; when a
design is *bound* to an event type, it renders with that type's live data (so an
embed banner can show the current challenge name, build version, …). Designs render
to PNG on demand at a stable URL, so they work both standalone (download / share)
and as the image of a customizable embed (see ``app/embed_templates.py``).
"""
