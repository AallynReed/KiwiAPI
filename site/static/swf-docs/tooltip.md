# tooltip.swf

> The Trove item tooltip panel that floats near the cursor or a selected item, displaying the item name (with optional rainbow/shadow effects), a vertical stack of stat rows, free-form text rows, and an optional side comparison panel. It reports its own size back to the engine after each layout recalculation.

**Document/main class:** `Tooltip` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 4 (including 1 `_fla` timeline symbol)

---

## Main class: `Tooltip`

`Tooltip` manages a dynamic, vertically stacked layout built from rows added by the game engine via `ExternalInterface` callbacks. It starts invisible and reveals itself as soon as `displayName` is set to a non-empty value. After each layout change it calls `ExternalInterface.call("NOTIFY_RESIZED", width, height)` so the engine can reposition the tooltip on screen, and every `draw()` pass that handled a data or style change ends with `ExternalInterface.call("RequestRedraw")`.

A `ComparePanel` instance is always present as a child (initially invisible), positioned to the left, right, or below the main panel on demand.

Besides the callbacks, the engine sets `displayName`, `minWidth`, `color`, `rainbow`, `shadow` and `quality` directly as properties. Only text and colours reach the SWF; no item, rarity or stat id is ever passed.

### Public methods

| Method | Description |
|---|---|
| `clear() : void` | Resets the tooltip: clears the name, hides the compare panel, clears all rows, starts a fresh `pendingRow`. The engine calls it (`CLEAR`) at the start of every tooltip. |
| `newRow() : void` | Commits the current `pendingRow` to `rows` (StackList) and creates a new empty `AlignmentCellsRow`. |
| `setRowCell(align, text, color, size, font) : void` | Adds a styled `TextField` into the current pending row at the given alignment slot. The text is set as `htmlText` at point size `size + 3` (+2 more on console). Wraps text if it exceeds background width. |
| `addStat(label, value, color, bonusLevel) : *` | Creates a `StatRow`, places it into the pending row, then commits with `newRow()`. `bonusLevel` sends the row's `bonusLevelClip` to frame `bonusLevel + 1`. Records the Y position of the first stat row for compare panel alignment. |
| `addLineBreak() : void` | Inserts a horizontal rule Shape into the pending row. |
| `clearStats() : void` | Delegates to `comparePanel.clear()` and invalidates the layout. |
| `setSidePanelEdge(edge:int) : void` | Positions the `ComparePanel` to left (`EDGE_LEFT=1`), right (`EDGE_RIGHT=2`), bottom (`EDGE_BOTTOM=3`), or hides it (`EDGE_HIDE=0`). |
| `getPrimaryPanelWidth() : int` | Returns `background.width` so the engine knows the primary panel's pixel width. |

### Key fields

| Field | Type | Role |
|---|---|---|
| `background` | `MovieClip` | Bitmap-filled panel (sprite 15), resized dynamically to fit content |
| `nameTextField` | `TextField` | Item name; a word-wrapping field 394 px wide in the SWF, placed at x 9.7. The 640 px `DEFAULT_MAX_WIDTH` wrap only applies when there is no `headerBar`, and the shipped SWF has one, so the field's own width decides where the name wraps |
| `rows` | `StackList` | Vertical stack container for all `AlignmentCellsRow` children |
| `pendingRow` | `AlignmentCellsRow` | Row being assembled before it is committed to `rows` |
| `qualityMeter` | `QualityMeter` | The item's star rating, placed at x 360 in the header; visible only when `quality >= 0` |
| `headerBar` | `MovieClip` | Header bar: a nine-slice bitmap (sprite 25 with a `DefineScalingGrid`). The code treats it as optional, but it is on the timeline on every frame, so the header layout mode is the one in use |
| `comparePanel` | `ComparePanel` | Side-by-side stat comparison panel |
| `_displayName` | `String` | Backing store for `displayName` property |
| `_color` | `uint` | Name text colour (default white, `0xFFFFFFFF`) |
| `_rainbow` | `Boolean` | Enables per-character rainbow colouring on the name |
| `_shadow` | `Boolean` | Enables drop-shadow + glow filter on the name |
| `_quality` | `int` | Star rating, 0–5 (-1 = hidden) |
| `padding` | `int` | Inner padding constant (8 px) |
| `firstStatRow` | `int` | Y position of the first stat row; used to align `comparePanel` |

### Frame scripts / timeline

- `frame1` — `stop()`, then immediately `gotoAndPlay("Console")`. The timeline has two labels, `PC` (frame 0) and `Console` (frame 10), and the jump has no platform check. On `Console`, `nameTextField` is a second instance (18 pt instead of 16 pt, same 394 px width) and `background`, `headerBar` and `qualityMeter` are repositioned.
- `frame11` — `stop()` — end of the Console label sequence.

