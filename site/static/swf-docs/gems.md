# gems.swf
> The Gem Forge UI panel, accessible from the character sheet. It displays 12 gem equipment slots (organised into blue/yellow/red/opal sets), a central upgrade slot, a Fuse button, gem stat readouts, a karma meter, gem insurance (booster) items, and upgrade result animations. The panel also shows tabs for navigating to the Character, Rewards, and PVP windows.

**Document/main class:** `Gems` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 73 (including asset wrappers and Gems_fla timeline symbols)

## Main class: `Gems`

`Gems` is the root UIComponent for the entire forge screen. The constructor initialises all gem slots (`gemSlot0`–`gemSlot11` plus `upgradeSlot`), wires tab-click listeners, sets up the ingredient list, insurance items, karma meter, and preview MovieClips. On console (`IsConsole()`), a full `DirectionalMapping` grid is built so the d-pad can traverse all interactive elements. In `configUI()`, Iggy is detected and a large set of `ExternalInterface` callbacks are registered; drag-drop is also registered via `SlotDragDropHelper`.

Three frame stops are defined: frame 1 (default), frame 11, and frame 21. These likely correspond to PC, console, and a second console variant, though their exact purpose is not spelled out in visible frame-script bodies.

### Public methods
- `tweenIn(idx : int) : void` — starts the stat-increase tween animation for the stat at position `idx`. Reads current vs. old stat value from `oldStatDict`; if there is a positive delta it creates `IggyTween` objects for alpha and scale, chained so `tweenOut` is called on completion.
- `tweenOut(idx : int) : void` — advances the tween chain. When all stats are done, kicks off fade-out tweens for each stat display, with an extra 0.5 s delay on the last one; the last tween calls `tweenComplete`.
- `tweenComplete() : void` — clears all animated stat text fields and calls `onResultAnimFinished()`.

### Key fields
- `gemSlot0`–`gemSlot11 : Slot` — the 12 gem equipment slots; every third slot (index 2, 5, 8, 11) is sized `slotSize_large` (77 px); the rest are `slotSize_small` (55 px). Each slot's `slotColor` child is set to one of `["blue","yellow","red","opal"]` based on `floor(slotIndex / 3)`.
- `upgradeSlot : Slot` — the XL forge slot (100 px) where gems are placed for upgrading.
- `fuseButton : LabelButton` — the Fuse/Upgrade button; disabled during result animation timer.
- `stats : MovieClip` — container for `statName0`–`statName2`, `statValue0`–`statValue2`, `gemLevel`, `powerRank`, `success` description, and `animation` sub-clip with four animated stat rows.
- `karmaMeter : MovieClip` — has a `karmaMask` child whose `scaleX` represents the fill level (0–1).
- `insuranceContainer : MovieClip` — holds `insuranceItem0`–`insuranceItem3`; each has a `slot`, `count`, and `buyButton`.
- `gemPreviews : MovieClip` — holds `preview0`–`preview2` showing upgrade outcome previews.
- `boosterCTA : TextField` — call-to-action text shown when no insurance is selected.
- `ingredientList : Array` — list of ingredient objects (`{slotTextureName, name, description, numHave, numNeed, txtColor}`).
- `insuranceList : Array` — tooltip data for insurance items.
- `insuranceListUpgrade : Array` — level values used to match the current upgrade tier and visually mark the matching insurance slot.
- `statList : Array` — current gem stat objects used for tween display.
- `oldStatDict : Dictionary` — stat values before the most recent upgrade; keyed by `statKeys` (`["stat0","stat1","stat2","powerRank"]`).
- `resultTimer : Timer` — 3-second one-shot timer; blocks re-upgrade during animation.
- `lastUpgradeResult : int` — one of five `GemUpgradeResult_*` constants.
- `TextFormat_Gold / _Green / _White : TextFormat` — pre-built formats for animated stat label colours (font "Open Sans", sizes 24/18/18).
- `BrightnessFilter : ColorMatrixFilter` — pre-computed brightening matrix used on the upgrade slot icon.
- `currentSelection : MovieClip` — tracks the currently d-pad-focused element on console.
- `tweenScale : Number` — 1.0 when all stats change (great success), 1.5 when only one stat changes (regular success).

