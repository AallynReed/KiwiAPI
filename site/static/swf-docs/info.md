# info.swf

> The Trove in-game chat / info-message log that displays timed text messages (channel messages, system notifications) in a scrolling stack. Messages auto-expire after 8 seconds and fade out over 0.5 seconds before being reaped. Supports a reduced capacity mode for Nintendo Switch (NX).

**Document/main class:** `Info` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 3

---

## Main class: `Info`

`Info` is the root document class. It holds a single child `infoLog:InfoLog` and acts as the Iggy entry point. Its only job is to register the `ADD_MESSAGE` callback and route messages to the correct `InfoLog` method depending on platform.

### Key fields

| Field | Type | Role |
|---|---|---|
| `infoLog` | `InfoLog` | The scrolling message log component |

### Runtime dependencies & integration

**`ExternalInterface.addCallback`:**
- `ADD_MESSAGE(channel, message, author, color)` → routes to `infoLog.onInfoMessageNX(...)` on NX, otherwise `infoLog.onInfoMessage(...)`.

---

## Other game-specific classes

### `InfoLog` (embeds `/_assets/assets.swf#symbol4`)

Extends `com.kiwi.Core.KiwiComponent`. Manages two arrays (`_renderers`, `_pendingRenderers`) and a 100 ms reaper `Timer`. All rendering happens inside a `Sprite` child named `container`.

**Constants:**
- `MaxMessages = 9` — maximum visible messages (PC/default).
- `MaxMessagesNX = 5` — maximum visible messages on NX.
- `MessageDuration = 8000` ms — time before a message begins fading.

**Key methods:**
- `onInfoMessage(index, channel, author, message, color)` — creates an `InfoItemRenderer`, sets its `expireTime` and `reapTime`, adds it to `container` and `_pendingRenderers`, enforces `MaxMessages` by trimming the oldest entry if over the limit, then calls `validateNow()`.
- `onInfoMessageNX(...)` — identical but calls `renderer.ConfigNX()` first and enforces `MaxMessagesNX`.
- `draw()` — flushes `_pendingRenderers` into `_renderers`, then lays out all renderers stacked upward (newest at bottom, oldest at top): iterates renderers in reverse, accumulating negative Y offsets so the list grows upward.
- `onReaperTimerComplete(TimerEvent)` — fires every 100 ms; removes renderers whose `reapTime` has passed (calls `stopFadeOut()` and removes from `container`), and calls `startFadeOut()` on renderers past their `expireTime`.
- `enforceMessageLimit(count, max)` — shifts and removes the oldest renderer from `_renderers` until within the limit.

**Timer:** `_reaperTimer = new Timer(100)` started in `config()`.

---

### `InfoItemRenderer` (embeds `/_assets/assets.swf#symbol3`)

Extends `com.kiwi.Templates.KiwiButton`, implements `com.kiwi.Interfaces.IListItemRenderer`. Renders a single info message line.

**Static constants:**
- `s_fadeOutSeconds = 0.5` — fade duration in seconds.
- `s_format` — `TextFormat("Comfortaa", 14, white, bold)` for PC.
- `s_formatNX` — `TextFormat("Comfortaa", 20, white, bold)` for NX (larger text).

**Key fields:**
- `primaryText : TextField` — the visible message text.
- `expireTime : Number` — Unix ms timestamp when the message starts fading.
- `reapTime : Number` — Unix ms timestamp when the renderer is removed (`expireTime + 500`).
- `_fadeOut : IggyTween` — tween driving the alpha fade (uses `IggyTween` with `fl.transitions.easing.None.easeOut`).

**Key methods:**
- `setData(obj)` — sets `_color`, then sets `primaryText.text` via `formatMessage(channel, author, message)` (which currently just returns `message`, ignoring channel/author).
- `ConfigNX()` — switches `primaryText.defaultTextFormat` to `s_formatNX`.
- `height` (override) — returns `primaryText.textHeight` so the log layout uses actual rendered text height.
- `startFadeOut()` — creates an `IggyTween` animating `alpha` from 1 → 0 over 0.5 s.
- `stopFadeOut()` — stops and nulls the tween.

**`IggyTween` usage:** `new IggyTween(this, "alpha", None.easeOut, 1, 0, s_fadeOutSeconds, true)` — the `true` final argument likely auto-starts the tween.

---

## Notable logic

- Messages are stacked upward: `draw()` iterates `_renderers` from last to first, subtracting each renderer's height from a running Y offset and assigning it as `renderer.y`. This means newer messages appear at the bottom.
- The reaper loop runs every 100 ms (not frame-rate dependent), giving consistent message lifetime regardless of frame rate.
- `formatMessage` is a static stub that ignores `channel` and `author`, displaying only the raw `message` string — channel/author formatting may be done server-side before the string is passed in.
- NX path enforces both a smaller display cap (5 vs 9) and a larger font size (20 vs 14).
