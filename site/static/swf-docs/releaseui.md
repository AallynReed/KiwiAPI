# releaseui.swf

> Displays a release/loading screen overlay in Trove that shows a primary message and a secondary instruction line. Supports both plain-text and HTML-rich text depending on whether the game is running on a console platform, via two ExternalInterface callbacks that push text from the game engine.

**Document/main class:** `ReleaseUI` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 1

## Main class: `ReleaseUI`

`ReleaseUI` is the document class and the only game-specific class in this SWF. The constructor registers two inbound ExternalInterface callbacks and calls the inherited `setupTranslation()` from `UIComponent`. Text rendering branches on `IsConsole()` (an Iggy global function) to use `htmlText` on console platforms and plain `text` on PC.

### Public methods

- `setText(param1:String) : void` — Sets `messageText`. On console: `messageText.htmlText = param1`; otherwise: `messageText.text = param1`.
- `setInstructionText(param1:String) : void` — Retrieves the `"instructionText"` named child `TextField` from `instructions` and assigns text (or `htmlText` on console).

### Key fields

- `messageText : TextField` — Primary message area displayed to the player (e.g. loading status, patch notes header).
- `instructions : MovieClip` — Container symbol; holds a child `TextField` named `"instructionText"` for secondary/control-hint text.

### Runtime dependencies & integration

- **ExternalInterface callbacks registered (always, not gated on `inIggy`):**
  - `"SET_TEXT"` → `setText(String)` — sets the primary message.
  - `"SET_INSTRUCTION_TEXT"` → `setInstructionText(String)` — sets the instruction/hint line.
- **`IsConsole()`** — Iggy global function (not in `IggyFunctions` class); controls whether text is assigned as `htmlText` or plain `text`.
- **`setupTranslation()`** — Inherited UIComponent method; called in the constructor to initialize any framework-level localization setup.
- No translate keys called directly in this class; no outbound ExternalInterface calls; no timers; no frame scripts.

## Other game-specific classes

None beyond `ReleaseUI`.

## Notable logic

- The `IsConsole()` branch enables HTML text on console (allowing rich formatting such as bold/color via markup) while keeping plain text on PC, likely to avoid HTML-parsing overhead or rendering differences on the PC client.
- `setInstructionText` navigates into `instructions` by child name (`getChildByName("instructionText")`) rather than holding a direct reference, making it dependent on the exact instance name set in the Flash authoring tool.
- ExternalInterface callbacks are registered unconditionally (no `IggyFunctions.inIggy` guard), unlike some other SWFs in this set.
