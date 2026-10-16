# tinyquestui.swf
> The Tiny Quest window: quests in engine-supplied collapsible sections, each row with name, level, difficulty stars, a countdown bar, the allies sent, a reward tooltip and Claim / Cancel / Insta-Complete buttons, plus Claim All; mouse and console D-pad navigation.

**Document/main class:** `TinyQuestUI` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 5 (`TinyQuestUI`, `TinyQuestRow`, `TinyQuestRowData`, `TinyQuestRowExternalDataSource`, `Hat`) + 9 `TinyQuestUI_fla` timeline classes + ~12 skin/asset wrappers

---

## Main class: `TinyQuestUI`

`TinyQuestUI` is the root component for the quest list window. On construction it registers frame scripts for frames 1, 11, and 21 (all `stop()`), sets `rowView.verticalStep = 20`, and on console starts an `ENTER_FRAME` listener that waits for the timeline to reach the console-layout target frame before sending `buttonLegend` to play "default". On `configUI()` it wires the "Claim All" button, populates the `DynamicRowView` with a `TinyQuestRow` template and a `TinyQuestRowExternalDataSource`, and registers ~20 `ExternalInterface` callbacks.

The window title is set to `$TinyQuestUI` via the component inspector on the `winHeader` slot.

The window art covers 406×691 at the origin of a 523×842.3 stage; the rest of the stage is empty and is where the reward tooltip and the scrollbar open.

### Public methods

- `setSection(sectionIdx : int) : void` — on PC scrolls the `rowView` to the section heading; on console calls `activateSelection()`.
- `setRow(sectionIdx, rowIdx : int) : void` — on PC scrolls to the row's absolute position (offset for rows beyond index 7); on console calls `activateSelection()`.
- `setProgress(sectionIdx, rowIdx, remaining, total : Number) : void` — delegates to `TinyQuestRow.setProgress`. Both values are in seconds.
- `setProgressText(sectionIdx, rowIdx, text) : void` — delegates to `TinyQuestRow.setProgressText`.
- `setAutoCompleteToken(sectionIdx, rowIdx, text) : void` — delegates to `TinyQuestRow.setInstaCompleteTokenText`; this is how the Insta-Complete token count reaches the row, as a string.
- `setClaimVisible / setCancelBtnVisible / setInstantCompleteBtnVisible(sI, rI, bool)` — show/hide per-row action buttons.
- `setQuestName / setQuestDesc / setQuestId(sI, rI, val)` — set text/ID on the row. The row's own `setQuestDesc` is empty and the row has no description field, so descriptions sent here are dropped.
- `setQuestLevel(sI, rI, level : int) : void` — delegates to `TinyQuestRow.setQuestLevel`.
- `setQuestStarDifficulty(sI, rI, stars : int) : void` — delegates to `TinyQuestRow.setQuestStarDifficulty`. These two callbacks are the only way level and difficulty arrive (see `TinyQuestRowData`); the engine sends them after the row data.
- `setRowSlotBlueprint(sI, rI, slotIdx, blueprint) : void` — sets icon on an ally slot.
- `setSlotList(sI, rI, count) : void` — creates N `Slot` instances in a row: the allies the quest went out with.
- `setRewardTooltipData(sI, rI, blueprint, amount, slotIdx) : void` — stages tooltip slot data.
- `setRewardTooltipXP(sI, rI, xp) : void` — sets tooltip XP value.
- `setRewardTooltipSlotCount(sI, rI, count) : void` — reserves slot arrays. The engine builds a row's reward tooltip in this order: `setRewardTooltipSlotCount` first, then `setRewardTooltipData` once per reward, then `setRewardTooltipXp` once.
- `onCollapseAll(event : DataEvent) : void` — collapses or expands all sections based on `event.data`.
- `setCategory(categoryIdx : int) : void` — clears and resets the row view.
- `resetFocus() : void` — resets console navigation to row 0 of the first section that has rows (section 0 if all are empty).
- `dispatchClaimAllQuest() / onClaimAllQuest(e)` — call `ExternalInterface.call("OnClaimAllQuest")`.

