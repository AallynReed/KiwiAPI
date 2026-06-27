# atlas.swf

> The interactive world map ("Atlas") that allows players to navigate between Trove's biomes and portals. It is a zoomable, draggable map of node icons connected by path lines, and it appears when the player opens the world-selection interface.

**Document/main class:** `Atlas` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 20 (Atlas, MapNode, MapNodeHub, NodePath, biomeRisingTides, plus 15 Atlas_fla timeline symbols)

---

## Main class: `Atlas`

`Atlas` is the root UIComponent that owns the entire world map. On construction it hides all biome art clips, sets up path connectors between map nodes via `setupNodes()`, selects the hub node as the default, and scales the map stage to platform-appropriate defaults. `configUI()` then registers `ExternalInterface` callbacks so the game engine can drive the UI, and starts an `ENTER_FRAME` listener to reveal biome art clips one per frame as a staggered animate-in sequence.

The map content lives inside `mapStage` (a `MovieClip`), which contains a nested `map` clip holding all named node and biome clips, a `paths` clip for the connector lines, a `gridLines` clip, and a `consoleCursor` clip. The overall component can be panned by dragging and zoomed by mouse wheel; on console/NX the cursor is a logical sprite that snaps to nodes.

### Public methods

- `showNextBiome(e:Event) : void` — `ENTER_FRAME` handler; pops one name off the `animateIn` queue each frame, makes that biome clip visible, and removes itself when the queue is empty.
- `setNodeState(nodeId:String, unlocked:Boolean) : void` — drives a named node (and its associated biome art clips and path connector) to the `"locked"` frame label when `unlocked` is `false`. No-op if the node clip is not found.
- `setUberTwelveIcons(textureName:String, slot:int) : void` — injects an `ObjectPreview` icon (150×150) into one of three icon slots (`firstIcon`, `secondIcon`, `thirdIcon`) on the `biomeRisingTides` clip at node `rt1`. Used to display the Uber-12 / Rising Tides rotating biome icons.
- `setSelected(nodeId:String) : void` — public entry point (called via `ExternalInterface`) that looks up the named node clip and delegates to `selectNode()`.
- `centerOnCurrentNode() : void` — tweens `mapStage` so the currently selected node is centred on screen, with a slight NX-specific offset (−100 x, −160 y). Uses `IggyTween` with `None.easeIn` over 0.1 s.

### Private / internal methods

- `selectNode(node:MovieClip) : void` — deselects the previous node (returns it to `"unselected"` or `"locked"`), sets `currentNode`, zeros the hover alpha, advances the new node to `"selected"` / `"consoleSelected"` / `"lockedSelected"` depending on state, then calls `ExternalInterface.call("SelectNode", name)` to notify the engine.
- `scaleMap(delta:Number) : void` — adjusts `mapStage.scaleX/Y` by `SCALE_DELTA` (0.05) × delta, clamped to [1.3, 2.8] (PC) or [1.8, 2.8] (console). Compensates the map pan offset so the centre of the view stays fixed, and rescales the `consoleCursor` to keep it visually constant size. On NX uses `IggyTween` to animate the re-center.
- `moveMap(dx:Number, dy:Number) : void` — called from `ExternalInterface` on console sticks; moves `consoleCursor` and scrolls `mapStage`. When the cursor enters a snap radius of a nearby node (and is moving toward it), snaps to that node, calls `selectNode` + `centerOnCurrentNode`, and starts `pauseMovement` timer (200 ms) to throttle further snapping.
- `moveHighlight(dx:int, dy:int) : void` — D-pad navigation; steps from the current node's grid position in the given direction up to 3 grid cells, snapping to the nearest found node and centering. Falls back to a pixel-distance walk if grid lookup fails.
- `setupNodes() : void` — iterates every path array, looks up each named clip in `mapStage.map`, stores it in `allNodes` keyed by grid point string, zeroes hover alpha, disables `mouseChildren`, and creates a `NodePath` connector stretched and rotated between consecutive nodes.
- `drawGrid() : void` — draws a 19×19 semi-transparent grid (265 px cell) on `mapStage.map.gridLines` for development reference. Visibility is controlled by `showGrid`.
- `findNearestNode(pt:Point, maxDist:Number) : MovieClip` — searches a 3×3 grid neighbourhood around `pt`, returns the closest node within `maxDist` squared distance.
- `snapToNearestNode(pt:Point, maxDist:Number) : Boolean` — thin wrapper around `findNearestNode`; selects the node and returns `true` if found and different from current.
- `onMouseMoveEvent(e:MouseEvent) : void` — shows/hides `hover` sub-clip alpha on whichever node clip the mouse is over.
- `onMouseEvent(e:Event) : void` — `MOUSE_DOWN` starts `mapStage.startDrag()` and, if the target is a `MapNode` or `MapNodeHub`, selects it. `MOUSE_UP` / `MOUSE_LEAVE` stops drag.
- `onMouseWheel(e:MouseEvent) : void` — delegates to `scaleMap(±1)`.
- `onStageResized(w:Number, h:Number, dpiScale:Number) : void` — scales the component, repositions and resizes the `background` clip to fill the viewport, notifies the engine via `ExternalInterface.call("NOTIFY_RESIZED", w, h)`.

