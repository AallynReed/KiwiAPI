# kiwistore.swf

> The Trove in-game store and the class changer (one SWF): purchasable products and class cards in a tabbed, scrollable tile grid, with Patron subscriptions, Starter Packs, sorting and a loot-probability panel.

**Document/main class:** `StoreBase` (extends `_kiwi.Core.UIComponent`) — [Embed symbol "symbol543" on `TileView`]
**SWF-specific classes:** 12 game-logic + 16 `KiwiStore_fla` timeline symbols + ~12 button/widget subclasses + ~30 asset wrappers

---

## Main class: `StoreBase`

`StoreBase` is the root document class. It owns the window chrome (header, close button, currency display), the category-tab bar, the product tile grid (`productsView:TileView`), a loot-probability side panel, a lightbox overlay, a status/blocking page, a sort header, and a console button-legend strip. It registers all `ExternalInterface` callbacks that the C++ game engine calls to drive the store and proxies some through to `TileView`.

### Public methods

- `toggleLightbox(e:Event = null) : void` — shows or hides the `lightbox` overlay; if the triggering `DataEvent` carries a `child` MovieClip, that child is added to the lightbox (used by `ZHPatronPopup` and similar overlays); otherwise all children are removed. Also syncs the per-tab `lightbox` visibility on each category tab.
- `onTabClick(e:MouseEvent) : void` — handles PC mouse clicks on category tabs; guards against clicks while the status page is visible, posts `Play_ui_window_tab` sound via `ExternalInterface.call("POST_SOUND_EVENT", ...)`, then calls `setCategory`.
- `addCategory(label:String, iconImage:String, index:int) : void` — creates a new `TabHeader` (or `tabIconHeader` if `iconImage` is non-empty), positions it to the right of the previous tab, registers it at `tabsIndex[index]` and in `categoryTabs`. `index` is the engine's category identifier (stored as the tab's `identifier`, and what `OnShowCategory` sends back); an identifier already present is skipped, since the engine repeats the list when the window reopens. The first tab added becomes the selected tab and triggers `onConfigured`.
- `updateBalance(credits:Number, points:Number) : void` — updates the credits and points TextFields in `credits_accountBalance` and `points_accountBalance` using `KiwiTextUtil.addDigitDelimiters`, then propagates to `productsView.updateBuyButtons`.
- `showStatusPage(message:String) : void` — makes `blockOpStatusPage` visible with the given message (e.g. `"$Store_Loading"`), and hides the lightbox.
- `hideStatusPage() : void` — hides `blockOpStatusPage`; restores the selected tab's `"selected"` frame.
- `setWindowTitle(title:String) : void` — sets `winHeader.title`.
- `updateButtonLegend() : *` — vertically centres the four `buttonLegend` text fields (`buttonATextField`, `buttonXTextField`, `buttonYTextField`, `buttonBTextField`) relative to their respective button clips using the global `vcenterTextfieldToClip` helper.

### Private / internal methods

