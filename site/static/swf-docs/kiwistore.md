# kiwistore.swf

> The Trove in-game item store (the "Kiwi Store"), presenting purchasable products in a tabbed, scrollable tile grid with support for Patron subscriptions, Starter Packs, class selection, and a loot-probability panel. Appears when the player opens the store from the main menu or HUD.

**Document/main class:** `StoreBase` (extends `_kiwi.Core.UIComponent`) — [Embed symbol "symbol543" on `TileView`]
**SWF-specific classes:** 12 game-logic + 16 `KiwiStore_fla` timeline symbols + ~12 button/widget subclasses + ~30 asset wrappers

---

## Main class: `StoreBase`

`StoreBase` is the root document class. It owns the window chrome (header, close button, currency display), the category-tab bar, the product tile grid (`productsView:TileView`), a loot-probability side panel, a lightbox overlay, a status/blocking page, a sort header, and a console button-legend strip. It registers all `ExternalInterface` callbacks that the C++ game engine calls to drive the store and proxies some through to `TileView`.

### Public methods

- `toggleLightbox(e:Event = null) : void` — shows or hides the `lightbox` overlay; if the triggering `DataEvent` carries a `child` MovieClip, that child is added to the lightbox (used by `ZHPatronPopup` and similar overlays); otherwise all children are removed. Also syncs the per-tab `lightbox` visibility on each category tab.
- `onTabClick(e:MouseEvent) : void` — handles PC mouse clicks on category tabs; guards against clicks while the status page is visible, posts `Play_ui_window_tab` sound via `ExternalInterface.call("POST_SOUND_EVENT", ...)`, then calls `setCategory`.
- `addCategory(label:String, iconImage:String, index:int) : void` — creates a new `TabHeader` (or `tabIconHeader` if `iconImage` is non-empty), positions it to the right of the previous tab, registers it at `tabsIndex[index]` and in `categoryTabs`, and selects it if it is the first tab. Calls `onConfigured` on first tab.
- `updateBalance(credits:Number, points:Number) : void` — updates the credits and points TextFields in `credits_accountBalance` and `points_accountBalance` using `KiwiTextUtil.addDigitDelimiters`, then propagates to `productsView.updateBuyButtons`.
- `showStatusPage(message:String) : void` — makes `blockOpStatusPage` visible with the given message (e.g. `"$Store_Loading"`), and hides the lightbox.
- `hideStatusPage() : void` — hides `blockOpStatusPage`; restores the selected tab's `"selected"` frame.
- `setWindowTitle(title:String) : void` — sets `winHeader.title`.
- `updateButtonLegend() : *` — vertically centres the four `buttonLegend` text fields (`buttonATextField`, `buttonXTextField`, `buttonYTextField`, `buttonBTextField`) relative to their respective button clips using the global `vcenterTextfieldToClip` helper.

### Private / internal methods

