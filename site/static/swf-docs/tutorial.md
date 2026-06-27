# tutorial.swf
> A paginated tutorial / help slideshow panel shown to players when they first start Trove or enter a new game mode. It presents a series of slides (each a timeline frame in a `textArea` MovieClip) with prev/next navigation buttons, an optional image panel rendered via `ObjectPreview`, and a close button that appears only on the final page. Separate slide variants exist for the Welcome, Adventure, Build Mode, Tip, and Geode tutorial contexts.

**Document/main class:** `Tutorial` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 17

---

## Main class: `Tutorial`

`Tutorial` manages a multi-page slideshow. The `textArea` MovieClip's frame position drives which slide content is visible (`currentPage + frameOffset`). Navigation is handled locally for display updates but all page-change requests are forwarded to the game engine via `ExternalInterface.call("OnSetPage", n)` so the C++ side controls authoritative state; the Flash side receives `onNext`/`onPrev`/`onExit` callbacks back through `ExternalInterface.addCallback`.

Constructor calls `addFrameScript` for frames 1, 11, and 21 (all `stop()`), adds the `ObjectPreview` image to `imgAnchor`, captures original dimensions, wires click listeners to `prevButton`, `m_backArrow`, `nextButton`, and `buttonCloseGreen`, and registers the stage-resize and navigation Iggy callbacks.

`configUI()` is inherited from `UIComponent`. `draw()` pushes `title` and `desc` strings into `textArea.titleField` and `textArea.descField` text fields when the DATA invalidation flag is set.

### Public methods

- `textRendered(e:Event) : void` — `Event.RENDER` handler fired after `textArea.gotoAndStop`; iterates `textFields<N>` child containers on the current frame to resolve single-character `$[X]` escape sequences, and on frame 4 specifically lays out 7 `tutorialTip` sub-clips by translating `$Tutorial_tip_N` keys via `IggyFunctions.translate`, setting `htmlText`, resizing height to `textHeight`, vertically centring the bullet, and stacking each tip below the previous one.

### Key properties (getters/setters)

- `texture : String` — sets `image.textureName`; drives the `ObjectPreview` slide image.
- `text : String` — gets/sets the label text on `buttonCloseGreen`.
- `frameOffset : int` (default `1`) — added to `currentPage` when calling `textArea.gotoAndStop`; allows the `textArea` timeline to reserve frame 1 for a blank/default state.
- `title : String` — written to `textArea.titleField`; invalidates DATA.
- `desc : String` — written to `textArea.descField`; invalidates DATA.
- `currentPage : int` — core pagination setter: clamps to `[0, numPages-1]`, controls visibility of `prevButton`, `nextButton`, `buttonCloseGreen`, `m_backArrow`, `consoleNextBtn`/`consoleBackBtn` and their labels, updates `consoleNextLabel` text between `"$Tutorial_NEXT"` and `"$Tutorial_GO"`, then calls `textArea.gotoAndStop(currentPage + frameOffset)` and schedules `textRendered` via `Event.RENDER`.
- `numPages : int` — resets `currentPage` to 0 when changed.

### Key fields

- `image : ObjectPreview` — 690×408 texture display added to `imgAnchor`; shows slide artwork.
- `screenFade : MovieClip` — full-screen dark overlay; repositioned and resized in `_onStageResized` to cover the viewport regardless of scale.
- `nextButton : LabelButton` — "Next" button, label `$tutorial_window_next`.
- `prevButton : LabelButton` — "Back" button, label `$tutorial_window_back`.
- `m_backArrow : BaseButton` — secondary back arrow (visible on last page if `currentPage > 0`).
- `buttonCloseGreen : BaseButton` — "Close" / "Done" button shown only on the final page.
- `textContainer : MovieClip` — holds `txt` text field; mouse interaction disabled.
- `textArea : MovieClip` — multi-frame content area; its child structure varies per frame and is accessed by `textRendered` as `textFields<frameNumber>`.
- `consoleNextBtn / consoleBackBtn : MovieClip` — console button prompts.
- `consoleNextLabel / consoleBackLabel : TextField` — console label text fields.
- `_frameOffset : * = 1` — backing store for `frameOffset`.
- `_currentPage : int`, `_numPages : int` — backing stores for pagination.
- `__setPropDict : Dictionary` — Flash component inspector helper; prevents double-initialisation of `nextButton` / `prevButton` per frame.

### Frame scripts / timeline

- Frame 1 (`frame1`) — `stop()` — default/standard tutorial layout.
- Frame 11 (`frame11`) — `stop()` — alternate layout (e.g. platform variant).
- Frame 21 (`frame21`) — `stop()` — second alternate layout.

`__setProp_handler` fires on `Event.FRAME_CONSTRUCTED` each frame; delegates to `__setProp_nextButton_Scene1_leftrightbuttons_0` and `__setProp_prevButton_Scene1_leftrightbuttons_0` which apply Flash component-inspector defaults (label, toggle, enabled, data) once per frame range.