- `configUI() : void` — overrides `UIComponent.configUI`; sets up mouse-over/out currency tooltip listeners, sort-header click listeners, probability button listener, and registers all ExternalInterface callbacks (or seeds dummy data in standalone mode).
- `setCategory(tab:TabHeader) : void` — PC tab-switch: clears `productsView`, resets spacing, marks the selected tab, calls `ExternalInterface.call("OnShowCategory", identifier)` with the tab's category identifier, and shows the loading status page. The engine answers with `clearProducts` and the category's products.
- `selectTab(index:int, queryString:String) : void` — engine-driven tab switch (`index` is a category identifier, looked up in `tabsIndex`); marks the tab selected on every platform and asks nothing back. On console it also clears the grid, switches to the carousel when `index` is 0 and the query string contains `"PLAYERCLASS"` (legend frame `StoreClasses`, `TutorialClassSelect` or `Classes`) or to the grid otherwise (`SetPatronView` with whether the query contains `"PATRON"`; legend frame `Store`, `Generic` or `StoreNoTabs`), throws if the query string is empty, and calls `ExternalInterface.call("SetNumTilesPerRow", 4)`.
- `setCurrencyInfoVisibility(visible:Boolean) : void` — shows/hides the currency icons and balance fields; on PC, also collapses `background` and `gray_background` heights to reclaim the header space.
- `setInstructions(text:String) : void` — hides all tabs, hides `tabAnchor`, and shows `headerMessage` with the given text.
- `setLootProbability(title:String, body:String) : void` — delegates to `lootProbability.populateLootText`; hides the panel if body is empty.
- `showLootProbability(visible:Boolean) : void` — directly toggles `lootProbability.visible`.
- `scrollTooltipText(delta:int) : void` — forwards scroll to `lootProbability` if visible, otherwise to `productsView.scrollTooltipText`.
- `sortClasses(sortField:String) : void` — manages sort-header icon frames (`"selected"`/`"default"`) and calls `productsView.sortClasses` with a priority-ordered Vector of field names: `level` → level, powerRank, releaseDate; `powerRank` → powerRank, level, releaseDate; anything else → releaseDate only. Called by the sort-header clicks (with the clip's instance name) and by the engine, which re-applies the sort it remembers from `OnSortClasses` when the window opens.
- `attemptHighlightTile(index:int, stationary:Boolean) : void` — if `productsView` has visible items, highlights immediately; otherwise defers via `ENTER_FRAME` until items appear. The engine calls `highlightTile` on PC as well; `TileView.highlightTile` returns `null` there, so it only has an effect on console.
- `highlightTile(index:int, stationary:Boolean) : void` — calls `productsView.highlightTile`, then updates the `buttonLegend` frame and text fields based on the highlighted `ProductTile`'s purchase panel currency IDs.
- `buttonLegendGotoAndStop(frame:String, callback:Function) : *` — animates the button legend to a named frame; guards against redundant transitions by comparing `currentLabel`; removes any pending `ENTER_FRAME` listener before starting a new transition.
- `setButtonLegendTextFields(tile:ProductTile) : void` — sets `buttonATextField`/`buttonYTextField` HTML text to the appropriate `$CreditsBuy_ButtonLegend`, `$CubitsBuy_ButtonLegend`, `$Buy_ButtonLegend`, or `$Store_View` (NX) localisation key based on the tile's currency IDs.
- `onConfigured() : void` — fires `ExternalInterface.call("Configured", position, 4)` to tell the engine the store is ready. `position` is the selected tab's position in `categoryTabs` (strip order, 0 when there are no tabs), not its category identifier. No products are sent until this call has been made.
- `onCurrencyMouseOver/Out` — shows/hides currency tooltips via `ExternalInterface.call("ShowTooltip"/"HideTooltip")`, passing the `$Currency_*` keys untranslated.

### Key fields

| Field | Type | Role |
|---|---|---|
| `productsView` | `TileView` | The scrollable product-tile grid. |
| `lootProbability` | `LootProbability` | Loot probability side panel. |
| `lightbox` | `MovieClip` | Full-screen dark overlay; children are popup panels. |
| `blockOpStatusPage` | `MovieClip` | Loading/blocking overlay with `statusMessage` child. |
| `categoryTabs` | `Array` | All `TabHeader` instances in display order. |
| `tabsIndex` | `Array` | Sparse array mapping category index → `TabHeader`. |
| `tabLabels` | `Vector.<String>` | Raw (untranslated) label strings indexed by tab index. |
| `selectedCategoryTab` | `TabHeader` | Currently active tab. |
| `sortHeader` | `MovieClip` | Sort header with `level`, `powerRank`, `releaseDate` child clips. |
| `buttonLegend` | `MovieClip` | Console button-legend strip (`buttonLegend_48` symbol). |
| `winHeader` | `WindowHeaderSmall` | Window title bar. |
| `credits_accountBalance`, `points_accountBalance` | `MovieClip` | TWC/TWP (Credits/Cubits) balance display clips. |
| `marketplaceText` | `TextField` | Static footer line holding `$Store_MarketplaceSuggestion` ("Don't see what you're looking for? Try checking out the MARKETPLACE!"); hidden by `hideMarketplaceText` and by `setCurrencyInfoVisibility(false)`. |
| `currencyVisibility` | `Boolean` | Whether the currency header is shown. |
| `canClose` | `Boolean` | Whether the close button is enabled. |

### Frame scripts / timeline

Three `stop()` frame scripts at frames 1, 11, and 21, which carry the root frame labels `PC`, `Console` and `ConsoleLoc` (stage 1210×761, 24 fps, 30 frames).

### Runtime dependencies & integration

- **ExternalInterface callbacks registered:**
  - `UPDATE_BALANCE(credits, points)` → `updateBalance`
  - `hideMarketplaceText()` → `hideMarketplaceText`
  - `addCategory(label, icon, index)` → `addCategory`
  - `showStatusPage(msg)` → `showStatusPage`
  - `hideStatusPage()` → `hideStatusPage`
  - `setWindowTitle(title)` → `setWindowTitle`
  - `setCurrencyInfoVisibility(bool)` → `setCurrencyInfoVisibility`
  - `setLootProbabilityVisibility(bool)` → `setLootProbabilityVisibility`
  - `setInstructions(text)` → `setInstructions`
  - `showCloseButton(bool)` → `showCloseButton`
  - `selectTab(index, queryString)` → `selectTab`
  - `setLootProbability(title, body)` → `setLootProbability`
  - `showLootProbability(bool)` → `showLootProbability`
  - `scrollTooltipText(delta)` → `scrollTooltipText`
  - `sortClasses(field)` → `sortClasses`
  - `highlightTile(index, stationary)` → `attemptHighlightTile`
  - `onConfigured()` → `onConfigured`
  - `productsView.ShowTooltip` → proxied directly from `TileView`
  - `TileView` registers the grid-level set itself — see its section.
- **ExternalInterface outbound calls:**
  - `"OnShowCategory"(identifier)` — tab selected; the argument is the category identifier from `addCategory`.
  - `"Configured"(position, tilesPerRow)` — store ready; `position` is the selected tab's place in the strip.
  - `"POST_SOUND_EVENT"("Play_ui_window_tab")` — tab-click sound.
  - `"ShowTooltip"(x, y, titleKey, bodyKey)` / `"HideTooltip"()` — currency hover tooltips.
  - `"SetPatronView"(bool)` — console: patron tab active.
  - `"SetNumTilesPerRow"(4)` — console: tile layout.
  - `"OnShowLootProbability"(bool)` — probability button clicked.
- **Translate keys used:** `$Store_Loading`, `$Currency_TWC_Title`, `$Currency_TWC_Description`, `$Currency_TWP_Title`, `$Currency_TWP_Description`, `$CreditsBuy_ButtonLegend`, `$CubitsBuy_ButtonLegend`, `$Buy_ButtonLegend`, `$Store_View`, `$Switch`, `$Trial_ButtonLegend`; static `$Store_MarketplaceSuggestion` and `$Store_SortBy` on the timeline.

---

## Class: `TileView`

Extends `_kiwi.Controls.ScrollableTileView`. `[Embed(source="/_assets/assets.swf", symbol="symbol543")]`. The scrollable, paginated product tile grid. Manages both a standard vertical grid layout and a horizontal carousel view (for class selection on console). Handles tile addition (`addProduct`, `addStarterPack`, `addInteractableProduct`, `addTrialProduct`, `addClassSelection`), tile updates (`updateBuyButtons`, `highlightTile`, `sortClasses`), and the carousel scroll/fade animation loop.

**Key fields:** `carouselTooltip:MovieClip`, `carouselFilterText:MovieClip`, `storePosCharacters/Default:MovieClip` (anchor points for tooltip repositioning), `itemsByProdCode:Dictionary`, `timer:Timer` (carousel animation), `scrollIndex:int`, `scrolling:Boolean`, `fadeIn:Boolean`, `lastSort:String`, `isNX:Boolean`, `enableTooltipTextScroll:Boolean`, `globalScale:Number`.

**ExternalInterface callbacks registered:** `clearProducts` (→ `clear`), `addProduct`, `addStarterPack`, `addProductPrice`, `addProductPriceString`, `addPromoData`, `setProductOwnership`, `enableInteraction`, `addInteractableProduct`, `addTrialProduct`, `addClassSelection`, `showSortHeader`, `updateDealsExpiration`, `addTextureLayer`, `addOffsetTextureLayer`, `addTextureLayerWithText`, `addOverlayedTextureWithText`, `isValidPatronBonus`, `unhighlightTile`, `primaryPurchaseOption`, `secondaryPurchaseOption`, `showTooltip`, `hideTooltip`, `setFilterText`, `setCarouselView`, `clearTooltipText`, `setProductVisible`, `rebuildVisibleItemList`, `centerRowVertically`.

**Building a category:** after `OnShowCategory` the engine sends `clearProducts` once, then one add call per product, keyed by product code from then on:
- `addProduct(image, name, code, owned, hasTradableGrants, info, informational)`, `addInteractableProduct(image, name, code, label, enabled, info)`, `addTrialProduct(image, name, code, label, enabled, info, trialLimits)`, `addStarterPack(image, name, code, owned, info, informational)` (a code containing `"PATRON"` builds a `PatronTile`, otherwise a `StarterPackTile`), and `addClassSelection(image, name, code, label, enabled, info, trialLimits, level, powerRank, shieldFrame, subclassName, subclassIcon)` (a non-empty `trialLimits` turns the switch into a trial).
- Product art: the `image` argument of `addProduct` arrives empty; every picture comes afterwards through `addTextureLayer(code, texture)` (or the offset/text variants, whose x/y offsets are added to the layer's position inside the tile's preview container). Texture names are file paths such as `ui/store/ui_store_mount_bundt.dds`.
- `addProductPrice(code, cost, currencyId, canPurchase, monthlyPrice, sale)` and `addProductPriceString(code, priceText, currencyId, saleText)` add buy buttons; `setProductOwnership(code, owned)` and `updateDealsExpiration(code, seconds)` update a tile later. Currency ids are `TWC` (Credits), `TWP` (Cubits) or a real-money ISO code.
- `addCodedItem` stores each tile's negative insertion index as its `releaseDate`, so the `releaseDate` sort key is the order the engine sent the products in.

**Carousel:** `setCarouselView(bool)` switches between vertical-grid and horizontal-carousel layout. Carousel navigation uses a `Timer` + `ENTER_FRAME` loop with eased scroll (`SCROLL_TIME=400ms`) and a fade-in (`FADE_TIME=1000ms`) for newly centred tiles. Tile depth is sorted so the centred tile is in front. `highlightTile`, `unhighlightTile`, `primaryPurchaseOption` and `secondaryPurchaseOption` take an index into `visibleItemList` in carousel mode and into `itemList` otherwise.

**Sorting:** `sortClasses(fields:Vector.<String>)` re-sorts `itemList` by the given `ClassSelectTile` fields in priority order (ties fall back to instance name). Choosing a key different from `lastSort` sorts descending and makes it `lastSort`; choosing the same key again sorts ascending and clears `lastSort`. Each call reports the result through `OnSortClasses(lastSort)`.

**Tooltip:** `ShowTooltip()` (registered by `StoreBase` as `productsView.ShowTooltip`) toggles `carouselTooltip` (a `CarouselTooltip_27`); `scrollTooltipText(delta)` scrolls its `tooltipText` when `enableTooltipTextScroll` is true.

**ExternalInterface calls out:** `"OnSortClasses"(lastSort)`, `"OnMouseOverProduct"(code)` (carousel highlight), `"OnMouseOutProduct"(code)` (carousel unhighlight), `"SetCarouselView"(bool)`, `"UpdateVisibleItemCount"(count)`. The purchase calls are made by the tiles themselves.

---

## Class: `ProductTile`

Extends `_kiwi.Core.UIComponent`. Base tile for all store items. Holds up to two `PurchasePanel` instances (`purchasePanel0`, `purchasePanel1`), an `interactPanel` (switch/unlock button) and a separate `trialPanel`, `ownedMC` (owned badge), `previewContainerMC` (item preview), and an `infoButton`/`infoPopup` pair.

**Key methods:**
- `addPrice(cost:Number, currencyId:String, canPurchase:Boolean, monthlyPrice:int, sale:String) : void` — fills `purchasePanel0`, or `purchasePanel1` when the first is already showing or a trial is enabled; `canPurchase` sets the button's `enabled`; `TWP`/`TWC` select the `points`/`credits` frame of the currency icon, any other currency shows the code as text. Hides the interact panel (and the trial panel unless a trial is enabled). A product that is owned and has no tradable grants ignores the price; the engine does send a live price for owned products, which the stock tile traces as `"canPurchase is invalid"`.
- `addTextureLayer(textureName:String, xOffset:int, yOffset:int) : ObjectPreview` — adds an `ObjectPreview` layer to the preview container; the tile stays invisible (`makeVisible = false`) until a layer reports loaded.
- `highlightTile() : void` / `unhighlightTile() : void` — outlines the tile background and restores/dims its brightness (the `stationary` flag belongs to `TileView.highlightTile`, where it skips the carousel scroll animation).
- `onPurchase(e:MouseEvent) : void` — calls `ExternalInterface.call("STORE.PURCHASE", productCode, currencyId, productName.text)` with the currency of the panel that was clicked; `purchaseOption0/1()` do the same for console.
- `onInteract(e:MouseEvent) : void` — calls `ExternalInterface.call("STORE.INTERACT", productCode)` (interact and trial buttons alike).
- `onMouseOverPreview/OutPreview` — call `ExternalInterface.call("OnMouseOverProduct"/"OnMouseOutProduct", productCode)` when the pointer enters/leaves the preview; no callback answers them.
- `toggleTileBrightness(bright:Boolean) : void` — sets a `ColorTransform` of 1 (bright) or 0.5 (dim) on the preview, purchase panels, interact panel and owned badge, and hides the info button. Console tiles start dimmed.
- `informational` setter — relabels the badge `$Store_Locked` ("LOCKED") instead of `$Store_Owned` ("OWNED").
- `static formatPrice(price, currencyId)` — real-money prices arrive in hundredths and are divided by 100, except `KRW`.

---

## Class: `PurchasePanel`

Extends `_kiwi.Core.UIComponent`. A small buy-button widget embedded in each `ProductTile`. Fields: `price:TextField`, `currency:TextField`, `purchaseButton:MovieClip`, `currencyIcon:MovieClip`. Properties: `cost` (formats number with digit delimiters into `price.text`), `currencyId` (stores the id and writes it into `currency.text`; the `currencyIcon` frame is set by `ProductTile.addPrice`), `showCurrencyText` (toggles `currency.text` visibility).

---

## Class: `PatronTile`

Extends `ProductTile`. Patron subscription tile. Adds `monthyPriceTxt` (sic), `totalPriceTxt`, `saleStickerTxt`, `goldOverlay`, its own `purchaseButton` (`$Store_Buy`), up to three localised text fields (`locField0/1/2`), and a `bonusPanel`. The product name is hidden: the pass's copy is read from `$Store_<productCode>` and split on newlines into the three `locField`s. Overrides `addPrice`: a `monthlyPrice` above 0 fills `$Store_MonthlyPrice` ("{0}/month") and `$Store_TotalPrice` ("Total: {0}"), real-money amounts going through `formatPrice` (hundredths); a non-empty `sale` shows `goldOverlay` and the `$Store_<sale>` sticker. `addPromoData(data)` shows `bonusPanel` with `$Welcome_Bonus` and the quantity after the last `_` of `data` (deferred to the panel's target frame on console). `onPurchase` sends the same `STORE.PURCHASE(productCode, currencyId, productName.text)` as the base tile.

