# inventory.swf
> The player's Inventory window, showing item slots organized across Adventure, Build, Currency, Geode, and Discovery tabs, plus Personal Chest tabs. Supports up to three bag pages with drag-and-drop, slot expansion purchase, and console/NX button-legend overlays.

**Document/main class:** `Inventory` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 9 (excluding framework, asset wrappers, and `_fla/` timeline symbols)

---

## Main class: `Inventory`

`Inventory` is the root display object for the Inventory window. It extends `UIComponent` and manages all slot data, tab navigation, bag pagination, currency/discovery sub-views, and drop targets. On construction it registers three frame scripts (frames 1, 11, 21 for PC / Console / ConsoleLoc layout variants) and hides all optional UI until the game calls `initDisplay`.

In the Iggy runtime (`IggyFunctions.inIggy == true`) the constructor-time `configUI()` registers 34 `ExternalInterface` callbacks. Outside Iggy (Flash IDE preview) it falls back to a 50-slot demo layout.

### Public methods

- `checkForExpansion() : void` — Shows bag buttons (0-2), trash button, and footer; ghosts bags that are not yet unlocked based on `numBags`.
- `toggleExpansion(visible:Boolean) : void` — Shows or hides the expansion purchase button row inside the slots clip.
- `showChestTabs(show:Boolean) : void` — Toggles visibility of chest tabs 1-6 and sets their numeric labels.
- `setPersonalChest(isPersonal:Boolean) : void` — Marks this inventory as a personal chest and shifts the sort button position.
- `showMainTabs(show:Boolean) : void` — Toggles the five main-mode tabs (Adventure/Build/Currency/Geode/Discovery).
- `setTextureBaseName(base:String) : void` — Sets the render-target base name for all existing slot entries: a slot's icon is `<base><absolute slot index>`, named after the slot rather than the item in it (a personal chest sends `PersonalChest.Thumbnail.slot_`).
- `setSlot(slotId:uint, name:String, description:String, quantity:uint, rarity:uint, showQuantity:Boolean, quality:int, flag:Boolean, percent:Number) : void` — Writes item data into `slotInfo[]` and, if the slot is in the currently visible bag page, updates the on-screen `Slot` widget. `slotId` is the absolute index across all bags. The stock screen ignores `description` (a single space when the item has none) and the eighth argument; no argument identifies the item's type.
- `clearSlot(slotId:uint) : void` — Resets a slot to empty in both `slotInfo[]` and the visible `Slot`.
- `setSlotCount(totalSlots:int, slotsPerBag:int) : void` — Resizes the `slotInfo` array, recalculates `numBags`, and refreshes the current bag view. The engine sends it every time the window opens.
- `showNewItemNotification(tabName:String, slotId:int, count:int) : void` — Makes a "new item" badge visible on the relevant tab and lights up the slot glow. Not registered with `addCallback` and not called inside the SWF: the engine calls the public method directly. `tabName` is `AdventureInventory`, `BuildInventory` or `CurrencyInventory` (the clip is `newItem<tabName>`); `count` is unused, and the badge writes back the value it read (`(n++).toString()`), so its number never advances.
- `updateSlotQuantityAttr(slotId, qty, rarity) : void` — Live-updates quantity and rarity on a visible slot without a full redraw.
- `switchTab(direction:int) : void` — Cycles the active tab by +1/-1; for player inventory cycles the five modes, for chests wraps through 1-6.

### Key fields

