# marketplace.swf

> The Trove player-driven auction marketplace, allowing players to search and purchase listings from other players (Buy tab) and create or manage their own sell listings (Sell tab). Uses Flux as the primary trading currency. Appears when the player opens the Marketplace from the in-game UI.

**Document/main class:** `Marketplace` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 7 game-logic + 8 `MarketPlace_fla` timeline symbols + ~19 button/widget subclasses + ~24 asset wrappers

---

## Main class: `Marketplace`

`Marketplace` is the document class and central controller. It manages two operating modes — `MODE_BUY` (0) and `MODE_SELL` (1) — surfaced as a `tabBuy`/`tabSell` `TabHeader` pair. The main content area is `tabContentMC` (a `tabContentMC_10` symbol) which jumps to frames 1/2 (PC Buy/Sell) or 3/4 (console Buy/Sell). The buy-side grid is `buyListingsView:MarketView`; the sell-side grid is `sellListingsView:MarketView`.

The constructor wires mouse listeners on both tabs and both `MarketView` instances for `MarketEvent.BUTTON_CLICK`, sets the initial instructions text, and links footer pagination buttons. `configUI()` registers ~40 ExternalInterface callbacks and calls `setupTranslation()`.

### Public methods / properties

- `get lastPageIndex() : int` — computes the zero-indexed page offset of the last page based on `numListings` (buy) or `maxPlayerListings` (sell) and `ITEMS_PER_PAGE` (12).

### Private / internal methods (selected)

- `configUI() : void` — registers all ExternalInterface callbacks; seeds dummy data in standalone mode. Also auto-sizes several sell-tab text fields using `KiwiTextUtil.resizeFont`.
- `setBuyTab(e:MouseEvent = null) : void` — saves current sell-form state (texture, quantity, price, comparison text) to local fields, navigates `tabContentMC` to frame 1 or 3, shows `buyListingsView`, hides `sellListingsView`, sets `newDrawMode = MODE_BUY`, schedules `drawUI` on next `RENDER`.
- `setSellTab(e:MouseEvent = null) : void` — saves filters, navigates `tabContentMC` to frame 2 or 4, shows `sellListingsView`, calls `ExternalInterface.call("GetSellListingsRange", 0, maxPlayerListings-1)`, schedules `drawUI` on next `RENDER`.
- `drawUI(e:Event) : void` — one-shot `RENDER` handler. On `MODE_BUY`: populates `itemFilterMC:KiwiComboBox` from `itemData`, wires search button and name filter. On `MODE_SELL`: wires `sellPrice`, `sellPricePerUnit`, `btnCreateListing`, item image click, quick-search button, and restores comparison price texts; calls `buttonLegendGotoAndStop("BuySell")`.
- `addListing(id, name, texture, quantity, price, currencyIndex, info, hasTimer, categories, timerSeconds) : void` — creates a `MarketplaceItem(STATE_CAN_BUY, ...)` and adds it to `buyListingsView`.
- `addPlayerListing(...) : void` — creates a `MarketplaceItem` with a sell-side state and adds it to `sellListingsView`; handles `STATE_CAN_CANCEL`, `STATE_CAN_CLAIM`, `STATE_CAN_CLAIM_EXPIRED`, `STATE_CAN_CLAIM_RESELL`, `STATE_CAN_CLAIM_EXPIRED_RESELL`.
- `purchaseListingSucceeded(id) : void` → calls `sellListingsView.updateItemAsSold(id, true)`.
- `purchaseListingFailed(id) : void` → calls `sellListingsView.updateItemAsSold(id, false)` (re-enables the button).
- `purchaseListingCancelled(id) : void` → same as failed.
- `cancelListingSucceeded/Failed`, `claimListingSucceeded/Failed`, `claimExpiredListingSucceeded/Failed`, `createListingSucceeded/Failed` — update corresponding item states or show error messages.
- `searchSucceeded() : void` — hides `listingsMessageText`, shows the buy view.
- `searchFailed() : void` — shows `listingsMessageText` with `"$Marketplace_NoResultsFound"`.
- `addCurrency(name, thumbnail, index) : void` — creates a `CurrencyData` and inserts it at `Marketplace.currencies[index]`.
- `setCurrencyAmount(index, amount) : void` — updates `Marketplace.currencies[index].quantity` and calls `buyListingsView.updateItemAsSold` for any item whose currency index matches and whose price exceeds the new amount (to grey-out unaffordable items).
- `getListingFee(fee:int) : void` — populates `tabContentMC.taxTextField` with the localised listing fee string; fee is in Flux (currencies[0]).
- `setMarketplaceComparisonText(allInfo, low, mean, high) : void` — updates the four sell-tab comparison price text fields.
- `setTaxInfo(minFee, weighedMin, weighedMax, maxFee, claimTax) : void` — stores fee range locals and updates `tabContentMC.listingFeeRatesText`.
- `onButtonClicked(e:MarketEvent) : void` — the central action dispatcher; branches on `e.state`:
  - `STATE_CAN_BUY` → `ExternalInterface.call("OnPurchaseListing", slotId, currencyIndex)`.
  - `STATE_CAN_CANCEL` → `ExternalInterface.call("OnCancelListing", slotId)`.
  - `STATE_CAN_CLAIM` / `STATE_CAN_CLAIM_RESELL` → `ExternalInterface.call("OnClaimListing", slotId)` or `"OnRemoveMarketplaceSaleListing"`.
  - `STATE_CAN_CLAIM_EXPIRED` / `_RESELL` → `ExternalInterface.call("OnClaimExpiredListing", slotId)` or resell.
  - `STATE_LOCKED` → `ExternalInterface.call("OnRequestInventoryOpen")` or opens the upsell view.
