# npcstore.swf
> The NPC vendor shop window shown when a player interacts with an in-game merchant. Displays a paginated grid of purchasable products with currency balance, deal timers, and purchase confirmation flow, supporting both mouse and controller input.

**Document/main class:** `NPCStore` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 8 (excluding button/skin stubs)

---

## Main class: `NPCStore`

Root window component. Manages the wallet display, pagination, multi-currency stacked wallet, time-remaining banner, and pending-transaction overlay. Delegates product grid management to `TileViewSimple`. Constructor sets up frame scripts (frames 1 and 11 — the latter sends `productsView` to its Console frame), initialises the `StackList`-based wallet and registers `ExternalInterface` callbacks in `configUI()`.

### Public methods

- `setBalance(balance:int, currencyIcon:String, currencyName:String, currencyDesc:String) : void` — updates `wallet.priceDisplay.price` text with digit-delimited balance, loads the currency icon into an `ObjectPreview` (35×35 for cubit/credit, 40×40 for others), hides legacy `creditBalance`/`cubitBalance` MCs, stores name/desc for tooltip, propagates balance to `productsView.setBalance`.
- `showPurchaseApproved(success:Boolean) : void` — if true, plays `wallet.gotoAndPlay("shine")` and fires sound event `Play_ui_shop_purchase_success` via `ExternalInterface.call("POST_SOUND_EVENT", ...)`.
- `ResetInternalWallet() : *` — clears the `walletStackList` and hides the "show all currencies" icon.
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
- `firstPage / lastPage : MovieClip` — jump-to-first/last buttons; labels `$Store_First` / `$Store_Last`; call `ExternalInterface.call("ChangePageExtreme", 0/1)`.
- `displayProgress : MovieClip` — blocking spinner overlay with `statusMessage` child.
- `timeLeft : MovieClip` — time-remaining banner, hidden until `SetTimeRemaining` is called; child `timeRemainingTxt` label is `$NPCSTORE_TIME_REMAINING`.
- `m_currencyName / m_currencyDesc : String` — stored currency tooltip strings.
- `m_tooltipVisible : Boolean` — tracks whether the currency tooltip is currently shown.

### Frame scripts / timeline

- Frame 1: `stop()` — default PC layout.
- Frame 11: `stop()` then `productsView.gotoAndPlay("Console")` — console layout variant.

### Runtime dependencies & integration

- `ExternalInterface.addCallback` registrations: `setBalance`, `showPurchaseApproved`, `productsView.UpdateProductCount`, `toggleCurrencyTooltip`, `setPendingTransaction`, `ResetInternalWallet`, `AddInternalWallet`, `ToggleInternalWalletDisplay`, `timeremaining`.
- Outbound `ExternalInterface.call`: `ChangePage(±1)`, `ChangePageExtreme(0/1)`, `TOOLTIP.SHOW(x, y, name, desc)`, `TOOLTIP.HIDE()`, `POST_SOUND_EVENT("Play_ui_shop_purchase_success")`.
- `IggyFunctions.translate`: `$NPCSTORE_TIME_REMAINING`.
- `KiwiTextUtil.addDigitDelimiters` — formats balance numbers.
- `KiwiTextUtil.resizeFont` — auto-shrinks font on first/last page buttons and time-remaining text.
- translate keys: `$Store_First`, `$Store_Last`, `$NPCStore_WinTitle`, `$NPCSTORE_TIME_REMAINING`, `$Store_Buy`, `$Store_Unlock`.

---

## Other game-specific classes

- `TileViewSimple` (extends `_kiwi.Controls.ScrollableTileView`, embeds `symbol161`) — 4-column scrollable tile grid. Constants: `TILES_PER_ROW=4`, `ROWS_BEFORE_SCROLL=2`, `SCROLL_TILE_PEEK=0.33`. Manages product list via `addProduct`/`updateProduct`/`setPrice`/`setCanBuy`. Highlights tiles with a gold `GlowFilter`. On console shows a `consoleTooltip` after 500ms on highlight. Calls out `OnPurchase(index, currencyId, productName)`, `OnConfigured(4, count)`, `OnUpdateWalletBalanceDisplay(index)`, `OnUpdateHoveredTile(index, 1)`.
- `ProductTileSimple` (extends `_kiwi.Core.UIComponent`, embeds `symbol62`) — single store item tile. Holds a 175×175 `ObjectPreview` for item art, 40×40 icon for currency, `PurchasePanel`, quantity text, info/restock popup buttons. Buy states: `CANBUY=0`, `CANTBUY_LOCKED=1`, `CANTBUY_OWNED=2`, `CANTBUY_WILLRESET=3`, `CANTBUY_OTHER=4`. Has a `CountdownTimer` for deal expiry and `pulseAnimTimer`.
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
- **Page navigation:** pagination buttons call `ChangePage(±1)` or `ChangePageExtreme(0/1)`; page label (`m_pageLabel`) and the prev/next/first/last button states are controlled from the game engine side.
- **Test/preview mode:** when not in Iggy, `configUI` inserts 12 dummy products directly into `productsView`.
