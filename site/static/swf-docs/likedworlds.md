# likedworlds.swf
> The Liked Worlds panel in Trove that displays the player's saved/liked club worlds in a scrollable list, allowing them to teleport to a world or remove it from their favourites. It appears in a small window and supports both PC and console layouts.

**Document/main class:** `LikedWorlds` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 4 (plus ~12 asset-wrapper/skin classes)

---

## Main class: `LikedWorlds`

`LikedWorlds` is the root UIComponent. The constructor:
1. Creates a `WorldsViewDataSource` instance and assigns it to `worldsView.dataSource`.
2. Listens on `worldsView` for custom `"ShowRemoveOption"` and `"HideButtons"` events.
3. In non-Iggy mode, populates 4 stub world entries for preview.
4. On console, hides itself and starts an `ENTER_FRAME` loop that waits for `onTargetFrame()` before calling `setupTranslation()`, showing the window, and hiding console buttons when the list is empty.
5. Calls `setupTranslation()` (inherited) and sets the header title via component inspector to `$LikedWorlds_WinTitle`.

`configUI()` sets `header.allowFontResize = true`.

### Key fields
- `worldsView : WorldsView` — the main scrollable list; backed by `dataSource`.
- `dataSource : WorldsViewDataSource` — manages the sorted array of `{worldId, worldName, clubName}` records; handles add/remove/clear with `DataChangeEvent` notifications.
- `closeButton : MovieClip` — window close button (not wired in AS; handled by framework or engine).
- `header : WindowHeaderSmall` — window title bar; title translate key `$LikedWorlds_WinTitle`; `allowFontResize = true`.
- `consoleRemoveButton : MovieClip` — console-only "Remove" action button; shown/hidden based on selection state.
- `consoleEnterButton : MovieClip` — console-only "Enter World" action button; hidden when list is empty.

### Event handlers
- `ShowRemoveButton(e:DataEvent)` — sets `consoleRemoveButton.visible = Boolean(e.data)` (true when a non-locked world is selected on console).
- `HideButtons()` — hides both console action buttons (dispatched by `WorldsView` when the item pool empties).
- `onEnterFrame()` — console initialisation gate; polls `onTargetFrame()`; once true, calls `setupTranslation()`, makes the window visible, and hides console buttons if list is empty.

### Frame scripts / timeline
- `frame1` — `stop()` (PC layout).
- `frame11` — `stop()` (console layout).

### Runtime dependencies & integration
- **IggyFunctions.inIggy** — checked indirectly via `WorldsViewDataSource` (registers ExternalInterface callbacks there).
- **translate key**: `$LikedWorlds_WinTitle` (window header), `$LikedWorlds_GotoWorld` (Enter button in `WorldListItem`).
- **IsConsole()** — gates console button visibility; also used in `WorldsViewDataSource.insertItemSorted` to choose `natCaseCompare` vs locale string comparison.
- **DataEvent** (`_kiwi.Controls.DataEvent`) — carries the `ShowRemoveOption` boolean payload.

---

## Other game-specific classes

### `WorldsViewDataSource`
Extends `_kiwi.Core.UIComponent`. Maintains `itemData:Array` of `{worldId, worldName, clubName}` objects. Registers ExternalInterface callbacks in Iggy: `clear`, `addItem`, `removeItem`.

- `addItem(worldId, worldName, clubName)` — inserts sorted by world name using `natCaseCompare` (console) or `localeCompare` (PC), dispatching `DataChangeEvent(DATA_CHANGE, ADD, [], insertIndex)`.
- `removeItem(worldId)` — finds by `worldId`; dispatches `PRE_DATA_CHANGE / REMOVE` before splicing.
- `clear()` — splices all; dispatches `REMOVE_ALL`.
- `getItemData(index) : Object`, `getItemDataCount() : int` — read accessors for the view.
- Uses `flash.utils.natCaseCompare` on console for natural-order sort.

### `WorldsView`
Embed symbol90; extends `_kiwi.Controls.ScrollableView`. Implements a virtualised, pool-based scrollable list of `WorldListItem` rows.

