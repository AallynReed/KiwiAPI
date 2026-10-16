# marketplace.swf

> The Trove player-driven auction marketplace, allowing players to search and purchase listings from other players (Buy tab) and create or manage their own sell listings (Sell tab). Uses Flux as the primary trading currency. Appears when the player opens the Marketplace from the in-game UI.

**Document/main class:** `Marketplace` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 7 game-logic + 8 `MarketPlace_fla` timeline symbols + ~19 button/widget subclasses + ~24 asset wrappers

---

## Main class: `Marketplace`

`Marketplace` is the document class and central controller. It manages two operating modes — `MODE_BUY` (0) and `MODE_SELL` (1) — surfaced as a `tabBuy`/`tabSell` `TabHeader` pair. The main content area is `tabContentMC` (a `tabContentMC_10` symbol) which jumps to frames 1/2 (PC Buy/Sell) or 3/4 (console Buy/Sell). The buy-side grid is `buyListingsView:MarketView`; the sell-side grid is `sellListingsView:MarketView`.

The constructor wires mouse listeners on both tabs and both `MarketView` instances for `MarketEvent.BUTTON_CLICK`, sets the initial instructions text, and links footer pagination buttons. `configUI()` registers `DRAGHOST.DROP` through `SlotDragDropHelper`, 38 ExternalInterface callbacks (plus 26 console-only ones) and calls `setupTranslation()`.

### Public methods / properties

- `get lastPageIndex() : int` — computes the zero-indexed page offset of the last page based on `numListings` (buy) or `maxPlayerListings` (sell) and `ITEMS_PER_PAGE` (12).

### Private / internal methods (selected)

