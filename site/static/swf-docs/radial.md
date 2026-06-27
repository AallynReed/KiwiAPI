# radial.swf
> Three-sector radial menu used to select mount actions in Trove. Appears when the player opens the mount radial wheel, presenting three labeled sectors the game can highlight programmatically.

**Document/main class:** `Radial` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 3

## Main class: `Radial`

`Radial` is the root component for the mount radial menu. In its constructor it hides the `buttonLegend` overlay, sets localized sector labels on the nested `radialThreeWay` clip, and registers a single `ExternalInterface` callback so the game can drive which sector is active. On console it defers showing `buttonLegend` until the first `ENTER_FRAME` (confirming the console runtime is live), then removes the listener.

### Public methods
- `goToSector(param1:int) : void` — calls `radialThreeWay.gotoAndStop("Sector" + param1)` to highlight the requested sector; also sets the internal `_targetFrame` field.

### Key fields
- `radialThreeWay : MovieClip` — the `MountRadial_3` symbol clip containing the three sector labels.
- `buttonLegend : MovieClip` — console button-hint overlay; hidden by default, shown on console after the first frame.

### Frame scripts / timeline
None on `Radial` itself; frame control is delegated to `radialThreeWay` via `goToSector`.

### Runtime dependencies & integration
- `IggyFunctions.inIggy` — gates `ExternalInterface` registration and label injection.
- `ExternalInterface.addCallback("goToSector", ...)` — game calls this to switch the active sector.
- `IsConsole()` — built-in Iggy runtime predicate; controls `buttonLegend` visibility.
- translate keys: `$Mount_Radial_1`, `$Mount_Radial_2`, `$Mount_Radial_3` — set as text on the three sector labels.
- `Event.ENTER_FRAME` — listened once on console to trigger `buttonLegend` reveal.

---

## Other game-specific classes

- `Label` (extends `_kiwi.Controls.Label`) — Embed `symbol3`; two-frame timeline clip that applies `TextFormat` size 18 (frame 1 "Default") or size 21 (frame 2 "Bold") to the text field. Used for sector text inside `MountRadial_3`.
- `Radial_fla.MountRadial_3` (extends `MovieClip`) — Embed `symbol16`; four-frame clip (`Sector1`–`Sector4`). Each frame script stops playback and calls `gotoAndStop("Bold")` on the matching `Label` while setting the others to `"Default"`, implementing the highlight-one-sector visual.

---

## Notable logic
- Sector highlighting works entirely through frame labels on `MountRadial_3` and state frames on `Label`. No AS3-drawn graphics; all visuals are embedded Flash symbols.
- On non-console (Flash IDE / desktop preview), no `ExternalInterface` callbacks are added and sector labels remain at their default text content.