### Key fields

- `rowView : DynamicRowView` — the scrollable sectioned list; vertical step 20 px.
- `dataSource : TinyQuestRowExternalDataSource` — feeds row data to the `DynamicRowView`.
- `winHeader : MovieClip` — window header; title `$TinyQuestUI`.
- `categories : MovieClip` — declared but not placed on the timeline, so it is `null` at runtime; nothing in the code touches it.
- `backBanner : MovieClip` — declared but not placed on the timeline either (the root holds only `rowView`, `claimAllBtn`, `winHeader` and `buttonLegend`).
- `buttonLegend : MovieClip` — console button-legend strip; driven to frames "default", "active", "claim", "cancel", "ConsoleLoc". The clip (`buttonLegend_22`) has only the first four labels.
- `claimAllBtn : LabelButton` — label set to "Claim All"; click fires `OnClaimAllQuest`.
- `currentNavMode : int` — `NavModeSections (0)` or `NavModeRows (1)`.
- `highlightedSection : int`, `highlightedRow : int` — console cursor position.
- `highlightedMetaCategory : int` — category-level cursor (tracked but not fully wired in this file).
- `active / completed / claim / cancel : int` — section index constants (0/1/2/3), used only to pick the console button-legend frame when a section is entered: 0 → `active`, 1 → `claim`, 3 → `cancel`, anything else (including 2) → `default`. Sections themselves arrive from the engine in call order.

### Frame scripts / timeline

- **Frame 1** (`frame1`, label `PC`): `stop()`.
- **Frame 11** (`frame11`, label `Console`): `stop()`.
- **Frame 21** (`frame21`, label `ConsoleLoc`): `stop()`, then `buttonLegend.gotoAndStop("ConsoleLoc")` — console layout frame.

Stage 523×842.3, 5 fps, 30 frames.

### Runtime dependencies & integration

- `ExternalInterface.addCallback` registrations: `setSection`, `setRow`, `setProgress`, `setProgressText`, `setAutoCompleteToken`, `setClaimVisible`, `setCancelBtnVisible`, `setInstantCompleteBtnVisible`, `setQuestName`, `setQuestDesc`, `setQuestId`, `setQuestLevel`, `setQuestStarDifficulty`, `setRowSlotBlueprint`, `setSlotList`, `moveHighlightVertical`, `activateSelection`, `previousNavMode`, `dispatchClaimAllQuest`, `resetFocus`, `setRewardTooltipData`, `setRewardTooltipXp`, `setRewardTooltipSlotCount`, `dispatchClaimRewards`. Every `TinyQuestRow` also registers `dispatchInstaCompleteQuest`, `dispatchCancelQuest` and `clearRewards` from its own `configUI`, and the data source registers the `DynamicRowView` set (`setSectionData`, `setSectionDataEmpty`, `setDataDirty`, `updateData`, `updateCurrentView`).
- The list is pulled: `GetSectionData` → one `setSectionData(index, numRows, name, infoText, collapsed)` per section, appended in call order (the index is ignored), and `GetRowData(section, row)`, which the engine answers by writing `TinyQuestRowData` fields into `dataSource.rowData` by name. After that first fill, updates to a row come through the per-row callbacks above, addressed by section and row index. The engine sends `setDataDirty` on every change to any quest, and an empty quest list is answered with `setSectionDataEmpty`.
- Console dispatches name no quest: the engine moves the cursor (`setSection`, `setRow`, `moveHighlightVertical`) and then calls `dispatchClaimRewards`, `dispatchCancelQuest` or `dispatchInstaCompleteQuest`. `dispatchClaimRewards` acts on the highlighted row; the other two are row methods and act on whichever row registered them, checking only that row's own button.
- `ExternalInterface.call("OnClaimAllQuest")` — claim-all button or callback.
- `ExternalInterface.call("CloseScreen")` — fired from `previousNavMode` when at top section level.
- `ExternalInterface.call("OnClaimRewards", instanceId)` — fired from `dispatchClaimRewards` when a row has a visible claim button.
- `ExternalInterface.call("OnSlotLeave")` — fired when unhighlighting a row.
- `GlowFilter` (inner, color `0xCCCC00`, strength 100) applied to the highlighted section heading on console; NX additionally shows the heading's `highlight` sub-clip. Highlighted rows show their own `highlight` clip on every console.
- `IggyFunctions.inIggy` guard on callback registration.
- Translate key: `$TinyQuestUI` (window header).

