# zonebanner.swf

> Displays a zone-entry banner overlay in Trove when the player enters a new area, showing the zone name (uppercased) and level range. The banner has a two-state timeline (shown/hidden) and exposes ExternalInterface callbacks so the game engine can push zone data and dismiss the overlay.

**Document/main class:** `ZoneBanner` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 1

## Main class: `ZoneBanner`

`ZoneBanner` is the document class and the only game-specific class in this SWF. The constructor wires two frame scripts (frames 1 and 11) to stop the timeline at its two rest states. `configUI()` registers the two inbound ExternalInterface callbacks. Visibility of all child elements is toggled collectively through `setVisibility`, rather than setting `this.visible`, allowing the parent container to remain in the display tree while content is hidden.

### Public methods

- `setZoneData(param1:String, param2:String) : void` — Sets `zoneText.text = param1.toLocaleUpperCase()`, `levelText.text = param2`, then calls `setVisibility(true)`.
- `hideWindow() : void` — Calls `setVisibility(false)`.
- `setVisibility(param1:Boolean) : *` — Toggles `.visible` on `zoneText`, `levelText`, and `background` simultaneously.
- `configUI() : void` *(override, protected)* — Registers the two ExternalInterface callbacks after the base UIComponent initializes.

### Key fields

- `zoneText : TextField` — Displays the zone name in all-caps (`toLocaleUpperCase`).
- `levelText : TextField` — Displays the level range or difficulty string for the zone.
- `background : MovieClip` — The decorative background panel; hidden/shown in sync with the text fields.

### Frame scripts / timeline

- **Frame 1** (`frame1`) — `stop()`. Rest state: banner hidden or at start.
- **Frame 11** (`frame11`) — `stop()`. Rest state: banner fully revealed (after any intro animation on frames 2–10).
- Both scripts registered in the constructor via `addFrameScript(0, this.frame1, 10, this.frame11)` (0-indexed, so frame indices 0 and 10 correspond to timeline frames 1 and 11).

### Runtime dependencies & integration

- **ExternalInterface callbacks registered (in `configUI`):**
  - `"SET_ZONE_DATA"` → `setZoneData(String, String)` — zone name and level text.
  - `"HIDE_WINDOW"` → `hideWindow()` — hides all visible children.
- No translate keys; no outbound ExternalInterface calls; no `Timer`; no `IggyFunctions.inIggy` guard on callbacks.

## Other game-specific classes

None beyond `ZoneBanner`.

## Notable logic

- Zone name is forced to locale-aware uppercase (`toLocaleUpperCase`) rather than ASCII `toUpperCase`, ensuring correct capitalisation for localised zone names in non-Latin scripts.
- Visibility is managed per-child rather than on the root clip, which keeps the SWF's stage area active for hit-testing or layout purposes even when the banner is not shown.
- The two `stop()` frame scripts prevent the timeline from looping; frames 2–10 presumably hold an entry animation that plays automatically when the SWF advances from frame 1.
