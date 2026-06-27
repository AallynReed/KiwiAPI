# delveselectorui.swf
> The "Dial-a-Depth" Delve selector popup, shown when a player chooses to enter a Delve dungeon. It lets the player pick a depth level (within server-defined min/max limits), see who else is queued, then click Ready or Rush to proceed.

**Document/main class:** `DelveSelector` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 3

---

## Main class: `DelveSelector`

`DelveSelector` is the root display object for the Delve depth-selection window. It extends `UIComponent` and presents a numeric depth picker (text input with increment/decrement/min/max buttons), a queued-players display, a current-depth info display, and a combined Ready/Rush button. In the Iggy runtime it registers 11 `ExternalInterface` callbacks. On construction it registers two frame scripts (frames 1 and 11 for PC / Console layouts), initializes the `quantityTextInput` component properties, and wires a per-frame `__setProp_handler` for the console `closeButton` which only exists on frame 11+.

### Public methods / accessors

- `get quantity() : Number` — Returns the current numeric value of `quantityTextInput.text`; returns 0 if non-numeric.
- `set quantity(value:Number) : void` — Clamps `value` to `[minQuantity, maxQuantity]`, writes it to `quantityTextInput.text`, and enables/disables increment and decrement buttons at the boundaries. No-op when `isReady == true`.
- `setDelveLimits(max:int, min:int) : void` — Sets `maxQuantity` and `minQuantity`; resets quantity to 1; dynamically resizes `quantityTextInput.bg` width based on digit count; repositions all picker controls (and console LT/RT/dpad/keyboard buttons) to be horizontally centred within `bg`.

### Key fields

- `quantityTextInput : TextInput` — Editable numeric field; restricted to `"0-9"`, max 4 chars (expanded by `setDelveLimits`).
- `incrementButton / decrementButton : BaseButton` — Step the depth value ±1.
- `minButton / maxButton : BaseButton` — Jump directly to min or max depth. Also used as asset symbols (`MinButton`, `MaxButton`).
- `readyRushButton : LabelButton` — Dual-state button; label is `$DialADepth_Ready` before ready, `$DialADepth_Rush` after `setReady(true)`.
- `closeButton : BtnGreen` — Console-only close/cancel button (visible from frame 11); label `$DialADepth_Close`.
- `closeButtonTopRight : WindowCloseButton` — PC close button (top-right X).
- `currentDepthInfo : TextField` — Displays current delve depth info string set by `setDelveData`.
- `currentQueuedInfo : TextField` — Displays queued-player info string set by `setQueuedPlayers`.
- `ownerName : TextField` — Owner name display field.
- `header : MovieClip` — Title bar; text set to `$DialADepth_Selector` on init.
- `dialADepthInstructions : TextField` — Instruction text set to `$DialADepth_Instructions`.
- `isReady : Boolean` — Tracks ready state; disables all picker controls when true.
- `maxQuantity / minQuantity : Number` — Server-provided depth bounds; default 1/1.
- `queuedPlayers : Array` — Declared but not populated by ActionScript (filled by `setQueuedPlayers` as a plain string).
- `dpad_console : btn_console_dpad_updowneast` — Console d-pad hint icon, repositioned by `setDelveLimits`.
- `button_console_lt / _rt / _south / _east : *` — Console button icons.
- `button_console_keyboard : btn_console_keyboard` — Keyboard icon shown on NX (replaced by `txtKeyboardButton` text).
- `txtKeyboardButton : TextField` — NX keyboard hint text; set to `$Keyboard_Desc_nx`.

### Frame scripts / timeline

- **frame 1** (`frame1`) — `stop()`. PC layout.
- **frame 11** (`frame11`) — `stop()`. Console layout; the console-variant `closeButton` (`BtnGreen`) becomes available here. `__setProp_closeButton_Scene1_ButtonLegend_10` fires via the `FRAME_CONSTRUCTED` handler to set its label to `$DropPrompt_Cancel`.

### Runtime dependencies & integration

**ExternalInterface callbacks registered (game → Flash):**
`onIncrement`, `onDecrement`, `onMinQuantityClicked`, `onMaxQuantityClicked`, `setDelveLimits`, `setDelveData`, `setReady`, `setQueuedPlayers`, `setHasSentRush`, `setStartingDepth`, `onCancel`

**ExternalInterface calls made (Flash → game):**
`Ready` (when readyRushButton clicked and not yet ready), `Rush` (when readyRushButton clicked and already ready), `Cancel` (close/escape), `quantityUpdated(quantity)` (on Enter key, on valid key-up, on increment/decrement/min/max)

**translate() keys used:** `$DialADepth_Selector`, `$DialADepth_Instructions`, `$DialADepth_Ready`, `$DialADepth_Rush`, `$DialADepth_Close`, `$DropPrompt_Cancel`, `$Keyboard_Desc_nx`

**Keyboard handling:** `KEY_DOWN` on the text field calls `invalidate(DATA)` for most keys and fires `Cancel` on Escape. `KEY_UP` validates the input value; on Enter it clamps and calls `quantityUpdated`.

**Focus:** `setStartingDepth` explicitly sets `stage.focus` to the text field and selects all text.

**Platform guards:** `IsConsole()` (hides PC close button, adds LT/RT/dpad to layout), `IsNX()` (shows `txtKeyboardButton` text instead of `button_console_keyboard` icon).

**`setHasSentRush`:** Disables all picker controls permanently once a Rush has been sent (prevents re-sending).

---

## Other game-specific classes

- `BtnGreen` — Embed `symbol15` (extends `_kiwi.Controls.LabelButton`). Green styled label button used as the console close/cancel button. Four frame-stop states (frames 10, 20, 30, 40 = up/over/down/disabled).
- `MinButton` — Embed `symbol34` (extends `_kiwi.Controls.BaseButton`). Icon button that jumps to minimum depth. Four frame-stop states.
- `MaxButton` — Embed `symbol24` (extends `_kiwi.Controls.BaseButton`). Icon button that jumps to maximum depth. Four frame-stop states.

**Asset wrappers (not detailed — 11 classes):** `btn_console_dpad_updowneast`, `btn_console_east`, `btn_console_lt`, `btn_console_rt`, `btn_console_south`, `btn_console_keyboard`, plus `btn_XBOne_B/png`, `btn_XBOne_LT/png`, `btn_XBOne_RT/png`, `btn_XBOne_A/png`, `btn_XBOne_DPAD_updowneast/png`, `keyboard/png`.

## Notable logic

- **Dynamic layout in `setDelveLimits`:** After receiving min/max from the server, the method calculates the total width of all picker controls (including platform-specific console icons) and derives a left-offset `_loc6_` to centre the entire row within `bg.width`. Console mode stacks increment/decrement above and below the dpad icon using y-offsets relative to the text input.
- **Ready/Rush dual state:** The single `readyRushButton` changes its label and behaviour based on `isReady`. When the player clicks Ready the controls are disabled immediately client-side; the server confirms via `setReady(true)`. From that point the button label becomes "Rush" and clicking it calls `Rush` (and disables itself to prevent double-send).
- **`setHasSentRush` lockout:** Distinct from `setReady`; permanently disables all input controls after a rush signal has been sent, regardless of ready state.