- `configUI() : void` — overrides `UIComponent.configUI`; sets up mouse-over/out currency tooltip listeners, sort-header click listeners, probability button listener, and registers all ExternalInterface callbacks (or seeds dummy data in standalone mode).
- `setCategory(tab:TabHeader) : void` — PC tab-switch: clears `productsView`, resets spacing, marks the selected tab, calls `ExternalInterface.call("OnShowCategory", index)`, and shows the loading status page.
- `selectTab(index:int, queryString:String) : void` — engine-driven tab switch (console path); on console, also switches between carousel view (class categories containing `"PLAYERCLASS"`) and grid view, sets the appropriate `buttonLegend` frame, and calls `ExternalInterface.call("SetNumTilesPerRow", 4)`.
- `setCurrencyInfoVisibility(visible:Boolean) : void` — shows/hides the currency icons and balance fields; on PC, also collapses `background` and `gray_background` heights to reclaim the header space.
- `setInstructions(text:String) : void` — hides all tabs, hides `tabAnchor`, and shows `headerMessage` with the given text.
- `setLootProbability(title:String, body:String) : void` — delegates to `lootProbability.populateLootText`; hides the panel if body is empty.
- `showLootProbability(visible:Boolean) : void` — directly toggles `lootProbability.visible`.
- `scrollTooltipText(delta:int) : void` — forwards scroll to `lootProbability` if visible, otherwise to `productsView.scrollTooltipText`.
- `sortClasses(sortField:String) : void` — manages sort-header icon frames (`"selected"`/`"default"`) and calls `productsView.sortClasses` with a priority-ordered Vector of field names.
- `attemptHighlightTile(index:int, stationary:Boolean) : void` — if `productsView` has visible items, highlights immediately; otherwise defers via `ENTER_FRAME` until items appear.
- `highlightTile(index:int, stationary:Boolean) : void` — calls `productsView.highlightTile`, then updates the `buttonLegend` frame and text fields based on the highlighted `ProductTile`'s purchase panel currency IDs.
- `buttonLegendGotoAndStop(frame:String, callback:Function) : *` — animates the button legend to a named frame; guards against redundant transitions by comparing `currentLabel`; removes any pending `ENTER_FRAME` listener before starting a new transition.
- `setButtonLegendTextFields(tile:ProductTile) : void` — sets `buttonATextField`/`buttonYTextField` HTML text to the appropriate `$CreditsBuy_ButtonLegend`, `$CubitsBuy_ButtonLegend`, `$Buy_ButtonLegend`, or `$Store_View` (NX) localisation key based on the tile's currency IDs.
- `onConfigured() : void` — fires `ExternalInterface.call("Configured", tabIndex, 4)` to tell the engine the store is ready.
- `onCurrencyMouseOver/Out` — shows/hides currency tooltips via `ExternalInterface.call("ShowTooltip"/"HideTooltip")`.

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
| `credits_accountBalance`, `points_accountBalance` | `MovieClip` | TWC/TWP balance display clips. |
| `currencyVisibility` | `Boolean` | Whether the currency header is shown. |
| `canClose` | `Boolean` | Whether the close button is enabled. |

### Frame scripts / timeline

Three `stop()` frame scripts at frames 1, 11, and 21 (same pattern as `background.swf`; labelled frames correspond to different store states).

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
- **ExternalInterface outbound calls:**
  - `"OnShowCategory"(index)` — tab selected.
  - `"Configured"(tabIndex, tilesPerRow)` — store ready.
  - `"POST_SOUND_EVENT"("Play_ui_window_tab")` — tab-click sound.
  - `"ShowTooltip"(x, y, titleKey, bodyKey)` / `"HideTooltip"()` — currency hover tooltips.
  - `"SetPatronView"(bool)` — console: patron tab active.
  - `"SetNumTilesPerRow"(4)` — console: tile layout.
  - `"OnShowLootProbability"(bool)` — probability button clicked.
- **Translate keys used:** `$Store_Loading`, `$Currency_TWC_Title`, `$Currency_TWC_Description`, `$Currency_TWP_Title`, `$Currency_TWP_Description`, `$CreditsBuy_ButtonLegend`, `$CubitsBuy_ButtonLegend`, `$Buy_ButtonLegend`, `$Store_View`, `$Switch`, `$Trial_ButtonLegend`, `$Buy_ButtonLegend`.

---

## Class: `TileView`

Extends `_kiwi.Controls.ScrollableTileView`. `[Embed(source="/_assets/assets.swf", symbol="symbol543")]`. The scrollable, paginated product tile grid. Manages both a standard vertical grid layout and a horizontal carousel view (for class selection on console). Handles tile addition (`addProductTile`, `addStarterPack`, `addPatronTile`, `addClassTile`), tile updates (`updateBuyButtons`, `highlightTile`, `sortClasses`), and the carousel scroll/fade animation loop.

**Key fields:** `carouselTooltip:MovieClip`, `carouselFilterText:MovieClip`, `storePosCharacters/Default:MovieClip` (anchor points for tooltip repositioning), `itemsByProdCode:Dictionary`, `timer:Timer` (carousel animation), `scrollIndex:int`, `scrolling:Boolean`, `fadeIn:Boolean`, `lastSort:String`, `isNX:Boolean`, `enableTooltipTextScroll:Boolean`, `globalScale:Number`.

**Carousel:** `setCarouselView(bool)` switches between vertical-grid and horizontal-carousel layout. Carousel navigation uses a `Timer` + `ENTER_FRAME` loop with eased scroll (`SCROLL_TIME=400ms`) and a fade-in (`FADE_TIME=1000ms`) for newly centred tiles. Tile depth is sorted so the centred tile is in front.