- `onListClicked` → validates price and quantity, calls `ExternalInterface.call("OnCreateListing", texture, qty, price, currency)`.
- `onSearchClicked` → calls `ExternalInterface.call("OnSearch", category, name, currency, collectState, sortByPrice)`.
- `onItemDropped(item)` → `SlotDragDropHelper` callback; sets `listingTexture` and the listing image, calls `ExternalInterface.call("requestListingPrice", texture)`, and shows the quick-search button.
- `saveFilters/saveSellFilters` → `ExternalInterface.call("OnSaveFilters", ...)` / `"OnSaveSellFilters"`.
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
| `numListings` | `int` | Current buy listing count, used for pagination. |
| `listingTexture` | `String` | Texture name of the item being listed for sale. |
| `listingPrice`, `listingQuantity`, `listingCurrency` | `int` | Sell-form state preserved across tab switches. |
| `listingComparisonInfo/Low/Mean/High` | `String` | Comparison price strings, preserved across tab switches. |
| `newDrawMode`, `newDrawModeSetup` | `int`, `Boolean` | Deferred mode-switch state for `drawUI`. |
| `showingUpsell` | `Boolean` | Whether the upsell (purchase-credits) tile is showing. |
| `m_listingTooltipShowing` | `Boolean` | Whether the sell-tab listing info tooltip is visible. |
| `pendingSelection` | `int` | Console: item index to select after the view populates. |

### Frame scripts / timeline

Frames 1 and 11 both call `stop()`. Frame 11 corresponds to a secondary timeline state (likely an alternate layout or loading state).

### Runtime dependencies & integration

- **ExternalInterface callbacks registered (all modes):**
  - Setup: `setPlayerListingsSize`, `setSellSlotPrice`, `setSellSlotUnlocked`, `addItemCategory`, `addCollectedState`, `addCurrency`, `addDeprecatedCurrency`, `setCurrencyAmount`, `setItemCategory`, `setCollectedState`, `setPriceSorting`, `setCurrency`, `setSellCurrency`, `setSearchString`, `setListedItem`, `clearListedItem`
  - Data: `addListing`, `addPlayerListing`, `searchSucceeded`, `searchFailed`
  - Transaction results: `purchaseListingSucceeded/Failed/Cancelled`, `cancelListingSucceeded/Failed`, `claimListingSucceeded/Failed`, `claimExpiredListingSucceeded/Failed`, `createListingSucceeded/Failed`
  - UI: `setBuyTab`, `setSellTab`, `setMarketplaceComparisonText`, `setTaxInfo`, `getListingFee`, `playerListingPurchased`, `quickSearch`
