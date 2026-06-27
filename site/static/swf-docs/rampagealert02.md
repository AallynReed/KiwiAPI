# rampagealert02.swf

> A second variant of the Rampage challenge alert animation for Trove. Functionally identical to `rampagealert.swf` — it displays the same localized warning and description text, plays a timeline animation, and calls `AnimationComplete` on the host when finished. The visual assets differ (hence the `02` suffix) but the ActionScript is byte-for-byte the same.

**Document/main class:** `RampageAlert` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 1

## Main class: `RampageAlert`

Identical implementation to `rampagealert.swf`. See that file's documentation for full details. The class name, field names, translate keys, and ExternalInterface contract are unchanged; only the embedded timeline graphics differ between the two SWFs.

### Public methods

- `onAnimationComplete(e:Event) : void` — `ENTER_FRAME` handler. Fires `ExternalInterface.call("AnimationComplete")` when `this.currentLabel == "AnimationComplete"`.

### Key fields

- `ANIMATION_COMPLETE_FRAME : String` *(private const)* — `"AnimationComplete"`.
- `warningContainer : MovieClip` — Contains `warning_txt:TextField` (title).
- `descriptionContainer : MovieClip` — Contains `description_txt:TextField` (description).

### Frame scripts / timeline

- No `addFrameScript` calls; animation end is detected via per-frame label polling.

### Runtime dependencies & integration

- **`IggyFunctions.translate` keys used:**
  - `"$Challenges_RampageAlertTitle"`
  - `"$Challenges_RampageAlertDescription"`
- **ExternalInterface outbound call:** `ExternalInterface.call("AnimationComplete")`.
- **Event:** `Event.ENTER_FRAME` (weak reference).
- No inbound ExternalInterface callbacks; no timers.

## Other game-specific classes

None beyond `RampageAlert`.

## Notable logic

- This SWF shares its document-class source with `rampagealert.swf` verbatim. Any behavioral difference between the two alerts is purely visual (timeline keyframes and embedded graphic symbols).
