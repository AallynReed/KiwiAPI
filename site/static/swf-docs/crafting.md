# crafting.swf
> The Crafting window is Trove's item-crafting UI, shown when the player interacts with a crafting bench. It presents a categorised recipe browser on the left, a detail/ingredient panel on the right, a quantity picker, and a live crafting-progress overlay. The window supports PC, Console, and ConsoleLoc platform variants via a 3-frame timeline.

**Document/main class:** `Crafting` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 17

---

## Main class: `Crafting`

`Crafting` is the root document class. It owns a `RecipePane` (left recipe browser), a `CraftActionPane` (right ingredient/quantity/craft panel), a `progressPane` overlay, and supporting header/close widgets. The game controls it entirely through `ExternalInterface` callbacks registered in `configUI()`.

Construction initialises a 1-second `Timer` (`hideProgressTimer`) used to hide the progress pane after completion, registers frame scripts at frames 0/10/20 for PC/Console/ConsoleLoc, sets `craftActionPane.visible = false`, `progressPane.visible = false`, and registers `onRecipeSelected` as the recipe-selection callback on `recipePane`.

### Public methods

- `selectRecipe(id:uint) : *` — calls `recipePane.selectRecipeById(id)` then `recipePane.synthesizeSlotClick(id)`, which sends `SLOT.ACTIVATE(id)` back to the engine; registered as `ExternalInterface` callback `SELECT_RECIPE`.
- `onCategoryCollapse() : void` — clears `recipeDetailHeading.title`, resets and hides `craftActionPane`; called by `RecipePane` when a category collapses.

### Key fields

- `recipePane : RecipePane` — left-panel recipe browser.
- `craftActionPane : CraftActionPane` — right-panel ingredient list, quantity picker, and craft button.
- `progressPane : MovieClip` — crafting-progress overlay (wraps a `ProgressPane` symbol).
- `heading : SecondaryHeader` — top-left section heading, set to `"$Crafting_Recipes"` via `IggyFunctions.translate`. `SET_HEADING` replaces it with one engine-formatted string built from `$Crafting_ProfessionSkillFormat` (`{0} (Skill: {1})`): the bench's profession name and skill.
- `recipeDetailHeading : SecondaryHeader` — shows the selected recipe name.
- `winHeader : WindowHeader` — title-bar component; inspector-initialised with title `"$Crafting_WinTitle"`.
- `closeButton : MovieClip`, `background : MovieClip`, `buttonLegend : MovieClip` — standard window chrome. `buttonLegend` is placed from the Console frame on only (controller hints such as `$Crafting_Legend_NumItems` "More/Less Items" and `$Crafting_Legend_CraftAll` "Craft All"); the PC frame has none.
- `undiscoveredRecipeDescription : TextField` — text `$Crafting_Recipe_Not_Discovered` ("You have not discovered this recipe yet"); shown when `UNDISCOVERED_MESSAGE(true)` is called; hidden by default via `listenForAllOnTarget`.
- `hideProgressTimer : Timer` — 1-second one-shot timer; on expire, hides `progressPane` and restores `recipePane`.

### Frame scripts / timeline

- **frame 0** — `stop()` (PC mode).
- **frame 10** — `stop()`; `buttonLegend.gotoAndStop("Console")`.
- **frame 20** — `stop()`; `buttonLegend.gotoAndStop("ConsoleLoc")`.

### Runtime dependencies & integration

- `IggyFunctions.inIggy` guards all callback registration; in editor mode, calls `setHeading("LoserJuice")` as a placeholder.
- `ExternalInterface` callbacks registered: `SELECT_RECIPE`, `SHOW_PROGRESS`, `HIDE_PROGRESS`, `SET_HEADING`, `SET_LOCKED` (no-op), `UNDISCOVERED_MESSAGE`.
- Outbound call: `ExternalInterface.call("CRAFT.CONFIGURED", recipePane.maxNumForSlots)` on stage resize (console only).
- `setupTranslation()` called in `configUI()`.
- `onStageResized` override fires `CRAFT.CONFIGURED` on console whenever the stage dimensions change (used by the game engine to know the recipe grid capacity).

---

## `RecipePane`

[Embed symbol244] Left-panel recipe browser. Manages an array of `BagContainerBasic` category containers (`categoryBags`) inside a `RecipeView` scroll view. Each category is collapsible; expanding one collapses all others.

