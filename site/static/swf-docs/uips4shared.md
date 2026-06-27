# uips4shared.swf

> PlayStation 4 (Western regions) shared component library. This is **not a screen** — it is a collection of button-icon and cursor assets loaded by the PS4 build's UI screens. It provides no application logic; every class is a thin embed wrapper.

**Document/main class:** none  
**SWF-specific classes:** 50

---

## Overview

The library contains three categories of class:

1. **Platform-neutral button wrappers** (`btn_console_*`) — 16 `MovieClip` subclasses that each embed a named symbol from `/_assets/assets.swf`. These use abstract gamepad-role names (north/south/east/west, lt/rt/lb/rb, dpad, menu, view, analog, keyboard) so that game screens can reference a single logical button name regardless of which platform they run on.

2. **PS4-specific button icons** (`btn_PS4_*`) — 20 `BitmapData` subclasses, each embedding a numbered PNG asset from `/_assets/`. These are the actual controller button graphics: face buttons, triggers, bumpers, sticks, DPAD variants, the PS home button, Options button, and touch-pad button.

3. **Cursor clips** — `cur_twp` and `cur_twc`: two `MovieClip` embeds from `assets.swf`, representing the two cursor states used on console (pointer and crosshair, or equivalent).

4. **Keyboard icon** — `keyboard/png`: a `BitmapData` embed for the on-screen keyboard prompt icon.

5. **Xbox icon stub** — `btn_console_XB`: a `MovieClip` embed; present in all four platform libs as a generic "home/system" button slot, backed here by a PS4 assets bundle symbol.

---

## Notable classes

### Platform-neutral `btn_console_*` wrappers (MovieClip, from `assets.swf`)

- `btn_console_north` — abstract face-button North (Triangle on PS4)
- `btn_console_south` — abstract face-button South (Cross/X on PS4)
- `btn_console_east` — abstract face-button East (Circle on PS4)
- `btn_console_west` — abstract face-button West (Square on PS4)
- `btn_console_lt` — Left Trigger
- `btn_console_rt` — Right Trigger
- `btn_console_lb` — Left Bumper (L1)
- `btn_console_rb` — Right Bumper (R1)
- `btn_console_menu` — Menu / Options button
- `btn_console_view` — View / Share button
- `btn_console_XB` — System/Home button stub
- `btn_console_dpad` — full D-pad icon
- `btn_console_dpad_north` — D-pad up
- `btn_console_dpad_south` — D-pad down
- `btn_console_dpad_east` — D-pad right
- `btn_console_dpad_west` — D-pad left
- `btn_console_dpad_updown` — D-pad up+down combined icon
- `btn_console_dpad_updowneast` — D-pad up+down+right combined icon
- `btn_console_analog_side_left` — left analog stick (side profile)
- `btn_console_analog_top_left` — left analog stick (top-down view)
- `btn_console_analog_side_right` — right analog stick (side profile)
- `btn_console_analog_top_right` — right analog stick (top-down view)
- `btn_console_keyboard` — keyboard/text-input icon

### PS4-specific button icons (BitmapData, from numbered PNGs)

- `btn_PS4_X/png` — Cross button (`51_btn_PS4_X.png`)
- `btn_PS4_circle/png` — Circle button (`53_btn_PS4_circle.png`)
- `btn_PS4_square/png` — Square button
- `btn_PS4_triangle/png` — Triangle button
- `btn_PS4_L1/png` — L1 bumper
- `btn_PS4_L2/png` — L2 trigger
- `btn_PS4_R1/png` — R1 bumper
- `btn_PS4_R2/png` — R2 trigger
- `btn_PS4_LThumb/png` — Left thumbstick press (L3)
- `btn_PS4_RThumb/png` — Right thumbstick press (R3)
- `btn_PS4_analogL/png` — Left analog stick icon
- `btn_PS4_analogR/png` — Right analog stick icon
- `btn_PS4_DPAD/png` — Full D-pad icon
- `btn_PS4_DPAD_up/png` — D-pad up
- `btn_PS4_DPAD_down/png` — D-pad down
- `btn_PS4_DPAD_left/png` — D-pad left
- `btn_PS4_DPAD_right/png` — D-pad right
- `btn_PS4_DPAD_updown/png` — D-pad up+down combined
- `btn_PS4_DPAD_updowneast/png` — D-pad up+down+right combined
- `btn_PS4_options/png` — Options button (129×87 px, wider than face buttons)
- `btn_PS4_PS/png` — PS home button
- `btn_PS4_touch/png` — Touch-pad button
- `btn_PS4_thumbtop/png` — Thumbstick top view
- `btn_PS4_thumbside/png` — Thumbstick side view

### Cursors

- `cur_twp` — cursor state A (symbol11 in assets.swf)
- `cur_twc` — cursor state B (symbol14 in assets.swf)

### Keyboard

- `keyboard/png` — keyboard prompt bitmap

---

## Platform-specific notes

- This is the **Western PS4** build. Cross (X) is confirm (asset index 51); Circle is cancel (asset index 53).
- No `btn_console_dpad_highlight` class — that symbol exists only in the Switch and Xbox libraries.
- All `btn_console_*` wrappers reference `/_assets/assets.swf` (a PS4-compiled asset bundle); the symbol indices differ from those in the Switch/Xbox variant of the same assets.swf.
