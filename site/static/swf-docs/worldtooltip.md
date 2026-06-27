# worldtooltip.swf
> Rich item/world-object tooltip panel displayed when the player hovers over or inspects an item in Trove. Supports a primary info panel with a name, stat rows, and an optional side compare panel showing stats of the currently equipped item.

**Document/main class:** `Tooltip` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 3

## Main class: `Tooltip`

`Tooltip` is the root component. It starts invisible and builds its layout from data pushed via `ExternalInterface`. The constructor hides the component, clears any previous content, and attaches a `ComparePanel` child. Layout is driven by an invalidation system inherited from `UIComponent`; `DATA` invalidation triggers `recalculateStaging()` and `STYLES` invalidation reapplies name-text color/effects. After every layout pass it calls `ExternalInterface.call("NOTIFY_RESIZED", width, height)` so the game host can reposition the tooltip.

### Public methods
- `clear() : void` — resets display name, hides compare panel, shrinks height to name field height, clears the `StackList`, starts a fresh pending row.
- `newRow() : void` — commits the current `pendingRow` into `rows` StackList and starts a new `AlignmentCellsRow`.
- `setRowCell(align:uint, text:String, color:uint, size:Number, font:String) : void` — creates a `TextField` with the given format and places it in the pending row at the specified alignment cell. Enables word-wrap if text exceeds panel width. On console, font size is bumped by 2.
- `addStat(label:String, value:String, color:uint, bonusLevel:int) : *` — wraps a `StatRow` (label+value) in the pending row's left cell and auto-advances to a new row.
- `addLineBreak() : void` — inserts a centered decorative horizontal line shape as a row.
- `clearStats() : void` — delegates to `comparePanel.clear()` and invalidates.
- `setSidePanelEdge(edge:int) : void` — positions `comparePanel` to the left (`EDGE_LEFT`), right (`EDGE_RIGHT`), or below (`EDGE_BOTTOM`) the primary panel, or hides it (`EDGE_HIDE`).
- `getPrimaryPanelWidth() : int` — returns `background.width`; called by the host to size the compare panel offset.

### Key fields
- `background : MovieClip` — the panel backdrop; its width and height are resized dynamically.
- `nameTextField : TextField` — displays the item/object name; supports multiline wrap for long names.
- `rows : StackList` — vertical container for all `AlignmentCellsRow` children.
- `pendingRow : AlignmentCellsRow` — the row currently being built before being committed to `rows`.
- `qualityMeter : QualityMeter` — star/quality indicator; positioned at x=360 and shown only when `quality >= 0`.
- `headerBar : MovieClip` — optional colored header bar; when present, controls header height layout.
- `comparePanel : ComparePanel` — side panel for gear comparison stats; created in constructor.
- `_color : uint` — name text color (default white `0xFFFFFFFF`).
- `_rainbow : Boolean` — when true, applies per-character rainbow coloring to the name.
- `_shadow : Boolean` — when true, applies a drop-shadow + glow filter to the name.
- `firstStatRow : int` — y-position of the first stat row, used to align the compare panel vertically.

### Frame scripts / timeline
None — `Tooltip` is a pure ActionScript component with no frame scripts.

### Runtime dependencies & integration
- `IggyFunctions.inIggy` — gates all `ExternalInterface` callbacks; in preview mode populates sample data instead.
- `ExternalInterface.addCallback(...)` — game registers: `NEW_ROW`, `SET_ROW_CELL`, `LINE_BREAK`, `ADD_STAT`, `CLEAR_STATS`, `SET_SIDEPANEL_EDGE`, `CLEAR`, `REDRAW`, `getPrimaryPanelWidth`.
- `ExternalInterface.call("NOTIFY_RESIZED", width, height)` — notifies host after every layout recalculation.
- `ExternalInterface.call("RequestRedraw")` — signals host that a visual redraw is needed after draw().
- `IsConsole()` — bumps font sizes by 2 in `setRowCell`.
- `fl.motion.Color` — used to build the rainbow color array (six Color transforms: red, orange, yellow, green, cyan, purple).
- `flash.filters.DropShadowFilter` / `GlowFilter` — applied to `nameTextField` for legendary/shadow items.

---

## Other game-specific classes

- `ComparePanel` (extends `_kiwi.Core.UIComponent`) — Embed `symbol12`; side panel that lists comparison `StatRow` entries via `COMPARISON.ADD_STAT` callback. Expands width to fit the widest stat row. Calls `ExternalInterface.call("RequestRedraw")` after each data invalidation. Exposes `ChildCount` and `StatListPosition` getters used by `Tooltip` for vertical alignment.
- `QualityMeter` (extends `_kiwi.Core.UIComponent`) — displays item quality as a frame-based clip. Clamps quality to 0–5 and calls `gotoAndStop("lvl" + quality)` on draw, matching frame labels `lvl0`–`lvl5`.

---

## Notable logic
- The tooltip width is always at least `DEFAULT_MIN_WIDTH` (450 px) and the name field word-wraps above `DEFAULT_MAX_WIDTH` (640 px).
- Rainbow coloring cycles through six preset `fl.motion.Color` objects character-by-character using `setTextFormat` on individual char ranges.
- `recalculateStaging()` handles three layout cases: compare panel taller than primary (expands height), no stat rows (vertically centers compare panel), or stat rows present (aligns compare panel's `StatListPosition` to `firstStatRow`).
- After every staging recalculation, `NOTIFY_RESIZED` is called so the C++ host can move the tooltip window.