- Pool size: 16 items (`ITEM_POOL_SIZE`).
- On construction: creates pool of `WorldListItem` clones, sets `vertScrollbarVisible = true`, `verticalStep = 3`.
- Registers ExternalInterface callbacks `selectIndex` and `getWorldIdFromIndex` in Iggy.
- `draw()` — responds to `SCROLL` and `DATA` invalidation; calls `updateLayout()`.
- `updateLayout()` — computes `getVisibleItemRange()` (based on scroll rect and item height), returns out-of-range items to pool, repopulates visible range, relayouts item positions, updates content size.
- `getVisibleItemRange()` — returns a `Range` object covering `[scrollTop / itemHeight, scrollBottom / itemHeight]` (with one item of padding).
- `relayoutItems()` — positions items at `y = dataIndex * itemHeight`; alternates `alternateColor` on even indices.
- `repopulateItems()` — pulls `WorldListItem` from pool, calls `setData(dataSource.getItemData(i))`, inserts sorted by `dataIndex`.
- `selectIndex(index:int, isLocked:Boolean)` — console: scrolls to position, sets `isSelected` on the matching item, dispatches `"ShowRemoveOption"`.
- `getWorldIdFromIndex(index:int) : String` — returns the `worldId` at a given data index.
- `returnItemToPool(i)` — removes from display, pushes to pool; dispatches `"HideButtons"` when items list empties.
- Responds to `DataChangeEvent`: ADD → `onItemAdded`, CHANGE → `onItemUpdated`, REMOVE_ALL → `onCleared`; pre-event REMOVE → `onItemWillRemove` (clears visible items that include the removed index).

### `WorldListItem`
Embed symbol27; extends `_kiwi.Core.UIComponent`. One row in the list.

- Fields: `worldTextField`, `clubTextField` (display name/club), `enterButton` (LabelButton, label `$LikedWorlds_GotoWorld`), `deleteButton` (LabelButton, label empty), `mc_row_bg` (alternating row background), `_worldId`, `_dataIndex`, `_isSelected`, `_alternateColor`.
- `setData(obj)` — sets world/club name text fields, `worldId`, `isSelected` (calls `enterButton.validateNow()`).
- `clone()` — returns a new `WorldListItem` pre-filled with current data; used to populate the pool.
- `onEnterWorld` — `ExternalInterface.call("OnEnterWorld", worldId)`.
- `onDeleteWorld` — `ExternalInterface.call("OnDeleteWorld", worldId)`.
- `set isSelected` — on console hides/shows `enterButton` based on selection; `mc_row_bg.visible` tracks `alternateColor`.
- Frame 1 (PC) / frame 10 (console) stops.

**Asset wrappers (~12 classes):** `ScrollArrowDown_*Skin` (disabled/down/over/up), `ScrollArrowUp_*Skin` (disabled/down/over/up), `ScrollThumb_*Skin` (down/over/up), `ScrollTrack_skin`, `ScrollBar_thumbIcon`, `focusRectSkin`, `btnKick`, `btnGreen_small`.

---

## Notable logic

- **Virtualised list**: `WorldsView` uses a 16-item pool and only creates/positions items in the visible scroll range plus one item of padding, keeping memory constant regardless of list length.
- **Sorted insert**: new worlds are always inserted in alphabetical order (natural order on console, locale-uppercase on PC); the sort fires a `DataChangeEvent(ADD, index)` so the view knows exactly where to invalidate.
- **Console selection model**: `selectIndex` scrolls the scrollbar to the proportional position of the selected index before relayouting, ensuring the selected row is visible. It then iterates displayed items to apply `isSelected`, and dispatches `ShowRemoveOption` if the world is not locked.
- **ExternalInterface calls** (outgoing): `OnEnterWorld(worldId)`, `OnDeleteWorld(worldId)` from `WorldListItem`; `selectIndex` and `getWorldIdFromIndex` are incoming callbacks registered on `WorldsView`.
- **DataChangeEvent chain**: `WorldsViewDataSource` dispatches standard FL DataChangeEvents; `WorldsView` listens and either partially or fully clears/repopulates the pool depending on change type.