- **Console-only callbacks:** `setFocusBuyItemType`, `setFocusBuyHideCollected`, `setFocusBuySortByPrice`, `setFocusBuyItemName`, `setFocusBuySearch`, `setFocusSellInventory`, `setFocusSellPriceTotal`, `setFocusSellPriceUnit`, `setFocusSellCreate`, `setFocusSellListingRates`, `setFocusQuickSearch`, `onItemFilterClicked`, `onToggleBuyHideCollection`, `onToggleBuySortByPrice`, `onSearchClicked`, `onListImageClicked`, `onListClicked`, `onItemSelected`, `onActionButtonClicked`, `onActionButton2Clicked`, `onInfoButtonToggled`, `onPageFirstClicked`, `onPageLeftClicked`, `onPageRightClicked`, `onPageLastClicked`, `onListingRatesClicked`
- **ExternalInterface outbound calls:**
  - `"OnSearch"(category, name, currency, collectState, sortByPrice)`
  - `"GetListingsRange"(start, end)`, `"GetSellListingsRange"(start, end)`
  - `"OnPurchaseListing"(slotId, currencyIndex)`
  - `"OnCancelListing"(slotId)`
  - `"OnClaimListing"(slotId)`, `"OnClaimExpiredListing"(slotId)`
  - `"OnCreateListing"(texture, qty, price, currency)`
  - `"OnDropIntoWindow"(texture)` — item drag-drop.
  - `"OnRemoveMarketplaceSaleListing"(slotId)` — resell.
  - `"OnRequestInventoryOpen"()` — locked tile click.
  - `"QuickSearch"(texture)`
  - `"requestListingPrice"(texture)`
  - `"OnSaveFilters"(...)` / `"OnSaveSellFilters"(...)`
  - `"OnModeChanged"(mode)`
  - `"TOOLTIP.SHOW"(x, y, title, body)` / `"TOOLTIP.HIDE"()`
- **Translate keys used (selected):** `$Marketplace_Instructions`, `$Marketplace_SearchButton`, `$Marketplace_CreateListingsButton`, `$Marketplace_ClaimButton`, `$Marketplace_CancelListingButton`, `$Marketplace_NoResultsFound`, `$Marketplace_ListingFee`, `$Marketplace_ComparisonDescriptionEmpty`, `$Marketplace_UnitPricePrefix`, `$Marketplace_UnitPriceSuffix`, `$Marketplace_UnitPriceLessThanOne`, `$Marketplace_ExpiresIn`, `$DigitGroupDelimiter`.
- **`SlotDragDropHelper`** — items can be dragged from inventory into the sell listing image slot, triggering `onItemDropped`.

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
- `updateItemAsSold(id, sold) : void` — finds the item by `itemId`; if `sold=true`, calls `item.updateAsSold()` (shows sold badge, hides price/button); if `sold=false`, calls `item.resetActionButton()` (re-enables the button after a failed transaction).
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
| `STATE_CAN_CLAIM_EXPIRED` | 4 | Player's expired listing (refund available) |
| `STATE_CAN_CLAIM_RESELL` | 5 | Sold, can also relist |
| `STATE_SOLD` | 5 | **Duplicate of STATE_CAN_CLAIM_RESELL** — constant collision, likely dead |
| `STATE_LOCKED` | 6 | Locked slot (requires unlock via credits/cubits) |
| `STATE_CAN_CLAIM_EXPIRED_RESELL` | 7 | Expired listing, can claim refund or relist |

**Constructor:** Takes up to 11 parameters (state, itemId, name, texture, quantity, currencyIndex, price, info, showUnitCost, categories, timerSeconds). Starts visible=false, becomes visible after 2 `ENTER_FRAME` ticks (via `onRendered`) to allow layout to settle. Routes to timeline frame 1, 2, 3, 5, or 6 based on state.

**Key fields:** `itemId:String`, `state:int`, `priceValue:Number`, `currencyIndex:int`, `actionButton:LabelButton`, `resellButton:BaseButton`, `listingIcon:ArtClip`, `currencyIcon:ArtClip`, `ownedMC:MovieClip`, `infoPopup:MovieClip`, `selectedMC:MovieClip`, `highlightFrame:MovieClip`, `pulseAnimTimer:Timer` (5 s interval, drives pulse animation on `infoPopup.timer`), `btnCredits:LabelButton`, `btnCubits:LabelButton` (for locked/upsell state).

