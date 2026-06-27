# subclassselect.swf
> Panel for selecting a subclass (secondary class passive) in Trove. Displays a scrollable tiled grid of available subclasses, each shown as a row with class name, level, power rank, ability icon, and a button to activate, switch, or (if locked/already active) view the class.

**Document/main class:** `SubClassSelect` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 5 (main + `SubClassRow` + `ButtonLegend_3` + `art` + button helpers)

## Main class: `SubClassSelect`

Contains a `ScrollableTileView` (`itemView`) for the grid of `SubClassRow` objects, and a `WindowHeaderSmall` (title `$subclass_select`). Populated entirely by `ExternalInterface` callbacks from the game engine; the panel itself fires `ExternalInterface.call("OnSelect", index)` when the player activates a slot.

### Public methods

- `clear() : void` — clears all items from `itemView`.
- `addSubClass(index, name, active, unlocked, isCurrentClass, level, powerRank) : void` — creates a `SubClassRow`, calls `row.init(...)`, wires its `actionButton` click, and appends to `itemView`.
- `setAbility(iconImage, displayName, description) : void` — updates the most recently added row's `icon.art` (`ArtClip`) and `abilityDescriptionText`.
- `setStat(statText) : void` — sets `nextTierText` on the most recently added row.
- `sortSubClasses() : void` — sorts `itemView` by: active first, then level descending, then powerRank descending, then index descending.
- `highlightSlot(index, direction) : void` — sets `rowHighlight.alpha = 1` and auto-scrolls into view if the row is near the edge of the viewport.
- `unhighlightSlot(index) : void` — sets `rowHighlight.alpha = 0`.

### Key fields

- `itemView : ScrollableTileView` — hosts `SubClassRow` items; vertical scrollbar always visible; spacing `(0,2,0,0)`; horizontal centering disabled.
- `__id0_ : WindowHeaderSmall` — header bar, non-interactive, title `$subclass_select`.

### Frame scripts / timeline

- **frame 1 / 11 / 21** — `stop()` for PC, Console, and ConsoleLoc layouts.

### Runtime dependencies & integration

- `ExternalInterface` callbacks: `clear`, `addSubClass`, `setAbility`, `setStat`, `sortSubClasses`, `highlightSlot`, `unhighlightSlot`, `activateSlot`.
- `ExternalInterface.call("OnSelect", index)` — fired when action button clicked or `activateSlot` is invoked.
- `setupTranslation()` called in `configUI`.
- `IsConsole()` — skips `verticalStep = 10` on console (native scroll).

---

## Other game-specific classes

### `SubClassRow` (extends `_kiwi.Core.UIComponent`) — Embed symbol42

Single row in the grid representing one subclass option.

**`init(index, name, active, unlocked, isCurrentClass, level, powerRank) : void`** — initialises all display fields, sets `actionButton` label and enabled state:
- Active → label `$ClassChanger_Active`, disabled, frame `"selected"`.
- Locked (not unlocked) → label `$ClassChanger_Locked`, disabled, frame `"locked"`.
- Is current PC class but not the active subclass → label `$ClassChanger_MyClass`, disabled, frame `"locked"`.
- Otherwise → label `$ClassChanger_Switch`, enabled.

**Fields:** `classNameText`, `levelText`, `powerRankText`, `abilityDescriptionText`, `nextTierText` (TextFields); `actionButton:LabelButton`; `icon:MovieClip` (contains an `ArtClip` named `art`); `rowHighlight:MovieClip`.

**Private state:** `_index:int`, `_level:int`, `_powerRank:int`, `_active:Boolean` — used by the sort comparator in `SubClassSelect`.

**Translate keys used in `init`:** `$Level_X` (replace `{0}`), `$NoRarityPowerRank` (replace `{0}`), `$ClassChanger_Active`, `$ClassChanger_Locked`, `$ClassChanger_MyClass`, `$ClassChanger_Switch`.

On console, defers via `ENTER_FRAME` until `onTargetFrame()`.

### `SubClassSelect_fla.ButtonLegend_3` (extends `MovieClip`) — Embed symbol110

Console button legend clip with a single child `buttonLegendSelect:MovieClip`; stops on frame 1.

### `art` (extends `_kiwi.Controls.ArtClip`) — Embed symbol25

Thin symbol wrapper for the subclass ability icon. Used inside `SubClassRow.icon`.

### Button helpers (no additional logic)

- `btnGreen_wide` (extends `LabelButton`) — Embed symbol48; wide green button variant; four-state frame stops.
- `btnGreenIcon_small` (extends `LabelButton`) — Embed symbol37; small icon+label green button; four-state frame stops.

### Asset wrappers (no logic, trivial embeds)

- `rarity_frame_*` (×8 PNG wrappers + `rarity_frame_stellar` MovieClip) — rarity tier border graphics used in slot decorators.
- Scroll skin wrappers (×10): `ScrollArrow*_*Skin`, `ScrollThumb_*Skin`, `ScrollTrack_skin`, `ScrollBar_thumbIcon`, `focusRectSkin` — standard kiwi scroll bar skins.

## Notable logic

- `addSubClass` and `setAbility`/`setStat` follow a push-then-configure pattern: the game engine calls `addSubClass` first, then immediately calls `setAbility` and `setStat` to finish populating the last-added row (`itemView.getItem(numItems - 1)`).
- The sort comparator uses `active > level > powerRank > index` priority order, placing the currently active subclass at the top of the list.
- `highlightSlot` performs auto-scroll: scrolls down if the row bottom is within 1.5× row height of the viewport bottom, scrolls up if the row top is within 0.5× row height of the viewport top.
- `activateSlot` is the console equivalent of clicking `actionButton`; it checks `actionButton.enabled` before firing.
