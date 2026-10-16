# collectionsnew.swf

> The Collections window, the only collections screen the client loads: six meta-categories (Badges, Combat, Crafting, FavsNStuff, Styles, Travel) of up to twelve categories each, and a lazily loaded sectioned list of collectables with search, a collected/uncollected/favourites filter, favourite reordering, marketplace links and console D-pad navigation.

**Document/main class:** `CollectionsNew` (extends `UIComponent`)
**SWF-specific classes:** 10 (plus 17 `CollectionsNew_fla/` timeline symbols and numerous asset/skin wrappers)

---

## Main class: `CollectionsNew`

Manages the full collections browsing experience. On construction it sets up the `DynamicRowView` scroll step (20 px), initializes console nav-mode state, and registers a `__setProp` call to set the window header title (`$Collections`). `configUI()` wires all click listeners for meta-category and category slots, sets up the breadcrumb/search-header event chain, instantiates `CollectableRowExternalDataSource` as the row view's data source with a `CollectableRow` template, calls `setupTranslation()`, and registers all `ExternalInterface` callbacks.

The window has three navigation layers: **meta-categories** (6 icon buttons, 3 per row), **categories** (up to 12 `SlotBasic` slots, 4 per row), and a **scrollable row view** showing `CollectableRow` entries grouped into collapsible sections. Console platforms use a four-level D-pad nav mode (`NavModeMetaCategories → NavModeCategories → NavModeSections → NavModeRows`) with highlight glow filters and NX-specific `highlight` child clips.

Frame scripts stop at frames 1, 11, 21. Frame 11 plays the Console animation on `searchHeader` and `metaCategories`. Frame 21 sets `buttonLegend` to `"ConsoleLoc"`.

### Public methods

- `clearSearchFocus(e:MouseEvent) : void` — clears stage focus when clicking outside the search box.
- `onBreadCrumbClicked(e:DataEvent) : void` — handles breadcrumb navigation; calls `SetSearchString("")` / `SetMetacategory(-1)` / `SetCategory(-1)` via `ExternalInterface`.
- `onSearchTextEntered(e:DataEvent) : void` — shows/hides meta-category, category, and row views based on search state.
- `onCollapseAll(e:DataEvent) : void` — calls `rowView.collapseAllSections()` or `expandAllSections()`.
- `onMetaCategoryClick(e:MouseEvent) : void` — toggles the active meta-category; calls `ExternalInterface.call("SetMetacategory", idx)`.
- `onCategoryClick(e:MouseEvent) : void` — selects a category; calls `ExternalInterface.call("SetCategory", idx)`. `idx` is the slot index (0–11) within the current meta-category, not a global category id.
- `setCategory(idx:int) : void` — hides meta/category panels, shows row view, clears it, sets breadcrumb level 2. `-1` is a real value: the engine uses it for the whole meta-category, and the breadcrumb is then left at level 1.
- `setSection(idx:int) : void` — scrolls to section or activates console nav.
- `setRow(section:int, row:int) : void` — scrolls to row position with multi-row offset support.
- `setSearchString(s:String) : void` — proxies to `searchHeader.searchString`.
- `getCurrentNavMode() : int` — returns current console nav mode constant.

### Key fields

- `winHeader : MovieClip` — window title bar, title set to `$Collections`.
- `searchHeader : CollectionsSearchHeader` — search box + filter combo + breadcrumbs + collapse button.
- `metaCategories : MovieClip` — container with `meta0`–`meta5` buttons (icons loaded by class name `metaIcon0`–`metaIcon5` via `getDefinitionByName`). Labels are `$MetaCategory0`–`$MetaCategory5`: Badges, Combat, Crafting, FavsNStuff, Styles, Travel in English. FavsNStuff only repeats collectables that also appear under one of the other five.
- `categories : MovieClip` — container with `category0`–`category11` `SlotBasic` slots.
- `rowView : DynamicRowView` — virtualized scrolling list of `CollectableRow` rows grouped into sections.
- `backBanner : MovieClip` — decorative back banner; height scaled on `onStageResized`.
- `buttonLegend : MovieClip` (`buttonLegend_65`) — console button legend with `primaryAction`, `secondaryAction`, `marketplace`, `favorite_swap`, `switchScreens` children; frame labels `RowNoAction`, `RowSingleAction`, `RowTwoAction`, `Navigation` (and `*Loc` variants for non-English).
- `dataSource : CollectableRowExternalDataSource` — data bridge to the game engine. Private, but the engine reaches its two buffers by path from the document class (`dataSource.rowData`, `dataSource.updateSectionData`; both strings are in the client), so the property name and the buffer field names are part of the contract.
- `currentNavMode : int` — one of `NavModeMetaCategories(0)`, `NavModeCategories(1)`, `NavModeSections(2)`, `NavModeRows(3)`.
- `allCategoryData : Array` — 6-element array of arrays; each entry holds category tuples `[name, description, icon, breadcrumbLabel]`, filled by `addCategory(meta, index, name, description, icon, label)`. The engine sends one `addCategory` per category, for all six meta-categories, as soon as the window opens and before anything else, so the screen never has to ask what exists.
- `highlightedMetaCategory/Category/Section/Row : int` — console cursor positions; special sentinels `SearchBoxIndex(-2)` and `DropdownIndex(-1)`.
- `_buttonLegendTargetFrame : String` — deferred frame label for button legend, set on `ENTER_FRAME` callback.
- `_showMarketplaceButtonLegend / _showSwitchScreensButtonLegend / _showSwapFavoriteButtonLegend : Boolean` — visibility flags for optional console button legend entries.