- `currentMode : String` — One of the five `MODE_*` constants; governs which sub-panel (slots vs. `currencyView`) is shown.
- `slotInfo : Array` — Flat array of plain Objects, one per slot across all bags, used as the backing store for bag-switch rehydration.
- `slotsPerBag : int = 55` — Slots per bag page (overridden by `setSlotCount`).
- `numSlots / numBags : int` — Total slot count and computed bag count.
- `selectedBag : int` — Index of the currently displayed bag page.
- `isPlayerInventory : Boolean` — Distinguishes player bag from chest windows (affects expansion, bag buttons, tab navigation). Set only by `initDisplay`, the engine's signal that this window is the player's own inventory.
- `isPersonalChestInventory : Boolean` — Set when this instance is showing a personal storage chest.
- `currencyView : CurrencyInventoryView` — The Currency/Discovery sub-panel; toggled visible when the matching tab is active.
- `slots : MovieClip` — Container for the 55 timeline-placed `slot_0` … `slot_54` (`Slot` instances), the `expansion` buy button and the row overlays `lockedSlot1`–`lockedSlot10`.
- `winHeader : WindowHeaderSmall` — Window title bar, initialized with title `"INVENTORY"`.
- `sortBtn : LabelButton` — Sort-bag button; hidden in currency mode.
- `buttonLegend : MovieClip` — Console button-legend overlay; has multiple named frames (`Console`, `ConsoleLoc`, `Move`, `NX_handheld`, `NX_handheld_move`).
- `isLeft : Boolean` — Whether the inventory is docked left (affects NX button-legend positioning).

### Frame scripts / timeline

- **frame 1** (`frame1`, label `PC`) — `stop()`. PC layout (default).
- **frame 11** (`frame11`, label `Console`) — `stop()`, then forces bag buttons and `buttonLegend` to the `"Console"` frame.
- **frame 21** (`frame21`, label `ConsoleLoc`) — `stop()`, then forces `buttonLegend` to the `"ConsoleLoc"` (localized console) frame.

### Runtime dependencies & integration

**ExternalInterface callbacks registered (game → Flash):**
`setTextureBaseName`, `setWindowName`, `showChestTabs`, `setPersonalChest`, `showMainTabs`, `selectTab`, `showUnlockPage`, `hideUnlockPage`, `clearSlots`, `setSlot`, `setCurrencySlot`, `addCurrencyHeading`, `currenciesPopulated`, `clearSlot`, `initDisplay`, `updateSlotQuantityAttr`, `markSlotEquipped`, `markSlotInUse`, `setSlotCount`, `checkForExpansion`, `setExpansionPrice`, `showBag`, `setShowSort`, `highlightSlot`, `unhighlightSlot`, `moveSlotCursor`, `setCurrencyCategory`, `switchTab`, `switchBag`, `purchaseRow`, `showLootCollectorDetails`, `showTrigger`, `setMoveMode`, `UIComponent.onStageResized`

Inherited from `_kiwi` and part of the same contract: `objectPreviewReady` (`ObjectPreview`; no slot icon resolves without it), `DRAGHOST.DROP` (`SlotDragDropHelper`, delivers drops to `onDrop`), and the clipboard/text-composition callbacks every root `UIComponent` registers. The engine also calls `showNewItemNotification` directly, without a registration.

**ExternalInterface calls made (Flash → game):**
`INVENTORY.CONFIGURED` (slots per row, slots per bag, max bags — console only, sent once the console frame is reached; max bags is 3 for the player inventory, else 0), `SetShownSlots(start, end)`, `OnSortBag`, `OnBuy` (`"TWC"`), `OnTabClicked(identifier)`, `POST_SOUND_EVENT` (`Play_ui_window_tab`), `INVENTORY.SET_EXPANSION_VISIBLE`, `DROP_ONTO_BAG`, `DROP_ONTO_TRASH`, `INVENTORY.DROP_ONTO_WINDOW`, `Height` (NX height query), `BeginTransfer(currencyId)` (sent by no other SWF)

**Drag-and-drop:** `SlotDragDropHelper.registerDropCallback(onDrop)` registers `onDrop` as `DRAGHOST.DROP(stageX, stageY, sourceEntityId, sourceComponent, sourceSlotId)` for items dropped anywhere; hit-tests bag buttons, trash, and individual `Slot` instances to route to the correct `DROP_ONTO_*` call.

**Tabs:** tab identifiers are `adventure`, `build`, `currency`, `geode`, `discovery` and `chest_1`–`chest_6`, both for the engine's `selectTab(identifier)` and for `OnTabClicked(identifier)`. An engine-sent `selectTab` goes through `setTab`, which echoes `OnTabClicked` back unless that tab is already selected; the engine answers `OnTabClicked` by sending that tab's contents. When the engine opens the window on a currency category it sends `selectTab("currency")` and then `setCurrencyCategory(n)`.

