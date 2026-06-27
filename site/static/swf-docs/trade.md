# trade.swf
> The player-to-player trade (and deconstructor) window in Trove. Shows a pending-player list while waiting for a trade partner, then switches to a two-panel item exchange view where both players can offer items, lock in, and accept the trade. Also reused for the Deconstructor collect flow.

**Document/main class:** `Trade` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 6 (plus 9 Trade_fla timeline symbols and several skin/asset wrappers)

---

## Main class: `Trade`

`Trade` is the root document class. The constructor registers frame scripts for frames 0 and 10, immediately calls `showPendingPlayers()`, and in the Iggy path registers four `ExternalInterface` callbacks and hides itself on console until `onTargetFrame()` is reached. `configUI()` hides `tradePanel.buttonLegend`. `onStageResized` scales `backBanner.height` proportionally to the stage height.

### Public methods

- `showPendingPlayers() : void` — sets `_mode = MODE_PLAYER_LIST`, shows `playerListPanel`, hides `tradePanel`, sets button legend to "Off".
- `initTradeSession(p1Name, p2Name, p1Max, p2Max, isDeconstruct) : void` — caches all parameters, sets `_mode = MODE_TRADE`, shows `tradePanel`, hides `playerListPanel`, sets button legend to "On", calls `tradePanel.configCategories(...)`. When `isDeconstruct` is `true`, sets `acceptButton.label` to `"$Deconstructor_Collect"` and on console sets `southButton` frame to "Collect".
- `setTitlebar(text:String) : void` — stores title and assigns it to `header.title`.
- `GainedFocus() : *` — makes `tradePanel.buttonLegend` visible; on NX console calls `PositioningButtons` to reposition the legend and adjusts `southButton.y = 75`.
- `LostFocus() : *` — hides `tradePanel.buttonLegend`.

### Key fields

- `playerListPanel : MovieClip` — panel shown while searching for a trade partner.
- `tradePanel : TradePanel` — the actual two-column item exchange panel.
- `header : WindowHeader` — title bar; updated via `setTitlebar`.
- `backBanner : MovieClip` — background banner; height scaled proportionally on resize.
- `closeButton : MovieClip` — hidden on console; triggers `TRADE.CLOSE_REQUEST` on click.
- `_mode : int` — `MODE_PLAYER_LIST (1)` or `MODE_TRADE (2)`.
- `SLOTS_PER_ROW : int = 6` — sent to game via `TRADE.CONFIGURED` on console init.
- `_player1Name / _player2Name : String`, `_player1Max / _player2Max : Number`, `_isDeconstruct : Boolean` — cached parameters for deferred console init.

### Frame scripts / timeline

- **Frame 0 (`frame1`)** — `stop()`. PC layout.
- **Frame 10 (`frame11`)** — `stop()`. Console layout: calls `tradePanel.gotoAndPlay("Console")`.

### Runtime dependencies & integration

- **Iggy callbacks registered:** `showPendingPlayers`, `initTradeSession`, `setTitlebar`, `GainedFocus`, `LostFocus`.
- **ExternalInterface calls out:** `TRADE.CONFIGURED` (with `SLOTS_PER_ROW`), `TRADE.CLOSE_REQUEST`.
- **translate keys:** `"$Deconstructor_Collect"` (used when `isDeconstruct` is true).
- `IsConsole()`, `IsNX()`, `PositioningButtons()` called in focus/init paths.
- `setupTranslation()` not called directly in `Trade` — delegated to `TradePanel`.

---

## Class: `TradePanel`

Sub-component managing the two-sided item display and accept flow. Extends `_kiwi.Core.UIComponent`.

### Responsibilities

Created as a named instance (`tradePanel`) on the `Trade` stage. In its constructor it creates two `BagContainer` instances (`disableCollapse = true`) for categories 0 and 1 and adds them to `offerContainer1`/`offerContainer2`. `configUI()` registers a `SlotDragDropHelper` drop callback and all per-panel Iggy callbacks.

### Public methods

- `configCategories(p1Name, p2Name, p1Max, p2Max) : void` — sets category label and capacity for each `BagContainer`.
- `addOfferedItem(category, icon, slotId, qty, color, showQty, quality) : void` — calls `offerCategories[cat].addObject(...)`. Enables `acceptButton` if local accept state is 0 and resets the accept-pending background.
- `removeOfferedItem(category, slotId) : void` — calls `offerCategories[cat].removeBySlotId(slotId)`. Disables `acceptButton` if both categories are empty.
- `updateOfferedItem(category, slotId, qty, color) : void` — delegates to `BagContainer.updateBySlotId`.
- `slotIdFromIndex(index:int) : int` — negative indices address category 1 (high bit masked off); non-negative address category 0.

### Private methods

- `cancelAccept()` — resets accept backgrounds, sets label to `"$TradingPost_LockIn"`, re-enables button.
- `indicateAccept(who, state)` — tracks `localPlayerAcceptState` and `otherPlayerAcceptState`. When local is one ahead of remote, disables button with `"$TradingPost_Waiting"` label. Otherwise enables with `"$TradingPost_LockIn"` (state 0) or `"$TradingPost_Accept"` (state 1+), and advances console `southButton` frame.
- `onDrop(x, y, icon, qty, slotId)` — calls `TRADE.DROP_ONTO_WINDOW` and plays `Play_ui_window_drop_interactive` sound.
- `onAccept(e:MouseEvent)` — calls `TRADE.ACCEPT`.
- `showControllerTooltip(index) / hideControllerTooltip(index)` — delegates to the appropriate `BagContainer` tooltip method; negative index targets category 1 using high-bit masking.
- `activateSlot(index) / deactivateSlot(index)` — same sign-based routing to `BagContainer.activateSlot/deactivateSlot`.
- `acceptButtonClick()` — console path: dispatches `MouseEvent.CLICK` on `acceptButton` programmatically.

