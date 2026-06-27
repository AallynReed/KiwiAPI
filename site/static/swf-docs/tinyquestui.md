# tinyquestui.swf
> The Tiny Quest UI window displays the player's active quests organised into collapsible sections (Active, Completed, Claim, Cancel), each row showing quest name, progress bar, reward tooltip, and action buttons. It opens via the quest-tracker or quest board and supports both mouse and console D-pad navigation.

**Document/main class:** `TinyQuestUI` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 5 (`TinyQuestUI`, `TinyQuestRow`, `TinyQuestRowData`, `TinyQuestRowExternalDataSource`, `Hat`) + 7 `TinyQuestUI_fla` timeline classes + ~12 skin/asset wrappers

---

## Main class: `TinyQuestUI`

`TinyQuestUI` is the root component for the quest list window. On construction it registers frame scripts for frames 1, 11, and 21 (all `stop()`), sets `rowView.verticalStep = 20`, and on console starts an `ENTER_FRAME` listener that waits for the timeline to reach the console-layout target frame before sending `buttonLegend` to play "default". On `configUI()` it wires the "Claim All" button, populates the `DynamicRowView` with a `TinyQuestRow` template and a `TinyQuestRowExternalDataSource`, and registers ~20 `ExternalInterface` callbacks.

The window title is set to `$TinyQuestUI` via the component inspector on the `winHeader` slot.

### Public methods

- `setSection(sectionIdx : int) : void` — on PC scrolls the `rowView` to the section heading; on console calls `activateSelection()`.
- `setRow(sectionIdx, rowIdx : int) : void` — on PC scrolls to the row's absolute position (offset for rows beyond index 7); on console calls `activateSelection()`.
- `setProgress(sectionIdx, rowIdx, remaining, total : Number) : void` — delegates to `TinyQuestRow.setProgress`.
- `setProgressText(sectionIdx, rowIdx, text) : void` — delegates to `TinyQuestRow.setProgressText`.
- `setAutoCompleteToken(sectionIdx, rowIdx, text) : void` — delegates to `TinyQuestRow.setInstaCompleteTokenText`.
- `setClaimVisible / setCancelBtnVisible / setInstantCompleteBtnVisible(sI, rI, bool)` — show/hide per-row action buttons.
- `setQuestName / setQuestDesc / setQuestId(sI, rI, val)` — set text/ID on the row.
- `setRowSlotBlueprint(sI, rI, slotIdx, blueprint) : void` — sets icon on an ally slot.
- `setSlotList(sI, rI, count) : void` — creates N `Slot` instances in a row.
- `setRewardTooltipData(sI, rI, blueprint, amount, slotIdx) : void` — stages tooltip slot data.
- `setRewardTooltipXP(sI, rI, xp) : void` — sets tooltip XP value.
- `setRewardTooltipSlotCount(sI, rI, count) : void` — reserves slot arrays.
- `onCollapseAll(event : DataEvent) : void` — collapses or expands all sections based on `event.data`.
- `setCategory(categoryIdx : int) : void` — clears and resets the row view.
- `resetFocus() : void` — resets console navigation to section 0, row 0.
- `dispatchClaimAllQuest() / onClaimAllQuest(e)` — call `ExternalInterface.call("OnClaimAllQuest")`.

### Key fields

- `rowView : DynamicRowView` — the scrollable sectioned list; vertical step 20 px.
- `dataSource : TinyQuestRowExternalDataSource` — feeds row data to the `DynamicRowView`.
- `winHeader : MovieClip` — window header; title `$TinyQuestUI`.
- `categories : MovieClip` — category tab bar (not manipulated in AS; driven by engine).
- `backBanner : MovieClip` — decorative banner.
- `buttonLegend : MovieClip` — console button-legend strip; driven to frames "default", "active", "claim", "cancel", "ConsoleLoc".
- `claimAllBtn : LabelButton` — label set to "Claim All"; click fires `OnClaimAllQuest`.
- `currentNavMode : int` — `NavModeSections (0)` or `NavModeRows (1)`.
- `highlightedSection : int`, `highlightedRow : int` — console cursor position.
- `highlightedMetaCategory : int` — category-level cursor (tracked but not fully wired in this file).
- `active / completed / claim / cancel : int` — section index constants (0/1/2/3).

### Frame scripts / timeline

- **Frame 1** (`frame1`): `stop()`.
- **Frame 11** (`frame11`): `stop()`.
- **Frame 21** (`frame21`): `stop()`, then `buttonLegend.gotoAndStop("ConsoleLoc")` — console layout frame.

### Runtime dependencies & integration

- `ExternalInterface.addCallback` registrations: `setSection`, `setRow`, `setProgress`, `setProgressText`, `setAutoCompleteToken`, `setClaimVisible`, `setCancelBtnVisible`, `setInstantCompleteBtnVisible`, `setQuestName`, `setQuestDesc`, `setQuestId`, `setRowSlotBlueprint`, `setSlotList`, `moveHighlightVertical`, `activateSelection`, `previousNavMode`, `dispatchClaimAllQuest`, `resetFocus`, `setRewardTooltipData`, `setRewardTooltipXp`, `setRewardTooltipSlotCount`, `dispatchClaimRewards`.
- `ExternalInterface.call("OnClaimAllQuest")` — claim-all button or callback.
- `ExternalInterface.call("CloseScreen")` — fired from `previousNavMode` when at top section level.
- `ExternalInterface.call("OnClaimRewards", instanceId)` — fired from `dispatchClaimRewards` when a row has a visible claim button.
- `ExternalInterface.call("OnSlotLeave")` — fired when unhighlighting a row.
- `GlowFilter` (inner, color `0xCCCC00`, strength 100) applied to highlighted sections and rows on console; NX additionally shows explicit `highlight` sub-clips.
- `IggyFunctions.inIggy` guard on callback registration.
- Translate key: `$TinyQuestUI` (window header).