---

## Other game-specific classes

- `TinyQuestRow` (extends `DynamicRowViewRow`, embeds `assets.swf#symbol168`) — the per-quest row widget. Holds a name `TextField`, `progressBar` MovieClip (with `filling` + `currentProgressText`), `questLevelText`, `questDifficultyIcon`, `highlight`, `claimBtn`, `cancelBtn`, `instantCompleteBtn`, `rewardTooltip`, `stackList`, `assignPetSlots`, ally `Slot` array, and `instanceId`. Key methods:
  - `setData(obj)` — populates all fields from data object (including `lvl` and `starDifficulty`); jumps to `frameLocked` frame; on console clears any highlight state.
  - `setQuestLevel(level)` — writes the quest level into `questLevelText`.
  - `setQuestStarDifficulty(stars)` — clamps `stars` to 1–3 and shows frame `1star`, `3star` or `5star` of `questDifficultyIcon`.
  - `clearHighlightState()` — hides the row `highlight`, the reward tooltip and its `highlight`, and disposes the tooltip slots.
  - `clearRewards()` — removes each reward `Slot` from whatever parent it has. Before the October 2026 build it only removed slots parented to the row itself, which they never are (they sit in `rewardTooltip.rewardTooltipPos`), so every hover left a set of slots behind.
  - `setProgress(remaining, total)` — fills `progressBar.filling.width` to `(total - remaining) / total` of 310 px and formats a `HH:MM:SS` countdown string from `remaining`; the text is left as it was when `remaining` is 0.
  - `setSlotList(count)` — dynamically creates `Slot` instances with hover tooltips; first slot gets a `Hat` child.
  - `setIcon(slotIdx, blueprint)` — sets an ally slot's icon through `forceIconImage`.
  - `onToolTipHover / RollOut` — builds/tears-down a live `rewardTooltip` with dynamic `Slot` children showing blueprint icons and amounts. On console the tooltip's own `highlight` clip is shown, sized to the tooltip background.
  - `swapRowUp / swapRowDown` — calls `ExternalInterface.call("SwapFavoritePositions", sectionIndex, rowIndex, targetIndex)`. Unreachable here: the row symbol has no `arrowUp` / `arrowDown` children, nothing attaches `onArrowUpClick` / `onArrowDownClick`, and `TinyQuestUI.swapRows` is not registered as a callback.
  - Action button clicks call `ExternalInterface.call("OnClaimRewards" / "OnCancelQuest" / "OnCompleteQuestInstantly", instanceId)`.
  - Slot hover calls `ExternalInterface.call("OnSlotEnter", instanceId, slotIdx, x, y)`; leaving calls `OnSlotLeave(slotIdx)` with the slot index only.
  - `setQuestDesc` and `setEquipped` have empty bodies and `isLocked()` always returns `false`. `setInstaCompleteTokenAmmount` assigns `param1.toString` (the method, missing its parentheses); nothing calls it, as the name is not in the client, which uses `setAutoCompleteToken` instead.
  - Console: frame labels `unlocked/unlockedConsole/locked/lockedConsole`; selection is shown with the row's `highlight` clip.
  - Four-frame timeline (frames 1–4, all `stop()`).