### Key fields

- `offerCategories : Object (Array)` — two `BagContainer` instances indexed 0 (local) and 1 (remote).
- `acceptButton : LabelButton` — lock-in/accept/collect/waiting button.
- `offerContainer1 / offerContainer2 : MovieClip` — visual containers for each BagContainer.
- `acceptBg1 / acceptBg2 : MovieClip` — semi-transparent "accepted" highlight; shown when `indicateAccept` fires.
- `buttonLegend : MovieClip` — console button overlay.
- `localPlayerAcceptState / otherPlayerAcceptState : int` — two-step acceptance state machine (0 = none, 1 = locked, 2 = accepted).

### Iggy callbacks registered in TradePanel

`configCategories`, `addOfferedItem`, `removeOfferedItem`, `updateOfferedItem`, `cancelAccept`, `indicateAccept`, `showControllerTooltip`, `hideControllerTooltip`, `getInventorySlotId`, `activateSlot`, `deactivateSlot`, `slotIdFromIndex`, and (console only) `acceptButtonClick`.

### ExternalInterface calls out from TradePanel

`TRADE.DROP_ONTO_WINDOW`, `POST_SOUND_EVENT("Play_ui_window_drop_interactive")`, `TRADE.ACCEPT`.

### translate keys

`"$TradingPost_LockIn"`, `"$TradingPost_Waiting"`, `"$TradingPost_Accept"`. `setupTranslation()` called in `configUI`.

---

## Class: `TradePlayerList`

Embed symbol167; extends `fl.controls.List`. Overrides `drawList()` to fix CellRenderer text field width to the full component width (rather than the default available-width calculation). Used as the list control in `playerListPanel`.

---

## Class: `InventoryRow`

Embed symbol61; extends `com.kiwi.Core.KiwiComponent`. Trivial asset-wrapper — no custom logic. Used as the item-row display in the trade grid or inventory panel.

---

## Trade_fla timeline symbols

- `Trade_fla/bannerTop_2` — Embed symbol179; top decorative banner; 15-frame animation, stops at frame 15.
- `Trade_fla/bannerBottom_5` — Embed symbol182; bottom decorative banner; same 15-frame structure.
- `Trade_fla/buttonLegend_41` — Embed symbol113; console button legend strip; exposes `tradeButtonLegend`, `switchButtonLegend`, `closeButtonLegend`, `southButton`; two stops (frame 0 PC / frame 10 console).
- `Trade_fla/categoryCapacity_50` — Embed symbol72; capacity indicator with `countTextField` and `maxCapacityTextField`; two stops.
- `Trade_fla/slotFrame_53` — Embed symbol35; four-state slot border clip (frames 1–4).
- `Trade_fla/equipped_56` — Embed symbol42; equipped indicator; frame 1 stops, frame 61 loops back to "Pulse".
- `Trade_fla/equiped_54` — Embed symbol37; equipped tween clip with `bmpTween` child; same loop-back pattern.
- `Trade_fla/qualityPips_62` — Embed symbol59; quality pip display; stops on frame 1.
- `Trade_fla/button_A_42` — Embed symbol106; console A-button prompt; stops on frame 1.

### Asset / skin wrappers (not individually documented)

Scroll bar skins (9): `ScrollArrowDown_*`, `ScrollArrowUp_*`, `ScrollThumb_*`, `ScrollTrack_skin`, `ScrollBar_thumbIcon`.
List cell skins (8): `CellRenderer_upSkin`, `CellRenderer_downSkin`, `CellRenderer_overSkin`, `CellRenderer_disabledSkin`, `CellRenderer_selectedUpSkin`, `CellRenderer_selectedDownSkin`, `CellRenderer_selectedOverSkin`, `CellRenderer_selectedDisabledSkin`.
Other skins: `List_skin`, `focusRectSkin`, `CloseIcon`, `CloseIconPressed`.
Rarity frame wrappers (16): `rarity_frame_common_png` through `rarity_frame_stellar` and `_over` variants (common, uncommon, rare, epic, legendary, relic, shadow, radiant1, resplendent, stellar, mystic, crystal, normal, mystic_over, crystal_over, normal_over).

---

## Notable logic

- **Slot index sign encoding:** Negative slot indices in `showControllerTooltip`, `hideControllerTooltip`, `activateSlot`, `deactivateSlot`, and `slotIdFromIndex` all decode the category-1 index by masking off the sign bit (`param & ~(1 << 31)`). This allows a single integer parameter to encode both the category (sign) and the slot index.
- **Two-step accept state machine:** `indicateAccept` implements a two-stage commit — lock-in first, then accept — by comparing `localPlayerAcceptState` and `otherPlayerAcceptState`. The button label transitions LockIn → Waiting → Accept to reflect the handshake.
- **Deconstructor reuse:** `initTradeSession` with `isDeconstruct = true` changes only the accept button label and console button frame; the full trade UI is otherwise unchanged.
- **NX button positioning:** `GainedFocus` calls the global `PositioningButtons()` (Iggy helper) to align the legend with the panel boundary, then hard-codes `southButton.y = 75` for NX layout.
- **Console deferred init:** On console, `Trade` is constructed invisible and defers all panel configuration until `onEnterFrame` confirms `onTargetFrame()`; at that point it calls `TRADE.CONFIGURED(SLOTS_PER_ROW)` before making itself visible.
