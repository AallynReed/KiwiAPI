# reticle.swf
> The player's reticle (crosshair) and nearby status-bar HUD shown in Trove during gameplay. It renders health and energy bars flanking the crosshair, a circular fuel/mount bar, and a hit-reaction animation. The SWF has no explicit document class; the components are independently embedded and driven by the game engine.

**Document/main class:** none — no top-level document class file is present; components self-register via `ExternalInterface` in their own `config()` lifecycle.
**SWF-specific classes:** 5

---

## Game-specific classes

### `crosshair`
`[Embed source="/_assets/assets.swf", symbol="symbol16"]`  
Extends `MovieClip`. A pure visual asset wrapper for the crosshair graphic. No logic; no added fields or methods. Placed on the timeline as a visual symbol.

### `hitReact`
`[Embed source="/_assets/assets.swf", symbol="symbol7"]`  
Extends `MovieClip`. A pure visual asset wrapper for the hit-reaction overlay animation (typically a red flash or directional indicator played when the player takes damage). No logic beyond the constructor.

### `Health`
`[Embed source="/_assets/assets.swf", symbol="symbol6"]`  
Extends `com.kiwi.Templates.StatusBarVertical`. A vertical status bar pre-styled as the player health indicator. Inherits all bar-fill, resize, and event logic from the framework template; no overrides. The game engine drives it via the framework's data hooks.

### `Energy`
`[Embed source="/_assets/assets.swf", symbol="symbol13"]`  
Extends `com.kiwi.Templates.StatusBarVertical`. Identical structure to `Health` but represents the player's energy/mana resource. Different embedded art symbol.

### `FuelBar`
`[Embed source="/_assets/assets.swf", symbol="symbol11"]`  
Extends `com.kiwi.Core.KiwiComponent`. The most complex class in the SWF — a circular arc-fill bar indicating mount fuel or similar consumable resource.

**Fields:**
- `_currentFuel : Number` — current fuel value (updated via `FUEL_UPDATED` callback).
- `_maxFuel : Number = 100` — maximum fuel value.
- `_maskLayer : MovieClip` — programmatically drawn arc mask applied over `_statusBar`.
- `_colorSet : Boolean` — tracks whether the green color transform has been applied to avoid redundant transforms.
- `_fadeOut : IggyTween` — pre-built tween that fades the component from alpha 0.75 → 0 over 1 s (used when fuel is full).
- `_statusBar : MovieClip` — the filled bar graphic; receives a green `ColorTransform` on init.
- `_statusIndicator : MovieClip` — a secondary indicator element that switches between green (fuel > 0) and red (fuel == 0) via `ColorTransform`.

**Lifecycle:**
- `config()` — registers `FUEL_UPDATED` callback; applies green color transform to `_statusBar`; sets `_statusBar.mask = _maskLayer`; creates the fade-out tween; sets initial alpha to 0 (hidden when full).
- `draw()` — called by the framework when data is invalidated; computes fill ratio `_currentFuel / _maxFuel`; redraws arc mask via two `DrawArc` calls (symmetric ±arc from 270°); updates `_statusIndicator` color; starts or stops the fade-out tween.

**Key logic — `DrawArc(startAngle, sweep, radius)`:**  
Draws a filled wedge shape into `_maskLayer.graphics` using quadratic Bézier curve segments. The mask is drawn in two symmetric halves (`+50%` and `−50%` of the sweep value relative to 270°), producing a circular arc that represents the fill level. The arc is approximated by splitting the angle into 45° sub-segments.

**Runtime integration:**
- `ExternalInterface.addCallback("FUEL_UPDATED", onFuelUpdated)` — receives `(currentFuel:Number, maxFuel:Number)` from the game.
- `IggyTween` used for the full-fuel fade-out animation (Strong easeOut).
- `flash.geom.ColorTransform` used for bar-color state switching (green / red).

---

## Notable logic

- **No document class** — the SWF relies on Flash IDE timeline placement of component instances; each component self-configures through the Kiwi framework's `config()` / `draw()` lifecycle.
- **Fuel arc uses arc-bisection Bézier** — `DrawArc` approximates a true circular arc with quadratic Bézier curves split into ≤45° segments; it is called twice per frame to render left and right halves symmetrically from the 270° (top) starting angle.
- **Full-fuel auto-hide** — when `ratio >= 1` the fade-out tween fires; when `ratio < 1` the tween is stopped and alpha is forced back to 1, so the bar auto-hides when the player's fuel is full and reappears immediately when it starts depleting.
- **Color state machine** — `_colorSet` flag prevents redundant `ColorTransform` allocations: the green transform is applied once when fuel goes above zero; the red transform is applied whenever fuel reaches exactly zero.
