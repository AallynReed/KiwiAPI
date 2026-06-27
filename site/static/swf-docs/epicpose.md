# epicpose.swf
> The Epic Pose UI panel lets players take a screenshot of their character in a posed state. It provides camera-angle selection via an arrow-select dropdown and rotate left/right buttons (PC only), with console variants that expose a hide-UI toggle and handle camera control through gamepad callbacks.

**Document/main class:** `EpicPose` (extends `UIComponent`)
**SWF-specific classes:** 5

## Main class: `EpicPose`

`EpicPose` is the root document class for the pose/screenshot overlay. It extends `_kiwi.Core.UIComponent`, which handles stage-resize and the `configUI` lifecycle hook. On construction it registers `addFrameScript` handlers for frames 0, 10, and 20, which correspond to PC, console-Durango, and console-Orbis layout states respectively. `configUI` wires all button listeners and, when running inside the Iggy runtime, registers four `ExternalInterface` callbacks so the game engine can push camera-angle data and issue screenshot commands.

Data flows entirely through `ExternalInterface`: the game engine populates the camera-angle dropdown via `addCameraAngle`/`clearCameraAngles`/`selectCameraAngle` callbacks, and the Flash side calls out to `OnScreenshot`, `OnRotateStart`, `OnRotateStop`, and `OnCameraChange` in response to user interaction. Translation keys (`$EpicPose_*`) are resolved by `setupTranslation()` (inherited from `UIComponent`).

### Public methods

- `EpicPose()` — Constructor. Registers frame scripts for frames 0/10/20, sets initial component inspector properties on `__id0_` and `screenshotButton`, and adds a `FRAME_CONSTRUCTED` listener to re-apply per-frame component properties as the timeline advances.

### Private / internal methods

- `configUI() : void` — Overrides UIComponent. Attaches mouse listeners on screenshot and rotate buttons (rotate only on non-console), attaches `CHANGE` listener on `cameraAngleSelect`, registers Iggy callbacks (`addCameraAngle`, `clearCameraAngles`, `selectCameraAngle`, `onScreenshotConsole`, `setHideUILabel`), and calls `setupTranslation()`.
- `onScreenshot(param1:MouseEvent) : void` — Plays a click sound via `POST_SOUND_EVENT` then calls `OnScreenshot` in the engine.
- `onScreenshotConsole() : void` — Console-callable wrapper; delegates to `onScreenshot(null)`.
- `onRotateLeftPress(param1:MouseEvent) : void` — Calls `OnRotateStart(true)` in the engine.
- `onRotateRightPress(param1:MouseEvent) : void` — Calls `OnRotateStart(false)` in the engine.
- `onRotateRelease(param1:MouseEvent) : void` — Calls `OnRotateStop` in the engine.
- `addCameraAngle(param1:String) : void` — Pushes a new label onto `cameraAngleSelect.choices`.
- `clearCameraAngles() : void` — Clears all choices from the `ArrowSelect`.
- `selectCameraAngle(param1:uint) : void` — Sets `cameraAngleSelect.selectedIndex`.
- `onCameraChange(param1:Event) : void` — Plays click sound and calls `OnCameraChange(selectedIndex)` in the engine.
- `setHideUILabel(param1:String) : void` — On Durango sets `screenshotButton.label`; on Orbis sets `hideUIButton.label`.

### Key fields

- `rotateText : TextField` — Text label shown near the rotate controls.
- `__id0_ : WindowHeader` — The window header component; title is set to `$EpicPose_Header`.
- `closeButton : MovieClip` — Window close button (wiring handled by parent/Iggy layer).
- `screenshotButton : LabelButton` — "Take Screenshot" button; label `$EpicPose_Screenshot`; on Durango doubles as the hide-UI button.
- `cameraAngleSelect : ArrowSelect` — Dropdown for choosing the camera angle preset.
- `backBanner : MovieClip` — Decorative background banner graphic.
- `rotateLeftButton : MovieClip` — Left-rotate hold button; PC only.
- `rotateRightButton : MovieClip` — Right-rotate hold button; PC only.
- `hideUIButton : MovieClip` — Orbis-specific button labelled `$EpicPose_HideUI`.
- `__setPropDict : Dictionary` — Guards per-frame component-inspector property application to avoid redundant re-sets.
- `__lastFrameProp : int` — Tracks last frame at which inspector props were applied.

