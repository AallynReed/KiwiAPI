# collections.swf
> The Collections window, used to browse and equip cosmetic collections (mounts, sails, styles, etc.) organized into named tabs and collapsible categories. Each category shows a grid of `SlotBasic` items that may be equipped, locked, or interactive depending on the collection type.

**Document/main class:** `Collections` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 4

---

## Main class: `Collections`

`Collections` is the root display object for the Collections window. It extends `UIComponent` and manages a dynamic set of tab headers and expandable/collapsible category bags. Tab instances (`TabHeader`) are added at runtime and laid out in rows of three. Categories are managed through a vertically scrolling `RowView` that contains `BagContainerBasic` instances. In the Iggy runtime, ~18 `ExternalInterface` callbacks are registered. Outside Iggy, a small test fixture adds 12 collections and 5 categories with 10 items each.

### Public methods

- `addCollection(label:String, identifier:String, draggable:Boolean) : void` — Creates a `TabHeader`, positions it in the next available tab slot (3 per row, anchored by `tabAnchor0`-`tabAnchor3`), and pushes it onto `collectionTabs`. Console layout uses reversed anchor order. Selects the first tab added.
- `addCategory(label:String, id:int, totalSlots:int, equippedSlotId:int) : int` — Creates a `BagContainerBasic` with 5 columns, appends it to the `RowView`, and stores it in `categoryBags[id]`. First category is auto-expanded on PC; all others start collapsed.
- `setCategory(id:int) : void` — Expands the category matching `id` and collapses all others, then calls `refreshLayout()`.
- `setCategoryItem(categoryId, slotId, locked, iconImage, frameType) : void` — Populates a specific `SlotBasic` inside a category bag; sets `locked`, `iconImage`, `frameType`, and `clickFeedback` (enabled only if the collection allows dragging and the slot is unlocked). Attaches `ROLL_OVER`/`ROLL_OUT` listeners for tooltip calls.
- `toggleHeaders(event) : void` — Called when a category heading is clicked; calls `setCategory` and fires `COLLECTIONS.SELECT_CATEGORY` to the game.
- `onTabClick(event:MouseEvent) : void` — Switches the active collection tab; fires `COLLECTIONS.SHOW_COLLECTION` with the numeric index and string identifier.
- `onCategoryHeadingClick(bag:BagContainerBasic) : void` — Fires a sound event and refreshes layout after a category is toggled.
- `refreshLayout() : void` — Re-stacks all `BagContainerBasic` items in the `RowView` and synchronizes tab frame states (`active`/`inactive`).
- `clearCategories() : void` — Removes all category bags from the `RowView` and clears `categoryBags`.
- `resetEquipped(categoryId:uint) : void` — Calls `unequipAllSlots()` on the category matching the given id.

### Key fields

- `categoryView : RowView` — Vertically scrollable container holding all `BagContainerBasic` category widgets.
- `winHeader : WindowHeader` — Window title bar.
- `collectionTabs : Array` — Ordered list of `TabHeader` instances (one per collection).
- `collectionDrag : Array` — Parallel boolean array to `collectionTabs`; tracks whether drag is allowed for each collection's items.
- `categoryBags : Dictionary` — Maps category id → `BagContainerBasic`.
- `addedCategories : int` — Count of categories added to the current collection view; used to decide collapsed state of new categories.
- `currentCategory : int = -1` — Id of the currently expanded category.
- `selectedTab : MovieClip` — Currently active `TabHeader`.
- `tabAnchor0`-`tabAnchor3 : MovieClip` — Positional anchors for tab row origins; on Console, row order is reversed (anchor3 first).
- `TABS_PER_ROW : Number = 3` / `SLOTS_PER_ROW : int = 5` — Layout constants reported to the game via `GET_CONFIGURED_DATA`.

### Runtime dependencies & integration

**ExternalInterface callbacks registered (game → Flash):**
`CLEAR_CATEGORIES`, `ADD_COLLECTION`, `ADD_CATEGORY`, `SET_COLLECTION`, `SET_CATEGORY`, `SET_WIN_TITLE`, `SET_EQUIPPED`, `SET_LOCKED`, `SET_CATEGORY_ITEM`, `REFRESH`, `GET_CONFIGURED_DATA`, `HIGHLIGHT_TAB`, `UNHIGHLIGHT_TAB`, `HIGHLIGHT_CATEGORY`, `UNHIGHLIGHT_CATEGORY`, `SWITCH_CATEGORY`, `COLLAPSE_CATEGORY`, `HIGHLIGHT_SLOT`, `UNHIGHLIGHT_SLOT`, `ACTIVATE_SLOT`

