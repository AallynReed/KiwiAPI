# controllernotification.swf
> A small toast-style notification popup that slides in from the bottom of the screen to display short controller-related messages (e.g. item pickups, prompts). Notifications auto-dismiss after a configurable timeout and smoothly animate in and out via IggyTween.

**Document/main class:** `ControllerNotificationManager` (extends `KiwiComponent`)
**SWF-specific classes:** 2

---

## Main class: `ControllerNotificationManager`

Root manager component. Overrides `config()` to register a single `ExternalInterface` callback `sendControllerNotification`. Maintains the currently-displayed `ControllerNotification` and manages clean teardown of an outgoing notification when a new one arrives.

### Public methods

- `sendControllerNotification(message:String) : void` — main entry point called from the game engine. Creates a new `ControllerNotification` centered horizontally on stage, positioned off the bottom edge (`y = stage.height`), with `alpha=0`. If a previous notification exists, immediately fast-exits it by calling `onNotificationExpired`. Adds the new notification as a child, starts its timer, fires sound `Play_ui_minigame_pickup_coin_small` via `ExternalInterface.call("POST_SOUND_EVENT", ...)`, and kicks off an `IggyTween` on the `stepIn` property (easeIn, 0→1 over 10 frames).

### Key fields

- `_notification : ControllerNotification` — the currently active (or most recently created) notification instance.
- `_goingAway : ControllerNotification` — the notification currently animating out.
- `_goingAwayTween` — the `IggyTween` controlling the exit animation; cancelled if a new notification arrives before it completes.
- `_notificationSfx : String` — `"Play_ui_minigame_pickup_coin_small"`, played on each new notification.

### Runtime dependencies & integration

- `ExternalInterface.addCallback` registration: `sendControllerNotification`.
- Outbound `ExternalInterface.call`: `POST_SOUND_EVENT("Play_ui_minigame_pickup_coin_small")`.
- `IggyTween` — drives both `stepIn` (slide+fade in, easeIn, 10 frames) and `stepOut` (slide+fade out, easeOut, 10 frames) animations on `ControllerNotification`.

### Notable lifecycle

`onNotificationExpired(n:ControllerNotification)`:
1. If `_goingAway` is already animating out, stops that tween and calls `onFinishedExit()` immediately.
2. Sets `_goingAway = n` and starts an `IggyTween` on `stepOut` with `motionFinishCallback = onFinishedExit`.

`onFinishedExit()` — removes `_goingAway` from the display list and clears all references.

---

## `ControllerNotification`

Individual notification pop-up. Extends `KiwiComponent`. Embeds from `/_assets/assets.swf` (`symbol6`).

### Constructor

`ControllerNotification(message:String, callback:Function, timeout:int = 5000)` — sets `messageTextArea.text`, auto-sizes the text field vertically, centers it, creates a `Timer` for `timeout` ms.

### Key fields

- `messageTextArea : TextField` — the visible message text.
- `startPosition / targetPosition : Number` — Y coordinates for the tween range (off-screen bottom to on-screen resting position).
- `_callback : Function` — called with `this` as argument when the timer fires.

### Public methods / tween targets

- `set stepIn(val:Number)` — tween setter. Interpolates `y` from `startPosition` toward `targetPosition` and `alpha` from 0 toward 1 as `val` goes 0→1.
- `set stepOut(val:Number)` — tween setter. Reverses: slides back toward `startPosition` and fades `alpha` toward 0 as `val` goes 1→0.
- `Start() / Stop() / Reset()` — delegate to the internal `Timer`.
- `onTimerComplete` — fires `_callback(this)` and stops the timer.

---

## Notable logic

- **Immediate preemption:** if `sendControllerNotification` is called while a notification is already displayed, the existing one is fast-exited (its `stepOut` tween is skipped; `onFinishedExit` is called synchronously) before the new one animates in. This prevents stacking.
- **Double-exit guard:** `onNotificationExpired` checks for an already-in-progress `_goingAway` tween and cancels it before starting a new exit — preventing orphaned display objects if `sendControllerNotification` is called extremely rapidly.
- **IggyTween integration:** both `stepIn` and `stepOut` are custom property setters, allowing `IggyTween` to drive composite animations (Y position + alpha simultaneously) through a single tween target property.
