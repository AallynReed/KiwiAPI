# collectionsnew.swf

> The main Collections window in Trove, showing all collectable cosmetic items (mounts, styles, etc.) organized into meta-categories and sub-categories. Supports search, filtering by collected/uncollected/favorites, section collapse, and console D-pad navigation.

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
- `onCategoryClick(e:MouseEvent) : void` — selects a category; calls `ExternalInterface.call("SetCategory", idx)`.
- `setCategory(idx:int) : void` — hides meta/category panels, shows row view, clears it, sets breadcrumb level 2.
- `setSection(idx:int) : void` — scrolls to section or activates console nav.
- `setRow(section:int, row:int) : void` — scrolls to row position with multi-row offset support.
- `setSearchString(s:String) : void` — proxies to `searchHeader.searchString`.
- `getCurrentNavMode() : int` — returns current console nav mode constant.

### Key fields

- `winHeader : MovieClip` — window title bar, title set to `$Collections`.
- `searchHeader : CollectionsSearchHeader` — search box + filter combo + breadcrumbs + collapse button.
- `metaCategories : MovieClip` — container with `meta0`–`meta5` buttons (icons loaded by class name `metaIcon0`–`metaIcon5` via `getDefinitionByName`).
- `categories : MovieClip` — container with `category0`–`category11` `SlotBasic` slots.
- `rowView : DynamicRowView` — virtualized scrolling list of `CollectableRow` rows grouped into sections.
- `backBanner : MovieClip` — decorative back banner; height scaled on `onStageResized`.
- `buttonLegend : MovieClip` (`buttonLegend_65`) — console button legend with `primaryAction`, `secondaryAction`, `marketplace`, `favorite_swap`, `switchScreens` children; frame labels `RowNoAction`, `RowSingleAction`, `RowTwoAction`, `Navigation` (and `*Loc` variants for non-English).
- `dataSource : CollectableRowExternalDataSource` — data bridge to the game engine.
- `currentNavMode : int` — one of `NavModeMetaCategories(0)`, `NavModeCategories(1)`, `NavModeSections(2)`, `NavModeRows(3)`.
- `allCategoryData : Array` — 6-element array of arrays; each entry holds category tuples `[name, description, icon, breadcrumbLabel]`.
- `highlightedMetaCategory/Category/Section/Row : int` — console cursor positions; special sentinels `SearchBoxIndex(-2)` and `DropdownIndex(-1)`.
- `_buttonLegendTargetFrame : String` — deferred frame label for button legend, set on `ENTER_FRAME` callback.
- `_showMarketplaceButtonLegend / _showSwitchScreensButtonLegend / _showSwapFavoriteButtonLegend : Boolean` — visibility flags for optional console button legend entries.

### Frame scripts / timeline

| Frame | Label | Action |
|-------|-------|--------|
| 0 | — | `stop()` |
| 10 | Console | `stop()`, plays `searchHeader` and `metaCategories` Console animations |
| 20 | ConsoleLoc | `stop()`, sets `buttonLegend` to `"ConsoleLoc"` |

### Runtime dependencies & integration

**ExternalInterface callbacks registered (inIggy):**
`addCategory`, `setEquipped`, `setLocked`, `isLocked`, `isVisible`, `setMetaCategory`, `setCategory`, `setSection`, `setRow`, `setCurrentMetaCategory`, `setSearchString`, `UIComponent.onStageResized`, `activateSelection`, `previousNavMode`, `consoleInputX`, `toggleFavorite`, `revertFavorite`, `moveHighlightHorizontal`, `moveHighlightVertical`, `highlightCurrentSelection`, `unhighlightCurrentSelection`, `setButtonLegendState`, `toggleSwitchScreensButtonLegend`, `swapRows`, `swapFavoriteUp`, `swapFavoriteDown`, `sectionsUpdated`, `consoleClearSearch`, `getCurrentNavMode`

**ExternalInterface calls (outbound):**
`SetMetacategory`, `SetCategory`, `SetCategory`, `SetSearchString`, `SetCollectedState`, `ActivateSearchInput`, `CloseScreen`, `SLOT.POINTER_ENTER`, `SLOT.POINTER_LEAVE`, `TOOLTIP.SHOW`, `TOOLTIP.HIDE`, `OnSlotEnter`, `OnSlotLeave`, `SwapFavoritePositions`, `OpenUrl`, `PositioningButtons` (NX)

**translate() keys:** `$Collections`, `$Collections_Home`, `$MetaCategory0`–`$MetaCategory5`, `$Collections_ShowAll`, `$Collections_HideCollected`, `$Collections_HideUncollected`, `$Collections_ShowFavorites`, `$Select_ButtonLegend`

**Events listened:** `MouseEvent.CLICK` (stage-wide, meta/category clicks), `CollectionsSearchHeader.EVENT_BREADCRUMB`, `EVENT_SEARCHTEXT`, `EVENT_COLLAPSEALL`, `Event.ENTER_FRAME` (console init + button legend deferred update)