**Key methods:**
- `set selected(bool) : *` — shows/hides `selectedMC` (PC) and `highlightFrame` (console).
- `onActionButtonClicked(e)` — dispatches `MarketEvent.BUTTON_CLICK` with `resell=false`.
- `onResellButtonClicked(e)` — dispatches `MarketEvent.BUTTON_CLICK` with `resell=true`.
- `resetActionButton()` — re-enables `actionButton` (for STATE_CAN_BUY: only if `priceValue <= currencies[currencyIndex].quantity`).
- `updateAsSold()` — hides `actionButton`, `itemPrice`, `currencyIcon`, `itemUnitCost`; shows `ownedMC` at frame 2 (sold badge).
- `updatePrice(price)` — updates `itemPrice.text`.
- `showUpsell(twcPrice, twpPrice, canAfford)` — switches to timeline frame 6 (upsell layout) and wires `btnCredits`/`btnCubits`.
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
- `tabContentMC_10` — the main content panel. Four frames (PC Buy / PC Sell / Console Buy / Console Sell). Contains all form fields: `itemFilterMC:KiwiComboBox`, `itemName:TextField`, `itemNameClear:BaseButton`, `btnSearchListings:LabelButton`, `sellPrice/sellPricePerUnit:TextField`, `sellQuantity:TextField`, `btnCreateListing:LabelButton`, `listingImage` (uses `image` ArtClip), `quickSearch:MovieClip`, `taxTextField`, `eventSticker:MovieClip`, `marketplaceComparisonAllInfo/Low/Mean/High:TextField`, `listingInfoBtn`, `listingFeeRatesText`.
- `ButtonLegend_68` — console button-legend strip. 7 labelled frames: `BuySell`, `BuyTilePurchasable`, `SellTilePurchasable`, `SellTileCancel`, `SellTileClaim`, `Resell`, `TileNonPurchasable`. Each frame displays appropriate controller button icons and action labels.
- `marketListingImage_40` — item image slot with a highlight ring; two frames (PC / Console layout).
- `OwnedProduct_76` — "owned/sold" badge MovieClip. Multiple frames for different sold/expired states used on `ownedMC`.
- `currencyIcon_79` — two-frame currency icon (frame 1: Flux, frame 2: other currency).
- `QuickSearch_42` — quick-search button clip; a single `stop()` frame.
- `timer_pulse_anim_71` — pulsing animation clip used within `infoPopup.timer` to draw attention to expiring listings.

---

## Asset wrappers (~24 classes — not detailed individually)

Approximately 7 Xbox One controller button PNG bitmaps (`btn_XBOne_A`, `btn_XBOne_X`, `btn_XBOne_Y`, `btn_XBOne_LB`, `btn_XBOne_RB`, `btn_XBOne_LT`, `btn_XBOne_RT`), 1 empty `SelectedMC` MovieClip symbol, and ~16 UI component skin stubs (`CellRenderer_*Skin`, `ComboBox_*Skin`, `ScrollArrowDown/Up_*Skin`, `ScrollThumb_*Skin`, `ScrollTrack_skin`, `List_skin`, `TextInput_disabledSkin`, `TextInput_upSkin`, `focusRectSkin`).

---

## Notable logic

- **Currency-aware affordability gating:** `setCurrencyAmount` not only updates the static currency quantity but also immediately iterates all buy tiles and calls `updateItemAsSold(id, false)` (reset) on those that become affordable or unaffordable — keeping the buy button enabled state in sync without requiring a full search refresh.
- **STATE_SOLD / STATE_CAN_CLAIM_RESELL collision:** Both constants share the integer value `5`. `STATE_SOLD` appears to be dead code — the actual sold state for buy-side items uses `updateAsSold()` driven by `purchaseListingSucceeded` rather than a constant.
- **Sell-form state preservation:** When switching from Sell to Buy, the constructor stores `listingTexture`, `listingQuantity`, `listingPrice`, and all four comparison price strings in instance fields, then restores them when the Sell tab is re-entered — so the user's in-progress listing is not lost.
- **Drag-and-drop listing creation:** `SlotDragDropHelper.registerDropCallback` enables players to drag an item from their inventory directly into the sell listing image slot, which triggers `requestListingPrice` and auto-populates the texture field.
- **Two-frame visibility defer:** `MarketplaceItem` starts invisible and sets `visible=true` only after 2 `ENTER_FRAME` cycles (counting via `frameCounter`), ensuring the Flash layout engine has fully positioned the component before it renders.
- **Quick Search:** The `quickSearch` button on the sell tab is only shown when a `listingTexture` is set. Clicking it or calling the `quickSearch` ExternalInterface callback calls `ExternalInterface.call("QuickSearch", texture)` to pre-populate the buy-tab search with the item being listed.
