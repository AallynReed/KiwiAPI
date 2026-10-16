# npcstore.swf
> The NPC vendor shop window shown when a player interacts with an in-game merchant. Displays a paginated grid of purchasable products with currency balance, deal timers, and purchase confirmation flow, supporting both mouse and controller input.

**Document/main class:** `NPCStore` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 8 (excluding button/skin stubs)

---

## Main class: `NPCStore`

Root window component. Manages the wallet display, pagination, multi-currency stacked wallet, time-remaining banner, and pending-transaction overlay. Delegates product grid management to `TileViewSimple`. Constructor sets up frame scripts (frames 1 and 11 — the latter sends `productsView` to its Console frame), initialises the `StackList`-based wallet and registers `ExternalInterface` callbacks in `configUI()`.

### Public methods

- `setBalance(balance:int, currencyIcon:String, currencyName:String, currencyDesc:String) : void` — updates `wallet.priceDisplay.price` text with digit-delimited balance, loads the currency icon into an `ObjectPreview` (35×35 for cubit/credit, 40×40 for others), hides legacy `creditBalance`/`cubitBalance` MCs, stores name/desc for tooltip, propagates balance to `productsView.setBalance`.
- `showPurchaseApproved(success:Boolean) : void` — if true, plays `wallet.gotoAndPlay("shine")` and fires sound event `Play_ui_shop_purchase_success` via `ExternalInterface.call("POST_SOUND_EVENT", ...)`. It carries only the flag, not which product was bought; the engine sends it right after refreshing the shown products.
- `ResetInternalWallet() : *` — clears the `walletStackList` and hides the "show all currencies" icon. The engine sends it, followed by one `AddInternalWallet` per currency, in answer to `OnUpdateWalletBalanceDisplay`, i.e. again on every tile hover, carrying the hovered product's currencies.
- `AddInternalWallet(balance:int, textureName:String) : *` — appends a `WalletInternal` row to the stacked wallet, adjusts the list's Y position upward and resizes `walletStackBackground` to fit all rows.
- `ToggleInternalWalletDisplay() : *` — toggles stacked wallet visibility (used by controller input).
- `toggleCurrencyTooltip() : *` — toggles the positioned name/description tooltip for the wallet currency icon; mutually exclusive with the stacked wallet panel.
- `setPendingTransaction(message:String, visible:Boolean) : *` — shows/hides `displayProgress` overlay with a status message (e.g. "Processing...").

### Key fields

- `productsView : TileViewSimple` — the scrollable 4-column product tile grid.
- `wallet : MovieClip` — wallet bar containing `priceDisplay`, `currencyIcon`, `creditBalance`, `cubitBalance` sub-clips.
- `walletStackList : StackList` — vertical stack of `WalletInternal` rows for multi-currency display.
- `walletStackBackground : MovieClip` — background panel that resizes to match stacked wallet height.
- `currentBalance_image : ObjectPreview` — 40×40 (or 35×35) icon shown in the wallet.
- `header : WindowHeader` — window title, set to translate key `$NPCStore_WinTitle`.
- `prevPage / nextPage : BaseButton` — pagination arrows; click calls `ExternalInterface.call("ChangePage", ±1)`.
- `m_pageLabel : TextField` — page indicator. Nothing in the SWF writes it; the engine sets its `text` to `"{page} / {count}"` by property when the store opens and on every page change.
- `firstPage / lastPage : MovieClip` — jump-to-first/last buttons; labels `$Store_First` / `$Store_Last`; call `ExternalInterface.call("ChangePageExtreme", 0/1)`.
- `displayProgress : MovieClip` — blocking spinner overlay with `statusMessage` child.
- `timeLeft : MovieClip` — time-remaining banner, hidden until `SetTimeRemaining` is called; child `timeRemainingTxt` label is `$NPCSTORE_TIME_REMAINING`.
- `m_currencyName / m_currencyDesc : String` — stored currency tooltip strings.
- `m_tooltipVisible : Boolean` — tracks whether the currency tooltip is currently shown.

### Frame scripts / timeline

- Frame 1: `stop()` — default PC layout.
- Frame 11: `stop()` then `productsView.gotoAndPlay("Console")` — console layout variant.
- Stage 1034×805 at 30 fps.

### Runtime dependencies & integration

- `ExternalInterface.addCallback` registrations: `setBalance`, `showPurchaseApproved`, `productsView.UpdateProductCount`, `toggleCurrencyTooltip`, `setPendingTransaction`, `ResetInternalWallet`, `AddInternalWallet`, `ToggleInternalWalletDisplay`, `timeremaining`.
- Outbound `ExternalInterface.call`: `ChangePage(±1)`, `ChangePageExtreme(0/1)`, `TOOLTIP.SHOW(x, y, name, desc)`, `TOOLTIP.HIDE()`, `POST_SOUND_EVENT("Play_ui_shop_purchase_success")`.
- `IggyFunctions.translate`: `$NPCSTORE_TIME_REMAINING`.
- `KiwiTextUtil.addDigitDelimiters` — formats balance numbers.
- `KiwiTextUtil.resizeFont` — auto-shrinks font on first/last page buttons and time-remaining text.
- translate keys: `$Store_First`, `$Store_Last`, `$NPCStore_WinTitle`, `$NPCSTORE_TIME_REMAINING`, `$Store_Buy`, `$Store_Unlock`.

---

## Other game-specific classes

