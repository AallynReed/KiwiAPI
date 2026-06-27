# waypointdisplay.swf

> Renders in-world waypoint markers on the Trove HUD — directional arrows and icons that track named points of interest (flags, bases, resources, low-health allies). The engine drives all updates through ExternalInterface; the SWF manages a live dictionary of `Waypoint` instances and scales them uniformly on demand.

**Document/main class:** `Root` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 2

---

## Main class: `Root`

Acts as the scene container for all active waypoints. On `configUI()` it registers four ExternalInterface callbacks covering the full waypoint lifecycle. It maintains a `Dictionary` of `Waypoint` instances keyed by string ID. Uniform scaling is handled by overriding `scaleX`/`scaleY` to store a single `_scale` value and propagate it to all children on the next `draw()`.

### Public methods / overrides

- `set scaleX(param1:Number) : void` / `get scaleX() : Number` — stores scale in `_scale`; invalidates DATA if changed.
- `set scaleY(param1:Number) : void` / `get scaleY() : Number` — mirrors `scaleX` (uniform scale).

### Key fields

- `_waypoints : Object` (weak-key `Dictionary`) — maps string waypoint ID → `Waypoint` instance.
- `_scale : Number` — current uniform scale factor; default `1`.

### Runtime dependencies & integration

**ExternalInterface callbacks registered** (in `configUI`):

| Callback | Signature | Behavior |
|---|---|---|
| `WAYPOINT.ADD` | `(id:String, type:String, color:uint)` | Creates a new `Waypoint` (or updates existing) with the given type and color; adds to display list. |
| `WAYPOINT.UPDATE` | `(id, type, color, distance:Number, x:Number, y:Number, onScreen:Boolean, rotation:Number)` | Calls `ADD` if missing, then sets all positional and state properties on the `Waypoint`. |
| `WAYPOINT.REMOVE` | `(id:String)` | Removes the `Waypoint` from the display list and deletes it from the dictionary. |
| `WAYPOINT.RESET` | `()` | Removes all waypoints and replaces the dictionary with a fresh one. |

---

## Other game-specific classes

### `Waypoint`

A single waypoint marker, extending `_kiwi.Core.UIComponent`. Embeds symbol `symbol26` from `/_assets/assets.swf`.

#### Key fields

- `arrow : MovieClip` — off-screen directional arrow indicator.
- `icon : MovieClip` — multi-frame clip; frame is selected by waypoint type.
- `distanceText : TextField` — numeric distance label.
- `shadowText : TextField` — identical shadow behind `distanceText`.
- `border : MovieClip` — border decoration; visible only when `onScreen` is `false`.
- `_type : String` — one of `Waypoint_Flag`, `Waypoint_Base`, `Waypoint_LowHealth`, `Waypoint_Resource`, `Waypoint_Unknown`.
- `_color : uint` — applied as a `ColorTransform` to `icon.icon`.
- `_distance : Number` — display distance; rounded to nearest integer for display.
- `_onScreen : Boolean` — when `false`, shows `border`; `Waypoint_LowHealth` also hides the entire widget when off-screen.

#### Property setters

- `set position(param1:Point)` — sets `x`/`y`, subtracting own dimensions to right/bottom-align to the point (only when coordinate is non-zero).
- `set rotation(param1:Number)` — overridden; rounds the value but currently performs no further action (stub/incomplete).
- `set type(param1:String)` — maps type string to an `icon.gotoAndStop()` frame: Flag→1, Base→2, LowHealth→3, Resource/Unknown/default→4. Invalidates STYLES.
- `set color(param1:uint)` — stores color and invalidates STYLES.
- `set distance(param1:Number)` — updates both `distanceText` and `shadowText` with `Math.round(distance)`.
- `set onScreen(param1:Boolean)` — toggles `border.visible`; hides entire sprite for `Waypoint_LowHealth` when off-screen.

#### `draw()` override

When STYLES is invalid, creates a `ColorTransform` with `color = _color` and applies it to `icon.icon.transform.colorTransform` to tint the inner icon.

---

## Notable logic

- Uniform scaling: `Root` intercepts `scaleX`/`scaleY` to enforce a single `_scale` value, then re-applies it to every `Waypoint` on the next `draw()` pass. This avoids distortion from independent axis scaling.
- Waypoint icon frames are hardcoded to specific type strings; unrecognized types fall back to frame 4 (generic resource icon).
- Distance display uses a shadow (`shadowText`) for legibility — same pattern as `notifications.swf`.
- `WAYPOINT.UPDATE` can implicitly create missing waypoints by falling through to `onWaypointAdd`, making the ADD callback optional from the engine's perspective.
