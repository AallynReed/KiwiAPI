# playerhud.swf
> The Player HUD is the persistent in-game overlay showing the player's class insignia, power rank, XP/prestige bar, active buff icons with cooldown radials, and the group/tether icon. It is always visible during normal gameplay.

**Document/main class:** `PlayerHUD` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 4 (`PlayerHUD`, `XpMeter`, `BuffIcon`, `prestigeMeter` / meter subclasses) + 3 `PlayerHUD_fla` timeline classes + ~4 asset wrappers

---

## Main class: `PlayerHUD`

`PlayerHUD` is the root HUD component. On construction it calls `super()` then registers frame scripts for frames 1, 11, and 21 (all `stop()`). All setup is deferred to `configUI()`, which hides the leader icon in the tether container, calls `useDiscoveryXPBar(false, false)` to set the default meter state, registers eight `ExternalInterface` callbacks (in Iggy), adds `ROLL_OVER/ROLL_OUT` tooltip listeners to the insignia, power rank, and all three XP meters, calls `setupTranslation()`, uppercases `powerRankLabel`, and adjusts the `powerRank` TextField's vertical position accounting for locale (Chinese: offset 0; others: −6 px).

In non-Iggy preview mode it calls `useDiscoveryXPBar(true, false)`, `setClassLevel("Knight", 12, 0.25, true, false, 0, "")`, and `setInsignia(5, 4)`.

### Public methods

- `useDiscoveryXPBar(geode : Boolean, prestige : Boolean) : void` — shows/hides the three XP meter variants: `geodeXpMeter` (Geode world), `troveXpMeter` (standard Trove), `trovePrestigeMeter` (prestige). Calls `updateLockPosition()` after.
- `setClassLevel(className, level, percent, locked, prestige, rollOverAmt, prestigeIcon) : void` — updates all three meters' `className` TextField width, `label`, and `percent`; sets the prestige meter's `ObjectPreview` icon and rollover-XP amount visibility; sets `lockIcon.visible`; calls `updateLockPosition()`.
- `updateLockPosition() : void` — positions `lockIcon.x` to the right of the active meter's class name text.
- `setPowerRank(pr : int) : void` — sets `powerRank.text`.
- `setInsignia(wingFrame, shieldFrame : int) : void` — calls `gotoAndStop` on `insignia.shield` and `insignia.wings` sub-clips.
- `updateBuff(idx, iconPath, timeRemaining, duration, stackCount, ?, bgColor, iconSize) : void` — creates a new `BuffIcon` at slot `idx` if needed (6-column grid, 50×50 px cells), sets icon texture and size, stack count, background color, and draws the cooldown radial sweep on `BuffIcon.cooldown.radial` using `Graphics.lineTo` with 8-point octagon approximation.
- `resizeBuffs(count : int) : void` — hides `buffs[count…]` to trim the visible list.
- `setTetherIcon(iconPath, isLeader : Boolean) : void` — sets `tetherArtContainer.groupIcon` ObjectPreview texture (resized to 35×35) and `leaderIcon.visible`.
- `updateTetherOutOfRange(outOfRange : Boolean) : void` — sets `tetherArtContainer.groupIcon.alpha` to 0.5 or 1.

### Key fields

- `insignia : MovieClip` — player insignia clip with `wings` and `shield` child clips (multi-frame symbol); tooltip on ROLL_OVER calls `ShowPowerRankTooltip`.
- `powerRank : TextField` — numeric power rank display; tooltip on ROLL_OVER calls `ShowPowerRankBreakdown`.
- `powerRankLabel : TextField` — "POWER RANK" label, uppercased on init.
- `tetherArtContainer : MovieClip` — contains `groupIcon : ObjectPreview` (group/tether icon) and `leaderIcon` (star or crown).
- `troveXpMeter : XpMeter` — standard Trove class XP bar.
- `geodeXpMeter : XpMeter` — Geode world XP bar (different layout, includes level in label).
- `trovePrestigeMeter : XpMeter` — prestige XP bar with additional `imgContainer.groupIcon : ObjectPreview` for prestige badge and `xpRollOverAmt/xpRollOverLabel` for overflow display.
- `lockIcon : MovieClip` — padlock shown when the class is locked.
- `buffs : Array` — dynamic array of `BuffIcon` MovieClip instances.
- `buffContainer : MovieClip` — parent container for all buff icons.
- `IMG_WIDTH : Number = 690`, `IMG_HEIGHT : Number = 408` — likely reference dimensions (unused in AS logic directly).
- `ICON_COLUMNS : int = 6` — buff grid column count.
- `TetherIconSize : Number = 35` — resize target for the tether icon.

### Frame scripts / timeline

- **Frame 1** (`frame1`): `stop()` — standard layout.
- **Frame 11** (`frame11`): `stop()` — alternate layout (console or second state).
- **Frame 21** (`frame21`): `stop()` — third layout state.

### Runtime dependencies & integration

