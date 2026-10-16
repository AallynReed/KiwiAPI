# playerhud.swf
> The Player HUD is the persistent in-game overlay showing the player's class insignia, power rank, XP/prestige bar, active buff icons with cooldown radials, and the group/tether icon. It is always visible during normal gameplay.

**Document/main class:** `PlayerHUD` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 4 (`PlayerHUD`, `XpMeter`, `BuffIcon`, `prestigeMeter` / meter subclasses) + 3 `PlayerHUD_fla` timeline classes + 6 asset wrappers

---

## Main class: `PlayerHUD`

`PlayerHUD` is the root HUD component. On construction it calls `super()` then registers frame scripts for frames 1, 11, and 21 (all `stop()`). All setup is deferred to `configUI()`, which hides the leader icon in the tether container, calls `useDiscoveryXPBar(false, false)` to set the default meter state, registers eight `ExternalInterface` callbacks (in Iggy), adds `ROLL_OVER/ROLL_OUT` tooltip listeners to the insignia, power rank, and all three XP meters, calls `setupTranslation()`, uppercases `powerRankLabel`, and adjusts the `powerRank` TextField's vertical position accounting for locale (Chinese: offset 0; others: −6 px).

In non-Iggy preview mode it calls `useDiscoveryXPBar(true, false)`, `setClassLevel("Knight", 12, 0.25, true, false, 0, "")`, and `setInsignia(5, 4)`.

### Public methods

- `useDiscoveryXPBar(geode : Boolean, prestige : Boolean) : void` — shows/hides the three XP meter variants: `geodeXpMeter` (Geode world), `troveXpMeter` (standard Trove), `trovePrestigeMeter` (prestige). "Discovery" is the game's internal name for Geode (`$WorldPortalType_Discovery` is "Geode Portal") and "prestige" its internal name for Paragon (`$Inventory_Subheading_Prestige` is "PARAGON"). Calls `updateLockPosition()` after.
- `setClassLevel(className, level, percent, locked, prestige, rollOverAmt, prestigeIcon) : void` — updates all three meters' `className` TextField width, `label`, and `percent`; sets the prestige meter's `ObjectPreview` icon (resized to 60×60) and rollover amount; sets `lockIcon.visible`; calls `updateLockPosition()`. The fifth argument is never read. `rollOverAmt` is written to `xpRollOverAmt`, the number beside the `$PlayerHUD_Rollover` ("Millenia") label; both are hidden when it is 0 or less.
- `updateLockPosition() : void` — positions `lockIcon.x` 9 px right of `troveXpMeter`'s class name when that meter is visible, otherwise 9 px right of `geodeXpMeter`'s level text (also while the prestige meter is the one showing).
- `setPowerRank(pr : int) : void` — sets `powerRank.text`.
- `setInsignia(shieldFrame, wingsFrame : int) : void` — the first argument is the `gotoAndStop` frame for `insignia.shield` (frames 1–20), the second for `insignia.wings` (frames 1–26).
- `updateBuff(idx, iconPath, timeRemaining, duration, stackCount, ?, bgColor, iconSize) : void` — creates a new `BuffIcon` at slot `idx` if needed (6-column grid, 50×50 px cells), sets icon texture and size (`iconSize` × `iconSize`), stack count (blank unless above 0), background color, and draws the cooldown radial sweep on `BuffIcon.cooldown.radial` using `Graphics.lineTo` with 8-point octagon approximation. The sixth argument (a `Boolean`) is never read, and no buff id or display name is passed: the slot index and icon texture are the only identifiers.
- `resizeBuffs(count : int) : void` — hides `buffs[count…]` to trim the visible list.
- `setTetherIcon(iconPath, isLeader : Boolean) : void` — sets `tetherArtContainer.groupIcon` ObjectPreview texture (resized to 35×35) and `leaderIcon.visible`.
- `updateTetherOutOfRange(outOfRange : Boolean) : void` — sets `tetherArtContainer.groupIcon.alpha` to 0.5 or 1.

### Key fields

- `insignia : MovieClip` — player insignia clip with `wings` (26 frames) and `shield` (20 frames) child clips, both stepped by `setInsignia`; tooltip on ROLL_OVER calls `ShowPowerRankTooltip`.
- `powerRank : TextField` — numeric power rank display; tooltip on ROLL_OVER calls `ShowPowerRankBreakdown`.
- `powerRankLabel : TextField` — holds `$Stat_PowerRank` ("Power Rank"), translated by `setupTranslation()` and then uppercased.
- `tetherArtContainer : MovieClip` — contains `groupIcon : ObjectPreview` (group/tether icon) and `leaderIcon`, which is character 39, an empty sprite with no art, so `setTetherIcon`'s leader flag has no visible effect in the stock file.
- `troveXpMeter : XpMeter` — standard Trove class XP bar.
- `geodeXpMeter : XpMeter` — Geode world XP bar (different layout, includes level in label).
- `trovePrestigeMeter : XpMeter` — prestige (Paragon) XP bar with additional `imgContainer.groupIcon : ObjectPreview` for prestige badge (`imgContainer` is the same symbol as `tetherArtContainer`) and `xpRollOverAmt`/`xpRollOverLabel` (`$PlayerHUD_Rollover`, "Millenia") for the rollover count.
- `lockIcon : MovieClip` — padlock shown when the class is locked.
- `buffs : Array` — dynamic array of `BuffIcon` MovieClip instances.
- `buffContainer : MovieClip` — parent container for all buff icons.
- `IMG_WIDTH : Number = 690`, `IMG_HEIGHT : Number = 408` — likely reference dimensions (unused in AS logic directly).
- `ICON_COLUMNS : int = 6` — buff grid column count.
- `TetherIconSize : Number = 35` — resize target for the tether icon.

