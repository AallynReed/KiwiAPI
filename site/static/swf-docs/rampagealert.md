# rampagealert.swf

> Displays a full-width animated alert strip (1920 × 160) when a Rampage challenge begins in Trove. The SWF plays a timeline animation, shows localized warning and description text, and signals the game engine when the animation finishes so the overlay can be dismissed.

**Document/main class:** `RampageAlert` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 1

## Main class: `RampageAlert`

`RampageAlert` is the document class and the only game-specific class in this SWF. All initialization happens in the constructor: localized strings are written to nested TextFields immediately, and an `ENTER_FRAME` listener is added to poll for the animation's end label. The class does not register any inbound ExternalInterface callbacks — the game simply instantiates the SWF and waits for the outbound `AnimationComplete` call, which is what closes the window; nothing else takes it away.

### Public methods

- `onAnimationComplete(e:Event) : void` — `ENTER_FRAME` handler. Checks `this.currentLabel` against the constant `"AnimationComplete"` each frame; when they match, calls `ExternalInterface.call("AnimationComplete")` to notify the host and (implicitly) signals that the overlay should be removed.

### Key fields

- `ANIMATION_COMPLETE_FRAME : String` *(private const)* — `"AnimationComplete"` — the timeline frame label that marks the end of the intro animation.
- `warningContainer : MovieClip` — Timeline symbol containing `warning_txt:TextField`, which is set to the translated title string at construction time.
- `descriptionContainer : MovieClip` — Timeline symbol containing `description_txt:TextField`, which is set to the translated description string at construction time.

### Frame scripts / timeline

- No `addFrameScript` calls; the end-of-animation signal is detected via an `ENTER_FRAME` label poll rather than a frame script.
- 119 frames at 24 fps with two labels: `Animation` (frame 1) and `AnimationComplete` (frame 119). Nothing calls `stop()`, so the timeline loops.
- Frame 1 places the full-width band, the two hazard-stripe rails (a 1752 × 16 bitmap), `descriptionContainer` and `warningContainer`. The text fields are authored in orange (`#FF5B00`) with the resolved English ("WARNING" at 83 pt, "AMPERIUM ATTACK" at 26 pt) and use an embedded `DefineFont3` "Open Sans Semibold", which is most of the file's size.

### Runtime dependencies & integration

- **`IggyFunctions.translate` keys used:**
  - `"$Challenges_RampageAlertTitle"` — text for `warningContainer.warning_txt`.
  - `"$Challenges_RampageAlertDescription"` — text for `descriptionContainer.description_txt`.
- **ExternalInterface outbound call:**
  - `ExternalInterface.call("AnimationComplete")` — fired once when the timeline reaches the `"AnimationComplete"` label.
- **Event:** `Event.ENTER_FRAME` listener registered with `useWeakReference = true`.
- No inbound ExternalInterface callbacks; no timers.

## Other game-specific classes

None beyond `RampageAlert`.

## Notable logic

- Because nothing stops the timeline, `currentLabel` equals `"AnimationComplete"` only on frame 119, so the `ENTER_FRAME` poll fires `AnimationComplete` once per 119-frame lap. In game the call closes the window.
- Localization strings are applied in the constructor, before the UIComponent lifecycle calls `configUI`, so the text is set as early as possible.