### Key fields

| Field | Type | Role |
|---|---|---|
| `mapStage` | `MovieClip` | Top-level container for the scrollable/zoomable map; position and scale are manipulated for pan/zoom. |
| `background` | `MovieClip` | Fills the screen behind the map; resized on `onStageResized`. |
| `consoleCursor` | `MovieClip` | Logical cursor sprite used on console/NX platforms; position tracks the selected node. |
| `allNodes` | `Dictionary` | Maps grid-point strings (e.g. `"(3, 2)"`) to their node `MovieClip` references for O(1) neighbour lookup. |
| `currentNode` | `MovieClip` | The currently selected map node. |
| `nodeSelectedLabel` | `String` | Frame label to use for selection: `"selected"` (PC/NX) or `"consoleSelected"` (console). |
| `allPaths` | `Array` | Nine path arrays (`path0`–`path8`) each listing ordered node IDs that form a chain. |
| `animateIn` | `Array` | Queue of biome/art clip names to reveal one-per-frame on startup. |
| `nodeArt` | `Object` | Map from node ID to array of associated biome art clip names shown/locked alongside the node. |
| `biomeRisingTides` | `MovieClip` | Direct timeline reference to the Rising Tides biome clip (also accessible via `mapStage.map`). |
| `firstRisingTidesIcon` / `secondRisingTidesIcon` / `thirdRisingTidesIcon` | `ObjectPreview` | 150×150 `_kiwi.Core.ObjectPreview` instances injected into the Rising Tides clip's icon slots. |
| `pauseMovement` | `Timer` | Single-shot 200 ms timer; prevents consecutive node snapping immediately after a snap. |
| `showGrid` | `Boolean` | Dev toggle; controls `gridLines` visibility. |
| `locationChanged` | `Boolean` | Set on `MOUSE_DOWN` drag start; used by `scaleMap` to rebase the map offset. |
| `lastHovered` | `MovieClip` | Tracks the last hovered node so its hover alpha can be zeroed when the mouse leaves. |

### Constants

| Constant | Value | Meaning |
|---|---|---|
| `INITIAL_X / INITIAL_Y` | 960 / 540 | Default `mapStage` anchor position (approximately screen centre at 1080p). |
| `MIN_SCALE / MAX_SCALE` | 1.3 / 2.8 | PC zoom range. |
| `CONSOLE_MIN_SCALE / CONSOLE_MAX_SCALE` | 1.8 / 2.8 | Console zoom range. |
| `DEFAULT_SCALE / CONSOLE_DEFAULT_SCALE` | 1.8 / 2.5 | Zoom on open. |
| `SCALE_DELTA` | 0.05 | Zoom step per wheel tick or `scaleMap` call. |
| `CURSOR_CONSOLE_SCALE` | 2.8 | Scale factor used to keep the console cursor a constant visual size as the map zooms. |
| `CURSOR_CONSOLE_SPEED_SCALE` | 2 | Multiplier applied to analog stick input before moving the cursor. |
| `GRID_SIZE` | 265 | Pixel spacing between map grid cells; used for node bucketing and grid drawing. |
| `SNAP_RADIUS` | 10000 | Squared distance threshold for console cursor snap-to-node. |
| `POST_SNAP_TIMEOUT` | 0.2 s | Dwell time after a snap before further snapping is allowed. |

### Map path topology

The map is divided into nine ordered chains:

| Path | Nodes (in order) |
|---|---|
| path0 (main) | tutorial01 → hub → a0 → a1 … a11 (14 nodes) |
| path1 | a5 → f6 |
| path2 | a7 → h8 |
| path3 | a9 → l10 |
| path4 | stlobby (standalone) |
| path5 | a10 → e11 |
| path6 | sky (standalone) |
| path7 | geode → geode11 |
| path8 | rt1 (standalone — Rising Tides) |

Each consecutive pair within a path gets a dynamically created `NodePath` connector stretched and rotated between them.

### Runtime dependencies & integration

- **`ExternalInterface` callbacks registered (engine → Flash):**
  - `UIComponent.onStageResized` → `onStageResized(w, h, dpiScale)`
  - `setNodeState(nodeId, unlocked)` — lock/unlock a node and its art
  - `setSelected(nodeId)` — programmatic node selection
  - `centerOnCurrentNode()` — re-centre camera
  - `scaleMap(delta)` — zoom in/out
  - `moveMap(dx, dy)` — analog stick pan (console)
  - `moveHighlight(dx, dy)` — D-pad navigation (console)
  - `setUberTwelveIcons(textureName, slot)` — Rising Tides icon injection

