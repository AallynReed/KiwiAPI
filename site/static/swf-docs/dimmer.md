# dimmer.swf
> A full-screen semi-transparent overlay used to darken the background behind modal dialogs and popups. Has no interactive elements; it simply stretches a dim `MovieClip` to fill the stage whenever the stage is resized.

**Document/main class:** `Dimmer` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 1

## Main class: `Dimmer`

Extremely minimal. Only one child: `dimbackground : MovieClip`. Overrides `onStageResized` to stretch and reposition `dimbackground` to cover the entire stage.

### Public methods
*(None beyond the inherited UIComponent interface.)*

### Key fields
- `dimbackground : MovieClip` — the visual dim layer; its `width` and `height` are set to the stage dimensions on every resize event, and its position is set to `(width/2, height/2)` — suggesting it is centre-registered.

### Frame scripts / timeline
*(None — no `addFrameScript` calls; single-frame SWF.)*

### Runtime dependencies & integration
- `onStageResized(w, h, scale)` override in `_kiwi.Core.UIComponent` — called by the framework when the Flash stage dimensions change; `Dimmer` responds by calling `resizeDimBackground(w, h)`.
- No `ExternalInterface` callbacks or calls.
- No translate keys.
- No Iggy runtime dependency.

---

## Notable logic
- This SWF is a pure layout utility. The entire game-specific code is 4 lines: override `onStageResized`, call `super`, set `dimbackground.width/height`, set `dimbackground.x/y`. All visual appearance (colour, alpha) is baked into the `dimbackground` symbol in the asset SWF.
