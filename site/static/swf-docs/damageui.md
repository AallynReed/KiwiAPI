# damageui.swf
> Full-screen vignette overlay that visualizes the player's health state in Trove. Appears whenever the player is at low or critical health, showing a red damage frame around the screen edges.

**Document/main class:** `DamageUIBase` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 2

## Main class: `DamageUIBase`

`DamageUIBase` manages a single full-screen `MovieClip` (`mc_damageFrameRed`) that shows one of two static pictures depending on the player's health threshold. The constructor sets the initial state to non-critical (low health picture) and registers an `OnStageResized` callback so the overlay always fills the viewport. There is no further data or tick logic — state is driven entirely through the `critical` setter. The client's registration block for this screen (`LowHealthUI`, `DamageUI.swf`) names only `critical` and `OnStageResized`: the engine writes `critical` as a property, and the screen has no way to clear itself — the engine shows and hides the whole element.

### Public methods / setters
- `set critical(param1:Boolean) : void` — calls `mc_damageFrameRed.gotoAndPlay("very low health")` when `true`, otherwise `gotoAndPlay("low health")`. These are timeline frame labels on the embedded symbol; the jump is a picture swap, not an animation. Written by the engine as a property assignment, not through a callback.

### Key fields
- `mc_damageFrameRed : MovieClip` — the `damage_frame_red_1` embedded clip; stretched to fill the stage. The sprite carries a `DefineScalingGrid`, so stretching its 195 × 156 picture nine-slices it into a full-screen rim.

### Frame scripts / timeline
None on `DamageUIBase`. The embedded `damage_frame_red_1` symbol stops itself at frames 10 and 20 via frame scripts (both call `halt()` → `stop()`), forming two distinct hold frames for the two health states.

### Runtime dependencies & integration
- `ExternalInterface.addCallback("OnStageResized", onStageResized)` — game notifies when viewport dimensions change; handler resizes both `this` and `mc_damageFrameRed` to the new `(width, height)`. This screen registers the name without the `UIComponent.` prefix; the base class's prefixed `UIComponent.onStageResized` is registered as well, so both names are live.
- `IggyFunctions` — imported but only the `inIggy` pattern is used implicitly (no explicit guard here; `ExternalInterface` is always registered).

---

## Other game-specific classes

- `DamageUI_fla.damage_frame_red_1` (extends `MovieClip`) — Embed `symbol5`; 20-frame clip with stop points at frame 10 and frame 20 via `halt()`. Frame 1 (`low health`) and frame 11 (`very low health`) each place one 195 × 156 bitmap picture at depth 9 and then hold it for ten identical frames, so neither segment animates. The `low health` picture is clear in the middle; the `very low health` one is a slightly stronger red that also tints the middle of the screen. Exposes `halt()` as a public method so frame scripts can call it cleanly.

---

## Notable logic
- The entire UI state machine has only two states (low / critical), driven by a single boolean setter — no score, timer, or incremental health value is tracked in Flash.
- `onStageResized` overrides the `UIComponent` base method, taking three `Number` parameters `(width, height, unknown)` and applying `width`/`height` to both the component and the clip. This keeps the vignette pixel-perfect regardless of window resize.
