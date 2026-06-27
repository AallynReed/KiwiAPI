# progression.swf
> An interactive skill-tree / progression map UI that displays a zoomable, pannable node graph showing unlocked, locked, and upgradeable abilities or content nodes. It appears in Trove when the player opens the Progression or Mastery map screen, allowing them to browse and select nodes to upgrade using skill points. A companion `ProgressionProgressMeter` component (embedded separately in the same SWF) renders the skill-point bar, milestone markers, and potential reward slots above the map.

**Document/main class:** `Progression` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 13

---

## Main class: `Progression`

`Progression` owns the entire map viewport: a `mapStage` MovieClip that is dragged, scaled, and panned by the player. All map nodes (`NodeA`, `NodeB`) are created at runtime and added as children of `mapStage.map`. Node metadata is stored in a `Dictionary` of `ProgressionValue` objects keyed by node name. Gradient connection lines between nodes are drawn procedurally into a `paths` MovieClip using `Graphics.lineGradientStyle`.

Constructor wires mouse listeners (move, wheel, down, up, leave, click, over, out), initialises the node dictionaries, positions and scales `mapStage` to default values, and calls `addFrameScript` for frames 19, 20, and 40. `configUI()` adds an `ENTER_FRAME` listener that reveals nodes one per frame (the `animateIn` intro sequence), attaches the `ObjectPreview` image child, and registers all Iggy/ExternalInterface callbacks. On consoles the cursor `MovieClip` is shown and button-legend text is populated; on PC both are hidden.

### Public methods

- `showNextNode(e:Event) : void` — ENTER_FRAME handler; pops one entry from `animateIn`, makes that named map child visible, removes itself when the array is empty (initial reveal animation).
- `setSelected(name:String) : void` — Iggy callback; looks up the named `ProgressionValue` and calls `selectNode()`.
- `addNode(name, parent, unlocked, upgraded, canUnlock, type, title, desc, xOffset, yOffset) : void` — Iggy callback; creates a `NodeA` (type 0) or `NodeB` (type 1) MovieClip, wraps it in a `ProgressionValue`, adds it to `mapStage.map`, registers it in both `nodes` and (if root) `rootNodes`, then invalidates DATA.
- `updateNode(name, unlocked, upgraded, canUnlock) : void` — Iggy callback; updates flags on an existing `ProgressionValue` and refreshes the node's frame label.

### Key fields

- `mapStage : MovieClip` — the scrollable / zoomable container; default scale `1.45`, position `(960, 540)`.
- `background : MovieClip` — full-screen backdrop, resized in `onStageResized`.
- `consoleCursor : MovieClip` — visible highlight reticle on console platforms; inverse-scaled to appear constant size.
- `m_buttonLegend : MovieClip` — console button-legend overlay; contains `legendButtons.closeButton`, `interact`, `center_btn`, `center`.
- `image : ObjectPreview` — 256×256 texture preview added to `mapStage.map.mapBackground.art`; texture name set via `SetBackground`.
- `nodes : Dictionary` — maps node name `String` → `ProgressionValue`; the authoritative runtime node registry.
- `allNodes : Dictionary` — secondary public dictionary (populated externally).
- `rootNodes : Array` — ordered list of names of nodes with no parent; used for left/right highlight navigation.
- `currentNode : MovieClip` — the currently selected node MovieClip.
- `paths : MovieClip` — `mapStage.map.paths`; receives all procedural gradient line drawing.
- `globalRotation : Number` — angular offset applied to the radial node layout.
- `guiMapRange : int` / `resolutionStageRange : Number` — coordinate mapping parameters set by `setGuiMapRange`; used in `rotateNode` to convert game-space offsets to screen positions.

### Frame scripts / timeline

- Frame 19 (`frame19`) — `stop()`
- Frame 20 (`frame20`) — `stop()`
- Frame 40 (`frame40`) — `stop()`

These correspond to distinct visual states of the root `Progression` timeline (e.g. different UI configurations or platforms).

### Runtime dependencies & integration

- **ExternalInterface callbacks registered:** `UIComponent.onStageResized`, `GainedFocus`, `LostFocus`, `setSelected`, `addNode`, `updateNode`, `setRotation`, `setGuiMapRange`, `centerOnCurrentNode`, `scaleMap`, `moveMap`, `moveHighlight`, `SetBackground`, `SetMapLocation`.
- **ExternalInterface calls outbound:** `SelectNode(name)`, `UIComponent.OnShowTooltip(x, y, title, desc, nodeName)`, `UIComponent.OnHideTooltip()`, `NOTIFY_RESIZED(w, h)`.
- `IggyFunctions.inIggy` guards all callback registration; `IsConsole()` / `IsNX()` switch console-specific layout.
- `IggyTween` used in `centerOnCurrentNode` to animate `mapStage.x/y` on console.
- Node state frame labels: `"Locked"`, `"Unlocked"`, `"CanUnlock"`, `"CanUpgrade"`, `"Upgraded"`, each optionally suffixed `"_Hovered"` or `"_Selected"`.
- Map drag uses `mapStage.startDrag()` / `stopDrag()`; mouse wheel calls `scaleMap(±1)` clamped between `MIN_SCALE=1.3` and `MAX_SCALE=2.8`.
- `rotateNode()` recursively lays out the node tree radially and draws gradient lines; the gradient uses eight colour stops at alpha 0→opaque→0 to fade line ends.

