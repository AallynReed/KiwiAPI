# loginui.swf

> The Trove login screen UI, presenting username/password fields and a login button. Supports both mouse/keyboard input and Iggy console controller navigation. Also hosts a secondary email-token dialog for two-factor authentication.

**Document/main class:** `LoginBase` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 4

---

## Main class: `LoginBase`

`LoginBase` is the root document class and acts as a thin shell. It holds a single child `form:LoginForm` which contains all interactive controls. On stage resize (console only) it calls `ExternalInterface.call("OnConfigured", NUM_SELECTIONS)` to notify the game engine of how many navigable selections exist (hardcoded to 3).

### Public methods

- `configUI() : void` (override) — delegates to super; no additional setup.
- `onStageResized(width, height, scale) : void` (override) — fires `OnConfigured` to the engine when running on console.

### Key fields

| Field | Type | Role |
|---|---|---|
| `form` | `LoginForm` | The visible login form component |
| `NUM_SELECTIONS` | `int` | Fixed at 3; reported to engine for controller focus cycling |

### Runtime dependencies & integration

- `ExternalInterface.call("OnConfigured", NUM_SELECTIONS)` — fired on stage resize (console path only via `IsConsole()`).

---

## Other game-specific classes

### `LoginForm` (embeds `/_assets/assets.swf#symbol31`)

The main interactive form. Extends `UIComponent`.

**Fields:**
- `userName : TextInput` — username field (max 256 chars, default text "Username").
- `passWord : TextInput` — password field (display as password, max 256 chars).
- `loginBtn : LabelButton` — login button; label key `$LoginUI_LoginButton`.
- `loginResult : Label` — result/error label, initially hidden.

**Callbacks registered via `ExternalInterface.addCallback`:**
- `SET_USER_DATA(user, pass, extra)` — pre-fills fields and advances focus.
- `ENABLE_FORM()` — re-enables fields and button after a failed login.
- `requestEmailToken()` — spawns the `AuthTokenDialog` overlay centered on stage.
- (Iggy-only) `activateSelection(index)`, `deactivateSelection(index)`, `select(index)`, `setText(text, index)` — controller navigation for the 3 selections (0=username, 1=password, 2=loginBtn).

**Calls out to engine:**
- `ExternalInterface.call("LOGIN", username, password)` — sent when the login button is clicked; disables the form to prevent double-submit.
- `ExternalInterface.call("METAFORGE")` — triggered by F6 key.

**Events listened:**
- `Event.CHANGE` on both text fields — re-evaluates `loginBtn.enabled`.
- `KeyboardEvent.KEY_DOWN` on both fields — Enter triggers login (non-console), F6 triggers Metaforge.
- `MouseEvent.CLICK` on `loginBtn`.

**Translate keys:** `$LoginUI_LoginButton` (set via component inspector, not `IggyFunctions.translate`).

---

### `AuthTokenDialog` (embeds `/_assets/assets.swf#symbol26`)

A modal overlay for email-token two-factor authentication. Extends `UIComponent`. Spawned by `LoginForm.requestEmailToken()`, centred on stage, hides `LoginForm` while visible.

**Fields:**
- `username / password : String` — copied from the parent form at spawn time.
- `instructionsTextField : TextField` — contains a `{account}` placeholder replaced by `setAccountName()`.
- `tokenTextInput : TextInput` — where the player types the emailed token.
- `okButton / cancelButton : LabelButton` — labels `$LoginUI_LoginButton` / `$LoginUI_CancelButton`.

**Calls out to engine:**
- `ExternalInterface.call("OnEmailTokenSubmitted", username, password, token)` — on OK click.
- `ExternalInterface.call("OnEmailTokenEntryCanceled")` — on Cancel click.

---

### `BtnGreen` (embeds `/_assets/assets.swf#symbol24`)

`LabelButton` subclass with a 4-state timeline (frames 10, 20, 30, 40), each with a `stop()` handler. Used inside `LoginForm`/`AuthTokenDialog` for action buttons.

---

## Notable logic

- `LoginForm.isLoginEnabled()` always returns `true`, so the button is enabled regardless of field content; the real guard is the `loginBtn.enabled = false` set immediately on click to prevent double-submission.
- Console navigation maps selection index 0→userName, 1→passWord, 2→loginBtn; `activateSelection` plays a "focused"/"over" frame, `deactivateSelection` resets to "normal"/"up".
- `requestEmailToken` hides the form (`this.visible = false`) and adds the dialog directly to `stage`, bypassing the component hierarchy.
