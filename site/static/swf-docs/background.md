# background.swf

> Fullscreen animated background displayed during Trove's login/launcher screen and loading states. Handles aspect-ratio-correct scaling of the background image, status/version text overlays, an optional game-mode name panel, and — on console builds — multi-frame timeline navigation for different background states.

**Document/main class:** `Background` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 1

---

## Main class: `Background`

`Background` is the sole game-specific class. It fills the entire display with a scaled background image (`imageMC`) and overlays a status bar (`statusDisplay`) that shows connection/loading status text and a build version string. On PC, a close button (`closeButton`) is also present. On console builds, the close button is forcibly hidden, a `centerText` region would track proportionally with the image during resize (no `centerText` instance is placed in this SWF), and a `setFrame` callback allows the C++ engine to switch between the labelled timeline frames, which carry identical content here.

All interactivity is driven exclusively via `ExternalInterface` callbacks registered at startup (only when `IggyFunctions.inIggy` is `true`).

### Public / internal methods

- `updateStatus(param1:String) : void` — sets `statusDisplay.txt_status.text`; displays the login/loading step strings pushed from the engine (the 25 `$LoadingStatus_*` keys in `ui.binfab` / `new.binfab`), each call overwriting the last.
- `updateVersion(param1:String) : void` — sets `statusDisplay.txt_version.text`; displays the game build string.
- `frame1() : *` — timeline frame-script for frame 1: calls `stop()`.
- `frame11() : *` — timeline frame-script for frame 11: calls `stop()`.
- `frame21() : *` — timeline frame-script for frame 21: calls `stop()`.

### Private methods

- `onStageResize(w:int, h:int) : void` — scales and repositions `imageMC` (or `replacementImage` if set) to fill the given dimensions while maintaining the original image aspect ratio; uses a height-first then width-fill fallback strategy. Also repositions `closeButton`, `statusDisplay`, and — on console — `centerText`.
- `setFrame(label:String) : void` — console-only; navigates the main timeline to the named frame label, then re-arms the `onEnterFrame` close-button-hide loop.
- `replaceBackgroundImage(textureName:String) : void` — instantiates an `ObjectPreview` (dynamic texture loader) using the initial `imageMC` dimensions, assigns `textureName`, and inserts it just above `imageMC` in the display list; subsequent resize calls use the `ObjectPreview` path. Every call builds a new preview and none is ever removed, so earlier previews stay on the display list (each new one is inserted directly above `imageMC`, beneath the older ones).
- `replaceText(text:String) : void` — sets `setableText.text`.
- `setGameModeName(name:String) : void` — shows `name` in a panel drawn in code at the bottom-right corner, just above `statusDisplay`; an empty or `null` name hides the panel. The panel is built on first use and re-laid out on every resize.
- `buildGameModePanel() / layoutGameModePanel() : void` — create the panel (`modeInfo` sprite holding `modePanelBG` and the `modeName` TextField) and size it: scale = stage height / 1080 clamped to 0.7–2.2; text in `Open Sans Bold` at 26 × scale, white; 22 × 10 padding, 28 margin, minimum width 180 (all × scale); rounded rect filled `0x0B1E2C` at 0.75 alpha with a `0x3E5A6E` outline at 0.9 alpha.
- `onEnterFrame(e:Event) : void` — console-only one-shot; hides `closeButton` (forces `alpha = 0`, `visible = false`) and removes itself.

### Key fields

