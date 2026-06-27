# terraformoverview.swf

> Shows a countdown overlay during a Terraformer activation sequence in Trove, displaying a live ticking timer and cancellation instructions. The countdown is driven by a Flash `Timer` calibrated against `getTimer()` for drift correction, and is started by a game-engine callback that passes the initial number of seconds remaining.

**Document/main class:** `TerraformOverview` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 1

## Main class: `TerraformOverview`

`TerraformOverview` is the document class and the only game-specific class in this SWF. The constructor registers two ExternalInterface callbacks (gated on `IggyFunctions.inIggy`) and calls `setupTranslation()`. The countdown timer is not started at construction; it starts only when the engine calls `startCountdown`. Each tick recomputes elapsed time from the original `getTimer()` snapshot rather than simply decrementing, preventing drift.

### Public methods

- `startCountdown(param1:Number) : void` — Stores `param1` as `initialSecondsLeft`, records `countdownStartMillisecs = getTimer()`, updates the display immediately, creates a repeating 1-second `Timer`, and starts it.
- `setCancelInstructions(param1:String) : void` *(private)* — Sets `cancelTextField.text = param1`.

### Key fields

- `countdownStartMillisecs : int` — Millisecond timestamp (from `getTimer()`) captured when `startCountdown` is called; used as the reference point for elapsed-time calculation.
- `initialSecondsLeft : Number` *(default 0)* — The total seconds passed to `startCountdown`; kept for the elapsed-time formula.
- `timer : Timer` *(default null)* — A repeating `Timer(1000, 0)` (fires every second, indefinitely) that drives `onTimerInterval`.
- `countdownText : TextField` — Displays the formatted countdown string.
- `cancelTextField : TextField` — Displays the cancel/abort instruction text set by the engine.

### Runtime dependencies & integration

- **`IggyFunctions.inIggy`** — Guards callback registration.
- **ExternalInterface callbacks registered:**
  - `"startCountdown"` → `startCountdown(Number)` — begins the visible timer.
  - `"setCancelInstructions"` → `setCancelInstructions(String)` — sets cancellation hint text.
- **`IggyFunctions.translate` key used:**
  - `"$TerraformCountdownText"` — localizable string template; the literal `{seconds}` placeholder is replaced with `int(secondsLeft)` via `String.replace`.
- **`TimerEvent.TIMER`** — listened on the internal `Timer` instance.
- No outbound ExternalInterface calls; no frame scripts.

## Other game-specific classes

None beyond `TerraformOverview`.

## Notable logic

- The countdown uses a drift-correcting formula: `remaining = initialSecondsLeft - (getTimer() - countdownStartMillisecs) / 1000`. This ensures the displayed value stays accurate even if Flash timer callbacks are delayed by frame rate drops.
- The timer is configured with `repeatCount = 0` (infinite) and stops itself when `remaining <= 0`. There is no ExternalInterface call on expiry — the game presumably hides the overlay based on its own server-side event.
- Seconds are truncated to integer via `int(param1)` before substitution, so the display counts down in whole seconds.
