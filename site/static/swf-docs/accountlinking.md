# accountlinking.swf
> The console account-linking flow shown when a player on a platform (Xbox / PS4 / NX) needs to link or create a Trion/Gamigo account before playing. Displays a split left-pane / right-pane layout: the left pane lists navigation options, the right pane swaps between several sub-screens (promo, new account, existing account, web-code link, and completion states).

**Document/main class:** `AccountLinking` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 10

---

## Main class: `AccountLinking`

Root coordinator for the two-pane linking UI. The constructor wires the `leftPane` and `rightPane` MovieClips into a `panes` array, sets focus to the left pane, and immediately registers `SET_ACCOUNT_NAME` with `ExternalInterface` so the game engine can push the console account name into the header.

In `configUI` (Iggy mode only) it registers the full set of Iggy callbacks that route navigation commands to whichever pane currently has focus.

### Public methods
- `setAccountName(name:String) : *` — Delegates to `LeftPane.setAccountName`; shows the signed-in account name at the top of the left pane.

### Key fields
- `leftPane : MovieClip` (typed as `LeftPane`) — Navigation / option list.
- `rightPane : MovieClip` (typed as `RightPane`) — Content area that swaps sub-screen MovieClips.
- `focusedPane : int` — 0 = left, 1 = right; controls which pane receives navigation commands.
- `panes : Array` — `[leftPane, rightPane]`; enables focus cycling.
- `PageMoveDistance : int = 10` — Step count for `dropdownPageMove` when scrolling the birth-year ComboBox.

### Runtime dependencies & integration
**Iggy callbacks registered:**
- `moveHighlight(dir:int)` — routes D-pad to focused pane.
- `activateSelection()` — confirms the focused pane's current item; may switch focus or call `ExternalInterface.call("EnableOrbisMedia", false)`.
- `previousMenu()` — navigates back; calls `ExternalInterface.call("EnableOrbisMedia", true)` when returning to the top-level promo screen.
- `toggleWebCodeLinkScreen()` — toggles between `ExistingUser` and `WebCodeLink` sub-screens on the right pane.
- `showWebCode(code, url)` — populates `WebCodeScreen` fields.
- `toggleHidePassword()` — toggles password display on whichever `LinkingScreen` is active.
- `openLinkSuccessScreen()` / `openLinkDisabledScreen()` — drive the right pane to `MenuTypeLinkComplete` / `MenuTypeLinkDisabled` and updates the left pane's `linkStatus`.
- `dropdownPageMove(dir:int)` — pages the birth-year dropdown by `PageMoveDistance` steps.
- `returnFocusToActivePane()` — re-applies focus to the active pane.
- `SET_ACCOUNT_NAME(name:String)` — registered unconditionally (not Iggy-gated).

---

## Other game-specific classes

### `LeftPane` (extends `UIComponent`) — [Embed symbol156]
Left navigation column. Manages two menu states (`MenuTypeLinkOrPlay = 0`, `MenuTypeNewOrExisting = 1`) and three link-status states (`Unlinked`, `Disabled`, `Complete`).

**Key logic:**
- `option0` / `option1` / `option2` are timeline MovieClips. `selectableItems` is rebuilt by `setUpScreen()` depending on `_curMenuType` and `_linkStatus`.
- `highlightSelection()` starts a looping IggyTween fade (alpha 1 → 0.4 → 1) on the selected item's `highlight` child; `unhighlightSelection()` stops it.
- `activateSelection()` returns a `RightPane.MenuType*` constant on success, or -1 (no-op / calls `ExternalInterface.call("OnPlayNow")` or `"OnViewEula"`).
- `setAccountName(name)` — sets `accountNameText.text`; the border is only shown on Durango.
- translate keys: `$AccountLinking_ButtonLegendBack`, `$AccountLinking_LinkAccount`, `$AccountLinking_PlayNow`, `$AccountLinking_NewAccount`, `$AccountLinking_ExistingAccount`, `$AccountLinking_ViewToS`.

### `RightPane` (extends `UIComponent`) — [Embed symbol157]
Right content area. Holds a single `curMenu:MovieClip` that is destroyed and replaced on every `curMenuType` change via `setUpScreen()`.

