# tooltip.swf

> The Trove item tooltip panel that floats near the cursor or a selected item, displaying the item name (with optional rainbow/shadow effects), a vertical stack of stat rows, free-form text rows, and an optional side comparison panel. It reports its own size back to the engine after each layout recalculation.

**Document/main class:** `Tooltip` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 4 (including 1 `_fla` timeline symbol)

---

## Main class: `Tooltip`

`Tooltip` manages a dynamic, vertically stacked layout built from rows added by the game engine via `ExternalInterface` callbacks. It starts invisible and reveals itself as soon as `displayName` is set to a non-empty value. After each layout change it calls `ExternalInterface.call("NOTIFY_RESIZED", width, height)` so the engine can reposition the tooltip on screen.

A `ComparePanel` instance is always present as a child (initially invisible), positioned to the left, right, or below the main panel on demand.

### Public methods

| Method | Description |
|---|---|
| `clear() : void` | Resets the tooltip: clears the name, hides the compare panel, clears all rows, starts a fresh `pendingRow`. |
| `newRow() : void` | Commits the current `pendingRow` to `rows` (StackList) and creates a new empty `AlignmentCellsRow`. |
| `setRowCell(align, text, color, size, font) : void` | Adds a styled `TextField` into the current pending row at the given alignment slot. Wraps text if it exceeds background width. |
| `addStat(label, value, color, bonusLevel) : *` | Creates a `StatRow`, places it into the pending row, then commits with `newRow()`. Records the Y position of the first stat row for compare panel alignment. |
| `addLineBreak() : void` | Inserts a horizontal rule Shape into the pending row. |
| `clearStats() : void` | Delegates to `comparePanel.clear()`. |
| `setSidePanelEdge(edge:int) : void` | Positions the `ComparePanel` to left (`EDGE_LEFT=1`), right (`EDGE_RIGHT=2`), bottom (`EDGE_BOTTOM=3`), or hides it (`EDGE_HIDE=0`). |
| `getPrimaryPanelWidth() : int` | Returns `background.width` so the engine knows the primary panel's pixel width. |

### Key fields

| Field | Type | Role |
|---|---|---|
| `background` | `MovieClip` | Resized dynamically to fit content |
| `nameTextField` | `TextField` | Item name; auto-sizes, wraps at `DEFAULT_MAX_WIDTH` (640) |
| `rows` | `StackList` | Vertical stack container for all `AlignmentCellsRow` children |
| `pendingRow` | `AlignmentCellsRow` | Row being assembled before it is committed to `rows` |
| `qualityMeter` | `QualityMeter` | Optional quality pip strip (visible only when `quality >= 0`) |
| `headerBar` | `MovieClip` | Optional coloured header bar (changes layout mode when present) |
| `comparePanel` | `ComparePanel` | Side-by-side stat comparison panel |
| `_displayName` | `String` | Backing store for `displayName` property |
| `_color` | `uint` | Name text colour (default white, `0xFFFFFFFF`) |
| `_rainbow` | `Boolean` | Enables per-character rainbow colouring on the name |
| `_shadow` | `Boolean` | Enables drop-shadow + glow filter on the name |
| `_quality` | `int` | Quality tier (-1 = hidden) |
| `padding` | `int` | Inner padding constant (8 px) |
| `firstStatRow` | `int` | Y position of the first stat row; used to align `comparePanel` |

### Frame scripts / timeline

- `frame1` — `stop()`, then immediately `gotoAndPlay("Console")` — suggests a console-specific layout label on the timeline.
- `frame11` — `stop()` — end of the Console label sequence.

### Runtime dependencies & integration

**`ExternalInterface.addCallback` registrations (Iggy-only):**
- `NEW_ROW` → `newRow`
- `SET_ROW_CELL` → `setRowCell`
- `LINE_BREAK` → `addLineBreak`
- `ADD_STAT` → `addStat`
- `CLEAR_STATS` → `clearStats`
- `SET_SIDEPANEL_EDGE` → `setSidePanelEdge`
- `CLEAR` → `clear`
- `REDRAW` → `onExternalRedrawRequest` (forces `draw()` + `validate()`)
- `getPrimaryPanelWidth` → `getPrimaryPanelWidth`

**Calls out to engine:**
- `ExternalInterface.call("NOTIFY_RESIZED", width, height)` — fired at the end of every `recalculateStaging()`.

**Filters applied programmatically:**
- `DropShadowFilter` + `GlowFilter` when `shadow = true`.
- Per-character `TextFormat` colour rotation (6-colour array) when `rainbow = true`.

**Non-Iggy preview mode:** populates the tooltip with hardcoded sample data (name "Name", three stats, two text rows) so it renders visibly in the Flash IDE.

---

## Other game-specific classes

### `ComparePanel` (embeds `/_assets/assets.swf#symbol18`)

Side panel that shows comparison stats for the currently equipped item. Extends `UIComponent`. Initially invisible with zero width; resizes to fit the widest `StatRow` added. Uses a `StackList` (`statsList`) for its rows. Exposes:

- `addStatComparison(label, value, color) : *` — registered as `COMPARISON.ADD_STAT` via `ExternalInterface`; creates a shrunk `StatRow`, updates panel width.
- `StatListPosition : int` (get) — Y of `statsList`, used by `Tooltip` to align the panel with the first stat.
- `ChildCount : int` (get) — number of entries in `statsList`.
- `clear() : void` — removes all stat rows.

### `QualityMeter` (embeds `/_assets/assets.swf#symbol22`)

Five-pip quality indicator strip (level_0 through level_4). Extends `UIComponent`. Has a 5-frame timeline with labels `lvl0`–`lvl4`; `draw()` calls `gotoAndStop("lvl" + quality)`. Quality is clamped to 0–5 (`MAX_QUALITY`).

### `Tooltip_fla/bonusClip_8` (embeds `/_assets/assets.swf#symbol9`)

Timeline symbol in the `Tooltip_fla` package. Six-frame clip (frames 1–6, each `stop()`), holding five `MovieClip` children named `level_0` through `level_4`. Likely the graphic used inside `QualityMeter` pips.

---

## Notable logic

- Width is always at least `DEFAULT_MIN_WIDTH` (450 px), capped at `DEFAULT_MAX_WIDTH` (640 px) for name wrapping.
- `recalculateStaging()` is the central layout pass: it sets `height` from `rows`, resizes `background`, aligns `comparePanel` (vertically centred on the first stat row if present), and emits `NOTIFY_RESIZED`.
- When `headerBar` is present the layout switches to a two-zone mode: header zone + body zone below it, with the `qualityMeter` vertically centred in the header.
- The `quality` setter directly manipulates `qualityMeter` without invalidating the component cycle, making it an immediate synchronous update.
