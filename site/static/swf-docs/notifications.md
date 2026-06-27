# notifications.swf

> Displays transient text notifications in the Trove HUD. Notifications appear stacked vertically, fade in on arrival, and fade out after a configurable timeout — with a FIFO cap of 3 simultaneous messages.

**Document/main class:** `NotificationManager` (extends `KiwiComponent`)
**SWF-specific classes:** 2

---

## Main class: `NotificationManager`

Manages a live queue of `Notification` instances. On init (`config()`), it registers the `ON_NOTIFICATION` ExternalInterface callback so the game engine can push messages. Incoming notifications are added to `_notifications`, positioned vertically, tweened in, and auto-expired via per-item timers. The FIFO cap (3) forcibly expires the oldest entry when the queue overflows. Departing notifications are tweened out via a single tracked `_goingAway` slot; the display list child is removed only after the tween completes.

### Public methods

- `NotificationManager()` — constructor; calls `super()`.

### Key fields

- `_notifications : Array` — ordered list of active `Notification` instances.
- `_previousHeight : Number` — running Y-cursor used for stacking; initialized to `25`.
- `_goingAway : Notification` — the notification currently fading out (only one at a time).
- `_goingAwayTween : IggyTween` — tween handle for the exit animation; cancelled and restarted if a new expiry arrives before the previous finishes.
- `FIFO_COUNT : Number` (static const `3`) — maximum simultaneous notifications.

### Runtime dependencies & integration

- **ExternalInterface callback** `ON_NOTIFICATION(param1:int, param2:uint, param3:String, param4:Number, param5:int)` — called by the game to push a notification. Parameters are: (unused int), color (uint), message text, timeout duration (ms), font size.
- **IggyTween** — used for both fade-in (`alpha` 0→1) and fade-out (`exit` 1→0) animations, with `None.easeIn` / `None.easeOut`.
- **`invalidateData()`** — triggers `draw()` to re-stack visible notifications via Y-position tweens.

---

## Other game-specific classes

### `Notification`

A single notification item, extending `KiwiComponent`. Embeds symbol `symbol4` from `/_assets/assets.swf`.

#### Constructor
`Notification(callback:Function, timeout:int, size:int, color:uint, msg:String)` — stores all display parameters, creates a one-shot `Timer` for the timeout.

#### Public methods

- `set exit(param1:Number) : void` — combined alpha + Y-offset setter used by the fade-out tween; slides the item upward as it fades.
- `get textHeight() : Number` — returns `textField.textHeight` for stack layout calculations.
- `SetSize(w:int, h:int) : void` — sets the component's width and height.
- `Start() : void` — starts the expiry timer.
- `Stop() : void` — stops the expiry timer.
- `Reset() : void` — resets the expiry timer.

#### Key fields

- `textField : TextField` — primary text display (Comfortaa font, bold, centered).
- `shadow : TextField` — identical text rendered black underneath for a drop-shadow effect.
- `_callback : Function` — reference to `NotificationManager.onNotificationExpired`; called when the timer fires.
- `_timeout : Timer` — one-shot timer; on `TIMER` event, invokes `_callback(this)`.
- `_color : uint` — text color (default white `0xFFFFFF`).
- `_size : uint` — font size (default `20`).

#### Key logic

`config()` applies a `TextFormat` (Comfortaa, bold, centered) to both `textField` and `shadow`, sets the shadow color to black (`0`), and writes `_msg` to both fields simultaneously.

---

## Notable logic

- **Stacking layout**: `draw()` iterates `_notifications` and tweens each item's `y` to a recalculated position (starting at `25`, stepping by `textHeight + 5`). This causes existing notifications to slide up whenever the queue changes.
- **Expiry overlap**: if a notification expires while another is still fading out, the in-progress tween is stopped immediately (`_goingAwayTween.stop()` + `onFinishedExit()`) before starting the new exit tween.
- **FIFO enforcement**: when `_notifications.length > 3`, the oldest entry (`_notifications[0]`) is stopped and immediately passed to `onNotificationExpired`, triggering its exit tween.