### Frame scripts / timeline

The SWF has three layout states encoded as timeline frames:

- **Frame 1 (`frame1`)** — Default / PC layout. Calls `stop()`.
- **Frame 11 (`frame11`)** — Console layout (Durango). Calls `stop()`, then sends `cameraAngleSelect` to its `"Console"` label.
- **Frame 21 (`frame21`)** — Console layout (Orbis). Calls `stop()`, then sends `cameraAngleSelect` to its `"Console"` label.

A `FRAME_CONSTRUCTED` handler (`__setProp_handler`) re-applies component inspector properties for `hideUIButton` (frames 1–20 and 21–30) and `rotateLeftButton`/`rotateRightButton` (frames 1–10) each time the frame changes.

### Runtime dependencies & integration

- **Iggy / ExternalInterface callbacks registered:** `addCameraAngle`, `clearCameraAngles`, `selectCameraAngle`, `onScreenshotConsole`, `setHideUILabel`.
- **ExternalInterface calls out:** `POST_SOUND_EVENT("Play_ui_window_click_item")`, `OnScreenshot`, `OnRotateStart(bool)`, `OnRotateStop`, `OnCameraChange(uint)`.
- **Translation keys:** `$EpicPose_Header`, `$EpicPose_Screenshot`, `$EpicPose_HideUI`.
- **Platform checks:** `IsConsole()`, `_platform == PLATFORM_DURANGO`, `_platform == PLATFORM_ORBIS` (inherited constants from UIComponent).
- **Kiwi controls used:** `WindowHeader` (`__id0_`), `LabelButton` (`screenshotButton`), `ArrowSelect` (`cameraAngleSelect`).

## Other game-specific classes

- `BtnGreen` — Extends `LabelButton`; embeds `assets.swf` symbol62. 4-state button skin (frames 10/20/30/40 each `stop()`).
- `btnArrowLeft` — Extends `BaseButton`; embeds `assets.swf` symbol48. Left-arrow button skin, same 4-state frame structure.
- `btnArrowRight` — Extends `BaseButton`; embeds `assets.swf` symbol39. Right-arrow button skin, same 4-state frame structure.
- `btn_console_analog_top_left` — Extends `MovieClip`; embeds `assets.swf` symbol24. Pure analog-stick icon graphic for console UI; no logic.
- `btn_console_analog_top_right` — Extends `MovieClip`; embeds `assets.swf` symbol5. Pure analog-stick icon graphic for console UI; no logic.

## Notable logic

- **Platform-branching via frame navigation:** the engine navigates the root timeline to frame 1, 11, or 21 to switch between the PC layout and two console variants (Durango / Orbis), rather than using conditional visibility toggling at runtime.
- **Hold-to-rotate pattern:** `rotateLeftButton` and `rotateRightButton` use `MOUSE_DOWN` / `MOUSE_UP` (not `CLICK`) so the engine receives a continuous rotate signal (`OnRotateStart`) until released (`OnRotateStop`). These buttons are hidden entirely on console.
- **Camera-angle list is engine-driven:** the `ArrowSelect` starts empty; the game pushes angles one at a time via `addCameraAngle` then selects the default with `selectCameraAngle`. This keeps the list data server/config-side rather than hard-coded in Flash.
- **Non-Iggy fallback:** when not running in Iggy (dev/browser preview) one test camera angle is added and index 0 is selected automatically so the control is functional.
