# background.swf

> Fullscreen animated background displayed during Trove's login/launcher screen and loading states. Handles aspect-ratio-correct scaling of the background image, status/version text overlays, and — on console builds — multi-frame timeline navigation for different background states.

**Document/main class:** `Background` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 1

---

## Main class: `Background`

`Background` is the sole game-specific class. It fills the entire display with a scaled background image (`imageMC`) and overlays a status bar (`statusDisplay`) that shows connection/loading status text and a build version string. On PC, a close button (`closeButton`) is also present. On console builds, the close button is forcibly hidden, a `centerText` region tracks proportionally with the image during resize, and a `setFrame` callback allows the C++ engine to switch between labelled timeline frames (e.g., different background art states).

All interactivity is driven exclusively via `ExternalInterface` callbacks registered at startup (only when `IggyFunctions.inIggy` is `true`).

### Public / internal methods

- `updateStatus(param1:String) : void` — sets `statusDisplay.txt_status.text`; displays connection or loading state strings pushed from the engine.
- `updateVersion(param1:String) : void` — sets `statusDisplay.txt_version.text`; displays the game build string.
- `frame1() : *` — timeline frame-script for frame 1: calls `stop()`.
- `frame11() : *` — timeline frame-script for frame 11: calls `stop()`.
- `frame21() : *` — timeline frame-script for frame 21: calls `stop()`.

### Private methods

- `onStageResize(w:int, h:int) : void` — scales and repositions `imageMC` (or `replacementImage` if set) to fill the given dimensions while maintaining the original image aspect ratio; uses a height-first then width-fill fallback strategy. Also repositions `closeButton`, `statusDisplay`, and — on console — `centerText`.
- `setFrame(label:String) : void` — console-only; navigates the main timeline to the named frame label, then re-arms the `onEnterFrame` close-button-hide loop.
- `replaceBackgroundImage(textureName:String) : void` — instantiates an `ObjectPreview` (dynamic texture loader) using the initial `imageMC` dimensions, assigns `textureName`, and inserts it just above `imageMC` in the display list; subsequent resize calls use the `ObjectPreview` path.
- `replaceText(text:String) : void` — sets `setableText.text`.
- `onEnterFrame(e:Event) : void` — console-only one-shot; hides `closeButton` (forces `alpha = 0`, `visible = false`) and removes itself.

### Key fields

| Field | Type | Role |
|---|---|---|
| `imageMC` | `MovieClip` | The static background image symbol (positioned and scaled to fill the screen). |
| `cropMask` | `MovieClip` | Mask clip applied to the image area. |
| `statusDisplay` | `MovieClip` | Bottom bar housing `txt_status` and `txt_version` TextFields, plus a `statusBG` shape that stretches to full width. |
| `closeButton` | `MovieClip` | PC-only close/exit button; hidden (alpha 0) on console. |
| `loadingAnim` | `MovieClip` | Loading spinner animation. |
| `centerText` | `MovieClip` | Console-only overlay region; position/size tracked proportionally to `imageMC` on resize. |
| `setableText` | `TextField` | Arbitrary text overlay, populated via the `replaceText` callback. |
| `replacementImage` | `ObjectPreview` | Set when `replaceBackgroundImage` is called; used in place of `imageMC` for resize and rendering. |
| `screenAspect`, `imageAspect` | `Number` | Pre-computed aspect ratios used in resize math. |
| `heightRatio`, `widthRatio` | `Number` | Pre-computed ratios of screen dimension to original image dimension. |
| `PlayRelativeX/Y/Width/Height` | `Number` | Console-only: proportional coordinates of `centerText` within `imageMC`, computed once in the constructor for use during resize. |

### Frame scripts / timeline

Three frames with `stop()` scripts at frames 1, 11, and 21. The console-only `setFrame(label)` callback drives navigation between these (and potentially other labelled frames representing different background art states).

### Runtime dependencies & integration

- **`IggyFunctions.inIggy`** — boolean gate; all `ExternalInterface` registration is skipped when running outside the Iggy/Scaleform runtime (e.g., in the Flash authoring tool).
- **`IsConsole()`** — free global function (from `IggyFunctions`); branches the constructor to hide the close button, compute proportional text positions, and register the `setFrame` callback.
- **ExternalInterface callbacks registered:**
  - `ON_RESIZE(w, h)` — resize and reposition handler.
  - `UPDATE_STATUS(str)` — status text updater.
  - `UPDATE_VERSION(str)` — version string updater.
  - `replaceBackgroundImage(textureName)` — swap background to a named texture.
  - `replaceText(str)` — update `setableText`.
  - `setFrame(label)` *(console only)* — navigate the timeline.
- **No ExternalInterface outbound calls** are made by this SWF.
- **`_kiwi.Core.ObjectPreview`** — used as the dynamic-texture image replacement when `replaceBackgroundImage` is called.
- No `translate()`/`$`-key localisation strings are used.
- No timers or custom events.

---

## Other game-specific classes

None. `Background` is the only game-specific class. The remainder of the SWF's scripts are shared framework files (`_kiwi/`, `fl/`) and the excluded `IggyFunctions.as`.

---

## Notable logic

- **Aspect-ratio fill strategy:** `onStageResize` first scales the image by `heightRatio` (height-first), producing a width. If that width is smaller than the stage width, it falls back to filling the full width (cropping height). This ensures the image always covers the entire screen without letterboxing.
- **Dynamic texture replacement:** `replaceBackgroundImage` layers a live `ObjectPreview` on top of the static symbol so the engine can push any texture by name at runtime without reloading the SWF.
- **Console close-button suppression:** Because the close button must not appear on consoles (the engine manages navigation), the constructor hides it and schedules an `ENTER_FRAME` listener that re-hides it every frame until confirmed invisible — guarding against any frame or layout pass that might re-show it after initialisation.