**Personal chest pages:** the six chest tabs share one container of `numSlots` (50) slots and one texture base (`setTextureBaseName` is sent once, with `PersonalChest.Thumbnail.slot_`), so the render target `PersonalChest.Thumbnail.slot_N` shows slot N of whichever page the engine filled last. On `OnTabClicked("chest_N")` the engine sends `clearSlots` and then `setSlot` for indices 0–49 for a page that is owned, or `showUnlockPage(priceCredits, walletCredits)` and no slots for one that is not. `showUnlockPage` is the only place a page's price is stated and is always about the selected page; `hideUnlockPage` is also sent while a tab change is in flight.

**Placement:** the stage is 451×968. The engine places the window by setting the root's `x`/`y` — towards the right edge of the display for the player's inventory (it can overhang the edge), towards the left for a personal chest.

**translate() keys used:** `$Inventory_Mode_Adventure`, `$Inventory_Mode_Build`, `$Inventory_Mode_Currency`, `$Inventory_Mode_Geode`, `$Inventory_Mode_Discovery`, `$Select_ButtonLegend`, `$Move_ButtonLegend`, `$SwitchScreen_ButtonLegend`, `$LRToggle_ButtonLegend`, `$CreditsBuy_ButtonLegend`, `$CubitsBuy_ButtonLegend`

**Events:** `MouseEvent.CLICK` on tab/bag/sort/expansion; `Event.ENTER_FRAME` (one-shot, console init); `SlotDragDropHelper` drop callback.

**Platform guards:** `IsConsole()`, `IsNX()`, `_locale` (used to pick `ConsoleLoc` frame for non-EN, non-ZH locales).

---

## Other game-specific classes

