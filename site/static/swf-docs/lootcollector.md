# lootcollector.swf

> The Loot Collector panel appears after fighting enemies or opening containers — it lists items waiting to be deconstructed (collected) or bought back from a previous deconstruction. The player can lock individual items to protect them, then collect or buy back the rest individually or all at once. The same window switches between "collect" and "buyback" modes depending on which station is active.

**Document/main class:** `LootCollector` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 15

---

## Main class: `LootCollector`

`LootCollector` is the root document class. It owns a scrollable list of loot rows (`LootList`/`LootRow`), an `InlineTooltip` for item stat details, a reward-slot preview area (`collectionInfoWindow`), and a button strip (`collectionButtonInfo`). The class tracks two mutually exclusive modes — **collect** (deconstruct items) and **buyback** (repurchase previously deconstructed items) — and two station contexts: generic and **Compost** (station id 8), which swaps all translate keys to Compost-specific strings.

In the constructor, frame scripts are registered for frames 0 and 15, the loot list is configured (spacing, scrollbar, events), the five reward preview slots are initialised and hidden, and the header component property `$Deconstructor_WinTitle` is written. `configUI()` fires after the component tree is ready and registers every public method as an `ExternalInterface` callback. In non-Iggy (test) mode it pre-populates 20 dummy rows instead.

### Public methods

- `addCollectableLoot(slotId:int, iconImage:String, name:String, masteryText:String, quantity:int, rarity:int, showQuantity:Boolean, quality:int) : void` — creates a `LootRow` MovieClip, assigns icon/rarity/quantity/quality to its embedded `Slot`, sets name and mastery text fields, and appends it to `lootList` via `addItemWithId`. On console, automatically calls `ShowSlotDetails` for the first row added.
- `updateLootQuantity(slotId:int, quantity:int) : void` — looks up the row by id and updates the slot's quantity display.
- `clearSlotDetails() : void` — clears the `statDetails` InlineTooltip and hides `collectionInfoWindow.helpText`.
- `refreshList() : void` — clears the entire list, slot details, and reward slots; called by the game when the panel is re-opened.
- `formatRewardSlots(count:int) : void` — shows `count` reward slots (rewardSlot0–4) centred horizontally and hides/clears any unused ones.
- `showSlotDetails(slotId:int, statLine1:String, statLine2:String, p4:int, p5:int, p6:Boolean, p7:int) : void` — delegates to `statDetails.showSlotDetails(...)`, moves the row to "selected" or "lockedSelected" frame, and shows `helpText`.
- `showInlineTooltip(p1:int, p2:String, p3:uint, p4:Boolean, p5:int, p6:uint) : void` — delegates to `statDetails.showInlineTooltip(...)` if the hovered row still exists.
- `toggleSlotLock(slotId:int) : void` — only active in MODE_COLLECT. Toggles the `locked` boolean on the `LootRow` MovieClip, moves it to the appropriate timeline frame (`locked`/`unselected`/`lockedSelected`/`selected`), calls `ExternalInterface.call("LockSlot", slotId, locked)`, and refreshes the collect-all button's enabled state.
- `collectAll(event:MouseEvent = null) : void` — iterates every child of `lootList.viewportMovieClip`; for each unlocked `LootRow` calls `DeconstructItem`/`BuyBackItem` (with `collectAll=true`), then calls `DeconstructAll`/`BuyBackAll` once at the end.
- `collectSlot() : void` — collects or buys back the currently highlighted slot (uses `_lastSlot`). Calls `DeconstructItem`/`BuyBackItem` if unlocked, or `DeconstructItemFailed`/`BuyBackItemFailed` if locked.
- `toggleMode(event:MouseEvent = null) : void` — calls `ExternalInterface.call("ToggleMode")` with no additional args; the server decides which mode to switch to.
- `addDeconstructReward(slotIndex:int, iconImage:String, quantity:Number, showQuantity:Boolean) : void` — populates one of the five `rewardSlot` entries in `collectionInfoWindow` and makes it visible.
- `collectLoot(slotId:int) : void` — removes the row from `lootList` by id, calls `ShowSlotDetails` on the returned next-row id, and refreshes the collect-all button.
- `setMode(mode:int, station:int) : void` — switches between MODE_COLLECT (0) and MODE_BUYBACK (1). Clears the list and detail pane, then updates the list title, help text, button labels, and button labels for both PC and console layouts based on mode + station. No-ops if mode is unchanged.
- `updateCollectAllButton() : void` — enables the collectAll button only if at least one row is unlocked; disabled state is only applied on non-console.
- `moveSlotCursor(direction:int) : void` — console D-pad navigation. Gets the adjacent row id from `lootList.getAdjacentItemId`, calls `ShowSlotDetails`, and scrolls the list if the row is outside the visible window (assumes 6.91-pixel row height, 5 rows visible, 3.55-pixel initial offset).
- `getPackedSlotPosition(index:uint) : int` — returns a 32-bit int with the global X in the high 16 bits and global Y in the low 16 bits for the row at the given viewport child index.
- `getPackedSlotSize(index:uint) : int` — returns a 32-bit int with width in the high 16 bits and height in the low 16 bits for the row at the given index.