- `TinyQuestRowData` — plain data-transfer object with fields: `name`, `instanceId`, `icon`, `locked`, `equipped`, `sleeping`, `duration`, `progressText`, `claimId`, `showClaimBtn`, `showCancelBtn`, `showInstantCompleteBtn`, `questDesc`, `showRewardTooltip`, `blueprint`, `amount`, `lvl`, `starDifficulty`. `clear()` resets to defaults (`lvl` and `starDifficulty` reset to 1). The engine writes these by name through `dataSource.rowData`, but `lvl`, `starDifficulty`, `progressText` and `showRewardTooltip` appear nowhere in `Trove_x64.exe`, so they are never written: `setData` always shows level 1 and one star until the `setQuestLevel` / `setQuestStarDifficulty` callbacks correct them.

- `TinyQuestRowExternalDataSource` (extends `DynamicRowViewExternalDataSource`) — bridges the kiwi framework's virtual-scroll system; overrides `getData(sectionIdx, rowIdx)` to call `rowView.setRowData` with a shared `TinyQuestRowData` instance, then clears it.

### TinyQuestUI_fla timeline symbols

- `TinyQuestProgressBar_38` (embeds `assets.swf#symbol133`) — progress bar MovieClip with `filling` child and `currentProgressText` TextField; 3-stop timeline.
- `CollapsedIcon_50` — collapse/expand icon (no logic).
- `equipped_59` — "equipped" indicator (2-frame).
- `buttonLegend_22` — main button-legend strip.
- `ButtonLegendClaimAll_25` — claim-all button legend clip.
- `subcategoryHeader_49` — subcategory heading graphic.
- `slotFrame_57` — slot frame graphic.
- `qualityPips_65` — quality pip indicator.
- `quest_difficulty_icon_41` (embeds `assets.swf#symbol148`) — difficulty icon with frame labels `1star`, `2star`, `3star`, `5star`, `complete`, `failed`; code only ever selects `1star`, `3star` and `5star`.

### Asset wrappers (no logic)

~12 classes: `SlotBackground`, `ScrollArrow*_*Skin` (×8), `ScrollThumb_*Skin` (×3), `ScrollTrack_skin`, `ScrollBar_thumbIcon`, `FavoriteCheckBox`, `btnClearText`, `btnGreen`, `btnGreenIcon_small`, `btnAutoAcceptComplete`, `claimBtn` (4-frame `BaseButton`), `marketplaceButton`, `Hat` (trivial MovieClip for the ally slot hat graphic).

---

## Notable logic

- **Two-level console navigation**: `currentNavMode` toggles between `NavModeSections` and `NavModeRows`. Pressing confirm in sections mode expands the section and drops into rows mode; pressing back in rows mode collapses and returns to sections. `buttonLegend` frame is updated to match context ("active", "claim", "cancel", "default").
- **Console highlight**: The highlighted section heading gets a gold inner glow (`color: 0xCCCC00`) applied dynamically, plus its `heading.highlight` sub-clip on NX. Rows use their `highlight` sub-clip on every console (before the October 2026 build rows also used the glow, with the clip on NX only).
- **Level and star difficulty**: Each row shows the quest's level and a 1/3/5-star difficulty icon, set from `TinyQuestRowData.lvl` / `starDifficulty` or the `setQuestLevel` / `setQuestStarDifficulty` callbacks (added in the October 2026 build). The client never writes the two buffer fields, so the values players see come from the callbacks.
- **Dynamic reward tooltip**: `TinyQuestRow.onToolTipHover` creates `Slot` instances at runtime inside `rewardTooltip.rewardTooltipPos`, sizing the tooltip background to fit, then destroys them on roll-out.
- **Progress bar HH:MM:SS**: `setProgress` derives hours/minutes/seconds from the `remaining` parameter (seconds) and displays them zero-padded, with no day field: a two-day quest reads as 48 hours.
- **Row recycling**: `TinyQuestRowExternalDataSource` uses a single shared `TinyQuestRowData` instance, calling `clear()` after each `getData` to satisfy the kiwi virtual-scroll contract.