### ExternalInterface callbacks (registered in constructor when `inIggy`)
`ADD_CATEGORY`, `UPDATE_CATEGORY`, `ADD_RECIPE`, `UPDATE_RECIPE`, `UPDATE_RECIPE_CANCRAFT`, `CLEAR_RECIPES`, `GET_PACKED_SLOT_POSITION`, `GET_PACKED_SLOT_SIZE`, `CHECK_RECIPE_IN_BAG`, `HIGHLIGHT_CATEGORY`, `UNHIGHLIGHT_CATEGORY`.

The engine sends `CLEAR_RECIPES` every time the window opens, and the bench arrives in one burst of `ADD_CATEGORY` / `ADD_RECIPE` calls; `addRecipe` calls `recipeView.refreshLayout()` once per recipe.

### Key methods
- `addCategory(name:String, index:int) : int` — creates a `BagContainerBasic`, pushes into `categoryBags[index]`, adds to `recipeView`. First category starts expanded (PC); all start collapsed (Console).
- `addRecipe(id:uint, name:String, categoryIndex:Number, iconImage:String, locked:Boolean, unlockCost:Number, canCraft:Boolean, check:Boolean) : void` — adds a `SlotBasic` to the appropriate `BagContainerBasic`; registers CLICK/ROLL_OVER/ROLL_OUT. The slot is darkened when neither `locked` nor `canCraft`, `check:true` hides its `check` child, and `unlockCost` is kept for the selection callback. No rarity is sent; every recipe slot gets `frameType = 1`.
- `updateRecipeCanCraft(...rest)` — variadic; takes triples `(id, canCraft, locked)` to batch-update slot darkened state.
- `selectRecipeById(id:uint) : void` — deselects old slot, selects new slot, invokes `selectedCallback`, on console calls `RECIPES.POINTER_ENTER` and auto-scrolls.
- `synthesizeSlotClick(id:uint) : void` — calls `slot.activate()` directly (used by `Crafting.selectRecipe`).
- `getPackedSlotPosition(id:uint) : int` / `getPackedSlotSize(id:uint) : int` — pack slot bounds into a single `int` (high 16 bits = x/w, low 16 bits = y/h) for the game engine.
- `highlightCategory(index:int) : void` — applies a yellow-green inner `GlowFilter` to the category heading, scrolls it into view, calls `RECIPES.SELECT` on its first slot, and collapses the category if it is open. On NX shows `highlight` MC.
- Outbound calls: `RECIPES.POINTER_ENTER`, `RECIPES.POINTER_LEAVE`, `RECIPES.SELECT`. A recipe click also runs `SlotBasic.activate()`, which sends `SLOT.ACTIVATE(recipeId)`; on this screen the engine reads that id as a recipe id. The slots' own `SLOT.POINTER_ENTER` / `SLOT.POINTER_LEAVE` fire on hover alongside the `RECIPES.*` pair.

### Key fields
- `categoryBags : Array` — indexed array of `BagContainerBasic` (one per category).
- `maxNumForSlots : int = 7` — max columns per category row.
- `selectedSlot : Number = -1` — currently selected recipe ID.
- `unlockCostsBySlotId : Dictionary` — maps slot ID → unlock cost passed to `selectedCallback`.
- `emptyCategory : BagContainerBasic` — fallback container used when no category index is provided.

---

## `CraftActionPane`

[Embed symbol199] Right-panel pane shown after a recipe is selected. Contains the ingredient list, quantity picker, and craft button.

### ExternalInterface callbacks
`SET_MARKETPLACE_URL`, `USE_MARKETPLACE_BUTTON`, `ADD_INGREDIENT`, `INGREDIENT_COUNT_CHANGED`, `INGREDIENT_QUANTITY_CHANGED`, `SET_MAX_QUANTITY`, `DECREMENT_QUANTITY`, `craftActionPane.SetMessage`, `RESET_INGREDIENTS`, `SET_QUANTITY_VISIBILITY`, `SET_LIMITED_TEXT_VISIBILITY`, `CRAFT_COUNT_INCREMENT`, `CRAFT_COUNT_DECREMENT`, `CRAFT_COUNT_MAX`, `CRAFT_REQUEST`, `CRAFT_COUNT_INCREMENT_N`, `CRAFT_COUNT_DECREMENT_N`, `setPurchasePrice`, `PURCHASE_REQUEST`, `SET_CAN_PURCHASE`.

