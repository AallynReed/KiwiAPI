# dailyrewards.swf
> The Daily Login Rewards window, showing up to 7 day-tiles representing the current weekly reward cycle. Each tile displays an item icon, quantity, name, Flux bonus (for Patron members), and the day number. The window appears once per login day and is populated entirely by the game engine via ExternalInterface.

**Document/main class:** `DailyRewards` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 2

## Main class: `DailyRewards`

Creates an array `m_rewardItems` of 7 `DailyRewardItem` references (`DayBtn0`–`DayBtn6`) and a Patron upgrade button from `buyPatronMC.buyPatron`. Registers `clearAllRewards` and `addNewReward` as `ExternalInterface` callbacks. Frame scripts stop at frames 1, 11, and 21.

### Public methods
- `clearAllRewards() : void` — Hides all 7 day tiles (resets the display before a new populate sequence).
- `addNewReward(ownReward, isPatron, itemName, itemQuantity, numFlux, fluxName, iconStringPath, isLastDay, skipAhead) : void` — Shows the next available tile (`m_numRewards` index) and calls `DailyRewardItem.setup(...)`.
- `onPatronBtnClick(e:MouseEvent = null) : void` — Calls `ExternalInterface.call("OnPatronBtnClick")`.

### Key fields
- `DayBtn0`–`DayBtn6 : DailyRewardItem` — the seven day slots, publicly accessible child references.
- `m_rewardItems : Array` — ordered list of the 7 `DailyRewardItem` references for sequential population.
- `m_numRewards : int` — counter that advances with each `addNewReward` call to track which slot to fill next.
- `buyPatronMC : MovieClip` — container; its `buyPatron` child is a `btnGreen` (`LabelButton`) labelled `$Patron_ButtonLegend`.
- `__id1_ : WindowHeaderSmall` — header with empty title string (title likely set graphically rather than via text).

### Frame scripts / timeline
- `frame1` / `frame11` / `frame21` — each `stop()`. Three states (possibly open animation stages or alternate layouts).

### Runtime dependencies & integration
- `ExternalInterface.addCallback("clearAllRewards", ...)`, `("addNewReward", ...)`.
- `ExternalInterface.call("OnPatronBtnClick")` — opens the Patron purchase flow.
- `IggyFunctions.inIggy` — patron button listener only registered when in Iggy (not in preview mode).

---

## Other game-specific classes

### `DailyRewardItem` (extends `_kiwi.Core.UIComponent`) — Embed symbol127
Individual reward day tile. Contains `itemSlot : Slot`, `infoTxt : MovieClip` (with inner text fields `dayTitle_txt`, `finalDayTitle_txt`, `finalReward_txt`, `finalDayGlow_mc`), `patronText : MovieClip` (with `currencyText` showing "+N FluxName"), `ShineOne` and `ShineTwo` shine effect `MovieClip`s. Frame labels include: `"yesPatronDone"`, `"yesPatron"`, `"noPatronDone"`, `"noPatron"`, `"default"` — controlling the visual state of the tile (owned/unowned, patron/non-patron, completed/pending). Frame scripts stop at frames 11, 106, and 201 (animation end-points for those states).

`setup(rewardIndex, ownReward, isPatron, itemName, itemQuantity, numFlux, fluxName, iconStringPath, isLastDay, skipAhead)`:
- Chooses the frame label based on `ownReward`, `isPatron`, and `skipAhead`.
- Sets day label text using `$DailyLogin_DayLabel` + `(rewardIndex + 1)`.
- Sets Patron currency text to `"+" + numFlux + " " + fluxName`.
- Sets item name text and adjusts its `y` position/height to bottom-align it (shrinks to `textHeight` and shifts up).
- Configures `itemSlot`: `iconImage`, `quantity`, `showQuantity` (true if > 1), `showQuantityWithX` (true if < 100), hides `slotFrame`.
- Shows `ShineOne`/`ShineTwo` only on the last day.
- Toggles between `dayTitle_txt`/`finalDayTitle_txt` and shows `finalReward_txt`/`finalDayGlow_mc` on last day.

### `DailyRewards_fla.PatronBtn_30` — Embed symbol132
Timeline symbol that wraps the `buyPatron` `btnGreen` button; sets its label to `$Patron_ButtonLegend` in `__setProp`.

### `DailyRewards_fla.equipped_19` — Embed symbol23
Two-frame timeline symbol (frames 1 and 2 both `stop()`). Likely a visual indicator for the "equipped" or "owned" state on a slot.

### `DailyRewards_fla.slotFrame_39` / `slotFrameLarge_17`
Additional `_fla` timeline symbols for the item slot border and large slot frame (standard 3–4 frame rarity-tier indicators).

### Asset wrappers (13 classes)
`rarity_frame_common_png`, `rarity_frame_uncommon_png`, `rarity_frame_rare_png`, `rarity_frame_epic_png`, `rarity_frame_legendary_png`, `rarity_frame_shadow_png`, `rarity_frame_relic_png`, `rarity_frame_resplendent_png`, `rarity_frame_radiant1_png`, `rarity_frame_stellar`, `slot_large`, `btnGreen`, `btnGreen_small`, `btnGreenIcon_small`, `dummy` — bitmap/skin asset classes, no logic.

## Notable logic
- **Sequential population**: The game engine calls `clearAllRewards` then `addNewReward` up to 7 times. The `m_numRewards` counter selects the target tile in order; there is no index parameter — tiles are always filled left-to-right.
- **Item name vertical alignment**: `setup` shrinks `itemNameTxt.height` to `textHeight` and subtracts the height delta from `y`, keeping the text bottom-anchored within its area regardless of text length.
- **Patron bonus overlay**: The `patronText.currencyText` field shows the Flux bonus for Patron members on top of the item tile, providing an incentive display even for non-patron viewers who see the "what you'd get" bonus.
- **`skipAhead` flag**: Distinguishes between tiles the player already collected normally (`ownReward && !skipAhead`) versus tiles they skipped over (perhaps by purchasing ahead), allowing different animation states (`yesPatronDone` / `noPatronDone`).