### Key fields

- `lootList : LootList` — the scrollable tile view holding all `LootRow` entries.
- `statDetails : InlineTooltip` — shows item stat info to the right of the list.
- `collectionInfoWindow : MovieClip` (`collectionInfoWindow_17`) — contains `rewardSlot0`–`rewardSlot4` (each a `slot_large`) and a `helpText` TextField.
- `collectionButtonInfo : MovieClip` (`collectionButtonInfo_18`) — contains `collectAllBtn` (`btnGreen`), `toggleModeBtn` (`btnBuyback`), `helpText`, and `lockText` TextFields.
- `collectAllText : TextField` — console-only label above the collect-all button.
- `collectText : TextField` — console-only label above the single-collect button.
- `button_console_north : MovieClip` — console button prompt visible only in collect/buyback modes.
- `consoleBtnLock : MovieClip` — console lock button, visible only in MODE_COLLECT.
- `m_header : WindowHeaderSmall` — window title bar; title set to `$Deconstructor_WinTitle`, `allowFontResize = true`.
- `mode : int` — current mode (-1 uninitialised, 0 = collect, 1 = buyback).
- `m_station : int` — station type id; 8 = Compost (changes all visible strings to Compost variants).
- `slotHovered : int` — id of the row under the mouse cursor (-1 if none).
- `_lastSlot : int` — id of the row last shown in the details pane; setter deselects the previous row's frame.

### Frame scripts / timeline

- **Frame 0** (`frame1`) — `stop()`.
- **Frame 15** (`frame16`) — `stop()`. (Two-frame states suggest an open/close animation or a PC/console layout switch at frame 16.)

### Runtime dependencies & integration

**Iggy / ExternalInterface callbacks registered (in → Flash):**
`addCollectableLoot`, `updateLootQuantity`, `clearSlotDetails`, `refreshList`, `formatRewardSlots`, `showSlotDetails`, `showInlineTooltip`, `toggleSlotLock`, `addDeconstructReward`, `collectLoot`, `moveSlotCursor`, `collectSlot`, `collectAll`, `setMode`, `getPackedSlotPosition`, `getPackedSlotSize`

**ExternalInterface calls (Flash → game):**
`ShowSlotDetails(slotId)`, `LockSlot(slotId, locked)`, `DeconstructItem(slotId, singleItem, collectAll)`, `DeconstructItemFailed(slotId, singleItem)`, `DeconstructAll()`, `BuyBackItem(slotId, singleItem)`, `BuyBackItemFailed(slotId, singleItem)`, `BuyBackAll()`, `ToggleMode()`

**IggyFunctions.inIggy** — gates the entire ExternalInterface setup; falls back to populating 20 test rows in Flash preview mode.

**IsConsole()** — global function call that branches the layout: hides `collectionButtonInfo` button strip and instead shows `consoleBtnLock`, `button_console_north`, `collectAllText`, `collectText`; also auto-focuses the first row on console.

**translate keys used:**
`$Deconstructor_WinTitle`, `$LootCollector_List_Title_Inventory`, `$LootCollector_List_Title_BuyBack`, `$LootCollector_Compost_List_Title_BuyBack`, `$LootCollector_CollectToEarn`, `$LootCollector_CompostToEarn`, `$LootCollector_BuyBackCost`, `$LootCollector_CollectionText_Right`, `$LootCollector_Compost_CollectionText_Right`, `$LootCollector_CollectionText_Lock`, `$LootCollector_CollectionBtnLabel`, `$LootCollector_Compost_CollectionBtnLabel`, `$LootCollector_BuyBackBtnLabel`, `$LootCollector_BuyBackText_Right`, `$LootCollector_Console_CollectAllBtnLabel`, `$LootCollector_Compost_Console_CollectAllBtnLabel`, `$LootCollector_Console_CollectBtnLabel`, `$LootCollector_Compost_Console_CollectBtnLabel`, `$LootCollector_Console_BuyBackAllBtnLabel`, `$LootCollector_Console_BuyBackBtnLabel`

---

## Other game-specific classes

### Top-level classes