### Runtime dependencies & integration

- **ExternalInterface callbacks registered:** `UIComponent.onStageResized`, `onExit`, `onPrev`, `onNext`.
- **ExternalInterface calls outbound:** `OnSetPage(pageIndex)` (prev/next navigation), `OnRequestClose()` (when Next is pressed on the last page).
- `IggyFunctions.translate("$Tutorial_tip_N")` called in `textRendered` for tip-frame localisation.
- `vcenterTextfieldToClip(tf, clip)` — non-class utility function called to vertically align tip text to its bullet icon.
- Translate keys used: `$tutorial_window_back`, `$tutorial_window_next`, `$Tutorial_BACK`, `$Tutorial_NEXT`, `$Tutorial_GO`, `$Tutorial_tip_1` through `$Tutorial_tip_7`.
- `IsNX()` check in `currentPage` setter forces console back-button to always be visible on Nintendo Switch.

---

## Other game-specific classes

- `AdventureText` (extends `UIComponent`, embeds symbol155) — slide content symbol for Adventure mode tips; 3 frame states (1, 11, 21).
- `WelcomeText` (extends `UIComponent`, embeds symbol220) — slide content symbol for the Welcome / intro screen; 3 frame states (1, 11, 21).
- `BuildText` (extends `UIComponent`, embeds symbol104) — slide content symbol for Build Mode tutorial; 3 frame states (1, 11, 21).
- `TipText` (extends `UIComponent`, embeds symbol57) — slide content symbol for the Tips page; 2 frame states (1, 11). Used as `textFields4` in `textarea_3`.
- `BuildModeHeader` (extends `UIComponent`, embeds symbol60) — header banner displayed when entering Build Mode; 2 frame states (frame 1 stop, frame 10 stop).
- `bullet` (extends `UIComponent`, embeds symbol55) — bullet-point icon used alongside tip text in the Tips slide; 2 frame states.
- `btnArrowLeft` (extends `BaseButton`, embeds symbol39) — left arrow button skin (4 button-state frames: 10, 20, 30, 40).
- `btnArrowRight` (extends `BaseButton`, embeds symbol9) — right arrow button skin (same 4-state structure).
- `btnLeft` (extends `LabelButton`, embeds symbol20) — labelled left / "Back" button skin; 4 frame states.
- `btnRight` (extends `LabelButton`, embeds symbol30) — labelled right / "Next" button skin; 4 frame states.

**`Tutorial_fla` timeline symbols (6):**
- `textarea_3` (symbol247) — the primary multi-frame text-area MovieClip; exposes `titleField:TextField`, `descField:TextField`, and per-frame child references `textFields1:WelcomeText`, `textFields2:AdventureText`, `textFields3:BuildText`, `textFields4:TipText`, `textFields5:TextField`. Stops at frames 1 and 12.
- `textareaLoc_21` (symbol266) — localised variant of `textarea_3`; same child structure minus `titleField`/`descField`; stops at frame 1.
- `TutorialSlide1_11` (symbol237) — slide 1 content symbol; 3 frame states.
- `TutorialSlide2_12` (symbol239) — slide 2 content symbol; 3 frame states.
- `TutorialHubText_13` (symbol240) — hub/world-map tutorial text symbol; 3 frame states.
- `TutorialGeodeSlide1_14` (symbol243) — Geode sub-biome tutorial slide; 3 frame states.

---

## Notable logic

- **Page navigation is split:** Flash handles only visual state (button visibility, `textArea` frame); the canonical page index lives on the C++ side. `onPrev`/`onNext` call `ExternalInterface.call("OnSetPage", n)` rather than directly setting `currentPage`; the C++ side sends `onPrev`/`onNext` back via registered callbacks to confirm, which then updates `currentPage`.
- **Tip layout (frame 4):** `textRendered` hard-codes a loop over `tutorialTip1`–`tutorialTip7` sub-clips inside `textFields4`. Each tip's text is set via `IggyFunctions.translate`, its height is clamped to `textHeight`, the bullet icon is vertically centred, and the next tip's `y` is placed `4px` below the current tip's bottom — a dynamic stack layout driven purely in ActionScript.
- **`$[X]` escape:** Single-character key escapes of the form `$[A]` (4-character strings) in text fields are stripped to just the character at index 2. This appears to be a workaround for button-icon characters that cannot be stored directly in FLA text fields.
- **`frameOffset`:** Allows the `textArea` timeline to use frame 1 as a blank default; actual slide content starts at frame `frameOffset` (default 1, so page 0 → frame 1, page 1 → frame 2, etc.). The game can override this by setting the property before setting `numPages`.
- **Screen fade resize:** `screenFade` is positioned at `(-w/scale, -h/scale)` with size `(4w/scale, 4h/scale)` to guarantee full-screen coverage across all resolution scales.
