# tinyquestofferui.swf
> The Tiny Quest offer/accept dialog shown when a player opens a Tiny Quest for review. It displays the quest title, difficulty star rating, required and reward item slots (paired pet/gear slots), XP and Flux cost, tag-filter buttons for selecting gear stats, and Accept / Insta-Complete / Consume action buttons.

**Document/main class:** `TinyQuestOfferUI` (extends `_kiwi.Core.UIComponent`) — no top-level embed; header title key `$TinyQuestUI`
**SWF-specific classes:** 13

---

## Main class: `TinyQuestOfferUI`

`TinyQuestOfferUI` is the root component for the Tiny Quest offer panel. On construction it binds three frame scripts (frames 0, 10, 20) then calls `configUI()` via the `UIComponent` lifecycle. All data enters through `ExternalInterface` callbacks registered during `configUI()`; the class never pulls data itself. Console support is detected via `IsConsole()` — if true, an `ENTER_FRAME` listener waits for `onTargetFrame()` before showing the button legend overlay.

### Public methods

- `setQuestTitleName(name:String) : *` — writes `name` to `tinyQuestHeader.questIDText.text`.
- `setQuestIcon(icon:String) : *` — sets `tinyQuestHeader.slot.iconImage`; hides the slot frame border.
- `setQuestLevel(level:int) : *` — stores `questDifficultyLevel` and updates `tinyQuestHeader.questLevelText`.
- `setQuestDesc(desc:String) : *` — stores `questDesc` for the info popup tooltip.
- `setDifficultyStar(index:int) : void` — navigates `tinyQuestHeader.questDifficultyIcon` to frames `"1star"`, `"3star"`, or `"5star"` (index clamped 0–2).
- `setDuration(seconds:Number) : void` — formats as `HH:MM:SS` and writes to `tinyQuestHeader.durationText`.
- `setXpReward(xp:int) : void` — formats with `FormatNumber()` and writes to `xpHeader.xpText`.
- `setCost(flux:int) : void` — formats and writes to `costTotal.costText`.
- `setBaseCost(flux:int) : void` — writes to `tinyQuestCost.baseFee`.
- `setAllyCost(flux:int) : void` — writes to `tinyQuestCost.alliesFee`; colors red (`0xFBAADC`) when `> 0`, white otherwise.
- `setAcceptButtonText(text:String) : void` — prefixes `"/t "` and assigns to `acceptBtn.label`.
- `setConsumeButtonText(text:String) : void` — prefixes two spaces and assigns to `consumeBtn.label`.
- `setAcceptAndCompleteButtonText(count:int) : void` — sets numeric label on `acceptAndCompleteBtn.acceptAndCompleteText`.
- `setAcceptButtonVisible(visible:Boolean) : *` — enables/disables `acceptBtn`; toggles `acceptBtnTooltip` visibility inversely.
- `setAcceptAndCompleteButtonEnable(enabled:Boolean) : *` — enables/disables `acceptAndCompleteBtn`; toggles `acceptAndCompleteBtnTooltip` inversely.
- `setSlotNumber(gearCount:int, itemCount:int) : void` — creates paired `PetSlot` instances (pet mode + item mode) laid out along two linear paths defined by `Start1/End1` and `Start2/End2` position anchors. Places a `Hat` overlay on the first gear slot. Pushes into `gearSlotArray` and `itemSlotArray`.
- `setSlotBlueprint(slotIndex:int, imagePath:String) : void` — sets `forceIconImage` on the gear or item slot; toggles `BGIconController` visibility and manages the cross (invalid-item) indicator.
- `setPetSlotFrame(slotIndex:int, frameIndex:int) : void` — drives the slot border to one of `["default","synergy","underlevel","required","underlvlSynergy"]` frame labels.
- `setTagButtons(count:int) : void` — creates `count` `StatFilterObj` filter buttons positioned left of `TagFilterPosition`.
- `setTargetTagButtonsStat(btnIdx:int, tagInt:int, textureName:String, title:String, desc:String) : void` — populates a stat-type filter button.
- `setTargetTagButtonsTag(btnIdx:int, tagStr:String, textureName:String, title:String, desc:String) : void` — populates a tag-string filter button (sets `tagInt = -1`).
- `applyFilter(btnIndex:int) : void` — programmatically dispatches a `CLICK` on the specified tag button.
- `selectSlot(slotIndex:int) : void` — programmatically dispatches a `CLICK` on a pet or item slot.
- `focusSlot(slotIndex:int) : void` — console only: highlights the target slot and triggers the accept-button tooltip via `ExternalInterface`.
- `focusToggleFilter(filterIndex:int) : void` — console only: shows the highlight only on the indicated tag button.
- `clearFocus() : void` — hides all slot and tag-button highlights.
- `clearRewards() : *` — removes and destroys all `RewardObject` children.
- `addReward(iconPath:String, count:int) : *` — creates a `RewardObject`, positions it relative to `rewardPosition`, sets `slot.data` to `rewardArray.length + 1 + rewardOffset (16)`.
- `clearRequired() : *` — removes and destroys all `RequiredObject` children; hides `tqRequired`.
- `addRequired(iconPath:String, itemName:String, have:int, need:int) : *` — creates a `RequiredObject`, positions it relative to `requiredPosition`, colors `amountHave` text red when `have < need`, max 4 items.
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
- `tinyQuestHeader : MovieClip` — sub-clip containing `questIDText`, `questLevelText`, `durationText`, `slot` (quest icon), `questDifficultyIcon`, `infoBtn`, `infoPopup`.
- `xpHeader : MovieClip` — XP display area with `xpText`, `qubeslyXpIcon` (tooltip trigger), `highlight`.
- `costTotal : MovieClip` — Flux cost display with `costText`, `iconFlux` (tooltip trigger), `highlight`.
- `tinyQuestCost : MovieClip` — cost breakdown with `baseFee` and `alliesFee` text fields.
- `acceptBtn : LabelButton` — primary accept button; starts disabled.
- `consumeBtn : LabelButton` — consume-quest button.
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
- `itemOffset : int = 7` — data ID offset separating pet slots (1–6) from item slots (8+).
- `rewardOffset : int = 16` — data ID offset for reward slots.
- `requiredOffset : int = 21` — data ID offset for required slots.

