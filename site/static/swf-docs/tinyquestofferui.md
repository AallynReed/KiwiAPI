# tinyquestofferui.swf
> The Tiny Quest offer: title, level, difficulty stars and duration, up to seven ally/item slot pairs, synergy filter buttons, required items, rewards, XP and Flux cost (ticket price plus low-level fee), and Embark / Insta-Complete / Get Voucher Instead buttons.

**Document/main class:** `TinyQuestOfferUI` (extends `_kiwi.Core.UIComponent`) — no top-level embed; header title key `$TinyQuestUI`
**SWF-specific classes:** 20 (+ 7 `TinyQuestOfferUI_fla` timeline symbols)

---

## Main class: `TinyQuestOfferUI`

`TinyQuestOfferUI` is the root component for the Tiny Quest offer panel. On construction it binds three frame scripts (frames 0, 10, 20) then calls `configUI()` via the `UIComponent` lifecycle. All data enters through `ExternalInterface` callbacks registered during `configUI()`; the class never pulls data itself. Console support is detected via `IsConsole()` — if true, an `ENTER_FRAME` listener waits for `onTargetFrame()` before showing the button legend overlay.

### Public methods

- `setQuestTitleName(name:String) : *` — assigns `name` to `tinyQuestHeader.questIDText.questName`. `questIDText` is a `QuestTitle` clip, so the setter takes HTML and auto-shrinks the font (25 pt downwards) until the title fits two lines, then re-centres it vertically against `backgroundA`.
- `setQuestIcon(icon:String) : *` — sets `tinyQuestHeader.slot.iconImage`; hides the slot frame border.
- `setQuestLevel(level:int) : *` — stores `questDifficultyLevel` and updates `tinyQuestHeader.questLevelText`.
- `setQuestDesc(desc:String) : *` — stores `questDesc` for the info popup tooltip.
- `setDifficultyStar(index:int) : void` — clamps `index` to 1–3 and navigates `tinyQuestHeader.questDifficultyIcon` to frame `"1star"`, `"3star"` or `"5star"`. Before the October 2026 build it clamped to 0–2 and looked up `index - 1`, so `"5star"` was never reachable and `0` selected no frame.
- `setDuration(seconds:Number) : void` — formats as `HH:MM:SS` and writes to `tinyQuestHeader.durationText`.
- `setXpReward(xp:int) : void` — formats with `FormatNumber()` and writes to `xpHeader.xpText`.
- `setCost(flux:int) : void` — formats and writes to `costTotal.costText`.
- `setBaseCost(flux:int) : void` — writes to `tinyQuestCost.baseFee`, the TICKET PRICE line.
- `setAllyCost(flux:int) : void` — writes to `tinyQuestCost.alliesFee`, the LOW-LEVEL FEE line (the surcharge for allies below the quest's level); colors it orange (`16487452` = `0xFB941C`) when `> 0`, white otherwise.
- `setAcceptButtonText(text:String) : void` — assigns the text to `acceptBtn.label` unchanged.
- `setConsumeButtonText(text:String) : void` — prefixes two spaces and assigns to `consumeBtn.label`.
- `setAcceptAndCompleteButtonText(count:int) : void` — sets numeric label on `acceptAndCompleteBtn.acceptAndCompleteText`.
- `setAcceptButtonVisible(visible:Boolean) : *` — enables/disables `acceptBtn`; toggles `acceptBtnTooltip` visibility inversely. The engine sends `false` for every reason the quest cannot be embarked (all active quest slots in use, not enough flux, missing required items) and never says which; nothing else the offer receives reports how many quests are already running.
- `setAcceptAndCompleteButtonEnable(enabled:Boolean) : *` — enables/disables `acceptAndCompleteBtn`; toggles `acceptAndCompleteBtnTooltip` inversely.
- `setSlotNumber(gearCount:int, itemCount:int) : void` — creates `max(gearCount, itemCount)` pairs of `PetSlot` instances (pet mode + item mode) laid out along two linear paths defined by `Start1/End1` and `Start2/End2` position anchors. Places a `Hat` overlay on the first gear slot. Pushes into `gearSlotArray` and `itemSlotArray`.
- `setSlotBlueprint(slotIndex:int, imagePath:String) : void` — sets `forceIconImage` on the gear or item slot; toggles `BGIconController` visibility and manages the cross indicator. `slotIndex` is zero-based: below `gearSlotArray.length` it names an ally slot, and `itemOffset + n` names item slot `n`. The item's cross shows when an item is set but its paired ally slot is empty.
- `setPetSlotFrame(slotIndex:int, frameIndex:int) : void` — drives the slot border to one of `["default","synergy","underlevel","required","underlvlSynergy"]` frame labels (what the quest wants from that slot). Same zero-based index as `setSlotBlueprint`, so item slots get frames of their own.
- `setTagButtons(count:int) : void` — creates `count` `StatFilterObj` filter buttons laid out right to left from `TagFilterPosition`, one button width plus 8 px apart.
- `setTargetTagButtonsStat(btnIdx:int, tagInt:int, textureName:String, title:String, desc:String) : void` — populates a stat-type filter button, records `buttonId`, and hides the button's blue (active) skin. Title and description arrive already translated. The textures are the `ui/tiny_quests/tinyquest_icon_tq_stat_*` set (18 stats) and, for the tag setter below, `tinyquest_icon_tq_tag_flying` / `tinyquest_icon_tq_tag_underwaterbreathing`.
- `setTargetTagButtonsTag(btnIdx:int, tagStr:String, textureName:String, title:String, desc:String) : void` — populates a tag-string filter button. Both setters now assign `tagInt = btnIdx`; neither uses the old `-1` sentinel, so the engine distinguishes stat from tag filters by whether `tagStr` is set.
- `applyFilter(btnIndex:int) : void` — programmatically dispatches a `CLICK` on the specified tag button.
- `selectSlot(slotIndex:int) : void` — programmatically dispatches a `CLICK` on a pet or item slot.
- `goToNextPetSlot(slotIndex:int) : void` — console only; forwards straight to `ExternalInterface.call("SLOT.ACTIVATE", slotIndex)` so the engine can advance the highlighted slot.
- `onSelectionActivated(slotIndex:int) : void` — console only; routes an activated selection to either `SLOT.PET.ACTIVATE` (index within `gearSlotArray`) or `SLOT.ITEM.ACTIVATE` (index minus `itemOffset`, within `itemSlotArray`). Index `0` is ignored.
- `focusSlot(slotIndex:int) : void` — console only: highlights the target slot and triggers the accept-button tooltip via `ExternalInterface`.
- `focusToggleFilter(filterIndex:int) : void` — console only: shows the highlight only on the indicated tag button.
- `clearFocus() : void` — hides all slot and tag-button highlights.
- `clearRewards() : *` — removes and destroys all `RewardObject` children.
- `addReward(iconPath:String, count:int) : *` — creates a `RewardObject`, positions it relative to `rewardPosition`, sets `slot.data` to `rewardArray.length + 1 + rewardOffset (16)`. The client works the reward out from the assigned allies and items, so every change to the line-up is followed by `clearRewards` and a fresh set of `addReward` calls.
- `clearRequired() : *` — removes and destroys all `RequiredObject` children; hides `tqRequired`.
- `addRequired(iconPath:String, itemName:String, have:int, need:int) : *` — creates a `RequiredObject`, positions it relative to `requiredPosition`, colors the `have` digits of `amountHave` red when `have < need`, max 4 items. `requiredItemName` and `amountHave` are single fields, so they show whichever `addRequired` came last.
- `setRequiredText(text:String) : void` — sets the label inside `tqRequired.requiredText`.
- `getPetSlotCount() : int` — returns `gearSlotArray.length`.
- `getItemSlotCount() : int` — returns `itemSlotArray.length`.
- `getFilterButtonCount() : int` — returns `tagButtons.length`.
- `dispatchStartQuest() : void` — calls `ExternalInterface.call("onQuestAccept")`.
- `dispatchInstaCompleteQuest() : void` — calls `ExternalInterface.call("onQuestAcceptAndComplete")`.
- `dispatchConsumeQuest() : void` — calls `ExternalInterface.call("onQuestConsume")`.
- `tQDescVisible(show:Boolean) : void` — toggles the info popup; mirrors `onInfoRollOver` toggle logic for console use.
- `expHighlightVisible(show:*) : void` — shows/hides `xpHeader.highlight` and calls `OnXpHeaderTooltip` or `OnHideTooltip`.
- `costHighlightVisible(show:*) : void` — shows/hides `costTotal.highlight` and calls `OnCostTooltip` or `OnHideTooltip`.
- `FormatNumber(n:int) : String` — formats an integer with comma-thousands separators.
- `durationFormatter(n:int) : String` — zero-pads a single integer to two digits.

### Key fields

- `m_header : WindowHeaderSmall` — window title bar; title key `"$TinyQuestUI"`, enabled=false (non-interactive header).
- `tinyQuestHeader : MovieClip` — sub-clip containing `questIDText` (a `QuestTitle`), `questLevelText`, `durationText`, `slot` (quest icon), `questDifficultyIcon`, `infoBtn`, and `infoPopup` (an `InfoPopup`).
- `tinyQuestRewards : MovieClip` — stage instance holding the reward strip; positioned on the timeline and not manipulated by the document class.
- `xpHeader : MovieClip` — XP display area with `xpText`, `qubeslyXpIcon` (tooltip trigger), `highlight`.
- `costTotal : MovieClip` — Flux cost display with `costText`, `iconFlux` (tooltip trigger), `highlight`.
- `tinyQuestCost : MovieClip` — cost breakdown with `baseFee` and `alliesFee` text fields, labelled by the static fields `baseFeeText` (`$TinyQuestUI_CostBaseFee`, "TICKET PRICE") and `alliesFeeText` (`$TinyQuestUI_CostLevelPenalty`, "LOW-LEVEL FEE").
- `acceptBtn : LabelButton` — primary accept (Embark) button; starts disabled.
- `consumeBtn : LabelButton` — consume-quest button, labelled "Get Voucher Instead" in English via `setConsumeButtonText`: it skips the quest for a voucher (`$TinyQuestUI_QuestConsumed`: "Tiny Quest skipped! You received {0} x {1} instead!"). The only action button with no rollover tooltip.
- `acceptAndCompleteBtn : MovieClip` — insta-complete button; starts disabled; has nested `acceptAndCompleteText`.
- `acceptBtnTooltip / acceptAndCompleteBtnTooltip : MovieClip` — invisible hit areas that show tooltips when the corresponding button is disabled.
- `buttonLegend : MovieClip` — console button-legend overlay, initially hidden until `onTargetFrame()` returns true.
- `TagFilterPosition : MovieClip` — positional anchor for tag filter buttons.
- `rewardPosition / requiredPosition : MovieClip` — positional anchors for dynamically placed reward/required items.
- `Start1/End1/Start2/End2 : MovieClip` — linear interpolation endpoints defining two rows of up to 4 + 3 slots.
- `gearSlotArray : Array` — holds `PetSlot` instances in pet mode.
- `itemSlotArray : Array` — holds `PetSlot` instances in item mode.
- `tagButtons : Array` — holds `StatFilterObj` filter buttons.
- `rewardArray / requiredArray : Array` — hold `RewardObject` / `RequiredObject` instances (max 4 each).
- `questDesc : String` — cached quest description HTML for the info popup.
- `questDifficultyLevel : int` — cached numeric difficulty level.
- `slotSize_large : int = 77` — slot pixel size used in `setSlotSize()` calls.
- `slotSize_mid : int = 65` — smaller slot size; declared but not currently referenced.
- `itemOffset : int = 7` — data ID offset separating pet slots (1–7) from item slots (8–14).
- `rewardOffset : int = 16` — data ID offset for reward slots.
- `requiredOffset : int = 21` — data ID offset for required slots.
- `itemPositionOffset_X / itemPositionOffset_Y : int = 35` — pixel offset applied to each item slot; the paired gear slot is placed back at the un-offset position, so the two slots sit diagonally staggered rather than overlapping.
- `buttonId : int = -1` — index of the tag button most recently populated by `setTargetTagButtonsStat`, stored as `tagButtons.length + btnIdx`.
- `_showSwitchScreensButtonLegend : Boolean = false` — reserved flag for the console screen-switch legend; not currently read.

### Frame scripts / timeline

- **Frame 0** (`frame1`, label `PC`): `stop()` — rests on default frame.
- **Frame 10** (`frame11`, label `Console`): `stop()` — the console layout.
- **Frame 20** (`frame21`, label `ConsoleLoc`): `stop()`, then navigates `buttonLegend` to label `"ConsoleLoc"` — activates console button layout. (`buttonLegend_40` itself has a single label, `offer`.)

Stage 918×750, 24 fps, 30 frames.

### Runtime dependencies & integration

- `ExternalInterface.addCallback` — 44 callbacks registered; game engine drives all data population.
- `ExternalInterface.call` outbound:
  - `"onQuestAccept"`, `"onQuestConsume"`, `"onQuestAcceptAndComplete"` — button action results.
  - `"ToggleFilter"(tagInt, tagStr, isActive)` — stat/tag filter toggle.
  - `"OnFilterBtnEnter"(title, desc, x, y)` — tag button hover tooltip.
  - `"OnHideTooltip"` — tooltip dismissal.
  - `"OnXpHeaderTooltip"(x, y)`, `"OnCostTooltip"(x, y)` — icon hover tooltips.
  - `"onQuestAcceptRollOver"(x, y)`, `"onQuestAcceptAndCompleteRollOver"(x, y)` — accept button tooltips.
  - `"SLOT.ACTIVATE"(id)`, `"SLOT.PET.ACTIVATE"(id)`, `"SLOT.ITEM.ACTIVATE"(id)` — console slot activation.
  - `"OnDropIntoWindow"` — drag-and-drop result: forwards arguments 3–5 of the `DRAGHOST.DROP` callback, then the `name` of the first ally `PetSlot` under the drop point, then the callback's last argument. The slots are created with `addChild`, so that name is the instance name Flash assigned. Nothing is sent when the drop misses every ally slot.
- `SlotDragDropHelper.registerDropCallback(onDrop)` — registered only in Iggy context; hit-tests gear slots on drop.
- `IsConsole()` / `IggyFunctions.inIggy` — platform guards throughout.
- Translate key: `"$TinyQuestUI"` (window header). Static timeline text carries `$TinyQuestUI_Cost`, `$TinyQuestUI_CostBaseFee`, `$TinyQuestUI_CostLevelPenalty`, `$TinyQuestUI_Required`, `$TinyQuestUI_Rewards`, `$TinyQuestUI_LVL`, and the console legend's `$TinyQuestUI_QuestEmbark`, `$TinyQuestUI_ButtonLegendConsume`, `$TinyQuestUI_ButtonLegendInstaComplete`, `$Select_ButtonLegend`.
- Two index schemes: the slot `data` ids (see Notable logic) are what `onSlotEnter`, `onSlotLeave`, `onSelectionActivated` and the outbound `SLOT.*` calls use; `setSlotBlueprint`, `setPetSlotFrame`, `focusSlot` and `selectSlot` take a zero-based index instead (ally `n`, or `itemOffset + n` for item `n`).

---

## Other game-specific classes

- `PetSlot` — Embeds `symbol89`; paired slot container with a `slot_large` (`slot`), `SlotIconController` (`BGIconController`), and a `cross` MovieClip (hidden by default). Used for both pet (head icon) and item (bag icon) modes — `setSlotNumber` instantiates it twice per row.
- `ItemSlot` — Embeds `symbol92`; structurally identical twin of `PetSlot` (`slot`, `BGIconController`, `cross`). Exported for linkage but never instantiated by the document class, which still uses `PetSlot` in item mode.
- `SlotIconController` — Embeds `symbol51`; switches between `head` (pet) and `bag` (item) sub-clips via `PetMode()`, `ItemMode()`, `Hide()`, `Show()`.
- `StatFilterObj` — Embeds `symbol19`; toggle filter button carrying **two** `LabelButton` skins — `button` (grey, shown when inactive) and `btnBlue` (shown when active) — plus an `ObjectPreview` image, `highlight` clip, and string/int tag metadata (`tagStr`, `tagInt`, `tagTitle`, `tagDesc`, `isActive`). `FixVisibility()` swaps which skin is visible. Both skins are configured as `toggle` buttons with empty labels and tooltips.
- `QuestTitle` — Embeds `symbol121`; the quest-name plate (`txt_questName`, `backgroundA`). Its `questName` setter accepts HTML and steps the point size down from 25 until the text fits within two lines and the original field height, then vertically centres it.
- `InfoPopup` — Embeds `symbol116`; the quest-description tooltip. A three-slice background (`bgStart` / `bgBody` / `bgEnd`) plus `textField`; `FixSize()` grows `bgBody` to the measured text height and pushes `bgEnd` below it.
- `RewardObject` — Embeds `symbol42`; reward slot container with a `Slot` and `CountText` field.
- `RequiredObject` — Embeds `symbol44`; required-item slot container with a `Slot` and `CountText` field.
- `slot_large` — Embeds `symbol87`; concrete `Slot` subclass used inside `PetSlot` and `ItemSlot`.
- `btnGreen` — Embeds `symbol102`; green `LabelButton` skin with 4 stop-frames (up/over/down/disabled at frames 10/20/30/40).
- `BtnEmbark` — Embeds `symbol147`; `LabelButton` skin for the quest Embark action, same 4-stop-frame layout.
- `btnVoucher` — Embeds `symbol149`; `LabelButton` skin for the voucher action, same 4-stop-frame layout.
- `btnStat` — Embeds `symbol13`; the inactive (grey) `LabelButton` skin behind `StatFilterObj.button`; 5 stop-frames at 10/20/30/40/50.
- `btnStatBlue` — Embeds `symbol17`; the active (blue) counterpart behind `StatFilterObj.btnBlue`; same 5-stop-frame layout.
- `btnAutoAcceptComplete` — Embeds `symbol135`; `BaseButton` skin for the insta-complete button, 4-stop-frame layout.
- `Hat` — Embeds `symbol105`; hat overlay placed on the first gear slot. It marks the slot the Golden Thread tutorial quest points players to: select "the HAT-WEARING, EMPTY ALLY SLOT" to assign an ally (`$Quest_GoldenThread_expertise_tinyquest_02_Description`).
- `art` — Embeds `symbol33`; plain `ArtClip` background/panel art asset.
- Asset wrappers (2 bitmap classes): `dummy` (52×52 placeholder PNG), `rarity_frame_normal_large_over_png` (76×76 slot hover overlay).

### `TinyQuestOfferUI_fla` package (timeline symbol classes)

- `buttonLegend_40` — Embeds `symbol244`; console button legend clip with sub-clips `consoleButtonConsume`, `consoleButtonEmbark`, `primaryAction`, `consoleButtonInstaComplete`; stops on frame 1.
- `slotFrameLarge_49` — Embeds `symbol66`; large slot border with the 5 state labels `setPetSlotFrame` uses — `default` (dark disc, grey ring), `synergy` (green), `underlevel` (orange), `required` (gold ring), `underlvlSynergy` (orange disc, green ring) — plus a sixth label, `console`, which is the only frame carrying the `highlightCircle` sub-clip.
- `slotFrame_27` — Embeds `symbol26`; smaller slot frame with 3 stop-frames.
- `quest_difficulty_icon_23` — Embeds `symbol227`; 6-state difficulty icon (6 stop-frames × 10 frames each), driven by `gotoAndStop("1star"/"3star"/"5star")`.
- `btnInfo2_39` — Embeds `symbol234`; info-button skin with 4 stop-frames (up/over/down/disabled).
- `equipped_52` — Embeds `symbol70`; 2-frame equipped indicator (frame 1 = hidden, frame 2 = shown).
- `qualityPips_56` — Embeds `symbol85`; quality pip indicator; stops on frame 1.

---

## Notable logic

- **Slot data ID scheme**: gear slots use IDs `1..gearCount`, item slots `itemOffset+1..itemOffset+count` (offset 7), reward slots start at `rewardOffset+1` (17), required slots at `requiredOffset+1` (22). That makes allies 1–7, items 8–14, rewards 17–20 and required 22–25, with 0 meaning no slot. The engine uses these IDs to refer back to specific slots via `onSlotEnter`/`onSlotLeave`, which decode them by range and mark the slot `selected`. `onSlotLeave` casts the `PetSlot` wrappers to `Slot`, which yields `null`, so it never clears anything.
- **Dual-row slot layout**: `setSlotNumber` distributes up to 4 slots on the `Start1→End1` segment then switches to `Start2→End2` for a second run, using linear interpolation with a divisor of 3 (first row) or 2 (second row). Each row places an item slot at the interpolated point plus `itemPositionOffset_X/Y` (35, 35) and its paired gear slot back at the un-offset point, so the pet and item slots sit diagonally staggered. The two segments end at seven pairs (4 + 3); an eighth ally would also take ID 8, the first item slot's.
- **Tag filter skin swap**: the active state of a `StatFilterObj` is now a skin swap, not a filter — `FixVisibility()` hides the grey `btnStat` skin and shows the blue `btnStatBlue` one (plus the `highlight` clip). The old inner-glow treatment is gone; only a vestigial `filters = []` clear remains on deactivation. On console, both skins are stepped between the `"up"` and `"down"` frame labels in lockstep.
- **Low-level fee coloring**: `alliesFee` (LOW-LEVEL FEE) is colored orange (`16487452` = `0xFB941C`) when the fee is positive, white when zero or negative. The `setTextFormat` range ends at `alliesFee.toString().length`, the length of the TextField object's own string form rather than of its text.
- **Console button legend**: the `buttonLegend` clip stays hidden until `onTargetFrame()` signals readiness via the `ENTER_FRAME` loop, then the listener is removed. On frame 20 the legend is positioned to `"ConsoleLoc"`.
- **Button labels**: `setAcceptButtonText` assigns its text unchanged (no prefix); only `setConsumeButtonText` pads its label, with two leading spaces.