---

## Class: `StarterPackTile`

Extends `ProductTile`. Starter pack / limited-time deal tile. Adds a `CountdownTimer` (shown by `updateDealsExpiration`), a `pulseAnimTimer:Timer`, and a `valueSticker:MovieClip`. `addPrice` formats real-money prices with the currency symbol and shows `$Store_Free` below 0.01; `addPriceString(priceStr, currencyId, saleText)` directly sets a pre-formatted price string (for real-money SKUs; `$Store_View` on NX). `populateSaleSticker(text)` shows a discount badge from the price call's sale argument, enlarging any part inside `[ ]`. `playPulseAnim()` triggers the `timer_pulse_anim_72` symbol. Overrides `owned` setter to hide the purchase panels when the pack is already owned.

---

## Class: `ClassSelectTile`

Extends `ProductTile`. Class-selection tile, used both in the PC grid and the console carousel. Adds `levelText:TextField`, `powerRankText:TextField`, `powerRankIcon:MovieClip`, `subClassLabel:TextField` (`$Store_SubClassLabel`), `subClassName:TextField`, `ClassIcon:ArtClip` (the equipped subclass's icon, 23×23), and `_level:int`/`_powerRank:int` sortable fields. `setClassData(level, powerRank, shieldFrame, subclassName, subclassIcon)` populates all these fields: `powerRankIcon` holds `wings` (hidden) and `shield`, a 20-frame sprite with one frame per power-rank tier, sent to `shieldFrame`; the level/power-rank readings are hidden when `level` is 0 (a class the player does not own); the subclass row appears only when `subclassName` is non-empty.

---

## Class: `PatronInfoTile`

Extends `_kiwi.Core.UIComponent`. Left-panel bonus-list display for the Patron tab. Declares a `bonuses:Array` of 12 localisation keys:

`$Store_Patron_LevelFaster`, `$Store_Patron_ChaosChests`, `$Store_Patron_MoreFlasks`, `$Store_Patron_JumpHigher`, `$Store_Patron_MineFaster`, `$Store_Patron_CraftFaster`, `$Store_Patron_BetterDrops`, `$Store_Patron_ArenaRewards`, `$Store_Patron_RingTime`, `$Store_Patron_DailyRewards`, `$Store_Patron_DailyCubits`, `$Store_Patron_GemKarma` — each with a `<key>Details` companion for the description.

On the frame after construction, for non-EN locales, writes those keys into the `bonusHeader`/`bonusDetails` fields and lays the entries out three to a row, hiding bonus entries not present in `_validPatronBonuses`. That list is filled by the engine's `isValidPatronBonus(key)` calls (one per live bonus); when it is empty every bonus is shown. The English timeline carries its own static set of keys, which includes `$Store_Patron_ExaltedAdventurine` where the code list has `ArenaRewards`.

---

## Class: `ZHPatronView`

Extends `_kiwi.Core.UIComponent`. Chinese-locale-only patron VIP tier selector. Holds three VIP tier MovieClips (`zhPatronVIP0`, `zhPatronVIP1`, `zhPatronVIP2`), a `Dictionary` keyed by product code, and a `Vector.<Array>` of tier product lists. `addProductPrice(prodCode, price, currencyId, monthlyPrice, saleSticker)` picks the VIP tier by matching a tier label inside the product code and fills the tier's first item via `populatePatronItem`. `showPatronPopup(tierIndex)` fires a `DataEvent(StoreBase.TOGGLE_LIGHTBOX)` carrying a `ZHPatronPopup` as the child.

---

## Class: `ZHPatronPopup`

Extends `_kiwi.Core.UIComponent`. Chinese-locale patron purchase popup (lightbox child). Holds up to 3 `patronItem` MovieClips and populates them via `ZHPatronView.populatePatronItem`. `handleMouseClick(e:MouseEvent)` calls `ExternalInterface.call("STORE.PURCHASE", prodCode, currencyId)` when a `LabelButton` inside an item is clicked. Has 3 frame scripts (frames 1, 11, 21 — all `stop()`).

---

## Class: `LootProbability`

Extends `_kiwi.Core.UIComponent`. Side panel displaying drop-rate probability text for loot boxes. Fields: `headerTF:TextField`, `messageTF:TextField`, `scrollPrompt:MovieClip`. `populateLootText(title, body)` sets the two text fields (`body` as HTML) and shows the scroll prompt only when the text overflows. `scrollTooltipText(delta)` scrolls `messageTF.scrollV`. `setTooltipScrollPromptState()` sets the prompt's up/down arrows to `inactive`/`pulse`/`active` from the scroll position.

---

## Class: `PaymentMethodsPage`

Extends `MovieClip` (dynamic). Payment method selection sub-page. Holds `accountMgmtButton`, `cancelButton`, `acceptButton`, and a `KiwiComboBox` (`paymentMethod`). The combo box data provider is initialised empty; the engine populates it via ExternalInterface. Minimal logic beyond component property setup.

---

## `KiwiStore_fla` timeline symbol classes (16 classes)

These are `_fla`-package symbol classes linked to Flash timeline symbols. Key ones:

- `BlockingStatus_36` — loading/status overlay with `statusMessage` child (`statusMessage_37`).
- `statusMessage_37` — `textField:TextField` with PC and Console frames.
- `CarouselTooltip_27` — tooltip panel: `tooltipTitle`, `tooltipText`, `scrollPrompt`; labelled frames `Console` (class carousel) and `ConsoleStore`.
- `buttonLegend_48` — console button-legend strip; 7 labelled frames: `Store`, `StoreClasses`, `Buy`, `Generic`, `Classes`, `StoreNoTabs`, `TutorialClassSelect`. Each frame shows a different set of button-icon + label combinations.
- `currencyIcon_88` (character 142) — 2-frame clip labelled `credits` (TWC) and `points` (TWP, Cubits).
- `SortIconLevel_41`, `SortIconPowerRank_44`, `SortIconReleaseDate_46` — sort column header icons with `default` and `selected` frames.
- `timer_pulse_anim_72` — pulsing glow animation for limited-time deal tiles.
- `lightboxTab_133` — lightbox background tab.
- `zhPatronItem_136`, `zhPatronVIP1_140`, `zhPatronVIP2_139`, `zhPatronVIP3_138` — ZH patron popup rows and VIP tier clips.
- `buttonPanel_89`, `btnInfo2_94`, `SortIconLevel_41` — minor button/icon wrappers.

---

## Asset wrappers (~30 classes — not detailed individually)

Approximately 8 `BitmapData` embed classes (currency icons, dummy placeholder, Xbox One controller button PNGs: `btn_XBOne_A`, `btn_XBOne_B`, `btn_XBOne_LT`, `btn_XBOne_RT`, `btn_XBOne_X`, `btn_XBOne_Y`) and ~22 skin stub classes (`CellRenderer_*Skin`, `ComboBox_*Skin`, `ScrollArrowDown/Up_*Skin`, `ScrollThumb_*Skin`, `ScrollTrack_skin`, `List_skin`, `focusRectSkin`, `PatronDarkBG`).

---

## Notable logic

- **Tab layout:** Tabs are added dynamically by the engine via `addCategory`; they self-position by summing widths of previous tabs with a −2px gap. A sparse `tabsIndex` array allows non-contiguous category indices.
- **Store and class changer share the file:** the class changer is this same SWF showing the class category. The engine sends it the same full category list as the store, and no call distinguishes it from the store's own Classes tab; class cards arrive through `addClassSelection`.
- **Console carousel:** The carousel is exclusively activated when `selectTab` is called with index 0 and a query string containing `"PLAYERCLASS"`. The centred tile is promoted in depth and other tiles are dimmed. Scroll uses a `Timer`-driven eased lerp over 400ms; on completion a 1000ms fade-in plays on the new centred tile.
- **Multi-currency buy buttons:** Each `ProductTile` supports two `PurchasePanel` slots for different currencies (e.g., TWC and TWP). `updateBuyButtons(credits, points)` only re-checks the `TWP` panels against the Cubits balance (the credits value is not used); credit buttons keep the `canPurchase` state the engine sent.
- **ZH locale path:** Detected globally via `_locale == LOCALE_ZH`; the entire Patron tab is replaced by `ZHPatronView` + `ZHPatronPopup` instead of `PatronTile`.
- **Standalone/dev mode:** When `IggyFunctions.inIggy` is false, `configUI` seeds dummy categories and starter pack tiles for layout previewing.