- **`ExternalInterface` calls (Flash → engine):**
  - `SelectNode(nodeName)` — fired whenever a node becomes selected
  - `NOTIFY_RESIZED(w, h)` — fired after stage resize

- **`IggyTween`** — used for smooth `mapStage.x/y` pan animations (0.1 s, `None.easeIn`) triggered by `centerOnCurrentNode`, `scaleMap` (NX only), and constructor (NX only).

- **`IggyFunctions.inIggy`** — guards `ExternalInterface` registration; when false (design-time / standalone), a local test state is applied instead.

- **`IsConsole()` / `IsNX()`** — global Iggy runtime functions detected at startup to branch scale limits, cursor behaviour, and pan offsets.

- **`_kiwi.Core.ObjectPreview`** — used for the three Rising Tides icon slots; texture name set via `setUberTwelveIcons`.

- **Mouse events** — `MOUSE_MOVE`, `MOUSE_WHEEL`, `MOUSE_DOWN`, `MOUSE_UP` on `stage`; `MOUSE_LEAVE` and `CLICK` on `mapStage`.

- **`Event.ENTER_FRAME`** — used transiently during the animate-in sequence; removed once all biome clips are revealed.

---

## Other game-specific classes

- `MapNode` — Embeds `assets.swf` symbol28; a generic world map node MovieClip with a `hover` sub-clip and 5 frame-stop labels (frames 1, 11, 21, 31, 41) corresponding to states: unselected, selected, consoleSelected, lockedSelected, locked.
- `MapNodeHub` — Embeds `assets.swf` symbol11; the hub (Trove HQ) node variant with 3 frame-stop labels (frames 1, 11, 21).
- `NodePath` — Embeds `assets.swf` symbol3; the connector line between nodes. Loops frames 1–19 and stops on frame 21 (`"locked"` state). Dynamically instantiated by `setupNodes()`, sized and rotated to span between consecutive nodes.
- `biomeRisingTides` — Embeds `assets.swf` symbol34; the Rising Tides biome art clip. Has three named child slots (`firstIcon`, `secondIcon`, `thirdIcon`) for `ObjectPreview` icons. Three looping animation sections end with `gotoAndPlay("unselected")`, `gotoAndPlay("selected")`, `gotoAndPlay("locked")` at frames 901, 912, 924 respectively.

**Atlas_fla timeline symbols (15 classes):** `biomeShadowTower_6`, `biomeEverdark_8`, `biomeForbiddenSpires_10`, `biomegiantlands_12`, `biomeIgneous_01_17`, `biomeSkyRealm_01_19`, `biomeSkylands_01_21`, `biomeDrowned_01_23`, `biomeDragonfire_25`, `biomeJurassic_27`, `biomeNeonCity_29`, `biomeStarter_31`, `biomeCandoria_33`, `biomePermafrost_37`, `biomeFaeForest_39`, `biomeDesertFrontier_35`, `biomeCursedVale_41`, `PrimeWorld_43`, `biomeGeode_02_4`. All follow the same pattern: embed an `assets.swf` symbol and register 3 frame-stop scripts (frames 1, 11, 21) for the unselected / selected / locked states. They are the visual art clips for each biome tile on the map.

---

## Notable logic

- **Staggered animate-in:** All biome art clips start hidden. Each `ENTER_FRAME` tick reveals the next one in the `animateIn` queue (20 biomes total), producing a sequential reveal animation on map open without any explicit timer.

- **Grid-bucketed node lookup:** Nodes are stored in `allNodes` keyed by grid cell coordinates (`Math.floor(x / 265 − 0.5)` × `Math.floor(y / 265 − 0.5)`). `findNearestNode` searches only the 3×3 neighbourhood around the query point, giving O(1)-ish performance regardless of total node count.

- **Console cursor vs. mouse:** On PC, standard mouse events drive panning and node hover. On console, an invisible `consoleCursor` sprite is moved by `moveMap()` (analog stick) and snapped to the nearest node when it passes within the snap radius and is moving toward it. After a snap the `pauseMovement` timer blocks further snapping for 200 ms. D-pad uses `moveHighlight()` which steps by grid unit from the current node.

- **NX-specific pan offset:** On Nintendo Switch (`IsNX()`), all centering tweens apply a −100 x / −160 y offset relative to the standard formula, and the pan uses a 0.6 scale factor instead of 0.5, presumably to account for different viewport proportions.

- **Zoom + pan coupling:** When the user has dragged the map away from its origin (`locationChanged = true`) and then zooms, `scaleMap` adjusts both `mapStage.map.x/y` (inner content) and `mapStage.x/y` (outer container) to keep the view centred on the zoom origin rather than the stage origin.

- **Rising Tides icons:** `setUberTwelveIcons` accepts a texture name and a slot index (0–2) and places an `ObjectPreview` (which resolves a named game texture at runtime) into the corresponding icon child of the `biomeRisingTides` clip, allowing the rotating Uber-12 biome to display its current world's icon dynamically.
