# message.swf

> A small in-game message/tutorial overlay in Trove used to display a contextual text message, an optional item or art image, an objective icon, and animated input-key highlights (keyboard or controller buttons). Appears during tutorials, quest prompts, or contextual hints.

**Document/main class:** `Message` (extends `UIComponent`)
**SWF-specific classes:** 1 (`Message`)

---

## Main class: `Message`

Manages a single message card. `configUI()` sets up the `highlightTimer` listener, configures `artClip` with the placeholder's dimensions, hides `artClipPlaceholder`, records `messageText`'s initial `Rectangle`, calls `updateMessageTextRect()` to expand the text area when no art is shown, and registers all `ExternalInterface` callbacks if running in Iggy.

No explicit frame scripts or timeline sections; the class extends `UIComponent` directly with no `addFrameScript` calls.

### Public methods (all registered as ExternalInterface callbacks)

- `setImage(path:String) : void` — loads an art image into `artClip` (un-ghosts it, sets `iconImage`, sets `data = 1`, validates, then calls `updateMessageTextRect` to shrink the text area to make room).
- `setText(text:String) : void` — sets `messageText.text` (plain) or `htmlText` (console). Single-line messages are vertically centered at `y=36`. Also repositions `objectiveIcon` to the left edge of the text.
- `setInputImage(frame:String) : void` — calls `inputImages.gotoAndStop(frame)`, registers a one-shot `Event.RENDER` listener (`frameRendered`), invalidates the stage, and clears all highlight state.
- `setHighlight(key:String) : void` — if `key` starts with `"$"`, translates it via `IggyFunctions.translate(key)` and records `"singleKey"` as the highlight name; otherwise records the raw MC name to highlight.
- `setObjectiveIcon(textureName:String) : void` — sets `objectiveIcon.textureName` and resizes it to 50×50.

### Key fields

- `artClip : ArtClip` — kiwi art image component; sized to match `artClipPlaceholder` dimensions. When `iconImage` is empty, `messageText` expands leftward to fill the space.
- `artClipPlaceholder : MovieClip` — invisible sizing reference for `artClip`.
- `messageText : TextField` — main message body. Width/x adjust dynamically based on whether art is present.
- `inputImages : MovieClip` — frame-based MC; each frame label is an input image name (key, controller button, etc.). Children are either `TextField`s or `MovieClip` containers with named sub-clips to highlight.
- `objectiveIcon : ObjectPreview` — small icon to the left of `messageText` for objective display; size 50×50.
- `messageTextInitialRect : Rectangle` — snapshot of `messageText` bounds taken at `configUI()` time; used as reference for width/x calculations.
- `singleKeyText : String` — translated label for a single-key input highlight; written into `inputImages.textField` after the frame renders.
- `highlightNames : Vector.<String>` — names of child MCs inside `inputImages` to animate.
- `highlightMCs : Vector.<MovieClip>` — resolved MC references gathered during `frameRendered`.
- `currentHighlight : int` — index into `highlightMCs` for the cycling fade animation.
- `highlightTimer : Timer` — single-shot 500 ms (first cycle) / 100 ms (subsequent cycles) timer that advances the highlight loop.

### Runtime dependencies & integration

**ExternalInterface callbacks registered (inIggy):**
`setImage`, `setText`, `setInputImage`, `setHighlight`, `setObjectiveIcon`

**No outbound ExternalInterface calls.**

**IggyFunctions:** `IggyFunctions.translate(key)` used to resolve `$`-prefixed single-key names in `setHighlight`.

**IggyTween:** Highlight fade-in/out is driven by `IggyTween` instances using `Strong.easeOut` (fade in, 0.15 s) and `Strong.easeIn` (fade out, 0.25 s). Callback chaining: `animateFadeIn → motionFinishCallback → animateFadeOut → motionFinishCallback → queueHighlight → highlightTimer → animateFadeIn`. The cycle resets with 500 ms delay between full cycles, 100 ms between consecutive highlights.

**Events listened:** `TimerEvent.TIMER` on `highlightTimer`, `Event.RENDER` on `inputImages` (one-shot, to post-process the newly stopped frame)

**Text field special handling in `frameRendered`:** After `inputImages.gotoAndStop`, the render callback iterates children. `TextField` children whose text matches the pattern `$[...]` have the bracketed content extracted (stripping the `$[` prefix and `]` suffix) as a direct MC name reference. `MovieClip` children have all their `MovieClip` grandchildren set to `alpha = 0` initially; those whose `.name` appears in `highlightNames` are collected into `highlightMCs` for animation. If a `singleKeyBMP` grandchild exists and `singleKeyText` is set, it is made visible and `inputImages.x` is shifted right by 40 px.

---

## Other game-specific classes

None beyond the main class.

### Message_fla/ timeline symbols (4 total)

- `keysMC_2` — embedded symbol 49; contains a `textField` (for key label text) and a `highlights` MovieClip (container for named highlight sub-clips). Stops at frame 0.
- `spacebarMC_3` — embedded symbol 12; contains a `singleKey` MovieClip. Stops at frame 0. Represents the spacebar input image.
- `tabMC_5` — embedded symbol 20; contains a `tab` MovieClip. Stops at frame 0. Represents the Tab key input image.
- `singleKey_9` — embedded symbol 35; contains a `singleKey` MovieClip. Stops at frame 0. Generic single-key input image.

**Asset wrappers (not detailed):** No png/skin wrappers in this SWF. Also present: `dummy.as`.

---

## Notable logic

- **Dynamic text layout:** When `artClip` has no image, `messageText.x` is moved left to `artClip.x` and its width is extended by `2 × (initialX - artClip.x)`, effectively centering the text in the full card width. When art is present, the original rect is restored.
- **Highlight cycling:** The animation is self-driving via `IggyTween` motion-finish callbacks rather than a frame loop. The timer introduces a pause between complete cycles (500 ms) and between individual highlights (100 ms) to create a paced tutorial-highlight effect.
- **Console vs PC text:** `setText` uses `messageText.htmlText` on console (allowing rich-text markup) and plain `messageText.text` on PC.
- **Input image frame labels:** The `inputImages` MC is expected to have frame labels matching the string argument passed to `setInputImage` — the set of available labels is defined in the timeline of the embedded asset, not in ActionScript.