### Frame scripts / timeline

| Frame | Label | Action |
|-------|-------|--------|
| 0 | PC | `stop()` |
| 10 | Console | `stop()`, plays `searchHeader` and `metaCategories` Console animations |
| 20 | ConsoleLoc | `stop()`, sets `buttonLegend` to `"ConsoleLoc"` (a label `buttonLegend` itself does not have; its labels are listed above) |

Stage 391×768, 24 fps, 30 frames.

### Runtime dependencies & integration

**ExternalInterface callbacks registered (inIggy):**
`addCategory`, `setEquipped`, `setLocked`, `isLocked`, `isVisible`, `setMetaCategory`, `setCategory`, `setSection`, `setRow`, `setCurrentMetaCategory`, `setSearchString`, `UIComponent.onStageResized`, `activateSelection`, `previousNavMode`, `consoleInputX`, `toggleFavorite`, `revertFavorite`, `moveHighlightHorizontal`, `moveHighlightVertical`, `highlightCurrentSelection`, `unhighlightCurrentSelection`, `setButtonLegendState`, `toggleSwitchScreensButtonLegend`, `swapRows`, `swapFavoriteUp`, `swapFavoriteDown`, `sectionsUpdated`, `consoleClearSearch`, `getCurrentNavMode`

**ExternalInterface calls (outbound):**
`SetMetacategory`, `SetCategory`, `SetSearchString`, `SetCollectedState`, `SetFavorite`, `ActivateSearchInput`, `CloseScreen`, `SLOT.ACTIVATE` (from `Slot` on click), `SLOT.POINTER_ENTER`, `SLOT.POINTER_LEAVE`, `TOOLTIP.SHOW`, `TOOLTIP.HIDE`, `OnSlotEnter`, `OnSlotLeave`, `SwapFavoritePositions`, `OpenUrl`. (`PositioningButtons`, used on NX for the button legend, is a `UIComponent` static helper, not an outbound call.)

**How the list is fed:** the screen pulls its contents; sections and rows only ever arrive in answer to a request. `GetSectionData` is answered with one `setSectionData(index, numRows, name, infoText, collapsed)` per section (appended in call order); `name` is usually a `$CollectionName_*` key and `infoText` is the section's collected/total count, already formatted by the engine with `$Collections_NumCollected` (`{0}/{1}`). `GetRowData(section, row)` is answered synchronously: the engine writes the `CollectableRowData` fields into `dataSource.rowData` by name before the call returns. `UpdateSectionData(section)` fills `dataSource.updateSectionData` (`numRows`, `sectionName`, `sectionInfoText`) the same way. Rows are only requested as `DynamicRowView` is about to draw them.

