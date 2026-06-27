# debugui.swf
> Minimal developer/debug overlay UI that displays a live key-value list of engine debug entries. Entries can be added, updated, or removed at runtime via ExternalInterface calls. Not shown to regular players; used internally during development.

**Document/main class:** `DebugBase` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 1

## Main class: `DebugBase`

Creates a `Sprite` container on `configUI` and maintains a `Dictionary` of `TextField` instances keyed by entry name. Each entry is a labelled text field rendered in Comfortaa 24pt green (`0xDDDDA2`). Fields stack vertically with 26px spacing, recalculated on resize or on any add/remove.

### Public methods

_(All entry management is via private handlers called through ExternalInterface.)_

### Key fields

- `_entries : Dictionary` — weak-keyed map from entry name (`String`) to its `TextField`.
- `container : Sprite` — display parent for all entry text fields; positioned at `(0, 0)` on resize.

### Runtime dependencies & integration

**ExternalInterface callbacks registered:** `ADD_ENTRY(value, name)`, `UPDATE_ENTRY(value, name)`, `REMOVE_ENTRY(name)`, `ON_RESIZE(width, height)`.

No `ExternalInterface` calls out; this is a pure display receiver.

**Text format:** Comfortaa, size 24, colour `0xDDDDA2` (light yellow-green), bold, left-aligned, auto-size LEFT.

**`draw()` override:** calls `positionElements()` when `DATA` invalidation is pending — iterates all `_entries` and sets `y` sequentially at 26px intervals.

**`ON_RESIZE`:** resets `container` to `(0,0)` and invalidates `DATA` to reflow.

## Other game-specific classes

None. This SWF contains only the single game-specific class `DebugBase`; all other files are shared framework (`_kiwi/`, `fl/`, `IggyFunctions.as`).

## Notable logic

- Entry display format: `"<name>: <value>"` — the name is always prepended, making the key visible alongside its value.
- `ADD_ENTRY` is a no-op if an entry with the same name already exists; use `UPDATE_ENTRY` to change an existing entry's value.
- No scroll container — entries simply stack and can overflow the stage at large counts.
- No translate keys; no Iggy-specific logic beyond the inherited `UIComponent` base.
