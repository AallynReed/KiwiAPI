# uips4jshared.swf

> PlayStation 4 **Japan** shared component library. This is **not a screen** — it is a collection of button-icon and cursor assets loaded by the PS4 Japan build's UI screens. Structure and class inventory are identical to [uips4shared.md](uips4shared.md); see that file for the full class inventory and overview. The sole functional difference is the swapped asset indices for the Circle and Cross face buttons, reflecting the traditional Japanese PS4 button convention (Circle = confirm, Cross = cancel).

**Document/main class:** none  
**SWF-specific classes:** 50

---

## Overview

See [uips4shared.md](uips4shared.md). All 50 classes are present with the same names and the same two-category structure (platform-neutral `btn_console_*` wrappers + PS4-specific `btn_PS4_*` bitmap icons).

---

## Notable classes

All classes are identical to uips4shared with one exception:

- `btn_PS4_X/png` — Cross button, embeds **`53_btn_PS4_X.png`** (index 53 in Japan vs 51 in the Western build)
- `btn_PS4_circle/png` — Circle button, embeds **`51_btn_PS4_circle.png`** (index 51 in Japan vs 53 in the Western build)

All other PS4-specific and platform-neutral classes are byte-for-byte identical to uips4shared.

---

## Platform-specific notes

- **Japan button layout**: Circle (○) is the confirm/primary action button in Japanese convention; Cross (✕) is cancel. This is reflected by the swapped embedded asset indices: Circle gets the lower-numbered asset (51) and Cross gets the higher (53), reversing the Western assignment.
- No `btn_console_dpad_highlight` class — same omission as the Western PS4 build.
- The `btn_PS4_options` bitmap retains the same non-square dimensions (129×87 px) and same asset path as the Western build.
- `cur_twp`, `cur_twc`, and all `btn_console_*` wrappers are identical to the Western variant including symbol indices.