- `configUI() : void` — registers all ExternalInterface callbacks; seeds dummy data in standalone mode. Also auto-sizes several sell-tab text fields using `KiwiTextUtil.resizeFont`.
- `setBuyTab(e:MouseEvent = null) : void` — saves current sell-form state (texture, quantity, price, comparison text) to local fields, navigates `tabContentMC` to frame 1 or 3, shows `buyListingsView`, hides `sellListingsView`, sets `newDrawMode = MODE_BUY`, schedules `drawUI` on next `RENDER`.
- `setSellTab(e:MouseEvent = null) : void` — saves filters, navigates `tabContentMC` to frame 2 or 4, shows `sellListingsView`, calls `ExternalInterface.call("GetSellListingsRange", 0, maxPlayerListings-1)`, schedules `drawUI` on next `RENDER`.
- `setPlayerListingsSize(n:int) : void` — sets `maxPlayerListings` (the number of sell slots `GetSellListingsRange` asks for) and switches to the Buy tab.
- `drawUI(e:Event) : void` — one-shot `RENDER` handler; on the first pass after a tab switch it reports `OnModeChanged(mode)`. On `MODE_BUY`: populates `itemFilterMC:KiwiComboBox` from `itemData`, wires search button and name filter. On `MODE_SELL`: wires `sellPrice`, `sellPricePerUnit`, `btnCreateListing`, item image click, quick-search button, and restores comparison price texts; calls `buttonLegendGotoAndStop("BuySell")`.
- `addListing(id:String, name, quantity:int, currencyIndex:int, price:int, texture, info, categories) : void` — creates a `MarketplaceItem(STATE_CAN_BUY, ...)` and adds it to `buyListingsView`. Item names arrive as translation keys and are resolved by the text field.
- `addPlayerListing(slot:int, name, quantity, currencyIndex, price, texture, locked, claimable, expired, resell, categories, secondsLeft:Number) : void` — creates a `MarketplaceItem` for one sell slot and adds it to `sellListingsView`. The state is resolved in this order, each line overriding the last: `quantity > 0` → `STATE_CAN_CANCEL` else `STATE_EMPTY`; `expired` → `STATE_CAN_CLAIM_EXPIRED`; `claimable` → `STATE_CAN_CLAIM`; `locked` → `STATE_LOCKED`; `claimable && resell` → `STATE_CAN_CLAIM_RESELL`; `expired && resell` → `STATE_CAN_CLAIM_EXPIRED_RESELL`. Every slot the player has not bought arrives as its own locked listing (owned slots are numbered first); only the first locked tile gets the upsell layout. Reaching `maxPlayerListings` tiles resets the page.
- `playerListingPurchased(slot, name, quantity, currencyIndex, price, texture, locked, claimable, expired, resell, categories) : void` — rebuilds one sell slot in place (same state rules) while the sell tab is showing.
- `purchaseListingSucceeded(id) : void` → calls `buyListingsView.updateItemAsSold(id)` (sold badge).
- `purchaseListingFailed(id, flag:Boolean) : void` → calls `buyListingsView.updateItemAsSold(id, flag)`: sold badge if `true`, re-enabled button if `false`.
- `purchaseListingCancelled(id) : void` → `buyListingsView.updateItemAsSold(id, false)`.
- `cancelListingSucceeded(slot)`, `claimExpiredListingSucceeded(slot)` — empty the slot. `claimListingSucceeded(slot, remaining)` — updates the slot's price to `remaining` when above 0, otherwise empties it. `createListingSucceeded(slot, name, quantity, currencyIndex, price, texture, categories, secondsLeft)` — puts the new listing in the slot and clears the sell form. The `...Failed(slot)` callbacks re-enable the slot's button; `createListingFailed()` re-enables Create Listing.
- `searchSucceeded(count:int) : void` — stores `count` as `numListings` (the total found), shows `$Marketplace_NoResults` ("Sorry, no results found.") when it is 0, and resets to the first page, which requests `GetListingsRange`.
- `searchFailed() : void` — clears `listingsMessageText` and re-enables the search button.
- `addCurrency(name, thumbnail, index) : void` — creates a `CurrencyData` at `Marketplace.currencies[index]` and adds it to the currency filter; `addDeprecatedCurrency` registers the data without the filter entry. Currency names are translation keys, translated at each use.
- `setCurrencyAmount(index, amount) : void` — updates `Marketplace.currencies[index].quantity`, refreshes the footer balance, and calls `resetActionButton` on every buy tile on the page, which re-checks each price against its currency balance.
- `getListingFee(fee:int) : void` — populates `tabContentMC.taxTextField` with `$Marketplace_ListingFee` and the fee in `currencies[0]` (Flux).
- `setMarketplaceComparisonText(allInfo, low, mean, high) : void` — updates the four sell-tab comparison text fields. The engine sends display text; the figures are per-unit averages and can carry a decimal (e.g. 39.6 Flux for Glim).
- `setTaxInfo(minFee, weighedMin, weighedMax, maxFee, isEvent:Boolean, claimTax:String) : void` — stores the fee range and claim tax for the listing-rates tooltip, shows `eventSticker` when `isEvent`, and on the sell tab re-requests the fee for the current price.
- `setListedItem(slotId:int, quantity:int, texture:String)` / `clearListedItem(...)` — stage or clear the item in the sell form; `slotId` is the inventory slot that `OnCreateListing` and `QuickSearch` later name.
- `setSellSlotPrice(twcPrice, twpPrice, canAfford)` — prices for the locked-slot upsell. `setSellSlotUnlocked(slot, canAfford)` — empties the bought slot and puts the upsell on the next one.
- `setItemCategory(index)`, `setCollectedState(int)`, `setPriceSorting(int)`, `setCurrency(index)`, `setSellCurrency(index)`, `setSearchString(text)` — the engine restoring the filters it remembers between openings; `setSearchString` is also how a quick search fills the name box. Sort by unit price is remembered for the session only.
- `onButtonClicked(e:MarketEvent) : void` — the central action dispatcher; branches on `e.state`:
  - `STATE_CAN_BUY` → `ExternalInterface.call("OnPurchaseListing", listingId, currencyIndex, from, to)`, where `from`/`to` is the page range shown (`listingIndex`, `listingIndex + resultsOnPage`).
  - `STATE_CAN_CANCEL` → `ExternalInterface.call("OnCancelListing", slot)`.
  - `STATE_CAN_CLAIM` / `STATE_CAN_CLAIM_RESELL` → `ExternalInterface.call("OnClaimListing", slot, resell)`.
  - `STATE_CAN_CLAIM_EXPIRED` / `_RESELL` → `ExternalInterface.call("OnClaimExpiredListing", slot, resell)`.
  - `STATE_LOCKED` → `ExternalInterface.call("OnPurchaseSellSlot", currencyIndex)` — 0 from `btnCredits`, 1 from `btnCubits`.
