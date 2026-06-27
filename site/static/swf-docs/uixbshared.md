# uixbshared.swf

> Xbox shared component library. This is **not a screen** — it is a collection of button-icon and cursor assets loaded by the Xbox build's UI screens. Structure is essentially identical to [uiswitchshared.md](uiswitchshared.md); see that file for the full class inventory. Differences from the Switch variant are noted below.

**Document/main class:** none  
**SWF-specific classes:** 48

---

## Overview

See [uiswitchshared.md](uiswitchshared.md). All 48 classes are present with the same names, the same Xbox One-branded `btn_XBOne_*` bitmap icons, the same platform-neutral `btn_console_*` wrappers (including `btn_console_dpad_highlight`), and the same cursor and keyboard classes.

---

## Notable classes

All classes are identical to uiswitchshared. Both the class names and the embedded asset paths/indices match exactly:

- `btn_XBOne_A/png` embeds `50_btn_XBOne_A.png` — same as Switch
- `btn_XBOne_XB/png` embeds `34_btn_XBOne_XB.png` — same as Switch
- `btn_XBOne_menu/png` embeds `30_btn_XBOne_menu.png` — same as Switch
- `btn_console_dpad_highlight` — symbol1 in this lib's `assets.swf` — same symbol index as Switch
- `btn_console_dpad_north` — symbol17, matching the Switch variant

---

## Platform-specific notes

- No class-level differences from uiswitchshared were found — all 48 class files have identical content including embedded asset filenames and symbol indices.
- The distinction between uixbshared and uiswitchshared is therefore entirely in the compiled `/_assets/assets.swf` bundle they reference at runtime, not in the ActionScript source.
- Both Xbox and Switch libs include `btn_console_dpad_highlight`, which is absent from the PS4 libs.
- PS4-specific classes (`btn_PS4_*`) are entirely absent, as in uiswitchshared.