**ExternalInterface calls made (Flash → game):**
`COLLECTIONS.CONFIGURED` (tab count, tabs per row, slots per row), `COLLECTIONS.SHOW_COLLECTION` (index, identifier), `COLLECTIONS.SELECT_CATEGORY` (id), `COLLECTIONS.POINTER_ENTER` (slotData, globalX, globalY adjusted for scroll), `COLLECTIONS.POINTER_LEAVE` (slotData), `POST_SOUND_EVENT` (`Play_ui_window_tab`, `Play_ui_window_header`)

**Highlight effects:** `highlightTab` and `highlightCategory` apply a `GlowFilter` (gold, `color 0xCCBB00`, inner, strength 100) to the tab or category heading; `unhighlightTab`/`unhighlightCategory` clear the filter array.

**Scroll-to on highlight:** `setScrollLocation` computes the total content height, derives a fractional scroll position, and sets `categoryView.scrollV` to bring highlighted items into view.

**Events:** `MouseEvent.CLICK` on tab headers; `"CATEGORY_TOGGLED"` custom event dispatched by `BagContainerBasic`; `ROLL_OVER`/`ROLL_OUT` on `SlotBasic` items for tooltip calls.

**Platform guard:** `IsConsole()` reverses tab anchor order.

---

## Other game-specific classes

- `RowView` — Embed `symbol122` (extends `_kiwi.Controls.ScrollableView`, implements `IEventDispatcher`). Vertically stacked layout container for `BagContainerBasic` rows. Manages item y-positions with `WINDOW_PADDING_Y = 8` and calls `SetContentSize` to update the scrollable area. `refreshLayout()` re-stacks all items and auto-scrolls to the last expanded bag.
- `InteractPanel` — Embed `symbol97`. Bare `MovieClip` asset wrapper (no logic); likely a template or interaction overlay placeholder.
- `Equipped` — Embed `symbol100`. Bare `MovieClip` asset wrapper; used as an equipped-state indicator symbol on slot items.

**Asset wrappers (not detailed — 9 classes):** `rarity_frame_common_png`, `rarity_frame_uncommon_png`, `rarity_frame_rare_png`, `rarity_frame_epic_png`, `rarity_frame_legendary_png`, `rarity_frame_relic_png`, `rarity_frame_radiant1_png`, `rarity_frame_resplendent_png`, `rarity_frame_shadow_png`.

**Skin classes (not detailed — 14 classes):** `ScrollArrow*`, `ScrollThumb*`, `ScrollTrack_skin`, `ScrollBar_thumbIcon`, `List_skin`, `CellRenderer_*Skin`, `focusRectSkin`.

**Other trivial symbols:** `SlotBackground`, `SlotBackgroundLocked`, `SlotFrameMedium`, `SlotFrameNormal`, `SlotFrameHigh`, `dummy`.

## Collections_fla timeline symbols (2 classes)

- `bannerBottom_5` — Embed `symbol135`. Decorative bottom banner; stops at frame 15 of its animation.
- `bannerTop_2` — Decorative top banner (no additional logic beyond the standard `MovieClip` constructor).

## Notable logic

- **Tab layout:** Tabs are positioned dynamically in rows of 3. Each new tab is placed to the right of the previous one in the same row; a new row begins when `collectionTabs.length % TABS_PER_ROW == 0`. Tabs are inserted into the display list at depth 15 (PC) or incrementally above 15 (Console) to control z-order.
- **Category accordion:** Only one category is expanded at a time. Selecting a new category calls `collapse()` on all others and then calls `refreshLayout()` to restack heights. The server can also collapse a specific category via `COLLAPSE_CATEGORY`.
- **Drag eligibility:** `collectionDrag[tabIndex]` encodes per-collection drag permission. `setCategoryItem` propagates this as `clickFeedback` on individual slots so the kiwi `SlotBasic` can respond correctly to clicks.
- **Tooltip positioning:** `COLLECTIONS.POINTER_ENTER` receives global coordinates adjusted by the current vertical scroll offset (`scrollV * verticalStep`) so the game can display the tooltip at the correct screen position.
- **Scroll-to-highlight:** Both `highlightTab` (no scroll needed) and `highlightCategory`/`highlightSlot` call `setScrollLocation`, which sums all category heights to compute a proportional scroll ratio and sets `categoryView.scrollV` accordingly.