---

## Other game-specific classes

- `TinyQuestRow` (extends `DynamicRowViewRow`, embeds `assets.swf#symbol160`) — the per-quest row widget. Holds a name `TextField`, `progressBar` MovieClip (with `filling` + `currentProgressText`), `claimBtn`, `cancelBtn`, `instantCompleteBtn`, `rewardTooltip`, `stackList`, `assignPetSlots`, ally `Slot` array, and `instanceId`. Key methods:
  - `setData(obj)` — populates all fields from data object; jumps to `frameLocked` frame.
  - `setProgress(remaining, total)` — fills `progressBar.filling.width` proportionally and formats a `HH:MM:SS` countdown string.
  - `setSlotList(count)` — dynamically creates `Slot` instances with hover tooltips; first slot gets a `Hat` child.
  - `onToolTipHover / RollOut` — builds/tears-down a live `rewardTooltip` with dynamic `Slot` children showing blueprint icons and amounts.
  - `swapRowUp / swapRowDown` — calls `ExternalInterface.call("SwapFavoritePositions", sectionIndex, rowIndex, targetIndex)`.
  - Action button clicks call `ExternalInterface.call("OnClaimRewards" / "OnCancelQuest" / "OnCompleteQuestInstantly", instanceId)`.
  - Slot hover/leave calls `ExternalInterface.call("OnSlotEnter" / "OnSlotLeave", instanceId, slotIdx, x, y)`.
  - Console: frame labels `unlocked/unlockedConsole/locked/lockedConsole`; `GlowFilter` highlight plus NX `highlight` clip.
  - Four-frame timeline (frames 1–4, all `stop()`).

- `TinyQuestRowData` — plain data-transfer object with fields: `name`, `instanceId`, `icon`, `locked`, `equipped`, `sleeping`, `duration`, `progressText`, `claimId`, `showClaimBtn`, `showCancelBtn`, `showInstantCompleteBtn`, `questDesc`, `showRewardTooltip`, `blueprint`, `amount`. `clear()` resets to defaults.

- `TinyQuestRowExternalDataSource` (extends `DynamicRowViewExternalDataSource`) — bridges the kiwi framework's virtual-scroll system; overrides `getData(sectionIdx, rowIdx)` to call `rowView.setRowData` with a shared `TinyQuestRowData` instance, then clears it.

### TinyQuestUI_fla timeline symbols

- `TinyQuestProgressBar_38` (embeds `assets.swf#symbol140`) — progress bar MovieClip with `filling` child and `currentProgressText` TextField; 3-stop timeline.
- `CollapsedIcon_49` — collapse/expand icon (no logic).
- `equipped_58` — "equipped" indicator (2-frame).
- `buttonLegend_22` — main button-legend strip.
- `ButtonLegendClaimAll_25` — claim-all button legend clip.
- `subcategoryHeader_48` — subcategory heading graphic.
- `slotFrame_56` — slot frame graphic.
- `qualityPips_64` — quality pip indicator.
- `storeButton_32` — store link button graphic.

### Asset wrappers (no logic)

~12 classes: `SlotBackground`, `ScrollArrow*_*Skin` (×8), `ScrollThumb_*Skin` (×3), `ScrollTrack_skin`, `ScrollBar_thumbIcon`, `FavoriteCheckBox`, `btnClearText`, `btnGreen`, `btnGreenIcon_small`, `btnAutoAcceptComplete`, `marketplaceButton`, `Hat` (trivial MovieClip for the ally slot hat graphic).

---

## Notable logic

- **Two-level console navigation**: `currentNavMode` toggles between `NavModeSections` and `NavModeRows`. Pressing confirm in sections mode expands the section and drops into rows mode; pressing back in rows mode collapses and returns to sections. `buttonLegend` frame is updated to match context ("active", "claim", "cancel", "default").
- **`GlowFilter` highlight**: Rather than a separate "selected" clip, the highlighted section/row has a gold inner glow (`color: 0xCCCC00`) applied dynamically. NX additionally toggles a `heading.highlight` or `row.highlight` sub-clip.
- **Dynamic reward tooltip**: `TinyQuestRow.onToolTipHover` creates `Slot` instances at runtime inside `rewardTooltip.rewardTooltipPos`, sizing the tooltip background to fit, then destroys them on roll-out.
- **Progress bar HH:MM:SS**: `setProgress` derives hours/minutes/seconds from the `remaining` parameter and displays them zero-padded.
- **Row recycling**: `TinyQuestRowExternalDataSource` uses a single shared `TinyQuestRowData` instance, calling `clear()` after each `getData` to satisfy the kiwi virtual-scroll contract.