- `CurrencyInventoryView` — Sub-panel for the Currency and Discovery tabs; embeds `symbol145`. Manages a `SpliceableTileView` (`currencyList`) populated with `CurrencyRow` and `CurrencyHeading` rows; three category filter buttons (Crafting / Specialized / Gem); handles drag-start via `SlotDragDropHelper`, `BeginTransfer` on right-click, and tooltip show/hide via `SLOT.POINTER_ENTER` / `UIComponent.OnHideTooltip`. Calls `changeCurrency` when a category filter is clicked. Category index 0/1/2 is Crafting/Specialized/Gem for both `changeCurrency` and the engine's `setCurrencyCategory`. `setCurrencySlot(slotData, id, name, icon, quantity, rarity, moduleText, textColor)` keys each row by `id` (stored as the row's `name`; it is the string `BeginTransfer` and the drag carry, where a grid square drags by its render target); a quantity of 0 removes the row and any heading left empty, and in Discovery mode `moduleText` (the module's level text) replaces the quantity. `currenciesPopulated` is the engine's end-of-list signal.
- `CurrencyRow` — Embed `symbol110`. A single currency list item: `slot` (`Slot`), `nameText`, `quantityText`, `backgroundUnlocked`. Frames: `unselected` (frame 1) / `selected` (frame 13).
- `CurrencyHeading` — Embed `symbol114`. Lightweight heading separator row with a `titleText` field; inserted between currency category groups. Also used as an end-of-list sentinel (`"END_OF_LIST"`).
- `ModuleRow` — Embed `symbol107`. Layout-identical to `CurrencyRow` but used in Discovery mode (shows module items, non-draggable). Same fields: `slot`, `nameText`, `quantityText`.
- `InventoryRow` — Embed `symbol115`. Bare `KiwiComponent` asset holding a row of six `Slot`s (`slot_0`–`slot_5`); it is the row `_kiwi.Controls.BagContainer` builds from, and the stock Inventory grid does not use it.
- `UnlockChest` — Embed `symbol184` (extends `UIComponent`). Panel shown by `showUnlockPage(priceCredits, walletCredits)` in place of the slots when the selected personal-chest page has not been bought; pages are bought with Credits. Exposes `walletCredits` and `priceCredits` get/set accessors. Buttons: `creditsBuyBtn` (calls `OnBuy("TWC")`), `addCreditsBtn` (calls `OnAddCredits`). Has PC (frame 1) and Console (frame 11) layouts for the account-balance sub-clip.
- `currencyResourceList` — Embed `symbol126`. A `SpliceableTileView` with a pre-configured vertical scrollbar; used as the scroll container inside `CurrencyInventoryView`.

**Asset wrappers (not detailed):** 24 `rarity_frame_*_png` bitmap classes, 13 `ScrollArrow*/ScrollThumb*/ScrollTrack*` skin classes, 3 `btn_XBOne_*_png` classes, `btn_console_*` (6 button symbol classes), `btnGreenIcon_small`, `btnSort`, `btn_bag_*` (3× PC + 3× console + 3× placeholder), `btn_trash`, `currency_credits`, `currency_points`, `focusRectSkin`, `dummy`.

## Inventory_fla timeline symbols (22 classes)

All in package `Inventory_fla`, all embed symbols from `_assets/assets.swf`:

- `ButtonLegend_11` — Console button-legend overlay clip; contains `buttonSelect`, `buttonMove`, `buttonNavigate`, `trigger`, `buttonLegendToggle`, LB/RB button clips. Frame labels: `Console` (1), `Move` (11), `ConsoleLoc` (21), `NX_handheld` (31), `NX_handheld_move` (43).
- `BalancePanel_52` — Account-balance display with `credits` and `addCreditsBtn`; two frames PC/Console.
- `expansion_32` — The "buy more slots" button row; contains `price` TextField and `saleTag`. Three frames: up/over/down.
- `slotFrame_20` — Per-slot rarity frame overlay; three frames.
- `tabAdventure_57`, `tabBuild_56`, `tabButton_58`, `tabCrafting_60`, `tabDiscovery_62`, `tabGeode_61` — Tab button symbols; frame states `active`/`inactive`.
- `qualityPips_31`, `equipped_22`, `btn_slot_basic_67`, `btn_slot_gem_69`, `btn_slot_specialized_68`, `btn_expansion_plus_33`, `expansionlocked_35`, `bannerTop_2`, `bannerBottom_5`, `btn_console_adventure_9`, `btn_console_build_10`, `triggerLegend_15` — Remaining timeline symbols (labels, decorative banners, slot-type buttons, console shortcut icons).

## Notable logic

- **Bag pagination:** `slotInfo[]` stores data for all bags flat. `showBag(n)` rehydrates the 55 visible `Slot` instances from the cached data, then calls `SetShownSlots` so the game backend knows which range is visible. `SetShownSlots(start, end)` is sent only from `initDisplay` and a bag change; the engine answers with `clearSlots` followed by `setSlot` calls addressed by absolute index within `[start, end)` of the current tab's inventory, and it honours any range, not just one bag. `clearSlots` carries no argument: `onClear` empties the currency list in Currency/Discovery mode and otherwise clears slot indices `0`–`slotsPerBag-1`.
- **Expansion placement:** `checkExpansionPlacement()` positions the buy-row widget directly below the last unlocked slot and shows row-lock overlays (`lockedSlot1`-`lockedSlot10`) for rows past `numSlots`.
- **Drop routing:** `onDrop` callback from `SlotDragDropHelper` hit-tests bag buttons and trash before iterating slots; fires `DROP_ONTO_BAG` (last argument: the engine's bag index 0–2), `DROP_ONTO_TRASH`, or `INVENTORY.DROP_ONTO_WINDOW` (last argument: the hit slot's `data` slot index, or -1) with entity-id and component info.
- **Locale-aware console layout:** `setMoveMode` checks available frame labels (`Console`, `ConsoleLoc`) and the `_locale` global to pick the right button-legend frame; NX handheld mode has additional reduced-height variants.
- **New-item glows:** `showNewItemNotification` sets a count badge on the tab clip and enables `newSlotItemGlow` on the individual slot (`slot_<slotId>`, except for the currency tab).
