# guideui.swf
> The Guide UI is Trove's in-game compendium and activity browser, displayed whenever the player opens the Guide (collection/achievement/store listing). It shows categorised content tiles (blueprints, activities, purchase options, badges, challenges, personal objectives) with a live search bar, a hide-completed checkbox, and a progress bar for the current guide collection.

**Document/main class:** `GuideUI` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 20

---

## Main class: `GuideUI`

`GuideUI` is the root document class. It owns two `ScrollableTileView` instances — `categoryView` (left column of category buttons) and `contentView` (right column of content rows) — and maintains a `Dictionary`-of-`Dictionary` called `categoryMap` keyed by category-id string then by entry-id. The game populates it entirely via `ExternalInterface` callbacks registered in `configUI()`.

On construction it calls `addFrameScript(0, frame1, 10, frame11, 20, frame21)` for PC / Console / ConsoleLoc platform branching, then `__setProp_mainTitle_Scene1_header_0()` to set up the timeline window-header component.

### Public methods

- `setMainTitle(title:String) : void` — sets the `mainTitle` movie-clip's `.title` property.
- `setCollectedLabel(label:String) : void` — updates the progress-bar's collected-label text, applying 3-unit letter-spacing.
- `setHideCollectedLabel(label:String) : void` — sets the "hide completed" checkbox label.
- `addCategory(id:String, label:String, iconImage:String, isCollected:Boolean) : void` — creates a `CategoryEntry` button, adds hover/click listeners, pushes it into `categoryView`, creates the `categoryMap[id]` slot. First category added auto-dispatches a CLICK to select itself.
- `selectCategory(id:String) : int` — deselects the previous category, clears `contentView`, repopulates it from `categoryMap[id]`, sorts, updates header dividers, calls `ExternalInterface.call("SelectCategory", id)`. Returns the category's index.
- `selectCategoryIndex(index:int) : void` — programmatically clicks a category by numeric index.
- `addHeader(categoryId:String, activityId:int, title:String, description:String, descriptionEmpty:String, hidden:Boolean) : void` — creates or updates a `GuideUIHeader` in `categoryMap`.
- `activateContext(categoryId:String, contextId:int, name:String, description:String, sortOrder:int) : void` — creates or updates an `ActivateContext` entry; sets `currentMethod` so subsequent `ingredientEntry`/`animatedEntry`/`textEntry`/`updateBlueprint` calls attach to it.
- `ingredientEntry(iconImage:String, name:String, have:Number, need:Number, rarity:String, tooltip:String, slotData:int) : void` — appends an `IngredientRow` to `currentMethod.ingredientsStackList`.
- `updateBlueprint(textureName:String, scale:Number) : void` — appends a `BlueprintEntry` to `currentMethod.ingredientsStackList`.
- `animatedEntry(textureName:String) : void` — appends an `AnimatedEntry` to `currentMethod.ingredientsStackList`.
- `textEntry(html:String) : void` — appends a raw `TextField` to `currentMethod.ingredientsStackList`.
- `purchaseMethodEntry(...12 params) : void` — creates or updates a `PurchaseMethodEntry` in `categoryMap`, adding a `PurchaseRow` child for each product.
- `resetPurchaseEntry() : void` — clears the `ingredientsStackList` of every `PurchaseMethodEntry` in the current category.
- `resetActivateContextEntry() : void` — clears the `ingredientsStackList` of every `ActivateContext` in the current category.
- `resetCategories() : void` — empties both tile-views and the full `categoryMap`; resets `categories` array.
- `setProgress(current:Number, total:Number) : void` — sets `guideProgressBar.filling.scaleX`.
- `setProgressBarVisibility(visible:Boolean) : void` — shows/hides the progress bar BG; also shows `GoldProgressBar`, hides `BlackProgressBar`.
- `focusCategory(index:int, direction:int) : void` — highlights the category at `index` by toggling `background.visible`; scrolls `categoryView` to keep it in view; calls `buttonLegend.gotoAndStop("category")`.
- `focusContent(index:int, direction:int) : void` — pages `contentView` vertically; calls `buttonLegend.gotoAndStop("content")`.
- `focusProduct(index:int) : void` — highlights `PurchaseRow.twcBackground`/`twpBackground` for the focused store item.
- `buyTWCProduct(index:int) : void` / `buyTWPProduct(index:int) : void` — synthesises a CLICK on the TWC or TWP `LabelButton` of the focused `PurchaseRow`.
- `focusSearchString(state:int) : void` — shows/hides the search background and sets stage focus; calls `buttonLegend.gotoAndStop("search")`.
- `activateSearchInput() : void` — calls `ExternalInterface.call("ActivateSearchInput")`.
- `disableClickEvents()` / `enableClickEvents()` / `disableSearchEvents()` / `enableSearchEvents()` — toggling event listeners for console nav flow.
- `toggleHideCompleted() : void` — programmatically fires a CLICK on `hideCollectedCheckBox`.
- `sizeOfCategory(id:String) : int` — count of non-`GuideUIHeader` entries in a category.
- `getCategoryId() : String`, `getCategoryCount() : int`, `getContentCount() : int`, `getProductCount() : int`, `getHeaderIndex() : int` — query helpers for game engine.

