# credits.swf
> A scrolling credits roll dialog shown in Trove when the player views the game credits. It displays HTML-formatted credits text that auto-scrolls upward at a fixed speed, ending with a callback to the game engine. On console builds a close button (B button icon) is also shown.

**Document/main class:** `Credits` (extends `_kiwi.Core.UIComponent`) — no top-level embed; dialog content is via `Credits_fla.Dialog_1` (embeds `symbol33` from `/_assets/assets.swf`)  
**SWF-specific classes:** 12

---

## Main class: `Credits`

`Credits` is the root UIComponent for the credits screen. Its constructor wires up the close button and, when running inside the Iggy runtime, registers an `OnRequestClose` ExternalInterface callback. On non-console platforms the `iconClose` (Xbox B-button glyph) is hidden. Data arrives via the `text` setter — once set it triggers a full redraw and starts the scroll animation.

### Key fields

- `dialog : MovieClip` — public reference to the `Dialog_1` timeline symbol; contains the header, close button, `mainContent` (scroll area), and `iconClose`.
- `ScrollSpeed : Number = 70` — pixels per second the credits text scrolls upward (constant).
- `scrolling : Boolean` — unused in the current code; scroll state is inferred from listener presence.
- `lastFrameTime : int` — timestamp captured each `ENTER_FRAME` tick via `getTimer()` for delta-time scroll calculation.
- `_text : String` — the raw HTML credits string received from the engine.

### Frame scripts / timeline

`Credits_fla.Dialog_1` installs frame scripts via `addFrameScript` indirectly through `BtnGreen` (frames 9, 19, 29, 39 — all call `stop()`), corresponding to the four button states (up, over, down, disabled) of the green close button.

### Runtime dependencies & integration

- **`IggyFunctions.inIggy`** — checked in constructor; if `true`, `ExternalInterface.addCallback("OnRequestClose", onRequestClose)` registers the close handler so the C++ game layer can trigger it.
- **`ExternalInterface.call("OnRequestClose")`** — fired when the user clicks the close button (or the engine calls the callback). Signals the game to dismiss the credits screen.
- **`ExternalInterface.call("OnCreditsRollFinished")`** — fired by `onEnterFrame` when the credits text has fully scrolled past the top boundary (`creditsFinished()` returns `true`). Notifies the game engine that the roll is complete.
- **`Event.ENTER_FRAME`** — listener added in `draw()` when new text data arrives; drives per-frame delta-time scrolling; removed when roll finishes.
- **`InvalidationType.DATA`** — standard Kiwi invalidation flag; triggers `draw()` override which resets scroll position, sets `htmlText` on the text field, auto-sizes it (`textfield.height = textfield.textHeight`), and restarts the enter-frame loop.
- **`configUI()` / `setupTranslation()`** — `configUI` override calls the inherited `setupTranslation()` to apply localized string substitutions.
- **translate keys** (set in `Dialog_1` component properties):
  - `$Credits_Header` — window header title string.
  - `$Close_ButtonLegend` — label on the green close button.
- **`IsConsole()`** — global function (not in this SWF's source; injected by runtime or framework) used to conditionally show/hide the `iconClose` console button glyph.

---

## Other game-specific classes

- `Credits_fla.Dialog_1` — timeline symbol class (embeds `symbol33` from `/_assets/assets.swf`); the main dialog shell containing a `WindowHeader` (`__id0_`, title `$Credits_Header`), a `BtnGreen` close button (label `$Close_ButtonLegend`), a `btn_console_east` icon (`iconClose`), and a `mainContent` MovieClip holding the scrolling `textfield`.
- `BtnGreen` — (embeds `symbol19` from `/_assets/assets.swf`) extends `_kiwi.Controls.LabelButton`; 40-frame button with 4 states (up/over/down/disabled), each halted by a `stop()` frame script. Used as the close button.
- `btn_console_east` — bare `MovieClip` subclass; represents the console East-button (B) glyph shown on console builds next to the close button.
- **8 embedded bitmap asset classes** (pure `BitmapData` wrappers, no logic): `trionteam2018` (1230×813 jpg), `team_photo` (800×400 jpg), `bsgstudio` (828×315 jpg), `trionteam` (980×702 jpg), `grannylogo.png` (2255×864 png), `wwise_logo` (679×204 png), `iggylogo.jpg` (1079×1379 jpg), `binklogo.jpg` (1500×1518 jpg), plus `btn_XBOne_B.png` (64×64 png). These are embedded image assets for display within the scrolling credits content area.

---

## Notable logic

- **Delta-time scroll:** `onEnterFrame` computes elapsed seconds as `(getTimer() - lastFrameTime) / 1000` and subtracts `ScrollSpeed * elapsed` from the text field's Y position each frame, giving frame-rate-independent scrolling at 70 px/sec.
- **Scroll reset:** `resetCreditsRoll()` positions `textfield.y` below the bottom of the `bounds` clip, so text starts off-screen and scrolls up into view.
- **Finish detection:** `creditsFinished()` checks whether `bounds.y > mainContent.textfield.y + mainContent.textfield.height` — i.e., the bottom edge of the text has scrolled above the top of the visible area — then fires `OnCreditsRollFinished` and removes the enter-frame listener.
- **Disabled close during scroll:** The close button starts disabled (`closeButton.enabled = false`) in the constructor and is only re-enabled once `text` is set (i.e., real content has arrived from the engine).
- **HTML content:** Credits text is set via `htmlText`, allowing the game engine to pass rich-text HTML (bold names, font tags, etc.) as the credits payload.