- `LootRow` — `MovieClip` subclass, embeds `symbol57`. Holds a `Slot` (icon/rarity/quantity/quality), `nameText` and `masteryText` TextFields, and `backgroundUnlocked` clip. Four timeline states on frames 0/12/24/36: **unselected**, **selected**, **locked**, **lockedSelected** (exact label names inferred from `gotoAndStop` calls in `LootCollector`). `locked` is a dynamic property set at runtime.
- `LootList` — `SpliceableTileView` subclass, embeds `symbol141`. Thin typed wrapper; all scrollable-list behaviour comes from the framework.
- `slot_large` — `Slot` subclass, embeds `symbol170`. Used as the five reward preview slots in `collectionInfoWindow`; sized to 77 px via `setSlotSize(77)`.
- `btnGreen` — `LabelButton` subclass, embeds `symbol160`. Four 10-frame button states (up/over/down/disabled). Used as the collect-all / buy-back-all primary action button.
- `btnGreen_small` — `LabelButton` subclass, embeds `symbol83`. Smaller variant of `btnGreen`; present in the library but not directly referenced in `LootCollector.as`.
- `btnGreenIcon_small` — `LabelButton` subclass, embeds `symbol73`. Icon-bearing small green button; present in the library.
- `btnBuyback` — `BaseButton` subclass, embeds `symbol150`. The mode-toggle button (`toggleModeBtn`) in `collectionButtonInfo`.
- `dummy` — `BitmapData` wrapping `/_assets/26_dummy.png` (52×52). Placeholder icon asset.

### LootCollector_fla timeline symbols

- `collectionInfoWindow_17` — `MovieClip`, embeds `symbol195`. Container for `rewardSlot0`–`rewardSlot4` (`slot_large`) and a `helpText` TextField. Reward slots are positioned and shown/hidden by `formatRewardSlots()`.
- `collectionButtonInfo_18` — `MovieClip`, embeds `symbol200`. Two-frame clip (frame 1 = collect mode button label, frame 2 = buyback mode label) holding `collectAllBtn` (`btnGreen`) and `toggleModeBtn` (`btnBuyback`), plus `helpText` and `lockText` TextFields. `__setProp` guards prevent duplicate initialisation.
- `listTitle_39` — `MovieClip`, embeds `symbol140`. Single-frame clip with a `title` TextField; used as the header row of `LootList`.
- `qualityPips_16` — `MovieClip`, embeds `symbol42`. Single-frame stop; displays quality pip indicators inside `Slot`.
- `slotFrame_47` — `MovieClip`, embeds `symbol21`. Three-frame clip (stop on each); rarity-border frame for regular-size slots.
- `slotFrameLarge_7` — `MovieClip`, embeds `symbol165`. Four-frame clip (stop on each); rarity-border frame for large (77 px) slots used in the reward preview area.
- `bonusClip_4` — `MovieClip`, embeds `symbol180`. Six-frame clip with `level_0`–`level_4` sub-clips; likely the quality/mastery bonus indicator shown in the tooltip or slot.
- `equipped_9` — `MovieClip`, embeds `symbol25`. Two-frame clip; equipped-item overlay badge on a slot.

### Asset wrapper classes (bitmap/shape only — not individually detailed)

9 classes: `rarity_frame_epic_png`, `rarity_frame_legendary_png`, `rarity_frame_crystal_png`, `rarity_frame_mystic_png`, `rarity_frame_radiant1_png`, `rarity_frame_rare_png`, `rarity_frame_relic_png`, `rarity_frame_resplendent_png`, `rarity_frame_shadow_png`, `rarity_frame_uncommon_png`, `rarity_frame_stellar` — all `BitmapData` subclasses embedding per-rarity border PNGs (77×77). Plus scrollbar skin stubs: `ScrollArrowDown_*Skin`, `ScrollArrowUp_*Skin`, `ScrollThumb_*Skin`, `ScrollTrack_skin`, `ScrollBar_thumbIcon`, `focusRectSkin` (pure shape symbols from the framework scroll bar).

---

## Notable logic

- **Lock-before-collect safety:** Left-clicking a row in MODE_COLLECT toggles its lock rather than collecting it. Right-click (or the console collect button) collects the row immediately. This prevents accidental destruction of desired items. The `collectAll` path skips any row whose `locked` flag is `true`.
- **Dual station context:** All user-visible strings branch on `m_station == STATION_COMPOST` (8). The Compost station shares the same window but presents entirely different action verbs ("Compost" vs "Collect/Deconstruct") via separate translate keys.
- **Console layout:** When `IsConsole()` is true, the `collectionButtonInfo` button strip is replaced by labelled controller-prompt clips (`consoleBtnLock`, `button_console_north`), D-pad navigation is driven by `moveSlotCursor`, and the panel auto-focuses the first row. Scroll position is computed using the hard-coded row height constant 6.91 px with a 3.55-offset baseline.
- **Packed position/size protocol:** `getPackedSlotPosition` and `getPackedSlotSize` encode two 16-bit values into a single 32-bit int using bit-shift and OR operations. This lets the C++ game layer query row positions for controller cursor overlay rendering without separate width/height/x/y calls.
- **Reward slot layout:** `formatRewardSlots(count)` centres up to 5 slots horizontally using the formula `x = 161 + (i - (count-1)/2) * (132 - (count-1)*17)`, compressing the spacing as more slots appear.