- `TileViewSimple` (extends `_kiwi.Controls.ScrollableTileView`, embeds `symbol161`) — 4-column scrollable tile grid. Constants: `TILES_PER_ROW=4`, `ROWS_BEFORE_SCROLL=2`, `SCROLL_TILE_PEEK=0.33`. Manages product list via `addProduct`/`updateProduct`/`setPrice`/`setCanBuy`. Highlights tiles with a gold `GlowFilter`. On console shows a `consoleTooltip` after 500ms on highlight. Calls out `OnPurchase(index, currencyId, productName)`, `OnConfigured(4, count)` (from `UpdateProductCount`), `OnUpdateWalletBalanceDisplay(index)`, `OnUpdateHoveredTile(index, 1/0)` on tile roll-over/out; the engine answers a hover by sending `highlightTile`.
  - **Two index spaces.** Every inbound product callback (`updateProduct`, `setPrice`, `setCanBuy`, `UpdateDealExpiration`, `ApplySaleSticker`, `highlightTile`, `unhighlightTile`, `purchaseItem`) addresses `itemList[n]`, the tile's list position in `addProduct` call order. Every outbound call sends the tile's `index`, the first argument `addProduct` was given.
  - **`setPrice(n, currencyName, currencyDesc, currencyId, price)`** calls `addPrice(price, currencyId, currencyName, currencyDesc)`: the fourth argument is the currency and the fifth the amount. `currencyId` is the currency's texture path, the same string its balance arrives with.
  - `addProduct(index, name, info, qty, icon, restock, failure)` ignores its seventh argument. `updateProduct` (same arguments, by list position) uses it: a non-empty failure text hides the art, quantity, info and restock buttons and shows the text in `hiddenFailureText`, and the next `setCanBuy` then shows the locked display unless the status is owned.
- `ProductTileSimple` (extends `_kiwi.Core.UIComponent`, embeds `symbol62`) — single store item tile. Holds a 175×175 `ObjectPreview` for item art, 40×40 icon for currency, `PurchasePanel`, quantity text, info/restock popup buttons. Buy states (`setCanBuy(status, text)`): `CANBUY=0`, `CANTBUY_LOCKED=1`, `CANTBUY_OWNED=2`, `CANTBUY_WILLRESET=3` (shows `text` as the note), `CANTBUY_OTHER=4` (any other value leaves the button visible and disabled). Has a `CountdownTimer` for deal expiry and `pulseAnimTimer`.
- `PurchasePanel` (embeds `symbol29`) — child of `ProductTileSimple`. Contains `purchaseButton` (`btnGreenIcon_small`, default label `$Store_Buy`), `ownedTextDisplay`, `lockedTextDisplay`, `resetTextDisplay`, `currencyIcon`, `creditIcon`, `points`.
- `WalletInternal` (extends `UIComponent`, embeds `symbol15`) — one row in the stacked multi-currency wallet. Constructor takes `(balance:int, textureName:String)`; same 35/40px icon sizing logic as `NPCStore`. 20-frame animation.
- `dummy` — embeds `/_assets/1_dummy.png` (48×48) as a `BitmapData` placeholder.
- Button stubs (6): `BtnGreen` (symbol72), `btnGreen_small` (symbol82), `btnGreenIcon_small` (symbol25), `btnPageExtreme` (symbol85), `btnArrowLeft` (symbol94), `btnArrowRight` (symbol103) — all extend `LabelButton` or `BaseButton`, 4-state frame stops only.
- Skin stubs (8): scroll arrow and thumb skins, `focusRectSkin` — bare `MovieClip` embeds.
- `NPCStore_fla` timeline clips (8): `BlockingStatus_26` (spinner overlay with Console frame), `InfoPopup_57` (2-frame popup text), `btn_console_LT/RT` (3-state controller trigger buttons), `walletPriceDisplay_31`, `timer_pulse_anim_47`, `btnInfo2_56`, `btnRestock2_58`.

---

## Notable logic

- **Multi-currency stacked wallet:** `AddInternalWallet` appends rows upward (negative Y shift per row) and expands the background panel height. Mouse-over/out or `ToggleInternalWalletDisplay` shows/hides the stack. If a currency tooltip is open, toggling the wallet hides it, and vice versa.
- **Currency icon sizing:** cubit.png and credit.png get a 35×35 `ObjectPreview`; all other currencies use 40×40. This sizing logic is duplicated in both `NPCStore.setBalance` and `WalletInternal`.
- **Tooltip positioning:** `mouseOverCurrency` resolves the correct source `DisplayObject` (wallet icon vs. per-product icon), calls `localToGlobal` to get stage coordinates, then fires `TOOLTIP.SHOW` with a 5px extra offset for Cubits/Credits.
- **Purchase sound:** `showPurchaseApproved` fires `POST_SOUND_EVENT("Play_ui_shop_purchase_success")` only when `param1` is true.
- **Page navigation:** pagination buttons call `ChangePage(±1)` or `ChangePageExtreme(0/1)`; both go to the engine's page setter, which does nothing when the page would not change or does not exist. The page label (`m_pageLabel`) is written by the engine. No inbound callback reports a page or page count.
- **What the engine sends:** a rebuild (on open, page change and store events) is `clearProducts`, then `addProduct` + `setPrice` per product, then a refresh, then `productsView.UpdateProductCount`. A refresh is `updateProduct`, `setPrice`, `setCanBuy` and sometimes `UpdateDealExpiration` per shown product, with `setCanBuy` always following `updateProduct` for the same tile; it runs on a purchase result (followed by `showPurchaseApproved`), on the purchase confirm dialog's OK (straight after the purchase is sent, before the server answers) and when a watched inventory item changes. The buy status is worked out from the engine's state at that moment and nothing re-sends it when other state lands later, so a club fixture bought just before the club update arrives can stay buyable until the window is reopened.
- **One listing per currency:** some merchants list the same item once per currency they accept (the Delve merchant sells for cubits and for flux), and the grid draws each listing as a separate tile.
- **Test/preview mode:** when not in Iggy, `configUI` inserts 12 dummy products directly into `productsView`.
