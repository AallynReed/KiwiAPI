# recipeconsumeprompt.swf
> A modal confirmation dialog shown when the player attempts to consume (learn) a recipe item. It lists one or more recipes with their icons and names, shows optional crafting requirements, and presents "Learn" and "Keep" (cancel) buttons. On console it defers setup until the playhead reaches the correct frame.

**Document/main class:** `RecipeConsumePrompt` (extends `UIComponent`)
**SWF-specific classes:** 3

---

## Main class: `RecipeConsumePrompt`

`RecipeConsumePrompt` is the root UIComponent for the recipe-consume confirmation dialog. On construction it registers frame scripts for frames 1 and 11 (PC vs console), then either calls `setup()` immediately (PC) or defers it via an `ENTER_FRAME` listener until `onTargetFrame()` is true (console). `setup()` computes the button-row margin, hides the requirements fields, wires mouse-click listeners on the two `LabelButton` controls, and registers all `ExternalInterface` callbacks. The engine then populates the dialog by calling `addRecipe()` one or more times, optionally `setRequirement()`, and `canLearn()`.

### Public methods

- `setup() : *` — initialises the dialog: computes `responseMargin`, hides requirements fields, registers button listeners, registers all ExternalInterface callbacks, calls `setupTranslation()` and `onSetupFinished()`.
- `addRecipe(name:String, image:String, canLearn:Boolean) : *` — instantiates a `RecipeRow` and adds it to `stacklist`; repositions the button row beneath the list and resizes `background` and `self.height` to fit; calls `ExternalInterface.call("NOTIFY_RESIZED", w, h)` after each addition.
- `clearRecipes() : void` — delegates to `stacklist.clear()`, resetting the list.
- `canLearn(param1:Boolean) : void` — enables or disables `learnButton`.
- `setRequirement(param1:String) : void` — sets `requiresTextField.text`, makes both requirements fields visible, and shifts `stacklist.y` down by the text field height.
- `setConfirmText(param1:String) : *` — overrides the learn button label.
- `setCancelText(param1:String) : *` — overrides the cancel button label.
- `setHeader(param1:String) : *` — sets `titleHeader.text`.
- `hideCancel(param1:Boolean) : void` — hides `cancelButton` and re-centres `learnButton` (and `learnControl` on console) horizontally when `true`.
- `onEnterFrame(param1:Event) : void` — console-only deferred setup; removes itself and calls `setup()` once `onTargetFrame()` is true.

### Key fields

- `learnButton : LabelButton` — "Learn" action button; default label key `"$RecipeConsume_Learn"`; click calls `ExternalInterface.call("onLearn")`.
- `cancelButton : LabelButton` — "Keep" / cancel button; default label key `"$RecipeConsume_Keep"`; click calls `ExternalInterface.call("onCancel")`.
- `requiresPrefixTextField : TextField` — static label prefix for the requirements line (e.g. "Requires:"); hidden until `setRequirement()` is called.
- `requiresTextField : TextField` — dynamic requirement text; hidden until `setRequirement()` is called.
- `stacklist : StackList` — vertical list container that holds `RecipeRow` children; grows downward as rows are added.
- `background : MovieClip` — background panel; its `height` is updated to match the computed dialog height after each recipe addition.
- `titleHeader : TextField` — dialog title; set externally via `setHeader()`.
- `learnControl : MovieClip` — console controller-button hint for the learn action; repositioned alongside `learnButton`.
- `cancelControl : MovieClip` — console controller-button hint for the cancel action; repositioned alongside `cancelButton`.
- `responseMargin : uint` — vertical gap between the stack list and the button row, computed from the initial layout in `setup()`.

### Frame scripts / timeline

- **frame 1** (`frame1`) — `stop()`. PC layout frame; `setup()` is called directly from the constructor.
- **frame 11** (`frame11`) — `stop()`. Console layout frame; `setup()` is deferred until this frame is reached via the `ENTER_FRAME` listener.

