# dropprompt.swf
> A modal quantity-picker dialog that appears when a player chooses to drop (discard) an item from their inventory. It displays a prompt message, a numeric text input, +/− step buttons, Min/Max buttons, and Confirm/Cancel buttons. Full console support is included with D-Pad, LT/RT, and on-screen keyboard hints.

**Document/main class:** `DropPrompt` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 10

---

## Main class: `DropPrompt`

`DropPrompt` is the root component for the item-drop confirmation panel. The constructor binds two frame scripts (frames 0 and 10) and applies component inspector defaults to the three button/input widgets, then defers to `UIComponent.configUI()`. During `configUI()` all button event listeners are wired, two `ExternalInterface` callbacks are registered (`setMax`, `setQuantity`), and six additional callbacks are registered that allow the C++ game engine to invoke the same handlers the buttons use. A `draw()` override re-clamps the `quantity` property whenever `DATA` is invalidated (triggered by any non-backspace/delete key press in the text field).

The `setMax()` method is the main layout driver: it dynamically repositions all controls horizontally so the control row is centered within `bg`, accounting for the variable digit-width of the max quantity. Console-specific controls (LT, RT, D-Pad, keyboard button) are included in the layout calculation only when `IsConsole()` is true.

### Public methods

- `get quantity() : Number` — reads and parses `quantityTextInput.text`; returns 0 on NaN.
- `set quantity(value:Number) : *` — clamps `value` to `[0, maxQuantity]`, updates the text input, and toggles `dropButton.enabled` (`> 0`), `incrementButton.enabled` (`< max`), and `decrementButton.enabled` (`> 0`).
- `set message(text:String) : void` — sets `promptMessage.text`.
- `set acceptButtonLabel(text:String) : void` — overrides `dropButton.label`.
- `setMax(max:Number) : void` — stores `maxQuantity`, resets `quantity` to 1, dynamically resizes `quantityTextInput.bg` to fit the digit count (12 px per char), and repositions all controls horizontally centered in `bg`. On console, also layouts D-Pad vertically centered on the input and stacks increment/decrement above/below it.
- `setQuantity(value:Number) : void` — clamps and applies `value`, then focuses `quantityTextInput.textField` and selects all text.

### Key fields

- `quantityTextInput : TextInput` — editable numeric field; `restrict="0-9"`, `maxChars=4` (grows to match `setMax` digit count).
- `dropButton : LabelButton` — confirm button; default label key `"$DropPrompt_Confirm"`.
- `cancelButton : LabelButton` — cancel button; default label key `"$DropPrompt_Cancel"`.
- `incrementButton : BaseButton` — steps quantity up by 1.
- `decrementButton : BaseButton` — steps quantity down by 1.
- `minButton : BaseButton` — jumps to `min(1, maxQuantity)`.
- `maxButton : BaseButton` — jumps to `max(0, maxQuantity)`.
- `promptMessage : TextField` — static prompt text shown above the controls.
- `bg : MovieClip` — panel background; its `width` is the centering reference for dynamic layout.
- `dpad_console : btn_console_dpad_updowneast` — D-Pad hint icon shown on console.
- `button_console_keyboard : btn_console_keyboard` — on-screen keyboard button icon; on NX (`IsNX()`) this is hidden and replaced by `txtKeyboardButton` with the translated string `"$Keyboard_Desc_nx"`.
- `button_console_south : btn_console_south` — south-button (A/Cross) hint icon.
- `button_console_east : btn_console_east` — east-button (B/Circle) hint icon.
- `button_console_lt : btn_console_lt` — left trigger hint icon.
- `button_console_rt : btn_console_rt` — right trigger hint icon.
- `txtKeyboardButton : TextField` — NX-only text replacement for the keyboard button icon.
- `maxQuantity : Number = 1` — upper bound for the quantity field.

### Frame scripts / timeline

- **Frame 0** (`frame1`): `stop()` — default (PC/generic) layout frame.
- **Frame 10** (`frame11`): `stop()` — console layout frame (controls repositioned by `setMax` at runtime regardless of frame).

### Runtime dependencies & integration

- `ExternalInterface.addCallback` registrations: `"setMax"`, `"setQuantity"`, `"onIncrement"`, `"onDecrement"`, `"onMinQuantityClicked"`, `"onMaxQuantityClicked"`, `"onDrop"`, `"onCancel"` — game engine can drive all interactions directly without mouse events.
- `ExternalInterface.call` outbound:
  - `"DropConfirm"(quantity)` — fired on drop button click or Enter key, only when quantity is valid and `<= maxQuantity`.
  - `"Cancel"()` — fired on cancel button click.
- `IggyFunctions.translate("$Keyboard_Desc_nx")` — NX platform keyboard hint string.
- `com.kiwi.Constants.KeyCodes` — used to detect `ENTER` (submit) and `BACKSPACE`/`DELETE` (skip invalidation).
- `InvalidationType.DATA` — invalidation type used to schedule quantity clamping after key input.
- `IsConsole()` / `IsNX()` — platform guards; console path adds LT/RT/D-Pad to the layout; NX path replaces the keyboard button with a text field.
- Translate keys: `"$DropPrompt_Confirm"` (drop button default), `"$DropPrompt_Cancel"` (cancel button default), `"$Keyboard_Desc_nx"` (NX keyboard hint).

---

## Other game-specific classes

- `BtnGreen` — Embeds `symbol76`; green `LabelButton` skin with 4 stop-frames at frames 10/20/30/40 (up/over/down/disabled). Used as the confirm/cancel button skin.
- `MaxButton` — Embeds `symbol30`; `BaseButton` skin for the Max button; same 4-stop-frame layout.
- `MinButton` — Embeds `symbol40`; `BaseButton` skin for the Min button; same 4-stop-frame layout.
- `DPAD_Console` — Embeds `symbol21`; `UIComponent` subclass representing the D-Pad directional icon with 2 frame states (frames 0 and 10).
- Console button icon wrappers (pure `MovieClip` embed stubs, no logic): `btn_console_south` (`symbol89`), `btn_console_east` (`symbol90`), `btn_console_lt` (`symbol88`), `btn_console_rt` (`symbol91`), `btn_console_keyboard` (`symbol92`), `btn_console_dpad_updowneast` (`symbol87`) — 6 asset-wrapper classes.

---

## Notable logic

- **Dynamic centering**: `setMax` computes the total width of all visible controls (`maxButton + gap + [console extras] + minButton + gap + textInput + gap + [dpad/increment] + [keyboard button]`) and offsets the leftmost control by `(bg.width - totalWidth) / 2`, producing a centered row regardless of digit count.
- **Input validation flow**: every non-destructive key press (`onKeyPressed`) calls `invalidate(InvalidationType.DATA)`, which schedules `draw()` to re-apply the `quantity` setter, clamping out-of-range typed values without interrupting mid-edit.
- **Enter key shortcut**: `checkEnterKey` on `KEY_UP` submits the drop if the Enter/Return key is pressed, equivalent to clicking the drop button.
- **`setQuantity` focuses the field**: after programmatically setting a quantity from the engine, the text field is focused and fully selected so the player can immediately type a replacement value.
- **Max=0 edge case**: if `setMax(0)` is called, `quantity` is forced to 0 and the layout is not recalculated (no controls shown at valid sizes), effectively making the drop impossible.
