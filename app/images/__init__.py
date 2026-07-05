"""User-designed images: a freeform, server-rendered (Pillow) image studio.

An ``ImageDesign`` is a canvas plus ordered layers (text / rect / image); text
layers may hold ``{variable}`` placeholders that bind to an event type's live data.
Designs render to PNG at a stable URL, so they work both standalone and as the image
of a customizable embed (see ``app/embed_templates.py``).
"""