### Key fields

- `categoryView : ScrollableTileView` — left-panel list of `CategoryEntry` buttons.
- `contentView : ScrollableTileView` — right-panel list of content entries.
- `categoryMap : Dictionary` — `{categoryId → {entryId → MovieClip}}`.
- `categories : Array` — ordered list of category ID strings (mirrors `categoryView` insertion order).
- `currentCategory : String` — currently selected category ID.
- `currentMethod : ActivateContext` — reference to the last `ActivateContext` or `IngredientEntry` created, used as the target for child-row insertions.
- `firstCategorySelected : Boolean` — prevents auto-selecting the first category more than once.
- `guideProgressBar : MovieClip` — contains `filling`, `progressBarBG`, `GoldProgressBar`, `BlackProgressBar`, `collectedLabelText`, `currentProgressText`.
- `searchGuideUI : MovieClip` — wraps `searchText : TextField` and `background : MovieClip`.
- `hideCollectedCheckBox : _kiwi.Controls.Checkbox` — bound to `OnHideCompleted` ExternalInterface call on click.
- `_debounceFrames : int` / `DEBOUNCE_FRAMES : int = 15` — search-input debounce counter; fires `ExternalInterface.call("SetSearchString", ...)` after 15 idle frames.
- `FIRST_CATEGORY_OFFSET : Number = 0.55` — horizontal centering multiplier applied to the sole visible category button.

### Frame scripts / timeline

- **frame 0** — `stop()` (PC mode).
- **frame 10** — `stop()` (Console mode).
- **frame 20** — `stop()` (ConsoleLoc mode).

### Runtime dependencies & integration

- `IggyFunctions.inIggy` guards all `ExternalInterface.addCallback` registrations.
- Callbacks registered: `setMainTitle`, `setCollectedLabel`, `setHideCollectedLabel`, `addHeader`, `selectCategory`, `addCategory`, `selectCategoryIndex`, `focusCategory`, `focusContent`, `getHeaderIndex`, `getCategoryId`, `getCategoryCount`, `getContentCount`, `getProductCount`, `sizeOfCategory`, `updateBlueprint`, `ingredientEntry`, `purchaseMethodEntry`, `resetPurchaseEntry`, `resetActivateContextEntry`, `animatedEntry`, `activateContext`, `textEntry`, `setProgress`, `setProgressText`, `resetCategories`, `setProgressBarVisibility`, `enableClickEvents`, `disableClickEvents`, `disableSearchEvents`, `enableSearchEvents`, `focusSearchString`, `activateSearchInput`, `focusProduct`, `buyTWCProduct`, `buyTWPProduct`, `toggleHideCompleted`.
- Outbound calls: `ExternalInterface.call("SelectCategory", id)`, `ExternalInterface.call("SetSearchString", text)`, `ExternalInterface.call("ActivateSearchInput")`, `ExternalInterface.call("OnHideCompleted", checked)`, `ExternalInterface.call("OnBuy", buttonName, productName)`, `ExternalInterface.call("INGREDIENT.POINTER_ENTER", ...)`, `ExternalInterface.call("INGREDIENT.POINTER_LEAVE", ...)`.
- `setupTranslation()` called in `configUI()`.

---

## Other game-specific classes