**Highlight:** Uses `GlowFilter` (inner glow, color `0xCCCC00`, strength 100) applied to the currently highlighted display object; NX uses an additional `highlight` child `visible` flag on each row/section clip.

---

## Other game-specific classes

- `CollectionsSearchHeader` — `UIComponent` (embedded symbol 278). Composite search/filter/breadcrumb bar. Owns `searchText` (TextField), `filterComboBox` (KiwiComboBox with ShowAll/HideCollected/HideUncollected/ShowFavorites options), `btnClearText`, `btnCollapse`, `breadcrumbs` (3-button MC). Dispatches `DataEvent` with types `BreadCrumbClicked`, `SearchTextPopulated`, `CollapseAll`. Registers Iggy callbacks `setCollectedState`, `setSearchString`. Calls `ExternalInterface.call("SetSearchString")` and `ExternalInterface.call("SetCollectedState")`. Frame 11 plays Console breadcrumb animation.
- `CollectableRow` — `DynamicRowViewRow` (embedded symbol 181). One row in the list. Shows item icon (`Slot`), name, type, mastery XP amounts for Trove and Geode, favorite checkbox (`Checkbox`), sleeping icon, marketplace button, and up/down swap arrows for favorite reordering. Calls `ExternalInterface.call("SetFavorite")`, `SwapFavoritePositions`, `OnSlotEnter`, `OnSlotLeave`, `OpenUrl`. Locked items display in muted color (`0xB9B9B9`). Frame labels `unlocked`/`locked` (and `*Console` variants).
- `CollectableRowData` — Plain VO class holding fields: `name`, `type`, `metaExperience`, `geodeMetaExperience`, `icon`, `locked`, `equipped`, `favorite`, `sleeping`, `isCategoryFavorite`, `marketplaceUrl`, `storeUrl`. `clear()` resets all to defaults.
- `CollectableRowExternalDataSource` — Extends `DynamicRowViewExternalDataSource`. Overrides `getData(section, row)` to call `super.getData()` (which triggers the Iggy bridge to fill `rowData`), then calls `rowView.setRowData()` and resets `rowData`.

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
- `masteryIconSmall_90` — small mastery icon (symbol not shown).
- `CollapsedIcon_95` — collapsed-section chevron icon.
- `subcategoryHeader_94` — section heading (symbol 121) with `highlight`, `titleText`, `infoText`, `collapsedIcon`; 3 frame-stop states.
- `slotFrameLarge_102` — large slot frame decoration.

**Asset wrappers (not detailed):** 22 top-level skin/png classes — `rarity_frame_*_png` / `*_over_png` (10 rarity tiers × 2), `CellRenderer_*Skin` (8), `ComboBox_*Skin` (4), `ScrollArrow*_*Skin` (8), `ScrollThumb_*Skin` (3), `ScrollBar_thumbIcon`, `List_skin`, `SlotBackground`, `SlotBackgroundLocked`, `Equipped` (bitmap embed), `btn_XBOne_RB/png`, `rarity_frame_stellar` (non-png variant). Also `dummy.as`, `metaIcon0`–`metaIcon5` (6 bitmap embed classes).

---

## Notable logic

- **Four-level console navigation:** The class maintains `currentNavMode` and `highlighted*` indices. D-pad moves are routed to `moveMetaCategoryHighlight*`, `moveCategoryHighlight*`, `moveSectionHighlightVertical`, `moveRowHighlightVertical`. Wrap-around is implemented per-layer. The `DropdownIndex(-1)` and `SearchBoxIndex(-2)` sentinels represent the filter combo and search box as virtual navigation targets above the meta-category grid.
- **Highlight glow:** `highlightMovieClip` applies an inner yellow glow filter; `unhighlightMovieClip` clears filters. NX additionally toggles a `highlight` child clip.
- **Deferred button legend update:** After calling `buttonLegend.gotoAndStop(frame)`, an `ENTER_FRAME` listener waits until `currentLabel == targetFrame` before writing text into `primaryAction.textField` and `secondaryAction.textField`, ensuring the MC has finished transitioning.
- **Meta-category icons:** Loaded dynamically at runtime via `getDefinitionByName("metaIcon" + i)`, producing a `Bitmap` added to `meta[i].iconAnchor`.
- **Locale branching:** Button legend frame names get a `"Loc"` suffix appended when `_locale.indexOf(LOCALE_EN) != 0`.
- **Favorite swap:** `CollectableRow.swapRowUp/Down` call `SwapFavoritePositions(section, rowIndex, targetRow)` over `ExternalInterface`. The main class's `swapRows` also calls `dataSource.getData()` on both positions to refresh them.
- **Stage resize:** `backBanner.height` is scaled to `stageHeight / scaleY * 1.5`.
