# calltoaction.swf
> A tutorial/onboarding overlay that draws a semi-transparent mask over the screen with a cutout ellipse highlighting a specific UI area, and places animated arrow indicators pointing to it. Shown by the game engine when it wants to direct the player's attention to a particular UI element.

**Document/main class:** `CallToAction` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 4

---

## Main class: `CallToAction`

`CallToAction` manages a full-screen darkening mask with a transparent elliptical cutout, plus a set of directional arrow sprites. The game engine builds the mask in three steps: `beginMask` → `setMaskDimensions` + `setUnmaskedEllipse` → `endMask`. Arrow indicators are added/removed separately via `addCallToAction` / `clearCallsToAction`.

All callbacks are registered in `configUI()`. When not running inside Iggy, the constructor runs a demo scenario: a 300×200 mask with a 70×20 ellipse hole centred at (100, 100).

### Public methods

- `clearMask() : void` — clears all programmatic graphics from `maskShape` and nulls `maskHoleShape`.
- `beginMask() : void` — calls `clearMask()`; starts a new mask definition sequence.
- `endMask() : void` — if `maskHoleShape` is set, calls `maskHoleShape.draw(maskShape.graphics, FILL_COLOR, FILL_ALPHA, width, height)` to render the filled overlay with the cutout; adds `maskShape` to `maskContainer` (only on first call); resizes `maskContainer` to `maskShapeWidth × maskShapeHeight`.
- `setUnmaskedEllipse(x:int, y:int, w:int, h:int) : void` — creates a new `EllipseShape` and stores it as `maskHoleShape`; this defines where the transparent hole will be cut.
- `setMaskDimensions(w:int, h:int) : void` — stores the full overlay dimensions.
- `addCallToAction(x:int, y:int) : void` — instantiates a `CallToActionArrow` MovieClip, positions it, adds it to the display list, and appends it to the `callsToAction` array.
- `clearCallsToAction() : void` — removes all `CallToActionArrow` instances from the display list and empties `callsToAction`.

### Key fields

- `FILL_COLOR : uint = 0x4D4D4D` (5066061) — the colour of the darkening overlay.
- `FILL_ALPHA : Number = 0.7` — opacity of the overlay; leaves the cutout area fully transparent.
- `maskContainer : MovieClip` — display container that holds `maskShape`; resized by `endMask`.
- `maskShape : Shape` — the programmatically drawn overlay (rectangle minus ellipse cutout).
- `maskShapeWidth / maskShapeHeight : int` — dimensions set by `setMaskDimensions`.
- `maskHoleShape : InputMaskShape` — the shape strategy object that knows how to draw the cutout; currently always an `EllipseShape`.
- `callsToAction : Array` — list of active `CallToActionArrow` MovieClips.

### Runtime dependencies & integration

**ExternalInterface callbacks registered (Iggy → Flash):**
| Callback | Method |
|---|---|
| `clearMask` | `clearMask` |
| `beginMask` | `beginMask` |
| `endMask` | `endMask` |
| `setUnmaskedEllipse` | `setUnmaskedEllipse` |
| `addCallToAction` | `addCallToAction` |
| `clearCallsToAction` | `clearCallsToAction` |
| `setMaskDimensions` | `setMaskDimensions` |

No calls are made from Flash back to the game engine; this overlay is purely display-driven.

**IggyFunctions:** `IggyFunctions.inIggy` gates callback registration only.

---

## Other game-specific classes

- `CallToActionArrow` — `[Embed symbol="symbol3"]` plain `MovieClip` wrapper; the animated arrow asset placed at game-specified coordinates. No added logic.
- `InputMaskShape` — abstract base class with a no-op `draw(graphics, color, alpha, width, height)` method; serves as the interface for mask-hole strategies.
- `EllipseShape` — extends `InputMaskShape`; stores `(x, y, width, height)` of the ellipse cutout. Its `draw()` method fills the entire overlay rectangle in two halves (top half above ellipse centre, bottom half below), each time leaving the ellipse quadrant unfilled by drawing the bounding rectangle minus the ellipse arc using Bézier curves. Uses a cubic-Bézier approximation with recursive de Casteljau splitting (`bezierSplit`, `cBez`, `drawBezierPts`) to approximate the ellipse arcs as quadratic curves acceptable to Flash's `Graphics.curveTo`.

---

## Notable logic

- **Strategy pattern for mask holes** — `InputMaskShape` is an extensible base; only `EllipseShape` is currently implemented, but additional shapes (rectangle cutout, polygon, etc.) could be added without changing `CallToAction`.
- **Ellipse drawn as two-half fill** — `EllipseShape.draw` renders the overlay in two rectangular fills split at the ellipse vertical midpoint, with the ellipse arc subtracted from each half. This avoids Flash's lack of a native "fill with hole" primitive.
- **Cubic → quadratic Bézier conversion** — Flash's `Graphics` API only supports quadratic Bézier curves. `EllipseShape` implements a recursive adaptive subdivision (`cBez`) to convert cubic Bézier control points into approximated quadratic segments, controlled by a flatness threshold parameter.
- **Single-child guard** — `endMask` checks `maskContainer.numChildren == 0` before adding `maskShape`, so repeated `endMask` calls don't stack multiple shape children.