**Menu types:**
- `MenuTypeLinkOrPlay (0)` → `PromoScreen`; sets `text0` = `$AccountLinking_Promo`.
- `MenuTypeNewUser (1)` → `NewUserScreen`; sets `text0` = `$AccountLinking_CreateTrionAccount`.
- `MenuTypeExistingUser (2)` → `ExistingUserScreen`; sets `text0` = `$AccountLinking_ExistingTrionAccount`.
- `MenuTypeWebCodeLink (3)` → `WebCodeScreen`; calls `ExternalInterface.call("OnGetCode")` and populates translate keys `$AccountLinking_LinkViaSignIn`, `$AccountLinking_WebLinkDescription`, `$AccountLinking_WebLinkSteps`, `$AccountLinking_WebLinkCheck`.
- `MenuTypeLinkComplete (4)` → `CompleteScreen`; sets `text0` = `$AccountLinking_Complete`.
- `MenuTypeLinkDisabled (5)` → `CompleteScreen`; sets `text0` = `$AccountLinking_Failed`.

**Key methods:** `moveHighlight`, `activateSelection`, `focusMenu`, `unfocusMenu`, `setWebCode`, `toggleHidePassword`, `closeSubMenus` (cancels birth-year dropdown on back).

### `LinkingScreen` (extends `UIComponent`)
Abstract base for `NewUserScreen` and `ExistingUserScreen`. Owns `curSelection:int` and `selectableItems:Array`. Implements `moveHighlight` (wrapping), `highlightSelection` (moves stage focus to the active `inputText`, colors checkbox yellow), `unhighlightSelection` (resets).

### `NewUserScreen` (extends `LinkingScreen`) — [Embed symbol102]
New-account form: email, password, birth-year ComboBox, opt-in toggle, ToS checkbox, confirm button.

**Key logic:**
- `setUpYearOfBirthComboBox()` — populates a `KiwiComboBox` with 100 years descending from `new Date().fullYear`, scroll size 20.
- `activateSelection()` — when the ComboBox item is selected, opens/closes the dropdown; when the confirm button is selected, calls `ExternalInterface.call("OnLinkNew", email, password, age, optIn, acceptTOS)`.
- `moveHighlight` — redirects D-pad into the ComboBox dropdown while `navigatingDropdown` is true.
- `toggleHidePassword()` — flips `passwordInput.inputText.displayAsPassword`.
- translate keys: `$AccountLinking_Email`, `$AccountLinking_Password`, `$AccountLinking_HidePassword`, `$AccountLinking_YearOfBirth`, `$AccountLinking_CreateAccount`, `$AccountLinking_OptInAgree`.

### `ExistingUserScreen` (extends `LinkingScreen`) — [Embed symbol104]
Log-in form: email, password, confirm button.

- `activateSelection()` — calls `ExternalInterface.call("OnLinkExisting", email, password)` on confirm; otherwise focuses the input field.
- translate keys: `$AccountLinking_Email`, `$AccountLinking_Password`, `$AccountLinking_LogIn`, `$AccountLinking_LinkViaWeb`, `$AccountLinking_ButtonLegendBack`.

### `WebCodeScreen` (extends `MovieClip`) — [Embed symbol13]
Pure display clip: exposes `text0`, `text1`, `text2`, `codeUrl`, `webCode` (inner clip with `header` and `codeText`), and `buttonLegend`. Populated entirely by `RightPane.setWebCode()`.

### `PromoScreen` (extends `MovieClip`) — [Embed symbol18]
Single-field display clip: `text0`. Populated by `RightPane`.

### `CompleteScreen` (extends `MovieClip`) — [Embed symbol106]
Single-field display clip: `text0`. Used for both success and disabled/failure states depending on which translate key is set.

### `btnGreenWide` (asset class — button skin used by `LinkingScreen` for confirm buttons)

### Timeline symbols (`AccountLinking_fla` package) — 6 classes
`Frame_2`, `InputComboBox_57`, `OptInToggle_54`, `LeftPaneOption_6`, `InputBox_51`, `acceptTOS_55` — timeline symbols / component-inspector wrappers; no custom logic beyond `stop()` frame scripts.

---

## Notable logic

- **Pane focus model:** Focus alternates between `LeftPane` and `RightPane` via `switchFocus()` (cyclic). Only the focused pane responds to navigation Iggy callbacks. The unfocused pane has its background dimmed (`gotoAndStop("normal")`).
- **Platform gate:** `_platform != PLATFORM_DURANGO` hides the `accountNameBorder` and `accountNameText` on non-Xbox platforms.
- **Orbis media:** Selecting "Play Now" from the left pane calls `ExternalInterface.call("EnableOrbisMedia", false)` (PS4-specific video control); going back to the promo re-enables it with `true`.
- **Birth-year age calculation:** Age sent to server is `currentYear - selectedYear`; the raw year is never transmitted.
- **IggyTween pulsing highlight:** `LeftPane.highlightSelection` chains two `IggyTween` instances (fade out then fade in) to create a continuous pulse on the selected item's highlight child.