### Frame scripts / timeline

- **Frame 0** (`frame1`): `stop()` — rests on default frame.
- **Frame 10** (`frame11`): `stop()` — intermediate frame, likely a layout variant.
- **Frame 20** (`frame21`): `stop()`, then navigates `buttonLegend` to label `"ConsoleLoc"` — activates console button layout.

### Runtime dependencies & integration

- `ExternalInterface.addCallback` — 38 callbacks registered; game engine drives all data population.
- `ExternalInterface.call` outbound:
  - `"onQuestAccept"`, `"onQuestConsume"`, `"onQuestAcceptAndComplete"` — button action results.
  - `"ToggleFilter"(tagInt, tagStr, isActive)` — stat/tag filter toggle.
  - `"OnFilterBtnEnter"(title, desc, x, y)` — tag button hover tooltip.
  - `"OnHideTooltip"` — tooltip dismissal.
  - `"OnXpHeaderTooltip"(x, y)`, `"OnCostTooltip"(x, y)` — icon hover tooltips.
  - `"onQuestAcceptRollOver"(x, y)`, `"onQuestAcceptAndCompleteRollOver"(x, y)` — accept button tooltips.
  - `"SLOT.ACTIVATE"(id)`, `"SLOT.PET.ACTIVATE"(id)`, `"SLOT.ITEM.ACTIVATE"(id)` — console slot activation.
  - `"OnDropIntoWindow"(itemPath, amount, slotName, altItem)` — drag-and-drop result.
- `SlotDragDropHelper.registerDropCallback(onDrop)` — registered only in Iggy context; hit-tests gear slots on drop.
- `IsConsole()` / `IggyFunctions.inIggy` — platform guards throughout.
- Translate key: `"$TinyQuestUI"` (window header).

---

## Other game-specific classes

