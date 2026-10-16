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
- Same timeline as `rampagealert.swf`: 119 frames at 24 fps, labels `Animation` (frame 1) and `AnimationComplete` (frame 119), no `stop()`, so it loops and reports once per lap. The text is authored in cyan (`#00FFFF`) instead of orange.

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
- **Authored vs. assigned description:** this file's text fields were authored with keys rather than English — `$Challenges_RampageAlertTitle` and `$Challenges_RampageAlertDescription02` (English "GIGASTORMER ATTACK", in `challenges.binfab`). The shared constructor overwrites the description with `$Challenges_RampageAlertDescription` ("AMPERIUM ATTACK"), so in game this variant reads AMPERIUM ATTACK in cyan and the `…Description02` key is never shown.