### Frame scripts / timeline

Stage 310 × 140, 24 fps, 30 frames in three labelled 10-frame blocks.

- **Frame 1** (`frame1`, label `PC`): `stop()` — the PC layout, which stays on this frame (there is no `PCLoc` label). Places `insignia` at (4, −16), `powerRankLabel` (6, 66), `powerRank` (10, 76), the three meters at (95, 18), `lockIcon` (145.5, −3.5), `buffContainer` (98, 51) and `tetherArtContainer` (30, 108).
- **Frame 11** (`frame11`, label `Console`): `stop()` — console layout: moves `buffContainer`, the three meters and `insignia`, and replaces `powerRankLabel`/`powerRank` with new fields. `UIComponent` picks it on console platforms for English and Chinese locales.
- **Frame 21** (`frame21`, label `ConsoleLoc`): `stop()` — the console layout with `powerRankLabel`/`powerRank` replaced again; picked on console platforms for every other locale.

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
- `ExternalInterface.call("ShowMasteryTooltip", mouseX, mouseY+30)` — dead: the private `showMasteryTooltip` handler is added as a listener nowhere, and the `masteryMeter` symbol is placed on no timeline, so the stock HUD never makes this call.
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
- `wings_16` (embeds `assets.swf#symbol116`) — wings sub-clip; 26 frames, each one 88×88 bitmap at (0, 0), `stop()` on frame 1. Frames 1–8 are `DefineBitsLossless2` with smoothed clipped fills (`0x41`); frames 9–26 are `DefineBitsJPEG3` with non-smoothed clipped fills (`0x43`, nearest-neighbour when scaled).
- `shield_17` (embeds `assets.swf#symbol157`) — shield sub-clip; 20 frames, each one 88×88 `DefineBitsJPEG3` bitmap at (0, 0) with a non-smoothed clipped fill (`0x43`), `stop()` on frame 1.

### Asset wrappers (no logic)

6 classes: `dummy`, `image` (base ObjectPreview class alias), `xpMeter` (decompiled as `xpMeter_2.as`), `xpMeterGeode`, `masteryMeter`, `prestigeMeter` — trivial subclasses with no game-logic code beyond inherited behavior (`dummy` is a 52×52 `BitmapData`; `image`, symbol 17, is the `ObjectPreview` placed as `BuffIcon.icon`, `cooldown.radial` and `groupIcon`). The four meter classes are `XpMeter` subclasses with `PC`/`Console` labels on frames 1/11 (`stop()` on each): `xpMeter` (symbol 59) is `troveXpMeter`, `xpMeterGeode` (38) is `geodeXpMeter`, `prestigeMeter` (49) is `trovePrestigeMeter`, and `masteryMeter` (13) is exported but placed on no timeline.

---

## Notable logic

- **Three XP bar modes**: `useDiscoveryXPBar(geode, prestige)` selects exactly one of three `XpMeter` instances to display. Geode mode shows `geodeXpMeter` and wins when both flags are true; prestige mode shows `trovePrestigeMeter` (with prestige badge `ObjectPreview` and optional Millenia rollover counter); otherwise `troveXpMeter`. Lock icon position is updated accordingly.
- **Cooldown radial sweep (pie chart)**: `updateBuff` draws a filled-arc sweep of radius 32 on `BuffIcon.cooldown.radial` using an 8-point octagon tessellation. It iterates `k = 0..7` computing angles `k * PI/4 - PI/2`, breaking when the angle reaches `(1 - timeRemaining/duration) * 2*PI - PI/2`, then draws a final `lineTo` at the exact sweep angle. The shaded wedge is the elapsed fraction, growing clockwise from 12 o'clock; fill opacity is 0.5 (dark overlay), and nothing is filled when `duration` is 0 or less. `PlayerHUD` has no frame or timer handler of its own, so the sweep only moves when `updateBuff` is called again.
- **Locale-aware layout**: `powerRank.y` is offset by −6 px for all locales except Chinese (`LOCALE_ZH`), working around Chinese character metrics differences.
- **Buff grid**: Buffs are laid out in a 6-column grid at 50×50 px intervals. If `updateBuff` is called for an index beyond the current `buffs` array length, a new `BuffIcon` is instantiated, positioned from `idx`, appended to the end of `buffs` and added as a child of `buffContainer`. Icons are never removed, only hidden by `resizeBuffs` and shown again by `updateBuff`. With `buffContainer` at x 98, the six columns run past the right edge of the 310 px stage.
- **`geodeXpMeter` level label**: Unlike the other meters where `label` is just a number, `geodeXpMeter.label` is set to `" - LVL " + level`, and `levelTextField.x` is repositioned to follow the `className` text width dynamically. A `highlight` child clip's `x` is also set proportionally to `percent * 244 - highlight.width`.
