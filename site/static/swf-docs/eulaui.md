# eulaui.swf
> Modal dialog shown at first launch (or on demand) requiring the player to read and accept the Trove End User Licence Agreement, and a secondary screen displaying third-party software credits/licences. Supports PC and console variants with scrollable text and Agree/Disagree buttons.

**Document/main class:** `EULAUI` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 9 (main + 4 `EULAUI_fla` timeline classes + 4 top-level helpers)

## Main class: `EULAUI`

Owns two child `MovieClip` panels — `eulaDialog` and `thirdPartyDialog` — and switches between them based on which viewer is loaded. Registers `ExternalInterface` callbacks in the constructor (before `configUI`). The EULA text is fed in via the `text` setter which triggers a `draw()` cycle. Console-specific callbacks (`scrollInDirection`, `setViewOnly`) are registered only when `IsConsole()` is true.

### Public methods

- `set text(value:String) : void` — stores EULA text, re-enables Agree/Disagree buttons, and invalidates `DATA` to trigger `draw()`.
- `setViewOnly(viewOnly:Boolean) : void` — hides accept UI and relabels Disagree as "$Close" for read-only display (e.g. viewing EULA from settings rather than at first launch).

### Key fields

- `eulaDialog : MovieClip` — main EULA panel (symbol `EulaDialog_1`); contains `scrollbar`, `agreeButton`, `disagreeButton`, `mainContent`, `eulaInformation` text, `acceptButtonImage`.
- `thirdPartyDialog : MovieClip` — third-party credits panel (symbol `ThirdPartyDialog_22`); contains `okButton`, `thirdPartyMainContent`.
- `_text : String` — cached EULA / third-party text string.
- `_viewOnly : Boolean` — suppresses Agree button when true.

### Frame scripts / timeline

- **frame 1** (`stop()`) — PC layout; both dialogs in default position.
- **frame 11** (`stop()`) — Console layout; sends `eulaDialog` and `thirdPartyDialog` to `"Console"` label.
- **frame 21** (`stop()`) — Console + localisation layout; sends both dialogs to `"ConsoleLoc"` label.

### Runtime dependencies & integration

- `ExternalInterface` callbacks: `ON_RESIZE`, `loadEULAViewer`, `loadThirdPartyViewer`; console-only: `scrollInDirection`, `setViewOnly`, `onCancel`.
- `ExternalInterface` calls out: `EULA.AGREE`, `EULA.DISAGREE`, `On3rdPartyOK`.
- `IggyFunctions.inIggy` gate (inherited via `configUI` → `setupTranslation()`).
- Mouse-wheel on `eulaDialog` adjusts `eulaDialog.scrollbar.scrollPosition` directly.
- `scrollInDirection` applies scaled velocity to whichever scroll bar is active (uses `lineScrollSize` and `pageScrollSize`).
- Third-party content is rendered by instantiating `ThirdPartyText`, sizing its `textfield`, setting `htmlText`, and calling `SetContentSize` / `updateScrollbar` on the `ScrollableView`.
- `ON_RESIZE` callback centres both dialogs on stage.

## Other game-specific classes

### `EULAUI_fla.EulaDialog_1` (extends `MovieClip`) — Embed symbol91
Timeline symbol for the EULA panel. Exposes `scrollbar:UIScrollBar`, `agreeButton:BtnGreen`, `disagreeButton:BtnGreen`, `mainContent:MovieClip` (the text area), `acceptButtonImage:btn_console_south`, `eulaInformation:TextField`, and a `WindowHeaderSmall` titled `$EULA_Header`. Frame 1 = PC scrollbar targets `eulaMainContent`; frames 11/21 = console, retargets scrollbar to `mainContent` and sends `mainContent` to `"Console"`.

### `EULAUI_fla.eulaMainContent_4` (extends `MovieClip`) — Embed symbol87
Simple two-frame clip holding `textfield:TextField` and a `mouseBlocker:MovieClip`. The `textfield` is the target of the EULA `UIScrollBar`.

### `EULAUI_fla.ThirdPartyDialog_22` (extends `MovieClip`) — Embed symbol98
Third-party panel. Holds `okButton:BtnGreen` (label `$OK`), `thirdPartyMainContent:MovieClip` (contains a `ScrollableView` named `scroll`), and `WindowHeaderSmall` titled `$ThirdParty_Header`. Sends `thirdPartyMainContent` to `"Console"` on frames 11/21.

### `EULAUI_fla.thirdPartyMainContent_23` (extends `MovieClip`) — Embed symbol94
Inner clip for third-party scroll area: `scroll:ScrollableView`, `textfield:TextField`, `mouseBlocker:MovieClip`.

### `ThirdPartyText` (extends `MovieClip`) — Embed symbol11
Minimal display object containing `textfield:TextField`. Created dynamically, sized to `ScrollableView` viewport width, then added as child of the `ScrollableView` for third-party content rendering.

### `BtnGreen` (extends `_kiwi.Controls.LabelButton`) — Embed symbol74
Reusable styled green button; four frame stop-scripts at frames 10/20/30/40 for up/over/down/disabled states.

### `btn_console_south` (extends `MovieClip`)
Plain `MovieClip` used as the console "A/South" button image on the accept prompt. No additional logic.

### Asset image wrappers (7 classes — no logic)
`iggylogo/jpg`, `OpenSSL_logo/png`, `grannylogo/png`, `curl_logo/png`, `bink/jpg`, `Bullet_Physics_Logo`, `wwise_logo`, `popcornfx_logo` — embedded third-party logo bitmaps/movie clips used in the third-party credits screen. Also: `btn_XBOne_A/png` — Xbox One A-button image.

## Notable logic

- **Dual-mode scrolling:** EULA uses a `UIScrollBar` component targeting the raw text field; third-party content uses `_kiwi.Controls.ScrollableView` with a dynamically added `ThirdPartyText` child, allowing HTML-formatted credits.
- `scrollInDirection` clamps input to ±20, then derives a velocity from `lineScrollSize` and `pageScrollSize`, enabling smooth analogue-stick scrolling on console.
- Translate keys present: `$EULA_Header`, `$EULA_Agree`, `$EULA_Disagree`, `$ThirdParty_Header`, `$OK`, `$Close`.
