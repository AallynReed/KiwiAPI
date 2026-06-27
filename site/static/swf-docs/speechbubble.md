# speechbubble.swf

> Displays a floating speech-bubble overlay anchored to the bottom-center of the screen, used in Trove for NPC or contextual dialogue. The bubble fades in and out on demand with a short alpha tween and redraws itself procedurally to fit the current text.

**Document/main class:** `SpeechBubble` (extends `MovieClip`)
**SWF-specific classes:** 2

---

## Main class: `SpeechBubble`

Owns the two `IggyTween` instances that handle fade-in and fade-out, and exposes `setText` and `show` as ExternalInterface callbacks for the game engine. In non-Iggy (preview) mode it self-populates with test text. After every text change or tween completion, it requests a redraw via `ExternalInterface.call("RequestRedraw")` / `ExternalInterface.call("TweenComplete")` using a deferred `Event.RENDER` listener so the engine can composite the frame.

### Public methods

- `setText(param1:String, param2:uint) : void` — sets `bubble.text` and `bubble.textColor`, then calls `recenterBubble()` and schedules a `RequestRedraw` on the next render event.
- `show(param1:Boolean) : void` — starts the appropriate fade tween (in or out). Guards against re-triggering an already-playing tween or a no-op alpha state.

### Key fields

- `bubble : MovieClip` — the embedded `Bubble` symbol instance; holds the text field and drawn shape.
- `fadeInTween : IggyTween` — tweens `alpha` from `0` to `1` over `TWEEN_TIME` (0.2 s); `motionFinishCallback` → `tweenComplete`.
- `fadeOutTween : IggyTween` — tweens `alpha` from `1` to `0` over `TWEEN_TIME` (0.2 s); `motionFinishCallback` → `tweenComplete`.
- `TWEEN_TIME : Number` (const `0.2`) — duration of both fade tweens in seconds.

### Runtime dependencies & integration

- **`IggyFunctions.inIggy`** — branch guard; ExternalInterface callbacks are only registered when running inside the Iggy runtime.
- **ExternalInterface callbacks registered**: `setText`, `show`.
- **ExternalInterface calls made**: `RequestRedraw` (after text change and after tween finishes), `TweenComplete` (after fade tween finishes, to signal the engine).
- **Events**: `Event.RENDER` used as a deferred one-shot hook after text/tween changes; listener is always removed after firing.

### Notable logic

- `recenterBubble()` positions `bubble` so it is horizontally centered and bottom-aligned to the stage: `x = (stageWidth - bubble.bubbleContainer.width) / 2`, `y = stageHeight - bubble.bubbleContainer.height`.
- `show()` cross-stops the opposite tween before starting the requested one, preventing simultaneous fade conflicts.

---

## Other game-specific classes

### `Bubble`

A self-drawing speech-bubble component extending `MovieClip`. Embeds symbol `symbol4` from `/_assets/assets.swf`.

Contains `bubbleContainer : MovieClip` (holds the procedurally drawn shape) and `textField : TextField`.

**Shape constants** — `CORNER_WIDTH 30`, `CORNER_HEIGHT 20`, `STEM_WIDTH 38`, `STEM_HEIGHT 34`, `EDGE_MIN 4`, `WIDTH_MIN 98`, `HEIGHT_MIN 40`.

When `text` is set, `recalculateSize()` expands the text field to fit content (respecting minimums), chooses `CENTER` alignment for single-line text and `LEFT` for multi-line, then calls `drawBubble(w, h)`. `drawBubble` clears and redraws a rounded-rectangle with a downward stem using `graphics.curveTo` / `lineTo` on a `Shape` added to `bubbleContainer`. The fill is near-opaque black (`0`, alpha `0.9`) with a 3-px white outline.

**Properties**: `text : String` (get/set via `textField.htmlText`), `textColor : uint` (get/set via `textField.textColor`).

---

## Notable logic

- The bubble shape is entirely procedural — no bitmaps. Rounded corners are drawn with `curveTo`, the stem is a triangle protruding from the bottom edge, offset to appear center-left of the bubble bottom.
- Text alignment is dynamic: single-line text is centered; multi-line wraps left-aligned.
- `bubbleContainer.numChildren == 0` guard ensures `bubbleShape` is only added once; subsequent `drawBubble` calls just clear and re-draw the same `Shape`.
