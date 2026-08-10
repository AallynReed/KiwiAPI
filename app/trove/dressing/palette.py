"""The hair and eye colours Trove's character creator offers.

These are NOT in the shipped game data. The recolour itself happens inside
``Trove_x64.exe``: no shader performs it, ``custom_heads_service.binfab`` carries no
palette, the locale tables name no swatches, ``ui/charcustomize.swf``'s ActionScript
constant pool holds only UI chrome, and a scan of the executable turns up no static
colour array near the customizer's own strings. ``KiwiColorPicker`` is handed its
choices at runtime.

So these were **transcribed from screenshots of the in-game picker**, in its own reading
order: six columns by three rows, left to right, top to bottom. That makes them accurate
to the eye and possibly a point or two out per channel. They live here, alone, so a
correction is a one-line edit rather than a hunt.
"""

from __future__ import annotations

# Hair Color, as the picker lays it out.
HAIR: tuple[str, ...] = (
    "#7a4e24", "#e01b0c", "#d6197a", "#3fd016", "#2a46b8", "#1e2224",
    "#b5762a", "#f08010", "#f221c7", "#2fd9a8", "#7a18e0", "#8a9096",
    "#c2b24a", "#f2e06a", "#f9a8d8", "#3fe0f0", "#6e8cf0", "#ffffff",
)

# Eye Color, likewise.
EYE: tuple[str, ...] = (
    "#7a1f1f", "#6b4a1f", "#1f5a33", "#1f5a9e", "#4a2a7a", "#1e2224",
    "#e01b0c", "#f08010", "#2fcf4a", "#5a93e8", "#8560c8", "#8a9096",
    "#f221c7", "#f2e020", "#3fe0a8", "#3fd8f0", "#a020f0", "#ffffff",
)

# The picker is a 6-wide grid; the client lays it out the same way rather than
# guessing a shape from the count.
COLUMNS = 6

BY_SLOT: dict[str, tuple[str, ...]] = {"hair": HAIR, "eyes": EYE}