### Key logic
- `quantityToCraft` setter enables/disables increment, decrement, max, and craft buttons; updates `craftQuantityTextField`. The getter returns 1 while `purchase` is visible.
- `setMaxQuantityForSelectedRecipe(max, yieldQty)` (`SET_MAX_QUANTITY`) — `max` is the most crafts the held materials allow; `yieldQty` is the number of items one craft makes, shown as the `yieldSlot` quantity badge (only when ≥ 0). Resets quantity to `min(1, max)`. `DECREMENT_QUANTITY` (only while quantity > 0) re-runs it with `max - 1`. The engine puts the quantity back to 1 as each craft lands.
- `addIngredient(iconImage, name, have, need, id)` (`ADD_INGREDIENT`) adds one `IngredientRow`; `INGREDIENT_COUNT_CHANGED(id, have)` and `INGREDIENT_QUANTITY_CHANGED(id, need)` find the row by that `id`.
- `craftActionPane.SetMessage(text)` is the engine's error line, drawn red in `messageTextField` above the quantity row; an empty string hides it. `SET_LIMITED_TEXT_VISIBILITY` toggles `limitedTextField` (`$Crafting_LimitedTime`, "Limited Time Recipe!", yellow).
- `SET_QUANTITY_VISIBILITY(false)` is what the engine sends for a recipe whose material requirements change with each craft.
- `onCraftClick` → `ExternalInterface.call("CRAFT_REQUEST", quantityToCraft)`. There is no in-flight guard; every click sends.
- `setPurchasePrice(price, _)` is sent for a recipe the game also sells for credits. It shows the `purchase` clip (price in `priceCreditsTextField`) in place of the `quantity` picker and leaves `craftActions` up, so the recipe can be unlocked for credits or crafted once from materials. The second argument is unused, and nothing in the SWF hides `purchase` again.
- `onBuyClick` → `ExternalInterface.call("OnBuy", "TWC")` (credit purchase path).
- Marketplace button calls `ExternalInterface.call("OPENURL", marketPlaceURL)`. `SET_MARKETPLACE_URL` is the screen's only marketplace link and is for the item the selected recipe makes; nothing is sent per ingredient. An empty string hides the button (and the console `button_console_lt`), and `USE_MARKETPLACE_BUTTON` presses it from the engine. `OPENURL` is the verb the client registers for the crafting screen; a `trove://marketplace/search?category=0&collected=0&search={0}` address (a template the client carries) opens the in-game marketplace, and the query text is used verbatim, with no percent-decoding.
- On NX, hides `button_console_keyboard` and shows `txtKeyboardButton` with `$Keyboard_Desc_nx`.
- Frame 10/20: `craftActions.gotoAndStop("Console")`.

---

## `ProgressPane`

[Embed symbol127] Crafting-in-progress overlay with a masked progress bar, remaining/finished item counts, item icon slots, and a cancel button.

- `update(remaining, finished, completion, iconImage)` — updates text fields, item slots, and bar fill.
- `setBarCompletion(ratio)` — scales `progressBar.maskMC.width`; switches between `"working"` and `"complete"` frame labels. On console, fires `ExternalInterface.call("CRAFT.COMPLETE")` when complete.
- `onCancelClicked` → plays sound event `"Play_ui_notification_crafting_CANCEL"`, then `ExternalInterface.call("CRAFT.CANCEL")`.
- Cancel button label uses translate key `"$Cancel"`; section header title uses `"$Crafting_Progress"`.
- Registers `ExternalInterface.addCallback("CRAFT_REQUEST_CANCEL", ...)` when in Iggy.

---

## Other game-specific classes

- `IngredientRow` — [Embed symbol86] One ingredient line: `Slot` (icon; `data` = the ingredient id from `ADD_INGREDIENT`; `clickFeedback` off, so a click sends nothing), name, have/need text fields. The name arrives as the item's translation key (e.g. `$prefabs_item_crafting_cupcake_item_name`, from the `prefabs_item_*` language files), which Iggy resolves when it is assigned to `nameTextField`. `haveTextField` is coloured red (0xFF0000) when `have < need`, green (0x2ABE62) otherwise. The `haveCount` / `needCount` setters rewrite the text on every platform but store the value and recolour only on console. Console path uses `ENTER_FRAME` for a 1-frame deferral to capture proper bounds.
- `PrereqsRow` — [Embed symbol38] Simple prerequisite display: `Slot` icon, name, and `"have/need"` text field. No interactive behaviour.
- `InventoryRow` — [Embed symbol76] Dynamic `MovieClip` holding 6 `Slot` children (`slot_0`…`slot_5`). Used for inventory display rows.
- `SectionHeader` — [Embed symbol90] Thin subclass of `_kiwi.Controls.SecondaryHeader`; provides a titled section divider within the craft detail panel. Title `"$Crafting_Progress"` set via inspector.
- `SubHeader` — [Embed symbol204] Another `SecondaryHeader` subclass; 2-frame (PC/Console) variant used for sub-section titles.
- `MaxButton` — [Embed symbol147] Extends `_kiwi.Controls.BaseButton`; 4-state button (frames 10/20/30/40) for the "craft max" quantity button.
- `marketplaceButton` — [Embed symbol179] Extends `BaseButton`; 4-frame marketplace link button.
- `btnGreen` — base `LabelButton` asset for the main Craft button (label `"$Craft"`, initially disabled).

