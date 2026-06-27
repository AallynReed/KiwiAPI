# armorhud.swf
> A compact HUD element that displays the player's current armor value as a hexagonal fill gauge with a numeric readout. Appears in adventure, PvP, build, and discovery modes; the frame graphic changes between adventure/PvP and build/discovery variants.

**Document/main class:** `ArmorHudUI` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 2

## Main class: `ArmorHudUI`

`ArmorHudUI` is a minimal HUD widget. The constructor validates that `fill_mc` and `fillMask_mc` exist on the timeline, caches `maskInitialY`, and immediately calls `updateUI()` to render the initial state. A single frame script at frame 1 calls `stop()`.

`configUI()` registers five Iggy callbacks. When not in Iggy (authoring preview) it defaults to `MODE_ADVENTURE` mode.

### Public methods
- `setCurrentArmor(value:Number) : void` — stores `currentArmor` if ≥ 0.
- `setMaxArmor(value:Number) : void` — stores `maxArmor` if ≥ 0.
- `updateUI() : void` — computes fill ratio `currentArmor / maxArmor`; moves `fillMask_mc.y` by `(1 - ratio) * armorFullHeight` (65 px full range) to clip `fill_mc` from the top; sets `armorText.text` to a digit-delimited number via `KiwiTextUtil.addDigitDelimiters`.
- `onUpdateUI(redraw:Boolean) : void` — calls `updateUI()` if `redraw` is true; otherwise no-op.
- `setVisibility(visible:Boolean) : void` — sets `this.visible`.
- `setMode(mode:String) : void` — sends `hexFrame_mc` to frame 2 for `"build"` or `"discovery"` (alternate graphic), frame 1 for `"adventure"` or `"pvp"` (default graphic); then calls `updateUI()`.

### Key fields
- `armorText : TextField` — numeric armor value display.
- `fill_mc : MovieClip` — the colored fill graphic (clipped by mask).
- `fillMask_mc : MovieClip` — mask that slides vertically to reveal `fill_mc` proportionally.
- `hexFrame_mc : MovieClip` — the hexagonal border; frame 1 = combat, frame 2 = build/discovery.
- `maxArmor : Number` — maximum armor for ratio calculation.
- `currentArmor : Number` — current armor value.
- `shieldWidth : Number = 55` — unused reference dimension.
- `armorFullHeight : Number = 65` — full pixel height of the fill range.
- `maskInitialY : Number` — `fillMask_mc.y` at construction (base of the gauge).
- Mode constants: `MODE_ADVENTURE = "adventure"`, `MODE_BUILD = "build"`, `MODE_PVP = "pvp"`, `MODE_DISCOVERY = "discovery"`.

### Frame scripts / timeline
- Frame 1 — `stop()`. Single static frame; no animation.

### Runtime dependencies & integration
**ExternalInterface callbacks registered (Iggy):**
- `setCurrentArmor` — updates stored armor value
- `setMaxArmor` — updates stored max armor
- `onUpdateUI` — conditionally redraws
- `setVisibility` — show/hide the widget
- `setMode` — switches frame graphic and redraws

**No ExternalInterface calls are made outward** — this widget is display-only; it only receives data from the game engine.

**`KiwiTextUtil.addDigitDelimiters`** — formats the armor number with thousands separators for display.

---

## Other game-specific classes

### `ArmorHUD_fla/hexFrame_mc_3` (extends `MovieClip`) — [Embed symbol14]
The hexagonal border frame symbol. Two frames (both `stop()`): frame 1 = default (adventure/PvP border style), frame 2 = alternate (build/discovery border style). Switched by `ArmorHudUI.setMode`.

---

## Notable logic
- **Mask-based fill:** Rather than scaling `fill_mc`, the gauge works by sliding `fillMask_mc` downward from the top: an empty gauge moves the mask to `maskInitialY + armorFullHeight` (fully covering the fill), while a full gauge leaves it at `maskInitialY` (fully revealing it). This preserves the hexagonal shape of the fill graphic.
- **Mode-gated frame:** Only two visual states exist for the frame border (`hexFrame_mc`); PvP uses the same frame as adventure, and discovery uses the same as build.
- **No stage-resize handling:** Unlike most UIComponent subclasses, `ArmorHudUI` does not override `onStageResized`; the widget is intended to be positioned and scaled externally by the HUD layout system.
