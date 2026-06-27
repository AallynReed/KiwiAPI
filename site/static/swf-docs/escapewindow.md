# escapewindow.swf
> The Escape Window is the in-game pause/escape menu that appears when a player presses Escape in Trove. It presents navigation buttons for Instructions, Store, Settings, Mod Loader (or Exit Tutorial), and Exit, and supports console D-pad highlight/activate callbacks.

**Document/main class:** `EscapeWindow` (extends `UIComponent`)
**SWF-specific classes:** 3

## Main class: `EscapeWindow`

`EscapeWindow` is the root document class for the escape/pause menu overlay. It extends `_kiwi.Core.UIComponent`. In its constructor it detects the console platform: if running on console, the Exit button is hidden (invisible and alpha 0) and the remaining three buttons (`instructionsButton`, `storeButton`, `settingsButton`) are shifted down by 30 pixels to fill the gap. `configUI` applies translation-key labels to all buttons and the window header, then registers four Iggy callbacks for console D-pad navigation (`highlightSelection`, `unhighlightSelection`, `activateSelection`, `setTutorial`).

All button actions resolve through a single `onButtonClicked` handler that reads an enum value from `__enum` (populated by the game engine at runtime) and calls `ON_BUTTON_CLICKED(uint)` outward via `ExternalInterface`.

### Public methods

- `EscapeWindow()` — Constructor. Conditionally hides Exit button and offsets console buttons, then calls `__setProp_winHeader_Scene1_header_0()` to configure the window header component inspector properties.

### Private / internal methods

- `configUI() : void` — Overrides UIComponent. Attaches `CLICK` listeners on all five buttons, assigns translation-key labels, sets the header text, calls `setupTranslation()`, and registers Iggy callbacks.
- `onStageResized(param1:Number, param2:Number, param3:Number) : void` — Overrides UIComponent. On console, calls `ESCAPE_WINDOW.CONFIGURED(consoleSelections.length)` to notify the engine of the selection count after the stage is laid out.
- `onButtonClicked(param1:MouseEvent) : void` — Switch on `param1.target` to map each button to its enum value from `__enum`, then calls `ON_BUTTON_CLICKED(uint)`.
- `highlightSelection(param1:uint) : *` — Iggy callback. Sends the button at `consoleSelections[param1]` to its `"over"` frame state.
- `unhighlightSelection(param1:uint) : *` — Iggy callback. Sends the button at `consoleSelections[param1]` to its `"up"` frame state.
- `activateSelection(param1:uint) : *` — Iggy callback. Directly calls `ON_BUTTON_CLICKED(param1)` bypassing the mouse event path.
- `setTutorial(param1:Boolean) : void` — Iggy callback. When `true`, relabels `modLoaderButton` to `$EscapeMenu_ExitTutorial`; when `false`, restores `$EscapeMenu_ModLoader`.

### Key fields

- `winHeader : MovieClip` — Custom window header; its `winTitleTextField.text` is set to `$EscapeMenu_Header`.
- `settingsButton : LabelButton` — Opens the Settings panel; label `$EscapeMenu_Settings`.
- `exitButton : LabelButton` — Exits the game; label `$EscapeMenu_Exit`. Hidden (visible=false, alpha=0) on console.
- `storeButton : LabelButton` — Opens the Store; label `$EscapeMenu_Store`.
- `instructionsButton : LabelButton` — Opens the Instructions/tutorial; label `$EscapeMenu_Instructions`.
- `modLoaderButton : LabelButton` — Opens the Mod Loader, or exits the tutorial when in tutorial mode; label `$EscapeMenu_ModLoader` / `$EscapeMenu_ExitTutorial`.
- `__enum : Object` — Enum mapping object injected by the engine (keys: `STORE`, `SETTINGS`, `EXIT`, `MODLOADER`, `INSTRUCTIONS`). Populated at runtime; Flash reads it for `ON_BUTTON_CLICKED` values.
- `consoleSelections : Array` — Ordered list of buttons exposed to D-pad navigation on console (`[instructionsButton, storeButton, settingsButton]`).
- `consoleButtonOffset : Number` (const = 30) — Vertical pixel offset applied to console buttons to compensate for the hidden Exit button.

### Frame scripts / timeline

No `addFrameScript` calls in `EscapeWindow`; the timeline has no scripted frames. Individual button skins (`BtnGreen`, `btnGreenIcon_small`) use frame scripts internally to halt at each button state.

### Runtime dependencies & integration

- **Iggy / ExternalInterface callbacks registered:** `highlightSelection`, `unhighlightSelection`, `activateSelection`, `setTutorial`.
- **ExternalInterface calls out:** `ON_BUTTON_CLICKED(uint)`, `ESCAPE_WINDOW.CONFIGURED(uint)`.
- **Translation keys:** `$EscapeMenu_Instructions`, `$EscapeMenu_Store`, `$EscapeMenu_Settings`, `$EscapeMenu_Exit`, `$EscapeMenu_Header`, `$EscapeMenu_ModLoader`, `$EscapeMenu_ExitTutorial`.
- **Platform checks:** `IsConsole()` (inherited global from UIComponent).
- **Kiwi controls used:** `LabelButton` (all five action buttons).
- **`onStageResized` override:** fires `ESCAPE_WINDOW.CONFIGURED` so the engine knows how many selectable items exist for D-pad cycling.

## Other game-specific classes

- `BtnGreen` — Extends `LabelButton`; embeds `assets.swf` symbol32. 4-state button skin (frames 1/2/3/4 each `stop()`).
- `btnGreenIcon_small` — Extends `LabelButton`; embeds `assets.swf` symbol11. Small icon-labelled green button skin, 4-state (frames 10/20/30/40 each `stop()`).

## Notable logic

- **Engine-owned enum pattern:** the `__enum` object is not defined in Flash; it is expected to be set on the component by the engine before any button is clicked. This lets the server-side code change the numeric identifiers for each menu action without recompiling the SWF.
- **Console layout shift:** rather than having a separate console timeline frame, the constructor directly mutates `y` positions of the three console-visible buttons by `consoleButtonOffset` (30 px) to fill the space vacated by the hidden Exit button.
- **D-pad navigation via index:** `consoleSelections` is an ordered array and the engine passes integer indices into it for `highlightSelection`/`unhighlightSelection`/`activateSelection`, decoupling the game from specific button object references.
- **Tutorial mode toggle:** the `modLoaderButton` doubles as an "Exit Tutorial" button; `setTutorial(true/false)` swaps its label at runtime without any layout change, reusing the same button slot.
- **`onStageResized` handshake:** the `ESCAPE_WINDOW.CONFIGURED` call in `onStageResized` acts as a ready signal to the engine, passing the count of console-navigable selections so the engine knows the valid index range before it starts sending D-pad events.