---

## Other game-specific classes

- `ProgressionValue` — plain data record: `node:MovieClip`, `parent:String`, `children:Array`, `unlocked`, `upgraded`, `canUnlock:Boolean`, `xOffset/yOffset:Number`, `title/desc:String`.
- `ProgressionProgressMeter` (extends `UIComponent`, embeds symbol128) — skill-point bar companion component; manages `m_markers:Array` of `ProgressionUpgradeMarker` and `m_rewards:Array` of `ProgressionPotentialReward`, draws a mask-scaled progress bar (`m_barMask`), exposes console D-pad highlight navigation (`moveHighlight`), and registers Iggy callbacks: `clearRewards`, `addReward`, `setBarProgress`, `addMarker`, `clearMarkers`, `invalidateCurrentSkillPoints`, `showResetButton`, `hideResetButton`, `moveProgressMeterHighlight`. Reset button fires `ExternalInterface.call("RESET_REQUEST")`.
- `ProgressionPotentialReward` (extends `UIComponent`, embeds symbol104) — reward icon slot displayed above the progress bar; shows a `Slot` art clip, a `m_percentChance:TextField`, and a `m_highlight:MovieClip`. Initialised via `initialize(claimRef, iconInstance, percentChance)`.
- `ProgressionUpgradeMarker` (extends `UIComponent`, embeds symbol108) — milestone tick mark on the progress bar; stores `m_assignedSkillPoints:int`, displays skill-point count, goes to frame label `"active"` or `"inactive"` via `setActive(bool)`.
- `ProgressionNode` (extends `MovieClip`, embeds symbol3) — generic node art with a `hover:MovieClip` child; 5 frame states (stops at frames 1, 11, 21, 31, 41).
- `MapNode` (extends `MovieClip`, embeds symbol137) — alternate node art with `hover:MovieClip`; same 5-state frame structure.
- `MapNodeHub` (extends `MovieClip`, embeds symbol136) — hub/root node art with `hover:MovieClip`; 3 frame states (frames 1, 11, 21).
- `NodeA` (extends `MovieClip`, embeds symbol68) — small node type (type 0); 15 frame-stop states (frames 1–71 in steps of 5) encoding all state/hover/select combos.
- `NodeB` (extends `MovieClip`, embeds symbol37) — large node type (type 1); identical 15-state frame layout to `NodeA`.
- `NodePath` (extends `MovieClip`, embeds symbol6) — animated path segment; frame 20 loops back to frame 1, frame 21 stops (loop / stop modes).
- `btnGreen` (extends `LabelButton`, embeds symbol147) — green label button skin used in the progression panel; 4 frame states (10, 20, 30, 40).
- `image` (extends `_kiwi.Controls.ArtClip`, embeds symbol148) — art clip used as a placeholder/thumbnail inside the map background.
- `dummy` (extends `BitmapData`) — 52×52 asset wrapper embedding `82_dummy.png`; trivial placeholder bitmap.

**`Progression_fla` timeline symbols (3):** `slotFrame_13` (symbol75, 3-state slot frame), `equipped_15` (symbol81, 2-state equipped badge), `qualityPips_21` (symbol98, 1-state quality pip row) — all pure embedded MovieClip symbols with only `stop()` frame scripts.

---

## Notable logic

- **Radial layout:** `draw()` calls `rotateNode()` recursively, distributing root nodes evenly around 360° and spreading children within each branch's angular slice. Node positions are derived from `xOffset`/`yOffset` mapped through `guiMapRange`/`resolutionStageRange` into screen space; the map must receive `setGuiMapRange` before layout is meaningful.
- **Gradient path lines:** Each edge is drawn with `lineGradientStyle` using a nine-stop alpha ramp so lines fade in from the parent and out at the child, with opacity controlled by the node's `unlocked` state (0 = invisible, 0.5 = dim, 1 = full).
- **Console cursor snap:** `moveMap()` (called by Iggy) moves the `consoleCursor` in world space and calls `snapToNearestNode()` which iterates all nodes and selects the closest within `SNAP_RADIUS=10000` squared units.
- **D-pad highlight navigation:** `moveMapHighlight(dx, dy)` walks the parent/children arrays: horizontal movement steps among siblings (or root nodes), vertical movement moves up to a parent or down to the first child.
- **`animateIn` intro:** Node visibility is staggered; nodes are hidden on the timeline and made visible one per `ENTER_FRAME` tick in the order listed in `animateIn` (`["hub","a","b","c"]` by default, overridable from game code before the UI loads).