### Runtime dependencies & integration

- **ExternalInterface callbacks registered (in `setup()`):**
  - `"addRecipe"` → `addRecipe(name, image, canLearn)`
  - `"clearRecipes"` → `clearRecipes()`
  - `"canLearn"` → `canLearn(enabled)`
  - `"setConfirmText"` → `setConfirmText(text)`
  - `"setCancelText"` → `setCancelText(text)`
  - `"setHeader"` → `setHeader(text)`
  - `"setRequirement"` → `setRequirement(text)`
  - `"hideCancel"` → `hideCancel(hide)`
- **ExternalInterface calls (out):**
  - `"onLearn"` — emitted when the player clicks the learn button.
  - `"onCancel"` — emitted when the player clicks the cancel/keep button.
  - `"NOTIFY_RESIZED"(width, height)` — emitted after each `addRecipe()` call so the engine can resize the containing window.
  - `"onSetupFinished"` — emitted (console only) when `setup()` completes, signalling the engine that the UI is ready for callbacks.
- **`setupTranslation()`** — inherited; applies localisation to static text.
- **`IsConsole()`** — global function; controls constructor branching (deferred vs immediate setup, visibility of `learnControl` / `cancelControl`).
- **`onTargetFrame()`** — inherited from `UIComponent`; gating check in the console ENTER_FRAME loop.

---

## Other game-specific classes

- `RecipeRow` (extends `UIComponent`, embeds `symbol23`) — single row in the recipe list. Constructor accepts `(name:String, image:String, canLearn:Boolean)`; renders a `Slot` (icon, 46×46 px) and a word-wrapping `nameTextField`; greys out the name text (`0x999999`) when `canLearn` is false; centres the slot+text group horizontally. Fixed `height` of 46 px. `configUI()` sets `slot.iconImage` and matches `width` to the parent.
- `BtnGreen` (extends `LabelButton`, embeds `symbol34`) — green-styled label button with frame stops at frames 10, 20, 30, 40 (corresponding to the four button states: up, over, down, disabled). Used as the styled button skin in this SWF.

## Asset wrappers (not individually documented)

11 bitmap asset classes — `rarity_frame_common_png`, `rarity_frame_uncommon_png`, `rarity_frame_rare_png`, `rarity_frame_epic_png`, `rarity_frame_legendary_png`, `rarity_frame_relic_png`, `rarity_frame_resplendent_png`, `rarity_frame_shadow_png`, `rarity_frame_stellar`, `rarity_frame_radiant1_png` — plus one additional `_png` frame class; all extend `BitmapData` and embed rarity-border PNG assets (77×77 px). They are used by `Slot` symbol rendering and carry no game logic.

---

## Notable logic

- **Dynamic height growth:** each call to `addRecipe()` appends a `RecipeRow` to `stacklist`, then recomputes the button-row Y as `stacklist.y + stacklist.height + responseMargin`, updates `learnButton.y`, `cancelButton.y` (and console hint clips), and expands `background.height` and `this.height`. After each resize it notifies the engine via `"NOTIFY_RESIZED"` so the host window can reposition the dialog on screen.
- **Requirements row shifting:** `setRequirement()` nudges `stacklist.y` down by `requiresTextField.height` after making the requirements fields visible, ensuring the recipe list does not overlap the requirements text. This shift is permanent — there is no corresponding undo.
- **Console vs PC setup deferral:** on console the constructor registers an `ENTER_FRAME` listener instead of calling `setup()` directly, waiting for `onTargetFrame()` to be true before wiring callbacks. This ensures the Flash timeline has settled on the console layout frame (frame 11) before the engine starts sending data.
- **Centring on cancel hide:** `hideCancel(true)` re-centres `learnButton` using `background.x + (background.width - learnButton.width) / 2`, accommodating variable button widths after `setConfirmText()` changes the label.