### Top-level game classes
- `GuideUIHeader` — [Embed symbol140] Content section header with title (`txt_name`) and description (`txt_description`/`txt_descriptionEmpty`). Resizes `background` to fit wrapped text. `activityId:int`, `sortOrder:int=0`, `showEmpty` toggles between two description strings. 3-frame platform variant (PC/Console/ConsoleLoc).
- `ActivateContext` — [Embed symbol257] A collapsible entry block with a title, description, and a `StackList` (`ingredientsStackList`) for child rows (blueprints, ingredient rows, animated previews, text). `sortOrder` equals the `activityId`. `reset()` clears the stack list. 3-frame platform variant.
- `CategoryEntry` — [Embed symbol229] Extends `LabelButton`; the clickable icon+label tile in `categoryView`. Carries `collectedTick`, `collectedEntryBG`, and `slot` sub-MovieClips managed by `GuideUI`.
- `PurchaseMethodEntry` — top-level class; a marketplace listing entry with an `ingredientsStackList` of `PurchaseRow` children.
- `PurchaseRow` — [Embed symbol58] One store product row: item icon `Slot`, name, description, TWC (`LabelButton`) and TWP (`LabelButton`) buy buttons with price labels. Fires `ExternalInterface.call("OnBuy", buttonName, productName)` on click. Rolls over/out fire `INGREDIENT.POINTER_ENTER/LEAVE`. Disabled when price < 0.
- `IngredientEntry` — [Embed symbol133] A crafting-recipe-style ingredient list entry (separate from the store flow): title `txt_questName` and a `StackList` of `IngredientRow` children. Registers `ExternalInterface.addCallback("ADD_INGREDIENT", ...)`.
- `BlueprintEntry` — [Embed symbol231] Large image preview entry; wraps an `ObjectPreview` embedded as `art`, scaled to `clampedScale` (clamped 150–300).
- `AnimatedEntry` — [Embed symbol252] Like `BlueprintEntry` but fixed 745×345 viewport; used for animated texture previews in guide content panels.
- `PreviewContainer` — [Embed symbol230] Thin subclass of `ObjectPreview`; used as an embeddable image container symbol.
- `BadgeEntry` — shared with questtracker; see questtracker docs. Embedded here as a reusable badge progress display.
- `ChallengeEntry` — shared with questtracker; see questtracker docs.
- `PersonalObjectiveEntry` — shared with questtracker; see questtracker docs.

### `GuideUI_fla` timeline symbols (10 classes)
`GuideProgressBar_25`, `newGuideProgressBarBG_26`, `ButtonLegend_32`, `AdventureFrameBackground_42`, `badgeRewardImage_52`, `CollectedEntryBG_58`, `slotFrame_61`, `equipped_63`, `clock_animation_74`, `badge_75`, `searchGuideUI_22`, `header_background_96` — frame-script-only timeline clips attached to the main timeline, mostly UI decoration.

### Asset-wrapper symbols (26 classes)
26 skin/shape classes: `CellRenderer_*Skin` (7), `ComboBox_*Skin` (4), `ScrollArrow*_*Skin` (8), `ScrollThumb_*Skin` (3), `ScrollTrack_skin`, `ScrollBar_thumbIcon`, `List_skin`, `TextInput_*Skin` (2), `SlotBackground`, `focusRectSkin`, `metaIcon0`, `dummy`.

---

## Notable logic

- **Debounced search**: `onSearchTextChanged` starts a 15-frame `ENTER_FRAME` countdown; only after 15 frames without further input does it call `ExternalInterface.call("SetSearchString", ...)` and then disables search events. The game re-enables them once it has processed the search.
- **Sort-on-invalidate**: Content entries carry `sortOrder` and `secondarySortOrder` integers. `sortContent()` is called whenever `validateSort` is true, sorting by `sortOrder` first then `secondarySortOrder`, with `GuideUIHeader` entries always sorting first within the same `sortOrder` group. After sorting, `updateHeader()` hides the divider on the first entry and shows `showEmpty` on headers with no following content.
- **Single-visible-category centering**: In `draw()`, if exactly one `categoryView` item is visible, its `x` is set to `(viewportWidth - width) * FIRST_CATEGORY_OFFSET` (0.55) to slightly right-of-centre the single item.
- **Collected category hover**: Categories flagged `isCollected` get separate `MOUSE_OVER/OUT/CLICK` handlers that drive a `collectedEntryBG` movie-clip through up/over/down states; non-collected categories reset all collected frames on click.
- **Console gating**: On console, `IsConsole()` gates `buttonLegend` label changes and platform-specific `htmlText` rendering. `IsNX()` is also tested in `PurchaseRow`.
