# inputpromptui.swf
> A modal text-input dialog used whenever the game needs the player to type a short string — for example, naming a character or entering text in a form field. It supports both mouse/keyboard and controller inputs, and notifies the game engine via ExternalInterface callbacks.

**Document/main class:** `InputPromptUI` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 4

## Main class: `InputPromptUI`

`InputPromptUI` owns the dialog box and wires up all user interactions. On construction it calls `addFrameScript(0, frame1, 10, frame11)`, registering two frame stops. In `configUI()` it sets up event listeners on child components (inside `this.dialog`) and registers six `ExternalInterface` callbacks so the game engine can drive the dialog at runtime. Focus is immediately given to the text input field.

### Public methods
None beyond the inherited UIComponent lifecycle.

### Key fields
- `dialog : MovieClip` — the embedded `InputDialog_1` clip; holds all child widgets.
- `inputPos, confirmPos, cancelPos : Point` — fixed positions used when `IsConsole()` is true; the engine repositions widgets accordingly.
- `buttonOffset : Point` — offset applied to console button-icon overlays (`okButton_image`, `canceButton_image`).
- `storedMessage : String` — declared but not used in visible logic (likely a leftover).

### Frame scripts / timeline
- **frame 1** (`frame1`): `stop()` — default PC layout.
- **frame 11** (`frame11`): `stop()` then `this.dialog.gotoAndStop("Console")` — switches the dialog MovieClip to its Console label, repositioning elements for gamepad UI.

### Runtime dependencies & integration

**ExternalInterface callbacks registered (game → Flash):**
| Callback | Handler | Description |
|---|---|---|
| `ON_RESIZE` | `_onStageResize(w,h)` | Centers dialog on stage. |
| `CLEAR_INPUT` | `onClearInput()` | Clears text field, disables OK button, restores focus. |
| `SET_MESSAGE` | `onSetMessage(text)` | Sets prompt message; auto-resizes background and repositions buttons if text overflows. Calls `NOTIFY_RESIZED(w,h)` back. |
| `SET_RESTRICTION` | `onSetAllowedCharacters(str)` | Sets `nameTextInput.restrict`. |
| `SET_MAX_INPUT_SIZE` | `onSetMaxInputSize(n)` | Sets `nameTextInput.maxChars`. |
| `ON_CONTROLLER_ACCEPT` | `onControllerAccept()` | Submits if input non-empty. |
| `ON_CONTROLLER_CANCEL` | `onControllerCancel()` | Cancels. |

**ExternalInterface calls dispatched (Flash → game):**
- `ACCEPTED(text)` — when the player confirms; also disables the text field to prevent re-editing.
- `OnCancel()` — when the player cancels.
- `NOTIFY_RESIZED(w, h)` — when the dialog height changes due to long message text.

**Events listened:**
- `Event.CHANGE` on `nameTextInput` — enables/disables OK button based on non-empty length.
- `KeyboardEvent.KEY_UP` on `nameTextInput` — submits on Enter (keyCode 13) if non-empty.
- `MouseEvent.CLICK` on `okButton` / `cancelButton`.

**translate keys:** `$OK`, `$Cancel` (set on button labels via `InputDialog_1` component inspector).

## Other game-specific classes

- `InputDialog_1` (package `InputPromptUI_fla`) — Embedded timeline symbol `symbol26`; dynamic MovieClip holding `background`, `okButton` (BtnGreen, initially disabled), `cancelButton` (BtnGreen), `nameTextInput` (TextInput, default restrict `A-Za-z`, maxChars 19), `messageTextField`, `okButton_image` (btn_console_south), `canceButton_image` (btn_console_east). Has two frame stops (frame 1 = PC, frame 11 = Console label).
- `BtnGreen` — Embedded symbol `symbol19`; extends `_kiwi.Controls.LabelButton`; 4-state button (10/20/30/40 frame stops for up/over/down/disabled states).
- `btn_console_south` — Asset-wrapper MovieClip for the console Accept button icon (South face button).
- `btn_console_east` — Asset-wrapper MovieClip for the console Cancel button icon (East face button).

## Notable logic
- When `SET_MESSAGE` is called and the message text is taller than the original field height, the dialog dynamically grows: it redistributes vertical spacing between the text field, input, and buttons, and the background clip height is stretched to match. Console icon positions are also recomputed using `buttonOffset`.
- The OK button starts disabled and only becomes enabled when `nameTextInput.length > 0`; the `onClipboardPaste` override also triggers the same check.
- Frame 11 exists specifically to reroute the `dialog` MovieClip to its "Console" frame label, changing the visual layout for gamepad play.
