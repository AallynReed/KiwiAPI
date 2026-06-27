# messagedialog.swf
> General-purpose modal confirmation dialog used throughout Trove for purchase prompts, unlock confirmations, and other binary or multi-choice decisions. The game fills it with a message and one or more labeled response buttons at runtime.

**Document/main class:** `MessageDialog` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 2

## Main class: `MessageDialog`

`MessageDialog` is a data-driven modal panel. It starts invisible (`configUI` hides it) and reveals itself 100 ms after `SETUP_COMPLETE` is called, giving layout a chance to settle before the first paint. The game sends content through `ExternalInterface` callbacks: `SET_MESSAGE` provides the body text and an optional disclaimer, and `ADD_RESPONSE` appends a `BtnGreen` button. After all responses are added, `SETUP_COMPLETE` triggers the reveal timer and (on console) places controller-button images next to each response button.

Layout is done in `draw()`: `KiwiTextUtil.colorize` applies rich-text color markup to `messageTextField`, then `centerResponses()` measures all `LabelButton` children in `buttonHolder`, arranges them left-to-right, and calls `resizeBG()` to fit the `background` MovieClip. `resizeBG` also notifies the host via `NOTIFY_RESIZED`.

### Public methods / setters
- `set message(param1:String) : void` — stores the message and invalidates `DATA`.
- `setupComplete() : void` — starts a 100 ms one-shot `Timer` to show the dialog; on console calls `addButtonImages()` to position gamepad glyphs.

### Key fields
- `background : MovieClip` — panel backdrop; width and height are set dynamically.
- `messageTextField : TextField` — main body text; colorized by `KiwiTextUtil.colorize`.
- `disclaimerIcon : MovieClip` — warning icon shown alongside the disclaimer (hidden when no disclaimer text).
- `disclaimerText : TextField` — secondary fine-print text (e.g. purchase terms); visible only when text length > 0.
- `buttonHolder : MovieClip` — contains `response_0` (pre-placed `BtnGreen`) and dynamically added `BtnGreen` children.
- `numResponses : Number` — count of response buttons added so far.
- `nextResponseOffset : *` — x-cursor for placing additional response buttons.
- `BUTTON_SPACING : Number = 10` — gap between buttons.
- `BUTTON_MARGIN : Number = 30` — horizontal padding inside each button.
- `BUTTON_IMAGE_SPACING : Number = 27` — extra room reserved for console gamepad-button images.
- `BUTTON_LABEL_MARGIN : Number = 12` — label inner margin.

### Frame scripts / timeline
None on `MessageDialog`. Button states are managed by `BtnGreen`'s own four-frame timeline (frames 10, 20, 30, 40 — up/over/down/disabled states).

### Runtime dependencies & integration
- `IggyFunctions.inIggy` — gates `ExternalInterface` callbacks; in preview mode, a sample message and three buttons are injected directly.
- `ExternalInterface.addCallback("SET_MESSAGE", setMessage)` — receives `(messageText:String, disclaimerText:String)`.
- `ExternalInterface.addCallback("ADD_RESPONSE", addResponse)` — receives `(label:String, enabled:Boolean)`.
- `ExternalInterface.addCallback("SETUP_COMPLETE", setupComplete)` — signals that all responses have been added.
- `ExternalInterface.call("MESSAGEDIALOG.RESPONSE", index)` — fired on button click; sends the zero-based response index back to the game.
- `ExternalInterface.call("NOTIFY_RESIZED", width, height)` — called after background resize in `resizeBG`.
- `_kiwi.Util.KiwiTextUtil.colorize(tf, text)` — parses color markup in message text.
- `IsConsole()` — controls extra button spacing and gamepad-glyph placement (`btn_north`, `btn_south`, `btn_east`).
- `flash.utils.Timer(100, 1)` + `TimerEvent.TIMER_COMPLETE` — deferred reveal after setup.
- `MouseEvent.CLICK` on each `BtnGreen` — `onResponseClick` reads `target.data` (the response index) and calls back to the game.

---

## Other game-specific classes

- `BtnGreen` (extends `_kiwi.Controls.LabelButton`) — Embed `symbol11`; four-state button clip (frames 10/20/30/40 = up/over/down/disabled) with `stop()` at each state boundary. Used as every response button in the dialog.

---

## Notable logic
- `addResponse` reuses the pre-authored `buttonHolder.response_0` for the first button and creates new `BtnGreen` instances for subsequent ones, each named `response_N` with `data = N` for click identification.
- Button auto-sizing: if `textField.textWidth + BUTTON_MARGIN * 2` exceeds the current button width, `setActualSize` widens the button to fit the label.
- On console with three responses: north/south/east gamepad glyphs are placed to the left of buttons 0/1/2 respectively; with two responses: south/east are used instead.
- The dialog is always horizontally centered: `buttonHolder.x` and `messageTextField.x` are both set to `background.width / 2 - child.width / 2` after each layout pass.