Stage 400×240, 30 fps, 20 frames.

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
- `ExternalInterface.call("RequestRedraw")` — fired at the end of `draw()` whenever it processed a `DATA` or `STYLES` invalidation, and by `ComparePanel.draw()` after it resizes. Setting `minWidth`, showing the quality meter, `setRowCell` and `clearStats` all invalidate `DATA`, so each of them leads to a re-layout and a redraw request on the next frame.

**How the engine fills it:**
- Every tooltip starts with `CLEAR`; the rows then arrive over several frames rather than in one burst, and some rows carry no visible text.
- All row text arrives already translated from the game's own strings. The Power Rank line (`$NoRarityPowerRank`, "POWER RANK {0}") is sent into the centre cell of its row; on dragons and some other items it comes after the description rather than near the top.

**Placement:** the engine opens the panel to the right of and below the anchor point the requesting screen passed (for example the x/y of an `OnSlotEnter` call) and only turns it around at the edge of the game surface. It does not know where the requesting window is, so an anchor taken on a window opens the tooltip over that window.

**Filters applied programmatically:**
- `DropShadowFilter` + `GlowFilter` when `shadow = true`.
- Per-character `TextFormat` colour rotation (6-colour array) when `rainbow = true`.

**Non-Iggy preview mode:** populates the tooltip with hardcoded sample data (name "Name", three stats, two text rows) so it renders visibly in the Flash IDE.

---

## Other game-specific classes

### `ComparePanel` (embeds `/_assets/assets.swf#symbol17`)

Side panel that shows comparison stats for the currently equipped item. Extends `UIComponent`. Initially invisible with zero width; resizes to fit the widest `StatRow` added. Uses a `StackList` (`statsList`) for its rows and the same bitmap panel sprite (15) as the main `background`. Exposes:

- `addStatComparison(label, value, color) : *` — registered as `COMPARISON.ADD_STAT` via `ExternalInterface`; creates a shrunk `StatRow`, updates panel width. The following `draw()` fits `background` to the stat list and calls `RequestRedraw`. `color` is the engine's better/worse colouring. `value` arrives already formatted, and the engine's digit grouping puts a stray separator in front of three- and six-digit figures (`-,249`, `,123,456`); the main stat list does not go through that formatter.
- `StatListPosition : int` (get) — Y of `statsList`, used by `Tooltip` to align the panel with the first stat.
- `ChildCount : int` (get) — number of entries in `statsList`.
- `clear() : void` — removes all stat rows.

### `QualityMeter` (embeds `/_assets/assets.swf#symbol21`)

The star rating in the tooltip header: five stars, `level_0` through `level_4`. Extends `UIComponent`. Its 30-frame timeline has six labels, `lvl0`–`lvl5`, five frames apart: `lvl0` shows all five stars unlit and each later label lights one more, so `lvlN` lights N stars. `draw()` calls `gotoAndStop("lvl" + quality)`. Quality is clamped to 0–5 (`MAX_QUALITY`).

### `Tooltip_fla/bonusClip_8` (embeds `/_assets/assets.swf#symbol8`)

Timeline symbol in the `Tooltip_fla` package. Six-frame clip (frames 1–6, each `stop()`, labelled `lvl0`–`lvl5`) holding five `MovieClip` children named `level_0` through `level_4`, one more of them placed on each frame. It is `StatRow`'s `bonusLevelClip` (placed inside symbol 12, the `StatRow` art), not part of `QualityMeter`: `StatRow.bonusLevel = n` shows frame `n + 1` and moves the clip 10 px past the end of the stat name.

---

## Notable logic

- Width is always at least `minWidth`, which is `DEFAULT_MIN_WIDTH` (450 px) by default and whenever the engine sets a non-positive value. `DEFAULT_MAX_WIDTH` (640 px) only caps name wrapping when there is no `headerBar`.
- The `quality` setter narrows `nameTextField` to 304 px whenever the star rating is shown (`quality >= 0`), so a name cannot run under it. Nothing widens it again: an item without stars shown after one with stars still wraps its name at 304 px.
- `recalculateHeaderHeight` only ever grows the header. A name taller than `headerBar` raises the bar, `background.y` and `rows.y`, and nothing lowers them, so one long name leaves every later tooltip with the taller header.
- `StatRow`'s value field is a fixed 56 px (64 px on console), right-aligned and never resized, so a value longer than about six characters loses its left end (`+13,508` loses its sign).
- `recalculateStaging()` is the central layout pass: it sets `height` from `rows`, resizes `background`, aligns `comparePanel` (vertically centred on the first stat row if present), and emits `NOTIFY_RESIZED`.
- When `headerBar` is present the layout switches to a two-zone mode: header zone + body zone below it, with the `qualityMeter` vertically centred in the header.
- The `quality` setter directly sets `qualityMeter`'s visibility, `quality` and x position; only when the meter is shown does it also invalidate `DATA`, which brings the re-layout and redraw request described above.