- `onListClicked` → with nothing staged calls `OnRequestInventoryOpen`; otherwise validates quantity and price and calls `ExternalInterface.call("OnCreateListing", emptySellSlot, inventorySlotId, quantity, 0, price, priceWithDelimiters)`.
- `onSearchClicked` → shows `$Marketplace_Searching`, disables the search button, sends the filters with `saveFilters()` and then calls `ExternalInterface.call("OnSearch")` with no arguments: the search runs on the filter state the engine just received. Triggered by the button or by Enter in the name box, never per keystroke.
- `onItemDropped(...)` → `DRAGHOST.DROP` callback from `SlotDragDropHelper`; forwards its third to fifth arguments to `ExternalInterface.call("OnDropIntoWindow", ...)`. The item is staged when `setListedItem` arrives.
- `onListImageClicked` → `OnRequestInventoryOpen()` when nothing is staged. A right-click on the listing image calls `OnRemoveMarketplaceSaleListing()` to un-stage the item.
- `quickSearchClick` / `quickSearch` callback → `ExternalInterface.call("QuickSearch", slotId)` with the staged inventory slot.
- `updateTaxDisplay(price)` → `ExternalInterface.call("requestListingPrice", price)` with the total price whenever it changes; the fee is shown by `getListingFee`.
- `saveFilters()` → on the buy tab `ExternalInterface.call("OnSaveFilters", categoryIndex, hideCollected, name, 0, sortByPrice)`; on the sell tab `OnSaveSellFilters(0)`.
- `onListingInfoRollOver/Out` → `TOOLTIP.SHOW` with `$Marketplace_ListingRatesTitle` and `$Marketplace_ListingRatesDescription` (fee range, currency, claim tax as the marketplace's cut) / `TOOLTIP.HIDE`.
- `buttonLegendGotoAndStop(frame:String) : void` — same deferred-frame pattern as `StoreBase`; drives `buttonLegend:MovieClip` to the named frame. Frames: `BuySell`, `BuyTilePurchasable`, `SellTilePurchasable`, `SellTileCancel`, `SellTileClaim`, `Resell`, `TileNonPurchasable`.
- Console-only focus methods (`setFocusBuyItemType`, `setFocusBuySearch`, etc.) — move platform focus to the appropriate form field or control via `FocusManager`.
- `frame1() : *` / `frame11() : *` — timeline stop scripts.

### Key fields

| Field | Type | Role |
|---|---|---|
| `currencies` | `static Vector.<CurrencyData>` | Global currency registry, indexed by integer ID. Index 0 = Flux. |
| `buyListingsView` | `MarketView` | Scrollable grid of `MarketplaceItem` tiles for buy listings. |
| `sellListingsView` | `MarketView` | Scrollable grid of player's own sell listings. |
| `tabContentMC` | `MovieClip` (`tabContentMC_10`) | Main form area; frames 1/3 = buy (PC/console), 2/4 = sell (PC/console). |
| `footerContentMC` | `MovieClip` (`footerContentMC_2`) | Footer with page navigation buttons and currency balance display. |
| `buttonLegend` | `MovieClip` (`ButtonLegend_68`) | Console button-legend strip; 7 named frames. |
| `itemData` | `DataProvider` | Category filter combo box data (item types). |
| `currencyData` | `DataProvider` | Currency filter combo box data. |
| `collectData` | `DataProvider` | "Collected/Uncollected" filter state data. |
| `maxPlayerListings` | `int` | Max sell slots, default 12; set by `setPlayerListingsSize`. |
| `numListings` | `int` | Total results of the last search (`searchSucceeded`), used for pagination. |
| `slotId` | `int` | Inventory slot of the item staged for sale (`setListedItem`); -1 when nothing is staged. |
| `listingTexture` | `String` | Texture name of the staged item, saved when leaving the sell tab. |
| `listingPrice`, `listingQuantity`, `listingCurrency` | `int` | Sell-form state preserved across tab switches. |
| `listingComparisonInfo/Low/Mean/High` | `String` | Comparison price strings, preserved across tab switches. |
| `newDrawMode`, `newDrawModeSetup` | `int`, `Boolean` | Deferred mode-switch state for `drawUI`. |
| `showingUpsell` | `Boolean` | Whether the upsell (purchase-credits) tile is showing. |
| `m_listingTooltipShowing` | `Boolean` | Whether the sell-tab listing info tooltip is visible. |
| `pendingSelection` | `int` | Console: item index to select after the view populates. |

### Frame scripts / timeline

Stage 1033×676, 24 fps, 20 frames; root labels `PC` (frame 1) and `Console` (frame 11). Frames 1 and 11 both call `stop()`; frame 11 is the console layout and also sends `tabContentMC`, `tabBuy`, `tabSell` and `footerContentMC` to their `"Console"` label.

### Runtime dependencies & integration

- **ExternalInterface callbacks registered (all modes):**
  - Setup: `setPlayerListingsSize`, `setSellSlotPrice`, `setSellSlotUnlocked`, `addItemCategory`, `addCollectedState`, `addCurrency`, `addDeprecatedCurrency`, `setCurrencyAmount`, `setItemCategory`, `setCollectedState`, `setPriceSorting`, `setCurrency`, `setSellCurrency`, `setSearchString`, `setListedItem`, `clearListedItem`
  - Data: `addListing`, `addPlayerListing`, `searchSucceeded`, `searchFailed`
  - Transaction results: `purchaseListingSucceeded/Failed/Cancelled`, `cancelListingSucceeded/Failed`, `claimListingSucceeded/Failed`, `claimExpiredListingSucceeded/Failed`, `createListingSucceeded/Failed`
  - UI: `setBuyTab`, `setSellTab`, `setMarketplaceComparisonText`, `setTaxInfo`, `getListingFee`, `playerListingPurchased`, `quickSearch`
- **Console-only callbacks:** `setFocusBuyItemType`, `setFocusBuyHideCollected`, `setFocusBuySortByPrice`, `setFocusBuyItemName`, `setFocusBuySearch`, `setFocusSellInventory`, `setFocusSellPriceTotal`, `setFocusSellPriceUnit`, `setFocusSellCreate`, `setFocusSellListingRates`, `setFocusQuickSearch`, `onItemFilterClicked`, `onToggleBuyHideCollection`, `onToggleBuySortByPrice`, `onSearchClicked`, `onListImageClicked`, `onListClicked`, `onItemSelected`, `onActionButtonClicked`, `onActionButton2Clicked`, `onInfoButtonToggled`, `onPageFirstClicked`, `onPageLeftClicked`, `onPageRightClicked`, `onPageLastClicked`, `onListingRatesClicked`
- **ExternalInterface outbound calls:**
  - `"OnSearch"()` — no arguments; preceded by `OnSaveFilters`, whose values the search uses.
  - `"GetListingsRange"(first, last)`, `"GetSellListingsRange"(0, maxPlayerListings - 1)` — inclusive index ranges (`resultsOnPage` is one less than the number of tiles).
  - `"OnPurchaseListing"(listingId, currencyIndex, first, last)` — `first`/`last` are the page range on screen, not the item.
  - `"OnCancelListing"(slot)`
  - `"OnClaimListing"(slot, resell)`, `"OnClaimExpiredListing"(slot, resell)`
  - `"OnPurchaseSellSlot"(currencyIndex)` — locked slot upsell; 0 = credits, 1 = cubits.
  - `"OnCreateListing"(emptySellSlot, inventorySlotId, quantity, 0, price, priceWithDelimiters)`
  - `"OnDropIntoWindow"(...)` — item drag-drop, forwarding three of the `DRAGHOST.DROP` arguments.
  - `"OnRemoveMarketplaceSaleListing"()` — right-click on the staged item.
  - `"OnRequestInventoryOpen"()` — listing image or Create Listing clicked with nothing staged.
  - `"QuickSearch"(inventorySlotId)`
  - `"requestListingPrice"(totalPrice)`
  - `"OnSaveFilters"(categoryIndex, hideCollected, name, 0, sortByPrice)` / `"OnSaveSellFilters"(0)`
  - `"OnModeChanged"(mode)`
  - `"TOOLTIP.SHOW"(x, y, title, body)` / `"TOOLTIP.HIDE"()`
- **Translate keys used (selected):** `$Marketplace_Instructions`, `$Marketplace_SearchButton`, `$Marketplace_CreateListingsButton`, `$Marketplace_ClaimButton`, `$Marketplace_CancelListingButton`, `$Marketplace_NoResults`, `$Marketplace_Searching`, `$Marketplace_ListingFee`, `$Marketplace_ListingRatesTitle`, `$Marketplace_ListingRatesDescription`, `$Marketplace_ComparisonDescriptionEmpty`, `$Marketplace_UnitPricePrefix`, `$Marketplace_UnitPriceSuffix`, `$Marketplace_UnitPriceLessThanOne`, `$Marketplace_ExpiresIn`, `$DigitGroupDelimiter`.
- **`SlotDragDropHelper`** — registers `DRAGHOST.DROP`; items can be dragged from inventory into the sell listing image slot, triggering `onItemDropped`.
- **Throttling:** the game throttles bursts of marketplace requests. A search sent while throttled comes back with no results, and extra claims fired in one burst come back failed.

---

## Class: `MarketEvent`

Extends `flash.events.Event`. Custom bubbling event (`BUTTON_CLICK = "Click"`) dispatched by `MarketplaceItem` when any action button is clicked. Carries `slotId:String`, `state:int`, `currencyIndex:int`, and `resell:Boolean`. `Marketplace.onButtonClicked` receives this and calls the appropriate ExternalInterface function.

---

## Class: `MarketView`

Extends `_kiwi.Controls.ScrollableTileView`. `[Embed(source="/_assets/assets.swf", symbol="symbol127")]`. A fixed-layout (no vertical scrollbar, no horizontal scroll-wheel) tile grid for marketplace listings. Spacing: `HORIZ_MARGIN=22`, `VERT_MARGIN=5`, `WINDOW_MARGIN=0`. Rows are NOT centred.

**Methods:**
- `get count() : int` — returns `itemList.length`.
- `getEmptyListingId() : int` — finds the first `STATE_EMPTY` slot index (for sell-slot management); returns `-1` if none.
- `updateItem(index, newItem) : void` — replaces the item at `index` in `itemList` in-place, preserving x/y position and `selected` state; triggers a data-invalidation pass.
- `updateItemAsSold(id, sold = true) : void` — finds the item by `itemId`; if `sold=true`, calls `item.updateAsSold()` (shows sold badge, hides price/button); if `sold=false`, calls `item.resetActionButton()` (re-enables the button after a failed transaction).
- `updateItemPrice(index, price) : void` — delegates to `MarketplaceItem.updatePrice`.
- `resetActionButton(index) : void` — re-enables the action button on the item at `index`.
- `selectItem(index) : MarketplaceItem` — sets `selected=true` on item at `index`, `false` on all others; returns the selected item.
- `onActionButtonClicked(index) : void` — console-driven; programmatically dispatches a `CLICK` on the item's `actionButton` (or `btnCredits` for locked items).
- `onActionButton2Clicked(index) : void` — console-driven; dispatches `CLICK` on `btnCubits` (locked) or `resellButton` (claim+resell states).
- `onInfoButtonToggled(index, visible) : void` — console-driven; calls `item.onInfoRollOver` or `onInfoRollOut`.

---

## Class: `MarketplaceItem`

Extends `_kiwi.Core.UIComponent`. `[Embed(source="/_assets/assets.swf", symbol="symbol68")]`. Represents a single marketplace listing tile.

**State constants:**

| Constant | Value | Meaning |
|---|---|---|
| `STATE_EMPTY` | 0 | Empty sell slot placeholder |
| `STATE_CAN_BUY` | 1 | A listing available for purchase |
| `STATE_CAN_CANCEL` | 2 | Player's active listing (can cancel) |
| `STATE_CAN_CLAIM` | 3 | Player's sold listing (funds ready to claim) |
| `STATE_CAN_CLAIM_EXPIRED` | 4 | Player's expired listing (claiming returns the item) |
| `STATE_CAN_CLAIM_RESELL` | 5 | Sold, can also relist |
| `STATE_SOLD` | 5 | **Duplicate of STATE_CAN_CLAIM_RESELL** — constant collision; referenced nowhere |
| `STATE_LOCKED` | 6 | Locked slot (requires unlock via credits/cubits) |
| `STATE_CAN_CLAIM_EXPIRED_RESELL` | 7 | Expired listing, can claim the item back or relist |

**Constructor:** Takes up to 11 parameters (state, itemId, name, texture, quantity, currencyIndex, price, info, showUnitCost, categories, timerSeconds). Starts visible=false, becomes visible after 2 `ENTER_FRAME` ticks (via `onRendered`) to allow layout to settle. Routes to timeline frame 1 (buy / active listing), 2 (claimable; `ownedMC` frame 2 sold or 3 expired), 3 (empty) or 5 (locked, `ownedMC` frame 4); `showUpsell` uses frame 6. `btnInfo` is shown only for buy and active listings that carry info text or a timer; hovering it shows `$Marketplace_ExpiresIn` with the countdown, or the info text.

**Key fields:** `itemId:String`, `state:int`, `priceValue:Number`, `currencyIndex:int`, `actionButton:LabelButton`, `resellButton:BaseButton`, `listingIcon:ArtClip`, `currencyIcon:ArtClip`, `ownedMC:MovieClip`, `infoPopup:MovieClip`, `selectedMC:MovieClip`, `highlightFrame:MovieClip`, `pulseAnimTimer:Timer` (5 s interval, drives pulse animation on `infoPopup.timer`), `btnCredits:LabelButton`, `btnCubits:LabelButton` (for locked/upsell state).

**Key methods:**
- `set selected(bool) : *` — shows/hides `selectedMC` (PC) and `highlightFrame` (console).
- `onActionButtonClicked(e)` — dispatches `MarketEvent.BUTTON_CLICK` with `resell=false`.
- `onResellButtonClicked(e)` — dispatches `MarketEvent.BUTTON_CLICK` with `resell=true`.
- `resetActionButton()` — re-enables `actionButton` (for STATE_CAN_BUY: only if `priceValue <= currencies[currencyIndex].quantity`).
- `updateAsSold()` — hides `actionButton`, `itemPrice`, `currencyIcon`, `itemUnitCost`; shows `ownedMC` at frame 2 (sold badge).
- `updatePrice(price)` — updates `itemPrice.text`.
- `showUpsell(twcPrice, twpPrice, canAfford)` — switches to timeline frame 6 (upsell layout) and wires `btnCredits`/`btnCubits`; a press dispatches the event with currency index 0 (credits) or 1 (cubits). The credit and cubit cube art beside those buttons is an unnamed shape (character 67, filled with bitmaps 65 and 66) placed on that frame, so it appears in no script.
- `static formatNumber(n:Number) : String` — inserts `$DigitGroupDelimiter`-localised thousands separators.
- `onInfoRollOver/Out` — toggles `infoPopup.visible`; shows countdown timer or info text.

**Unit cost display logic:** If selling `quantity > 1` and `showUnitCost=true`, computes price-per-unit with smart decimal rounding (0 or 1 decimal place), using `$Marketplace_UnitPriceLessThanOne` if the unit cost is below 1.

---

## Class: `CurrencyData`

Simple value object. Fields: `name:String`, `quantity:int`, `thumbnail:String`. Populated by `Marketplace.addCurrency` and stored in the static `Marketplace.currencies` vector.

---

## Class: `image`

Extends `_kiwi.Controls.ArtClip`. Thin subclass used as the sell-tab listing image slot (`tabContentMC.listingImage`). No additional logic beyond the ArtClip base class.

---

## `MarketPlace_fla` timeline symbol classes (8 classes)

- `footerContentMC_2` — footer bar containing `btnPageFirst`, `btnPageLeft`, `btnPageRight`, `btnPageLast` (pagination), plus a currency balance display area.
- `tabContentMC_10` — the main content panel. Four frames (PC Buy / PC Sell / Console Buy / Console Sell). Contains all form fields: `itemFilterMC:KiwiComboBox`, `itemName:TextField`, `itemNameClear:BaseButton`, `btnSearchListings:LabelButton`, `sellPrice/sellPricePerUnit:TextField`, `sellQuantity:TextField`, `btnCreateListing:LabelButton`, `listingImage` (uses `image` ArtClip), `quickSearch:MovieClip`, `taxTextField`, `eventSticker:MovieClip`, `marketplaceComparisonAllInfo/Low/Mean/High:TextField`, `listingInfoBtn` (hover shows the listing rates through `TOOLTIP.SHOW`). Frame labels: `PC` on frame 1, `Console` on frame 3.
- `ButtonLegend_68` — console button-legend strip. 7 labelled frames: `BuySell`, `BuyTilePurchasable`, `SellTilePurchasable`, `SellTileCancel`, `SellTileClaim`, `Resell`, `TileNonPurchasable`. Each frame displays appropriate controller button icons and action labels.
- `marketListingImage_40` — item image slot with a highlight ring; two frames (PC / Console layout).
- `OwnedProduct_76` — badge MovieClip used as `ownedMC`; frame 1 `$Marketplace_EmptyListingSlot`, frame 2 `$Marketplace_SoldListing` ("Sold!"), frame 3 `$Marketplace_ExpiredListing` ("Expired!"), frame 4 the locked-slot art.
- `currencyIcon_79` — two-frame currency icon labelled `credits` and `points`, used by the `PurchasePanel` symbol. Listing tiles show their currency through an `ArtClip` with the currency's thumbnail instead.
- `QuickSearch_42` — quick-search button clip; a single `stop()` frame.
- `timer_pulse_anim_71` — pulsing animation clip used within `infoPopup.timer` to draw attention to expiring listings.

---

## Asset wrappers (~24 classes — not detailed individually)

Approximately 7 Xbox One controller button PNG bitmaps (`btn_XBOne_A`, `btn_XBOne_X`, `btn_XBOne_Y`, `btn_XBOne_LB`, `btn_XBOne_RB`, `btn_XBOne_LT`, `btn_XBOne_RT`), 1 empty `SelectedMC` MovieClip symbol, and ~16 UI component skin stubs (`CellRenderer_*Skin`, `ComboBox_*Skin`, `ScrollArrowDown/Up_*Skin`, `ScrollThumb_*Skin`, `ScrollTrack_skin`, `List_skin`, `TextInput_disabledSkin`, `TextInput_upSkin`, `focusRectSkin`).

---

## Notable logic

- **Currency-aware affordability gating:** `setCurrencyAmount` not only updates the static currency quantity but also immediately calls `resetActionButton` on every buy tile on the page, which re-enables or disables each buy button by comparing its price with its currency balance — keeping the buy button enabled state in sync without requiring a full search refresh.
- **STATE_SOLD / STATE_CAN_CLAIM_RESELL collision:** Both constants share the integer value `5`. `STATE_SOLD` is never referenced — the actual sold state for buy-side items uses `updateAsSold()` driven by `purchaseListingSucceeded` rather than a constant.
- **Sell-form state preservation:** When switching from Sell to Buy, `setBuyTab` stores `listingTexture`, `listingQuantity`, `listingPrice`, and all four comparison price strings in instance fields, then restores them when the Sell tab is re-entered — so the user's in-progress listing is not lost.
- **Drag-and-drop listing creation:** `SlotDragDropHelper.registerDropCallback` registers `DRAGHOST.DROP`, so players can drag an item from their inventory directly into the sell listing image slot. The drop is forwarded as `OnDropIntoWindow`, and the item is staged when `setListedItem(slotId, quantity, texture)` arrives; a right-click on the image (`OnRemoveMarketplaceSaleListing`) puts it back.
- **Two-frame visibility defer:** `MarketplaceItem` starts invisible and sets `visible=true` only after 2 `ENTER_FRAME` cycles (counting via `frameCounter`), ensuring the Flash layout engine has fully positioned the component before it renders.
- **Quick Search:** The `quickSearch` button on the sell tab is only shown while an item is staged. Clicking it or calling the `quickSearch` ExternalInterface callback calls `ExternalInterface.call("QuickSearch", slotId)` with the staged item's inventory slot; the engine puts the item's name in the buy tab's search box through `setSearchString`.