### `Crafting_fla` timeline symbols (4 classes)
- `CraftingActions_25` — [Embed symbol186] The bottom action bar: `yieldSlot:Slot`, `craftButton:btnGreen`, `marketPlaceAction:marketplaceButton`, console button references. Craft button starts disabled.
- `slotFrame_28`, `quantity_38`, `ProgressBar_51`, `ButtonLegend_54`, `craftable_purchase_45`, `equipped_30` — decoration/control clips attached to the main timeline. `craftable_purchase_45` [Embed symbol192] is the `purchase` clip: a credits icon (shape 190, filled with the 24×24 bitmap 189), `purchaseButton` and `priceCreditsTextField`, placed in the `quantity` picker's row (y 433.5 against 410).

### Asset-wrapper symbols (27 classes)
`rarity_frame_*_png` / `rarity_frame_*_over_png` (14 rarity frame variants for common/uncommon/rare/epic/legendary/relic/shadow/stellar/radiant1/resplendent), `ScrollArrow*_*Skin` (8), `ScrollThumb_*Skin` (3), `ScrollTrack_skin`, `ScrollBar_thumbIcon`, `SlotBackground`, `SlotFrameNormal`, `SlotFrameMedium`, `SlotFrameHigh`, `SlotBackgroundLocked`, `focusRectSkin`, `CloseIcon`, `CloseIconPressed`, `btn_console_analog_top_right`, `dummy`, `rarity_frame_stellar` (non-_png variant).

---

## Notable logic

- **Recipe selection flow**: `Crafting.selectRecipe(id)` → `RecipePane.selectRecipeById(id)` → invokes `selectedCallback(slot, unlockCost)` → `Crafting.onRecipeSelected` sets `recipeDetailHeading.title`, shows/resets `craftActionPane`. The slot's `locked` state gates whether `craftActionPane` is shown at all.
- **Progress reports**: the engine answers `CRAFT_REQUEST` for a timed recipe with repeated `SHOW_PROGRESS(remaining, finished, completion, iconImage)` calls: the two counts plus a 0–1 completion fraction (the pane clamps it at 1). The SWF works nothing out itself. Reports run on the engine's own clock, so where crafting is fast a batch can finish before they catch up, and the last report need not show the batch complete; `ProgressPane.showCompleted()`, which would close the counts out, is never called. A recipe with no craft time gets no `SHOW_PROGRESS` at all. `HIDE_PROGRESS` ends a finished craft and a cancelled one alike.
- **Progress hide delay**: `HIDE_PROGRESS` does not hide immediately — it starts `hideProgressTimer` (1 second), and only if the pane is showing. Only `onFinishHideProgress` (timer callback) actually hides the pane and restores `recipePane`. `SHOW_PROGRESS` does not stop a pending timer, so a craft that starts reporting within that second is still swapped back to the recipe list when the timer fires, until its next `SHOW_PROGRESS` shows the pane again.
- **Console recipe layout**: On console, `onStageResized` reports `maxNumForSlots` to the engine. `RecipePane.selectRecipeById` also forces the selected category to expand and scrolls the slot into view via `setScrollLocation`.
- **Packed slot coordinates**: `getPackedSlotPosition` and `getPackedSlotSize` bit-pack 2D values into a single `int` (high 16 = x/width, low 16 = y/height) as a compact IPC mechanism.
- **Category highlight glow**: `highlightCategory` applies an inner yellow-green `GlowFilter` (colour 0xCCFF00, strength 100, blur 2) to the category heading to indicate a search match.
- **Undiscovered recipe**: `UNDISCOVERED_MESSAGE(flag)` sets `undiscoveredRecipeDescription.visible` to `flag`. Both that call and the constructor's initial hide go through `listenForAllOnTarget`, which only defers them until every component has reached its platform target frame (at once on PC). Nothing else in the SWF hides the text.
