# atlasinfobox.swf
> Tooltip-style info box displayed on the Atlas (world map) when a portal or location is selected. Shows the world name, a description, and a zone-level (Uber) selector that lets the player choose a difficulty tier before entering.

**Document/main class:** `AtlasInfoBox` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 1

## Main class: `AtlasInfoBox`

Renders a panel with a world name (`worldName:TextField`), world description (`worldInfo:TextField`), a `ComboBox` for Uber tier selection (`UberSelector`), a zone-level label, an Enter button (`enterButton:LabelButton`), and four console shoulder-button icons (`LB`, `RB`, `RB_U`, `LB_D`).

On construction, `addFrameScript` attaches handlers for frames 1 and 20. `configUI()` initialises text format for the combo box (white Open Sans 12px), sets the `$Atlas_ZoneLevel` translate key on the label, registers `ExternalInterface` callbacks, and conditionally shows/hides the Enter button and console navigation icons depending on `IsConsole()`.

### Public methods

- `addZoneLevel(level:Number, label:String) : void` — adds one entry to `UberSelector` and reveals the selector once a second item exists.
- `clearZoneLevels() : void` — removes all combo box entries and resets `selectedIndex` to 0.
- `changeSelectedLevel(delta:Number) : void` — increments/decrements the selected index and fires `ExternalInterface.call("OnSelectedZoneLevelChanged", data)`.
- `setSelectedLevel(index:Number) : void` — directly sets the combo box selection by index.

### Key fields

- `enterButton : LabelButton` — "Go to World" button; PC only; click fires `ExternalInterface.call("EnterPortal", zoneLevel)`.
- `worldName : TextField` — HTML-capable; `setWorldName()` adjusts `y` for two-line names and calls `KiwiTextUtil.resizeFont` when lines > 2.
- `worldInfo : TextField` — repositioned vertically when the Uber selector is shown or hidden.
- `UberSelector : ComboBox` — zone-level dropdown; change event calls `OnSelectedZoneLevelChanged`.
- `ZoneLevelLabel : TextField` — translate key `$Atlas_ZoneLevel`.
- `LB / RB / LB_D / RB_U : MovieClip` — console shoulder-button indicators for cycling Uber levels; visible only when `IsConsole()` is true and levels exist.
- `originalWorldNameY / originalWorldInfoY : Number` — saved baseline positions for vertical re-anchoring.
- `selectedIndex : int` — tracks current combo selection independently of the widget.

### Frame scripts / timeline

- **frame 1** (`stop()`) — default PC layout.
- **frame 20** (`stop()`) — a second layout state (console / localisation variant implied by the `PCLoc` label check in `setSelectorVisible`).

### Runtime dependencies & integration

- `IggyFunctions.inIggy` gate before registering `ExternalInterface` callbacks.
- `IggyFunctions.translate("$Atlas_ZoneLevel")` — localised label.
- `ExternalInterface` callbacks registered: `UIComponent.onStageResized`, `setWorldName`, `addZoneLevel`, `clearZoneLevels`, `changeSelectedLevel`, `setSelectedLevel`.
- `ExternalInterface` calls out: `OnSelectedZoneLevelChanged(zoneLevel)`, `EnterPortal(zoneLevel)`.
- `KiwiTextUtil.resizeFont` used to shrink long world names.
- `IsConsole()` — toggles Enter button vs. shoulder-button navigation.

## Other game-specific classes

Asset skin wrappers (pure symbol embeds, no logic beyond `stop()` frame scripts): 19 files — `CellRenderer_*Skin` (×7), `ComboBox_*Skin` (×4), `ScrollArrow{Up,Down}_*Skin` (×8), `ScrollThumb_*Skin` (×3), `ScrollBar_thumbIcon`, `ScrollTrack_skin`, `List_skin`, `TextInput_*Skin` (×2), `focusRectSkin`.

## Notable logic

- `setSelectorVisible(visible)` repositions `worldInfo.y` differently for PC vs. console vs. `PCLoc` frame label, and adjusts the combo box `x`/`y` accordingly.
- The `UberSelector` is hidden until at least two zone levels have been added (`this.UberSelector.length >= 1`).
- Font colour for the combo box is hardcoded white (`0xFFFFFF`) in Open Sans 12pt via a `TextFormat` applied to the dropdown renderer style.