### Frame scripts / timeline
- **frame 1** (`frame1`): `stop()`
- **frame 11** (`frame11`): `stop()`
- **frame 21** (`frame21`): `stop()`

### Runtime dependencies & integration

**ExternalInterface callbacks registered (game → Flash, only in Iggy):**
| Callback | Description |
|---|---|
| `setPatronStatus(bool)` | Stores patron status flag. |
| `previewUpgrade(...)` | Stub — body empty. |
| `addStat(name, value, doAnim, singleStatChanged)` | Appends a stat to `statList`; updates the static display; if `doAnim` and the result timer is running after a success, triggers tween animation. |
| `addIngredient(texName, name, desc, numHave, numNeed)` | Adds an ingredient row to the required-items panel; colours the count red if `numHave < numNeed`; re-evaluates fuse button state. |
| `resetIngredients()` | Clears ingredient display. |
| `clearForge()` | Resets forge to empty state (karma to 0, stats hidden, upgrade slot cleared). |
| `selectInsurance(idx, ...modifiers)` | Selects an insurance item; shows/hides gem previews; sets modifier frame stops on preview clips. |
| `setInsuranceInfo(idx, texName, count, name, desc, upgradeLevel)` | Populates an insurance slot's icon, count, and tooltip data; shows/hides buy button. |
| `setGemKarma(current, max)` | Sets `karmaMeter.karmaMask.scaleX = current/max`. |
| `fuseGems(event)` | Bound to `onFuseClick` — same as clicking Fuse. |
| `moveSelection(dx, dy)` | Moves d-pad focus using `DirectionalMapping`. |
| `highlightSlot(bool)` | Forces highlight to `upgradeSlot`. |
| `getCurrentSlotId()` | Returns `data` of the currently focused slot. |
| `activateSlot()` | Activates the focused slot or calls `OnInsuranceItemSelected`. |
| `purchaseInsurance(idx)` | Triggers buy flow for an insurance item. |
| `moveGemToSlot(texName, itemId, qty, slotIdx)` | Calls `OnDropOntoSlot` for a programmatic gem move. |
| `setFuseState(enabled, label, desc, level, powerRank, rarity, quality, showInstr, instrKey, upgradeResult, canUpgrade, selectedInsuranceLvl)` | Master state-setter called after the server confirms the state; updates fuse button, stats, previews, insurance slot visual states, and triggers upgrade result animation. |

**ExternalInterface calls dispatched (Flash → game):**
- `OnUpgradeRequest` — when Fuse is clicked.
- `CheckUpgradeAvailability` — after result animation completes.
- `OnDropOntoSlot(texName, itemId, qty, slotData)` — on drag-drop or `moveGemToSlot`.
- `OnTransferSlot(slotData)` — on right-click of a slot.
- `OnShowTab(tabId)` — tab navigation (0=Character, 2=Rewards, 3=PVP).
- `POST_SOUND_EVENT(eventName)` / `POST_RTPC_SOUND_EVENT(event, rtpc, level)` — upgrade result sound events: `Play_ui_gem_break`, `Play_ui_gem_double_win`, `Play_ui_gem_success`, `Play_ui_gem_fail`, `Play_ui_gem_restore`.
- `UIComponent.OnShowTooltip(x, y, title, desc)` / `UIComponent.OnHideTooltip()` — ingredient, insurance, and preview tooltips.
- `OnBuyInsuranceItem(slotData)` / `OnInsuranceItemSelected(slotData)` — insurance interactions.

**IggyTween usage:** `tweenIn` / `tweenOut` create `IggyTween` instances (alpha, scaleX, scaleY) using `Strong.easeIn` for the in-phase and `Exponential.easeIn` for the out-phase. Callback chaining (`motionFinishCallback`) drives the sequential per-stat reveal.

**SlotDragDropHelper:** `SlotDragDropHelper.registerDropCallback(onDrop)` — `onDrop` hit-tests every slot in `gemSlots` against the dropped coordinates and fires `OnDropOntoSlot`.