- `ExternalInterface.addCallback("useDiscoveryXPBar", useDiscoveryXPBar)`.
- `ExternalInterface.addCallback("setClassLevel", setClassLevel)`.
- `ExternalInterface.addCallback("setPowerRank", setPowerRank)`.
- `ExternalInterface.addCallback("setInsignia", setInsignia)`.
- `ExternalInterface.addCallback("updateBuff", updateBuff)`.
- `ExternalInterface.addCallback("resizeBuffs", resizeBuffs)`.
- `ExternalInterface.addCallback("setTetherIcon", setTetherIcon)`.
- `ExternalInterface.addCallback("updateTetherOutOfRange", updateTetherOutOfRange)`.
- `ExternalInterface.call("ShowPowerRankTooltip", mouseX, mouseY+30)` — insignia hover.
- `ExternalInterface.call("ShowPowerRankBreakdown", mouseX, mouseY+30)` — power rank field hover.
- `ExternalInterface.call("ShowMasteryTooltip", mouseX, mouseY+30)` — (method present but listener not wired in configUI; likely used in extended subclass or leftover).
- `ExternalInterface.call("ShowLevelTooltip", mouseX, mouseY+30)` — XP meter hover.
- `ExternalInterface.call("HideTooltip")` — all ROLL_OUT handlers.
- `_locale == LOCALE_ZH` check — adjusts `powerRank.y` offset for Chinese locale.
- `ColorTransform` via `changeColor(clip, color)` — used to tint `BuffIcon.iconBackground`.

---

## Other game-specific classes

- `XpMeter` (extends `UIComponent`) — reusable XP bar component. Has `className : TextField`, `levelTextField : TextField`, and `xpBar : MovieClip`. `label` (level text) and `percent` (fill fraction) are data-invalidation properties; `draw()` updates `levelTextField.text` and `xpBar.scaleX` (proportionally to `_barScaleX * percent`). Captures initial `xpBar.scaleX` as `_barScaleX` via a frame listener (`listenForFrame`/`setUpXpBar`). Used for all three meter instances (`troveXpMeter`, `geodeXpMeter`, `trovePrestigeMeter`) with layout differences managed by the parent.

- `BuffIcon` (extends `MovieClip`, embeds `assets.swf#symbol22`) — buff icon tile. Contains `cooldown : MovieClip` (with `radial : Sprite` for the pie-wedge sweep), `icon : image` (ObjectPreview), `displayCount : TextField`, `iconBackground : MovieClip`. No logic; all population done by `PlayerHUD.updateBuff`.

### PlayerHUD_fla timeline symbols

- `powerRank_15` (embeds `assets.swf#symbol158`) — the insignia MovieClip containing `wings` and `shield` child clips; single-frame stop.
- `wings_16` (embeds `assets.swf#symbol116`) — wings sub-clip; single-frame stop.
- `shield_17` (embeds `assets.swf#symbol157`) — shield sub-clip; single-frame stop.

### Asset wrappers (no logic)

~4 classes: `dummy`, `image` (base ObjectPreview class alias), `xpMeter_2`, `xpMeterGeode`, `masteryMeter`, `prestigeMeter` — trivial MovieClip or UIComponent subclasses with no game-logic code beyond inherited behavior; `xpMeter_2`/`xpMeterGeode`/`masteryMeter`/`prestigeMeter` are styled variants of the XP bar used in timeline symbol placement.

---

## Notable logic

- **Three XP bar modes**: `useDiscoveryXPBar(geode, prestige)` selects exactly one of three `XpMeter` instances to display. Geode mode shows `geodeXpMeter`; prestige mode shows `trovePrestigeMeter` (with prestige badge `ObjectPreview` and optional rollover-XP overflow counter); otherwise `troveXpMeter`. Lock icon position is updated accordingly.
- **Cooldown radial sweep (pie chart)**: `updateBuff` draws a filled-arc sweep on `BuffIcon.cooldown.radial` using an 8-point octagon tessellation. It iterates `k = 0..7` computing angles `k * PI/4 - PI/2`, breaking when the angle exceeds `(1 - timeRemaining/duration) * 2*PI - PI/2`, then draws a final `lineTo` at the exact sweep angle. Fill opacity is 0.5 (dark overlay).
- **Locale-aware layout**: `powerRank.y` is offset by −6 px for all locales except Chinese (`LOCALE_ZH`), working around Chinese character metrics differences.
- **Buff grid**: Buffs are laid out in a 6-column grid at 50×50 px intervals. If `updateBuff` is called for an index beyond the current `buffs` array length, a new `BuffIcon` is instantiated and added as a child of `buffContainer`.
- **`geodeXpMeter` level label**: Unlike the other meters where `label` is just a number, `geodeXpMeter.label` is set to `" - LVL " + level`, and `levelTextField.x` is repositioned to follow the `className` text width dynamically. A `highlight` child clip's `x` is also set proportionally to `percent * 244 - highlight.width`.