**Engine behaviour:** the engine echoes every outbound `SetMetacategory` / `SetCategory` back through `setMetaCategory` / `setCategory` and then calls `setDataDirty`, which makes the row view ask for its sections again. Opened normally, the engine starts on meta-category `-1`, category `-1` (the home page). When another window opens Collections on a particular collection (the character sheet's equipment slots, the marketplace), the engine pushes that state in on the first frame, right after the `addCategory` burst: `setMetaCategory`, `setCategory`, `setSearchString` with a term that narrows the list, and `setCollectedState(3)` (Show Favorites). The character sheet's mount slot, for example, arrives as meta-category 5 (Travel) with category `-1`. Without being asked it also sends `setEquipped`, `setLocked` (with the row's new icon), `revertFavorite`, `swapRows`, `updateCurrentView` and `setDataDirty`.

**Slots:** a row's slot `data` is `(sectionIndex << 16) | rowIndex`. `SLOT.ACTIVATE` with that number equips the collectable and `SetFavorite(section, row, on)` stars it; both are read against whatever filter the engine currently holds. Hovering a row's slot sends `SLOT.POINTER_ENTER` (from `Slot`) and then `OnSlotEnter(data, x, y, scrollOffsetY)` (from `CollectableRow`); the item tooltip comes from `OnSlotEnter`, since `tooltipName` is never set on a collectable slot.

**Client:** `Trove_x64.exe` names this file `CollectionsNew.swf` and names no other collections SWF; the older `collections.swf` is not loaded.

**translate() keys:** `$Collections` (no entry in any shipped language file), `$Collections_Home`, `$MetaCategory0`–`$MetaCategory5`, `$Collections_ShowAll`, `$Collections_HideCollected`, `$Collections_HideUncollected`, `$Collections_ShowFavorites`, `$Select_ButtonLegend`

**Events listened:** `MouseEvent.CLICK` (stage-wide, meta/category clicks), `CollectionsSearchHeader.EVENT_BREADCRUMB`, `EVENT_SEARCHTEXT`, `EVENT_COLLAPSEALL`, `Event.ENTER_FRAME` (console init + button legend deferred update)

**Highlight:** Uses `GlowFilter` (inner glow, color `0xCCCC00`, strength 100) applied to the currently highlighted display object; NX uses an additional `highlight` child `visible` flag on each row/section clip.

---

## Other game-specific classes

- `CollectionsSearchHeader` — `UIComponent` (embedded symbol 278). Composite search/filter/breadcrumb bar. Owns `searchText` (TextField), `filterComboBox` (KiwiComboBox with ShowAll/HideCollected/HideUncollected/ShowFavorites options), `btnClearText`, `btnCollapse`, `breadcrumbs` (3-button MC). Dispatches `DataEvent` with types `BreadCrumbClicked`, `SearchTextPopulated`, `CollapseAll`. Registers Iggy callbacks `setCollectedState`, `setSearchString`. Calls `ExternalInterface.call("SetSearchString")` on every change to the search text (on console only when Enter is pressed) and `ExternalInterface.call("SetCollectedState", index)` with the dropdown index: 0 Show All, 1 Hide Collected, 2 Hide Uncollected, 3 Show Favorites. The engine's `setCollectedState(n)` selects the same index. Frame 11 plays Console breadcrumb animation.
- `CollectableRow` — `DynamicRowViewRow` (embedded symbol 181). One row in the list. Shows item icon (`Slot`), name, type, mastery XP amounts for Trove and Geode, favorite checkbox (`Checkbox`), sleeping icon, marketplace button, and up/down swap arrows for favorite reordering. Calls `ExternalInterface.call("SetFavorite")`, `SwapFavoritePositions`, `OnSlotEnter`, `OnSlotLeave`, `OpenUrl`. Locked items display in muted color (`0xB9B9B9`). Frame labels `unlocked`/`locked` (and `*Console` variants); `setData` always goes to `locked` / `lockedConsole` and shows the lock state by toggling `backgroundLocked` / `backgroundUnlocked`. The swap arrows are shown only when `isCategoryFavorite` is set. `storeUrl` is stored, but `onStoreButtonClicked` is never attached to anything (the row has no store button), so only the marketplace link can be opened.
- `CollectableRowData` — Plain VO class holding fields: `name`, `type`, `metaExperience`, `geodeMetaExperience`, `icon`, `locked`, `equipped`, `favorite`, `sleeping`, `isCategoryFavorite`, `marketplaceUrl`, `storeUrl`. `clear()` resets all to defaults. The engine writes these by name: `metaExperience` / `geodeMetaExperience` are the Trove and Geode mastery points the collectable is worth, `locked` means not collected, `equipped` means worn or active, `sleeping` drives the row's sleeping icon (a tome whose charge is used up reports it), and `isCategoryFavorite` marks a row in an ordered favourites section.
- `CollectableRowExternalDataSource` — Extends `DynamicRowViewExternalDataSource`. Overrides `getData(section, row)` to call `super.getData()` (`GetRowData(section, row)`, during which the engine fills `rowData`), then calls `rowView.setRowData()` and resets `rowData`. The one buffer serves every row.

### CollectionsNew_fla/ timeline symbols (17 total)

- `bannerTop_2` — decorative top banner (symbol 333); stops at frame 15.
- `bannerBottom_5` — decorative bottom banner.
- `metaContent_29` — container MC for `meta0`–`meta5` buttons (symbol 359); frame 11 plays each child's Console animation.
- `metaButton_30` — individual meta-category button (symbol 358) with `highlight`, `iconAnchor`, `label` children; 4 frame-stop states.
- `breadcrumbs_36` — breadcrumb bar (symbol 271) with `button0`–`button2`; frame 11 plays Console animation on each.
- `breadcrumbButton_37` / `breadcrumbSecondaryButton_38` — individual breadcrumb button symbols.
- `btnCollapse_61` — collapse-all toggle button (symbol 276).
- `buttonLegend_65` — console button legend (symbol 373); 8 frame-stop states (Navigation, RowNoAction, RowSingleAction, RowTwoAction + Loc variants); children: `marketplace`, `favorite`, `switchScreens`, `primaryAction`, `secondaryAction`, `favorite_swap`.
- `ButtonLegendFavorite_70` — favorite button legend entry symbol.
- `slotFrame_77` / `equipped_79` — slot frame decoration and equipped indicator.
- `qualityPips_84` — quality pip indicator (symbol 66).
- `masteryIconSmall_90` — small mastery icon (symbol 176) with frame labels `trove`, `troveLocked`, `geode`, `geodeLocked`; used for a row's `troveMasteryIcon` and `geodeMasteryIcon` (the latter is sent to `geode`).
- `CollapsedIcon_95` — collapsed-section chevron icon.
- `subcategoryHeader_94` — section heading (symbol 121) with `highlight`, `titleText`, `infoText`, `collapsedIcon`; 3 frame-stop states (`PC`, `Console`, `ConsoleLoc`). `infoText` shows the section's collected/total count from `setSectionData`.
- `slotFrameLarge_102` — large slot frame decoration.

**Asset wrappers (not detailed):** 22 top-level skin/png classes — `rarity_frame_*_png` / `*_over_png` (10 rarity tiers × 2), `CellRenderer_*Skin` (8), `ComboBox_*Skin` (4), `ScrollArrow*_*Skin` (8), `ScrollThumb_*Skin` (3), `ScrollBar_thumbIcon`, `List_skin`, `SlotBackground`, `SlotBackgroundLocked`, `Equipped` (bitmap embed), `btn_XBOne_RB/png`, `rarity_frame_stellar` (non-png variant). Also `dummy.as`, `metaIcon0`–`metaIcon5` (6 bitmap embed classes).

---

## Notable logic

- **Four-level console navigation:** The class maintains `currentNavMode` and `highlighted*` indices. D-pad moves are routed to `moveMetaCategoryHighlight*`, `moveCategoryHighlight*`, `moveSectionHighlightVertical`, `moveRowHighlightVertical`. Wrap-around is implemented per-layer. The `DropdownIndex(-1)` and `SearchBoxIndex(-2)` sentinels represent the filter combo and search box as virtual navigation targets above the meta-category grid.
- **Highlight glow:** `highlightMovieClip` applies an inner yellow glow filter; `unhighlightMovieClip` clears filters. NX additionally toggles a `highlight` child clip.
- **Deferred button legend update:** After calling `buttonLegend.gotoAndStop(frame)`, an `ENTER_FRAME` listener waits until `currentLabel == targetFrame` before writing text into `primaryAction.textField` and `secondaryAction.textField`, ensuring the MC has finished transitioning.
- **Meta-category icons:** Loaded dynamically at runtime via `getDefinitionByName("metaIcon" + i)`, producing a `Bitmap` added to `meta[i].iconAnchor`.
- **Locale branching:** Button legend frame names get a `"Loc"` suffix appended when `_locale.indexOf(LOCALE_EN) != 0`.
- **Favorite swap:** `CollectableRow.swapRowUp/Down` call `SwapFavoritePositions(section, rowIndex, targetRow)` over `ExternalInterface`. The engine does the reorder and calls `swapRows(section, from, to)` back, which re-reads both positions with `dataSource.getData()`. A `SetFavorite` the engine refuses comes back as `revertFavorite(section, row)`, which flips that row's checkbox back.
- **Stage resize:** `backBanner.height` is scaled to `stageHeight / scaleY * 1.5`.