**translate keys (visible):** `$GemInstructions_empty`, `$Level_X`, `$NoRarityPowerRank`, `$Leaderboard_Category_PowerRank`, `$GemPreview1`/`2`, `$GemPreviewName1`/`2`/`3`, `$GemPreviewState1`/`2`/`3`, `$UpgradeLabel`, `$GemTooltip_Karma_Bar_Title`, `$GemTooltip_Karma_Bar_Desc`.

## Other game-specific classes

### Top-level slot/UI wrappers (asset-backed MovieClips)
- `slot` — extends `_kiwi.Controls.Slot`; embed symbol128. Standard small gem slot.
- `slot_large` — (inferred from name, same pattern) large gem slot.
- `slot_xlarge` — XL upgrade slot.
- `SlotBackground`, `SlotBackgroundLocked`, `SlotFrameNormal`, `SlotFrameMedium`, `SlotFrameHigh` — slot frame and background visual variants.
- `Equipped` — embed symbol64; plain MovieClip for the equipped indicator overlay.
- `meter_karma` — karma bar clip.
- `art`, `image` — generic art/image wrapper clips.
- `btnGreen`, `btnGreen_small`, `btnGreenIcon_small` — extends `_kiwi.Controls.LabelButton`; button skin variants.
- `dummy` — empty stub MovieClip.
- `rarity_frame_stellar` — non-_png rarity frame clip (has code, unlike the pure bitmap wrappers).

### Rarity frame asset wrappers
30 classes ending in `_png` or `_over_png` or `_large_over_png` for rarities: common, uncommon, rare, epic, legendary, relic, mystic, shadow, radiant1, stellar, resplendent, crystal. All are pure bitmap-embed symbols — no logic.

### Gems_fla timeline symbols (23 classes)
All in package `Gems_fla`; embedded from `/_assets/assets.swf`. Key ones:

- `tabHeader_mastery_1`, `tabHeader_pvp_3`, `tabHeader_char_2`, `tabHeader_gems_4` — tab header MovieClips with 3-frame stops (normal / hover / active states).
- `insurance_5`, `insurance_slot_6` — insurance section clips.
- `gem_impact_gr_13`, `gem_impact_14` — impact effect clips.
- `modifier_15` — modifier indicator clip.
- `gemlevelincrease_16` — level-up animation clip.
- `required_items_19` — ingredients panel clip.
- `color_frame_large_23`, `slotFrameLarge_26`, `animatedHighlightLarge_27`, `slot_frame_small_32`, `slotFrame_34`, `animatedHiglight_35`, `slotFrameXL_40` — slot frame visuals at various sizes.
- `equipped_43` — equipped indicator sub-clip.
- `qualityPips_47` — quality pip display.
- `stats_52` — stats panel (contains `statName0`–`2`, `statValue0`–`2`, `gemLevel`, `powerRank`, `success`, `animation`).
- `successtextfield_56` — success label text field clip.
- `power_rank_57` — power rank display clip.

## Notable logic
- **Upgrade result animation pipeline:** `setFuseState` calls `handleUpgradeResult(upgradeResult, level)` which fires a `BitmapAnimator.playShakeAnimation` on the upgrade slot and plays a Wwise sound event. Then `addStat` calls (one per stat) accumulate `statList` and trigger `tweenIn(0)` which chains through all stats. After the last tween fades out, `tweenComplete` → `onResultAnimFinished` → `ExternalInterface.call("CheckUpgradeAvailability")`.
- **Karma meter:** `setGemKarma(current, max)` drives `karmaMeter.karmaMask.scaleX` directly as a linear fill indicator.
- **Ingredient colour coding:** numHave vs numNeed comparison colours the count text red (`#FF3333`) or white; tooltip widths are calculated dynamically based on description string length (60 vs 82 px).
- **Console directional navigation:** A complete adjacency graph is built at construction time using `DirectionalMapping` children. `moveSelection` walks the graph. `highlightSelection` applies a gold `GlowFilter` (color `0xCCCC00`, strength 100) on non-Slot items; on NX it also shows/hides a `highlight` sub-clip.