| Field | Type | Role |
|---|---|---|
| `imageMC` | `MovieClip` | Chid 14: an opaque pure-white 1920×1080 bitmap plate at depth 1 (positioned and scaled to fill the screen); the engine's picture arrives above it through `replaceBackgroundImage`. |
| `cropMask` | `MovieClip` | Declared only: no instance is placed in this SWF and no code references it. |
| `statusDisplay` | `MovieClip` | Chid 328, bottom bar housing `txt_status` and `txt_version` TextFields, a `statusBG` clip that stretches to full width, and an unnamed 150-frame travelling glow (chid 327). |
| `closeButton` | `MovieClip` | `_kiwi.Controls.WindowCloseButton` (chid 11); PC-only close/exit button; hidden (alpha 0) on console. |
| `loadingAnim` | `MovieClip` | Chid 316: the tumbling-cube loading animation, 301 frames of vector art. |
| `centerText` | `MovieClip` | Console-only overlay region; position/size tracked proportionally to `imageMC` on resize. Not placed in this SWF, so the resize branch is skipped. |
| `setableText` | `TextField` | Arbitrary text overlay (chid 330, with an embedded Times New Roman font), populated via the `replaceText` callback. |
| `replacementImage` | `ObjectPreview` | Set when `replaceBackgroundImage` is called; used in place of `imageMC` for resize and rendering. |
| `modeInfo` | `Sprite` | Game-mode name panel (`modePanelBG` shape + `modeName` TextField); `null` until `setGameModeName` first receives a name. |
| `stageW`, `stageH` | `Number` | Last stage size from `ON_RESIZE`, used to lay out the game-mode panel. |
| `screenAspect`, `imageAspect` | `Number` | Pre-computed aspect ratios used in resize math. |
| `heightRatio`, `widthRatio` | `Number` | Pre-computed ratios of screen dimension to original image dimension. |
| `PlayRelativeX/Y/Width/Height` | `Number` | Console-only: proportional coordinates of `centerText` within `imageMC`, computed once in the constructor for use during resize. |

### Frame scripts / timeline

Stage 1920×1080, 30 fps, 30 frames. Frame labels `Background` (1), `Start` (11) and `Loading` (21), each with a `stop()` script; there are no other labels. Everything is placed on frame 1 and no later frame adds or removes anything, so the three labelled frames are visually identical. The console-only `setFrame(label)` callback drives navigation between them. The root holds exactly five named children: `imageMC`, `closeButton`, `loadingAnim`, `statusDisplay`, `setableText`.

### Runtime dependencies & integration

- **`IggyFunctions.inIggy`** — boolean gate; all `ExternalInterface` registration is skipped when running outside the Iggy/Scaleform runtime (e.g., in the Flash authoring tool).
- **`IsConsole()`** — method inherited from `_kiwi.Core.UIComponent` (true on Orbis/Durango); branches the constructor to hide the close button, compute proportional text positions, and register the `setFrame` callback.
- **ExternalInterface callbacks registered:**
  - `ON_RESIZE(w, h)` — resize and reposition handler.
  - `UPDATE_STATUS(str)` — status text updater.
  - `UPDATE_VERSION(str)` — version string updater.
  - `replaceBackgroundImage(textureName)` — swap background to a named texture.
  - `replaceText(str)` — update `setableText`.
  - `setGameModeName(str)` — show or hide the game-mode name panel (added in the October 2026 build).
  - `setFrame(label)` *(console only)* — navigate the timeline.
- **No ExternalInterface outbound calls** are made by `Background` itself. The `ObjectPreview` it creates calls `UIComponent.CheckTextureExists(textureName)` and binds the texture when the engine answers through `objectPreviewReady(textureName)`.
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
- **Console close-button suppression:** Because the close button must not appear on consoles (the engine manages navigation), the constructor sets its alpha to 0 and adds an `ENTER_FRAME` listener that hides it (`alpha = 0`, `visible = false`) on the next frame and removes itself; `setFrame` re-arms the listener after every label jump.
- **Login status sequence:** the login reports its steps in this order: `LS: Version Check`, `AS: Login`, `LS: Requesting login`, `LS: Login finished`, `LS: Added to Login Queue`, `LS: Queue Position: 1`, `LS: Received login response`, `LS: Received user acknowledgement`, `LS: Assigned to world`, `World: Connecting`, `World: Version Check`. `$LoadingStatus_LoadWorld` (`Entering World: {0}%%`) arrives many times with a rising percentage, and a step the engine retries (login-server polling, every five seconds) is re-sent unchanged. Three keys are failures rather than steps: `VersionRejected`, `PlatformSessionCreateFail`, `PlatformSessionJoinFail`.
- **Opacity:** Iggy does not honour the SWF's `SetBackgroundColor` (`#666666`); the screen is opaque only because of the white `imageMC` plate and the engine picture over it.
