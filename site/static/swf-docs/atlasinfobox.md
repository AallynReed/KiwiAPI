# atlasinfobox.swf
> Tooltip-style info box displayed on the Atlas (world map) when a portal or location is selected. Shows the world name, a description, and a zone-level (Uber) selector that lets the player choose a difficulty tier before entering.

**Document/main class:** `AtlasInfoBox` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 1

## Main class: `AtlasInfoBox`

Renders a panel with a world name (`worldName:TextField`), world description (`worldInfo:TextField`), a `ComboBox` for Uber tier selection (`UberSelector`), a zone-level label, an Enter button (`enterButton:LabelButton`), and four console shoulder-button icons (`LB`, `RB`, `RB_U`, `LB_D`).

On construction, `addFrameScript` attaches handlers for frames 1 and 20. `configUI()` initialises text format for the combo box (white Open Sans 12px), sets the label to the translated `$Atlas_ZoneLevel` ("Difficulty") plus a hard-coded `":"`, registers `ExternalInterface` callbacks, and conditionally shows/hides the Enter button and console navigation icons depending on `IsConsole()`.

### Public methods

- `addZoneLevel(level:Number, label:String) : void` — adds one entry (`{label, data: level}`) to `UberSelector` and reveals the selector once a second item exists. `level` is the world's difficulty number; `label` arrives already formatted as `<difficulty> - <name>`, e.g. `15 - Long Shade` (`$ZoneLevel_Uber_12`).
- `clearZoneLevels() : void` — removes all combo box entries and resets `selectedIndex` to 0.
- `changeSelectedLevel(delta:Number) : void` — increments/decrements the selected index and fires `ExternalInterface.call("OnSelectedZoneLevelChanged", data)`. The call fires even when the step would leave the list and the index stays put.
- `setSelectedLevel(index:Number) : void` — directly sets the combo box selection by index, without calling the engine back.

### Key fields

- `enterButton : LabelButton` — Enter button, label key `$LikedWorlds_GotoWorld` ("ENTER"); click listener added on PC only, and the `Console` frame has no `enterButton`. Click fires `ExternalInterface.call("EnterPortal", data)` with the selected difficulty number, or `0` when the list is empty. The engine sets `enterButton.enabled` directly, to `false` when the power rank requirement is not met.
- `worldName : TextField` — HTML-capable; `setWorldName()` drops `y` to 74 when the name wraps and, past two lines, calls `KiwiTextUtil.resizeFont(worldName, 14, 2)` to shrink the font a point at a time (floor parameter 14) until it fits on two lines.
- `worldInfo : TextField` — repositioned vertically when the Uber selector is shown or hidden. The class never writes its text; the engine sets `worldInfo.htmlText` directly (see below).
- `UberSelector : ComboBox` — zone-level dropdown; change event calls `OnSelectedZoneLevelChanged`.
- `ZoneLevelLabel : TextField` — translate key `$Atlas_ZoneLevel`.
- `LB / RB / LB_D / RB_U : MovieClip` — console shoulder-button indicators for cycling Uber levels; visible only when `IsConsole()` is true and levels exist.
- `originalWorldNameY / originalWorldInfoY : Number` — saved baseline positions for vertical re-anchoring.
- `selectedIndex : int` — tracks current combo selection independently of the widget.

### Frame scripts / timeline

The timeline carries three layouts, one per frame label:

- **frame 1, `PC`** (`stop()`) — default PC layout.
- **frame 10, `PCLoc`** (no frame script) — replaces the `worldName`, `worldInfo` and `ZoneLevelLabel` fields and moves the selector and console icons; `enterButton` stays. `setSelectorVisible` checks for this label.
- **frame 20, `Console`** (`stop()`) — replaces the three text fields again; `enterButton` is removed from the timeline before this frame.

### Runtime dependencies & integration

- `IggyFunctions.inIggy` gate before registering `ExternalInterface` callbacks.
- `IggyFunctions.translate("$Atlas_ZoneLevel")` — localised label.
- `ExternalInterface` callbacks registered: `UIComponent.onStageResized`, `setWorldName`, `addZoneLevel`, `clearZoneLevels`, `changeSelectedLevel`, `setSelectedLevel`.
- `ExternalInterface` calls out: `OnSelectedZoneLevelChanged(zoneLevel)` (combo `Event.CHANGE` and `changeSelectedLevel`, never `setSelectedLevel`), `EnterPortal(zoneLevel)`. Both send the selected entry's `data`, the difficulty number the engine passed to `addZoneLevel`.
- Properties the engine writes directly by name: `worldInfo.htmlText` and `enterButton.enabled` (both names sit in the executable's `AtlasInfoBox.swf` string block), so both members are part of the engine contract.
- `worldInfo` text is formatted engine-side from `$PowerRankRequirementTooltipFormat` ("{0}\n\n{1}Power Rank {2} required.\nSuggested class level {3}.") or, when the requirement is not met, `$PowerRankRequirementUnfullfilledTooltipFormat` ("{0}\n\n{1}<font color=#FF2B2B>{2}</font>\nSuggested class level {3}."). `{0}` is the portal's own description; `{1}` is empty except for `$UberTwelveAtlasAddition` ("Included Biomes:\n{0}") on the Rising Tides world and one `$HigherStatRequirement` line ("Requires {0} of {1} or greater") per required stat on the top uber difficulty; `{2}` is the power rank, or in the unfulfilled form the whole unmet-requirement sentence in red (which on the top difficulty can name a stat such as Light rather than power rank); `{3}` is the suggested class level range. Iggy renders the unquoted `color=#FF2B2B` attribute.
- `KiwiTextUtil.resizeFont` used to shrink long world names.
- `IsConsole()` — toggles Enter button vs. shoulder-button navigation.

## Other game-specific classes

Asset skin wrappers (pure symbol embeds, no logic beyond `stop()` frame scripts): 19 files — `CellRenderer_*Skin` (×7), `ComboBox_*Skin` (×4), `ScrollArrow{Up,Down}_*Skin` (×8), `ScrollThumb_*Skin` (×3), `ScrollBar_thumbIcon`, `ScrollTrack_skin`, `List_skin`, `TextInput_*Skin` (×2), `focusRectSkin`.

## Notable logic

- `setSelectorVisible(visible)` repositions `worldInfo.y` differently for PC vs. console vs. `PCLoc` frame label, and adjusts the combo box `x`/`y` accordingly.
- The `UberSelector` is hidden until at least two zone levels have been added (`this.UberSelector.length >= 1`).
- Font colour for the combo box is hardcoded white (`0xFFFFFF`) in Open Sans 12pt via a `TextFormat` applied to the dropdown renderer style.