**Sorting:** `sortClasses(fields:Vector.<String>)` sorts the `visibleItemList` by up to two `ClassSelectTile` fields in sequence; supports level and powerRank ascending.

**Tooltip:** `ShowTooltip(x, y, title, body, scrollable)` displays a `carouselTooltip` or `CarouselTooltip_27` panel; `scrollTooltipText(delta)` scrolls `messageTF` when `enableTooltipTextScroll` is true.

**ExternalInterface calls out:** `"STORE.PURCHASE"(prodCode, currencyId)`, `"STORE.INTERACT"(prodCode)`, `"STORE.INFO"(prodCode, visible)`.

---

## Class: `ProductTile`

Extends `_kiwi.Core.UIComponent`. Base tile for all store items. Holds up to two `PurchasePanel` instances (`purchasePanel0`, `purchasePanel1`), an `interactPanel` (trial/interact button), `ownedMC` (owned badge), `previewContainerMC` (item preview), and an `infoButton`/`infoPopup` pair.

**Key methods:**
- `addPrice(cost:Number, currencyId:String, panelIndex:int) : void` — populates the appropriate `PurchasePanel` with cost and currency, enables the purchase button, greys it out if balance insufficient.
- `addTextureLayer(textureName:String, layerIndex:int) : void` — sets an `ObjectPreview` texture within the preview container.
- `highlightTile(stationary:Boolean) : void` / `unhighlightTile() : void` — applies a highlight glow frame to the tile border; stationary mode keeps the highlight persistent.
- `onPurchase(e:MouseEvent) : void` — calls `ExternalInterface.call("STORE.PURCHASE", prodCode, currencyId)` via the parent `TileView`.
- `onInteract(e:MouseEvent) : void` — calls `ExternalInterface.call("STORE.INTERACT", prodCode)`.
- `toggleTileBrightness(dim:Boolean) : void` — applies a color-matrix filter to dim non-highlighted tiles in carousel mode.

---

## Class: `PurchasePanel`

Extends `_kiwi.Core.UIComponent`. A small buy-button widget embedded in each `ProductTile`. Fields: `price:TextField`, `currency:TextField`, `purchaseButton:MovieClip`, `currencyIcon:MovieClip`. Properties: `cost` (formats number with digit delimiters into `price.text`), `currencyId` (controls `currencyIcon` frame and `currency.text`), `showCurrencyText` (toggles `currency.text` visibility).

---

## Class: `PatronTile`

Extends `ProductTile`. Patron subscription tile. Adds `monthlyPriceTxt`, `totalPriceTxt`, `saleStickerTxt`, `goldOverlay`, up to three localised bonus-text fields (`locField0/1/2`), and a `bonusPanel`. Overrides `addPrice` to populate the monthly/total price display and sale sticker; overrides `onPurchase` to include a subscription period in the ExternalInterface call. The `goldOverlay` is shown when the tile is owned.

---

## Class: `StarterPackTile`

Extends `ProductTile`. Starter pack / limited-time deal tile. Adds a `CountdownTimer`, a `pulseAnimTimer:Timer`, and a `valueSticker:MovieClip`. `addPriceString(priceStr)` directly sets a pre-formatted price string (for real-money SKUs). `populateSaleSticker(text)` shows a discount badge. `playPulseAnim()` triggers the `timer_pulse_anim_72` symbol. Overrides `owned` setter to hide the purchase panels when the pack is already owned.

---

## Class: `ClassSelectTile`

Extends `ProductTile`. Class-selection tile for the carousel view. Adds `levelText:TextField`, `powerRankText:TextField`, `powerRankIcon:MovieClip`, `subClassLabel:TextField`, `subClassName:TextField`, `ClassIcon:ArtClip`, and `_level:int`/`_powerRank:int` sortable fields. `setClassData(level, powerRank, shieldFrame, subClassName, classIcon)` populates all these fields and sets the shield frame on `powerRankIcon`.

---

## Class: `PatronInfoTile`

Extends `_kiwi.Core.UIComponent`. Left-panel bonus-list display for the Patron tab. Declares a `bonuses:Array` of 12 localisation keys:

`$Store_Patron_LevelFaster`, `$Store_Patron_ExtraXP`, `$Store_Patron_DropRate`, `$Store_Patron_MagicFind`, `$Store_Patron_Chaos`, `$Store_Patron_StylePoints`, `$Store_Patron_MasteryCoin`, `$Store_Patron_FluxBonus`, `$Store_Patron_LootChest`, `$Store_Patron_DungeonBonus`, `$Store_Patron_SeedBonus`, `$Store_Patron_GemKarma`.

On the frame after construction, for non-EN locales, repositions `bonusIcon/bonusFrame/bonusHeader/bonusDetails` child clips into a 3-column grid, hiding bonus entries not present in `_validPatronBonuses`.

---

## Class: `ZHPatronView`

Extends `_kiwi.Core.UIComponent`. Chinese-locale-only patron VIP tier selector. Holds three VIP tier MovieClips (`zhPatronVIP0`, `zhPatronVIP1`, `zhPatronVIP2`), a `Dictionary` keyed by product code, and a `Vector.<Array>` of tier product lists. `addProductPrice(prodCode, tierIndex, priceStr, currencyId)` routes items to the correct VIP tier slot via `populatePatronItem`. `showPatronPopup(tierIndex)` fires a `DataEvent(StoreBase.TOGGLE_LIGHTBOX)` carrying a `ZHPatronPopup` as the child.

---

## Class: `ZHPatronPopup`

Extends `_kiwi.Core.UIComponent`. Chinese-locale patron purchase popup (lightbox child). Holds up to 3 `patronItem` MovieClips and populates them via `ZHPatronView.populatePatronItem`. `handleMouseClick(e:MouseEvent)` calls `ExternalInterface.call("STORE.PURCHASE", prodCode, currencyId)` when a `LabelButton` inside an item is clicked. Has 3 frame scripts (frames 1, 11, 21 — all `stop()`).

---

## Class: `LootProbability`

Extends `_kiwi.Core.UIComponent`. Side panel displaying drop-rate probability text for loot boxes. Fields: `headerTF:TextField`, `messageTF:TextField`, `scrollPrompt:MovieClip`. `populateLootText(title, body)` sets the two text fields. `scrollTooltipText(delta)` scrolls `messageTF.scrollV`. `setTooltipScrollPromptState(visible)` shows/hides the scroll prompt indicator.

---

## Class: `PaymentMethodsPage`

Extends `MovieClip` (dynamic). Payment method selection sub-page. Holds `accountMgmtButton`, `cancelButton`, `acceptButton`, and a `KiwiComboBox` (`paymentMethod`). The combo box data provider is initialised empty; the engine populates it via ExternalInterface. Minimal logic beyond component property setup.

---

## `KiwiStore_fla` timeline symbol classes (16 classes)

These are `_fla`-package symbol classes linked to Flash timeline symbols. Key ones:

- `BlockingStatus_36` — loading/status overlay with `statusMessage` child (`statusMessage_37`).
- `statusMessage_37` — `textField:TextField` with PC and Console frames.
- `CarouselTooltip_27` — tooltip panel: `tooltipTitle`, `tooltipText`, `scrollPrompt`.
- `buttonLegend_48` — console button-legend strip; 8 labelled frames: `Store`, `StoreNoTabs`, `StoreClasses`, `Classes`, `Buy`, `TutorialClassSelect`, `Generic`, `Console`. Each frame shows a different set of button-icon + label combinations.
- `currencyIcon_88` — 2-frame clip (frame 1: Credits/TWC icon; frame 2: Cubits/TWP icon).
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
- **Console carousel:** The carousel is exclusively activated for categories whose query string contains `"PLAYERCLASS"`. The centred tile is promoted in depth and other tiles are dimmed. Scroll uses a `Timer`-driven eased lerp over 400ms; on completion a 1000ms fade-in plays on the new centred tile.
- **Multi-currency buy buttons:** Each `ProductTile` supports two `PurchasePanel` slots for different currencies (e.g., TWC and real money). `updateBuyButtons(credits, points)` greys out panels the player cannot afford.
- **ZH locale path:** Detected globally via `_locale == LOCALE_ZH`; the entire Patron tab is replaced by `ZHPatronView` + `ZHPatronPopup` instead of `PatronTile`.
- **Standalone/dev mode:** When `IggyFunctions.inIggy` is false, `configUI` seeds dummy categories and starter pack tiles for layout previewing.