- `PetSlot` — Embeds `symbol87`; paired slot container with a `slot_large` (`slot`), `SlotIconController` (`BGIconController`), and a `cross` MovieClip (hidden by default). Used for both pet (head icon) and item (bag icon) modes.
- `SlotIconController` — Embeds `symbol47`; switches between `head` (pet) and `bag` (item) sub-clips via `PetMode()`, `ItemMode()`, `Hide()`, `Show()`.
- `StatFilterObj` — Embeds `symbol15`; toggle filter button combining a `LabelButton` (`button`), an `ObjectPreview` image, `highlight` clip, and string/int tag metadata (`tagStr`, `tagInt`, `tagTitle`, `tagDesc`, `isActive`).
- `RewardObject` — Embeds `symbol38`; reward slot container with a `Slot` and `CountText` field.
- `RequiredObject` — Embeds `symbol40`; required-item slot container with a `Slot` and `CountText` field.
- `slot_large` — Embeds `symbol83`; concrete `Slot` subclass used inside `PetSlot`.
- `btnGreen` — Embeds `symbol12`; green `LabelButton` skin with 4 stop-frames (up/over/down/disabled at frames 10/20/30/40).
- `btnAutoAcceptComplete` — Embeds `symbol104`; `BaseButton` skin for the insta-complete button, same 4-stop-frame layout.
- `Hat` — Embeds `symbol90`; decorative hat overlay placed on the first gear slot.
- `art` — Embeds `symbol31`; plain `ArtClip` background/panel art asset.
- Asset wrappers (2 bitmap classes): `dummy` (52×52 placeholder PNG), `rarity_frame_normal_large_over_png` (76×76 slot hover overlay).

### `TinyQuestOfferUI_fla` package (timeline symbol classes)

- `buttonLegend_29` — Embeds `symbol193`; console button legend clip with sub-clips `consoleButtonConsume`, `consoleButtonEmbark`, `primaryAction`, `consoleButtonInstaComplete`; stops on frame 1.
- `slotFrameLarge_36` — Embeds `symbol62`; large slot border with 5 labeled stop-frames (default/synergy/underlevel/required/underlvlSynergy) and a `highlightCircle` sub-clip.
- `slotFrame_18` — Embeds `symbol24`; smaller slot frame with 3 stop-frames.
- `quest_difficulty_icon_16` — Embeds `symbol171`; 6-state difficulty icon (6 stop-frames × 10 frames each), driven by `gotoAndStop("1star"/"3star"/"5star")`.
- `btnInfo2_27` — Embeds `symbol183`; info-button skin with 4 stop-frames (up/over/down/disabled).
- `equipped_39` — Embeds `symbol66`; 2-frame equipped indicator (frame 1 = hidden, frame 2 = shown).
- `qualityPips_43` — Embeds `symbol81`; quality pip indicator; stops on frame 1.

---

## Notable logic

- **Slot data ID scheme**: gear slots use IDs `1..gearCount`, item slots `itemOffset+1..itemOffset+count` (offset 7), reward slots start at `rewardOffset+1` (17), required slots at `requiredOffset+1` (22). The engine uses these IDs to refer back to specific slots via `onSlotEnter`/`onSlotLeave`.
- **Dual-row slot layout**: `setSlotNumber` distributes up to 4 slots on the `Start1→End1` segment then switches to `Start2→End2` for a third row, using linear interpolation with `_loc11_` as the divisor.
- **Tag filter glow**: clicking an active `StatFilterObj` applies an inner black `GlowFilter` (blur 20, alpha 1, strength 1, quality 2) as the active visual; deactivating removes all filters.
- **Ally cost coloring**: `alliesFee` text is colored orange-gold (`0xFBAADC` — actually `0xFBAADC` hex ≈ pink, integer `16487452`) when the ally discount is positive (surcharge), white when zero or negative.
- **Console button legend**: the `buttonLegend` clip stays hidden until `onTargetFrame()` signals readiness via the `ENTER_FRAME` loop, then the listener is removed. On frame 20 the legend is positioned to `"ConsoleLoc"`.
- **`setAcceptButtonText` prefix**: the label is prefixed with `"/t "`, suggesting a tab-stop or translate-token convention consumed by the `LabelButton` renderer.
