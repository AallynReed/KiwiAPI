# uiswitchshared.swf

> Nintendo Switch shared component library. This is **not a screen** — it is a collection of button-icon and cursor assets loaded by the Switch build's UI screens. For structural context see [uips4shared.md](uips4shared.md); differences from that PS4 baseline are documented below.

**Document/main class:** none  
**SWF-specific classes:** 48

---

## Overview

The library follows the same two-tier architecture as the PS4 shared libs:

1. **Platform-neutral `btn_console_*` wrappers** — same 16 logical-role `MovieClip` classes as the PS4 libs, plus one additional class unique to Switch and Xbox: `btn_console_dpad_highlight`.

2. **Xbox One-branded button icons** (`btn_XBOne_*`) — 20 `BitmapData` subclasses embedding numbered PNG assets. The Switch build uses XBox One button artwork (A/B/X/Y face buttons, XB home button) rather than Nintendo-branded artwork; this is a common practice for Switch Unity/Scaleform titles that reuse an Xbox asset set.

3. **Cursors** — same `cur_twp` and `cur_twc` clips as the PS4 libs.

4. **Keyboard icon** — same `keyboard/png` bitmap.

5. **`btn_console_XB` stub** — present as in the PS4 libs.

Total class count is 48 rather than 50 because the Switch lib has no PS4-only assets (`btn_PS4_options`, `btn_PS4_PS`, `btn_PS4_touch`, `btn_PS4_thumbtop`, `btn_PS4_thumbside` are absent), but adds `btn_console_dpad_highlight` and `btn_XBOne_XB` (home button) plus the full set of Xbox face/shoulder icons.

---

## Notable classes

### Platform-neutral `btn_console_*` wrappers

Same set as uips4shared (north/south/east/west, lt/rt/lb/rb, dpad and its variants, analog sticks, menu, view, keyboard, XB) **plus**:

- `btn_console_dpad_highlight` — D-pad highlight/focus indicator clip (symbol1 in this lib's `assets.swf`). **Not present in either PS4 lib.**

### Switch/Xbox One button icons (BitmapData, from numbered PNGs)

- `btn_XBOne_A/png` — A button (`50_btn_XBOne_A.png`, 64×64)
- `btn_XBOne_B/png` — B button
- `btn_XBOne_X/png` — X button
- `btn_XBOne_Y/png` — Y button
- `btn_XBOne_LT/png` — Left Trigger
- `btn_XBOne_RT/png` — Right Trigger
- `btn_XBOne_LB/png` — Left Bumper
- `btn_XBOne_RB/png` — Right Bumper
- `btn_XBOne_LThumb/png` — Left thumbstick press
- `btn_XBOne_RThumb/png` — Right thumbstick press
- `btn_XBOne_analogL/png` — Left analog stick
- `btn_XBOne_analogR/png` — Right analog stick
- `btn_XBOne_DPAD/png` — Full D-pad
- `btn_XBOne_DPAD_UP/png` — D-pad up
- `btn_XBOne_DPAD_DOWN/png` — D-pad down
- `btn_XBOne_DPAD_LEFT/png` — D-pad left
- `btn_XBOne_DPAD_RIGHT/png` — D-pad right
- `btn_XBOne_DPAD_updown/png` — D-pad up+down combined
- `btn_XBOne_DPAD_updowneast/png` — D-pad up+down+right combined
- `btn_XBOne_menu/png` — Menu/hamburger button (`30_btn_XBOne_menu.png`)
- `btn_XBOne_view/png` — View/back button
- `btn_XBOne_XB/png` — Xbox/home button (`34_btn_XBOne_XB.png`)

### Cursors

- `cur_twp`, `cur_twc` — same cursor states as in the PS4 libs

### Keyboard

- `keyboard/png` — keyboard prompt bitmap

---

## Platform-specific notes

- Uses Xbox One artwork for all face/shoulder button icons, not Nintendo Switch-branded Joy-Con icons.
- `btn_console_dpad_highlight` is present here (and in uixbshared) but **absent from both PS4 libs**.
- The `btn_XBOne_XB` home button asset (`34_btn_XBOne_XB.png`) is identical between uiswitchshared and uixbshared — same file, same index.
- PS4-specific classes (`btn_PS4_options`, `btn_PS4_PS`, `btn_PS4_touch`, `btn_PS4_thumbtop`, `btn_PS4_thumbside`) are entirely absent.
- Symbol indices in `btn_console_*` wrappers differ from the PS4 builds (e.g. `btn_console_dpad_north` is symbol17 here vs symbol18 in uips4shared), reflecting a different symbol table in this platform's `assets.swf`.
