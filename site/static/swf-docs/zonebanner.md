# zonebanner.swf

> Displays the zone-entry banner overlay in Trove when the player enters a new area, showing the zone name (uppercased) over the engine's difficulty text. The timeline holds one static layout on a `PC` frame and a `Console` frame, and the engine pushes zone data through an ExternalInterface callback; showing and removing the screen is left to the engine.

**Document/main class:** `ZoneBanner` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 1

## Main class: `ZoneBanner`

`ZoneBanner` is the document class and the only game-specific class in this SWF. The constructor wires two frame scripts (frames 1 and 11) to stop the timeline on its two platform frames. `configUI()` registers the two inbound ExternalInterface callbacks. Visibility of all child elements is toggled collectively through `setVisibility`, rather than setting `this.visible`, allowing the parent container to remain in the display tree while content is hidden.

### Public methods

- `setZoneData(param1:String, param2:String) : void` — Sets `zoneText.text = param1.toLocaleUpperCase()`, `levelText.text = param2`, then calls `setVisibility(true)`. `param1` is the zone name, `param2` the difficulty text; both are printed as received, with no parsing or translation.
- `hideWindow() : void` — Calls `setVisibility(false)`. The engine never sends `HIDE_WINDOW`; it closes the screen itself.
- `setVisibility(param1:Boolean) : *` — Toggles `.visible` on `zoneText`, `levelText`, and `background` simultaneously.
- `configUI() : void` *(override, protected)* — Registers the two ExternalInterface callbacks after the base UIComponent initializes.

### Key fields

- `zoneText : TextField` — Displays the zone name in all-caps (`toLocaleUpperCase`). For some zones the engine sends the untranslated key instead of the name (Medieval Highlands arrives as `$Objective_MedievalHighlands`, which `objective.binfab` resolves to "Medieval Highlands"), and the field prints the key uppercased. Authored placeholder: "Sands of Superstition".
- `levelText : TextField` — Displays the difficulty text exactly as the engine formats it: a difficulty name from the `$ZoneLevel_*` keys (`_Novice`, `_Adept`, `_Elite`, `_Master`, then the uber tiers `_Uber_1`–`_Uber_12`, "Uber" through "Long Shade", e.g. "Moonless Dark"), "Tier N" in the Geode caves, or a bare number. It is not a level range; the authored "10" is placeholder text.
- `background : MovieClip` — The decorative backdrop (sprite character 3, a 478×78 near-black horizontal gradient bitmap, about 25% opaque at the centre and fading out at both ends); hidden/shown in sync with the text fields.

### Frame scripts / timeline

- **Frame 1** (`frame1`, label `PC`) — `stop()`. The PC layout.
- **Frame 11** (`frame11`, label `Console`) — `stop()`. The console layout; `UIComponent`'s constructor jumps here on Durango/Orbis. Both text fields are removed and placed again as new instances with the same bounds, position and font sizes; `background` is placed once and carries over.
- Frames 2–10 and 12–20 hold no display-list changes: there is no animation on the timeline (20 frames, 24 fps, 367×100 stage).
- Both scripts registered in the constructor via `addFrameScript(0, this.frame1, 10, this.frame11)` (0-indexed, so frame indices 0 and 10 correspond to timeline frames 1 and 11).

### Runtime dependencies & integration

- **ExternalInterface callbacks registered (in `configUI`):**
  - `"SET_ZONE_DATA"` → `setZoneData(String, String)` — zone name and difficulty text. Sent on every zone entry, while the loading screen is still up, and sent again with the same two strings whenever the Esc menu is opened, which brings this screen back with it.
  - `"HIDE_WINDOW"` → `hideWindow()` — hides all visible children. Registered but never sent by the engine.
- `SET_ZONE_DATA` is the only call the engine makes to this screen; the inherited `UIComponent.onStageResized` is not called either.
- No translate keys; no outbound ExternalInterface calls; no `Timer`; no `IggyFunctions.inIggy` guard on callbacks.

## Other game-specific classes

None beyond `ZoneBanner`.

## Notable logic

- Zone name is forced to locale-aware uppercase (`toLocaleUpperCase`) rather than ASCII `toUpperCase`, ensuring correct capitalisation for localised zone names in non-Latin scripts.
- Visibility is managed per-child rather than on the root clip, which keeps the SWF's stage area active for hit-testing or layout purposes even when the banner is not shown.
- The two `stop()` frame scripts hold the movie on whichever platform frame `UIComponent` selected. Frames 2–10 are empty, so there is no entry or exit animation: the banner appears and disappears with the engine opening and closing the screen.
- Because `HIDE_WINDOW` never arrives, the three children stay visible after the first `SET_ZONE_DATA` for the rest of the session; the last zone is still drawn when the screen reopens with the Esc menu, and the re-sent strings overwrite it with the same text.
