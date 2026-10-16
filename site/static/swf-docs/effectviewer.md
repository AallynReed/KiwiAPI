# effectviewer.swf
> A scrollable panel that lists all active status effects on the player, supporting search by name and a "removeable only" filter. It appears when the player opens the Effects/Buffs viewer window and exposes controls to remove applicable effects.

**Document/main class:** `EffectViewer` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 3 (`EffectViewer`, `Effect`, `FavoriteCheckBox`) + 1 `EffectViewer_fla` timeline class + ~13 skin/asset wrappers

---

## Main class: `EffectViewer`

`EffectViewer` is the root UI component for the effect-viewer window. On construction it registers two frame scripts (frames 1 and 11, both `stop()`), then in `configUI()` sets up the tile-view layout, highlights, search listeners, and — when running inside Iggy — registers six `ExternalInterface` callbacks so the game engine can push data and drive navigation.

On console the initial D-pad selection is forced to `(0, 0)` and the search text is made selectable/focusable by `MoveSelection`.

### Public / internal methods

- `AddEffect(entityId, localizedName, description, iconPath, timeRemaining, canRemove, bgColor) : void` — instantiates an `Effect`, calls `SetupEffect`, and appends it to `itemView`. The engine sends the list one `AddEffect` per effect and afterwards names an effect only by `entityId`. `timeRemaining` is in seconds (`<= 0` hides the countdown); `iconPath` is a texture name given to the row's `ObjectPreview`; `bgColor` is the effect's category colour, applied as a colour transform to the row's `Icon.iconBackground`.
- `UpdateEffects(deltaTime : Number) : void` — called by the engine every frame with the elapsed time in seconds. Ticks every effect's timer, re-filters the list against the current search string and "removeable only" flag, alphabetically sorts matching effects, then reconciles the tile view (removes non-matching items, adds newly-matching ones).
- `RemoveEffect(entityId : int) : void` — removes a single effect from both the internal `_effects` vector and the tile view.
- `MoveSelection(dx : int, dy : int) : void` — console D-pad navigation; `yIndex == -1` means the header row (search box / checkbox), `yIndex >= 0` means an effect tile. Shows/hides `btnSelect` and `btnSelectText` based on `CanRemove()` of the focused effect.
- `Select() : void` — activates the currently focused item: calls `ExternalInterface.call("ActivateSearchInput")` if the search box is focused, toggles the checkbox if the checkbox is focused, or calls `effect.Select()` (which fires `ExternalInterface.call("RemoveEffect", entityId)`) if an effect tile is focused.
- `Clear() : void` — empties the tile view.

### Key fields

- `itemView : ScrollableTileView` — the scrollable grid that holds `Effect` instances.
- `SearchBox : MovieClip` — contains a `searchText` TextField and a `btnClearText` button.
- `checkboxShowRemoveable : MovieClip` — contains a `Checkbox` sub-component with label `$EffectViewer_ShowOnlyRemoveable`.
- `_effects : Vector.<Effect>` — master list of all effects, regardless of filter.
- `selectedEffect : int` — entity ID of the currently highlighted effect.
- `yIndex : int` — row cursor (`-1` = header, `0+` = effect rows).
- `xIndex : int` — column cursor within the header row (0 = SearchBox, 1 = checkbox).
- `_searchString : String` — current search filter text.
- `_showOnlyRemoveable : Boolean` — whether to filter to removeable effects only.
- `btnSelect : MovieClip`, `btnSelectText : TextField`, `footerBackground : MovieClip` — footer remove-button area; visibility mirrors `CanRemove()` of the selected effect.

### Frame scripts / timeline

- **Frame 1** (`frame1`): `stop()` — holds the default "loaded" state.
- **Frame 11** (`frame11`): `stop()` — the `Console` frame label (frame 1 is `PC`). It removes `closeButtonTopRight` and places the controller footer: `footerBackground`, `btnSelect` (`btn_console_south`) with `btnSelectText`, and `btnClose` (`btn_console_east`) with `btnCloseText`.
- The 675 × 600 root timeline places its own window chrome on frame 1 (`bg`, `background`, `closeButtonTopRight`, `header`) along with `SearchBox`, `itemView` and `checkboxShowRemoveable`.

### Runtime dependencies & integration

- `ExternalInterface.addCallback("ClearEffects", Clear)` — game calls to wipe the list.
- `ExternalInterface.addCallback("UpdateEffects", UpdateEffects)` — game calls each tick with a deltaTime.
- `ExternalInterface.addCallback("AddEffect", AddEffect)` — game calls to push a new effect.
- `ExternalInterface.addCallback("RemoveEffect", RemoveEffect)` — game calls to remove an effect by ID.
- `ExternalInterface.addCallback("MoveSelection", MoveSelection)` — console D-pad handler.
- `ExternalInterface.addCallback("Select", Select)` — console confirm handler.
- `ExternalInterface.call("ActivateSearchInput")` — tells the game to open the on-screen keyboard.
- `ExternalInterface.call("RemoveEffect", entityId)` (from `Effect.Select`) — asks the game to remove an effect, under the same name as the inbound `RemoveEffect` callback the game answers with.
- `IggyFunctions.inIggy` guard — all callbacks/listeners skipped in standalone preview.

---

## Other game-specific classes

- `Effect` (extends `UIComponent`, embeds `assets.swf#symbol41`) — represents a single effect row tile. Stores entity ID, localized name, description, time remaining, removeable flag, icon texture string, and background color. `SetupEffect(…)` / `TransferEffect(other)` populate fields; `UpdateEffect(dt)` ticks the countdown and formats it via `GetTimeString()` (seconds / minutes / hours / days). `PassesFilter(searchStr, showOnlyRemoveable)` drives visibility in `EffectViewer.UpdateEffects`. `Select()` calls `ExternalInterface.call("RemoveEffect", entityId)` and posts sound `Play_ui_button_select`. Translate keys used: `$ClubUI_AdventureNpc_Remove`, `$EffectViewer_Seconds`, `$EffectViewer_Minutes`, `$EffectViewer_Hours`, `$EffectViewer_Days`.
- `FavoriteCheckBox` (extends `_kiwi.Controls.Checkbox`, embeds `assets.swf#symbol14`) — styled checkbox asset; no additional logic beyond four frame-stop scripts.

### EffectViewer_fla timeline symbols

- `CheckBox_26` (embeds `assets.swf#symbol182`) — MovieClip wrapper around a `Checkbox` component with a `Highlighted` child. Sets the checkbox label to `$EffectViewer_ShowOnlyRemoveable` via component inspector.

### Asset wrappers (no logic)

13 skin/asset classes at the top level: `CellRenderer_*Skin` (×6), `ComboBox_*Skin` (×3), `ScrollArrow*_*Skin`, `ScrollThumb_*Skin`, `ScrollTrack_skin`, `ScrollBar_thumbIcon`, `List_skin`, `focusRectSkin`, `SlotBackground`, `metaIcon0`, `art`, `btnClearText` — all are pure embedded graphic symbols or trivial MovieClip wrappers.

---

## Notable logic

- **Alphabetical sort on every tick**: `UpdateEffects` sorts the `matchingEffects` array by `GetLocalizedName().toLowerCase()` on every call (i.e., every game frame), then reconciles the tile view rather than rebuilding it from scratch — removing missing items first, then inserting newly-visible ones.
- **Time formatting tiers**: `GetTimeString()` auto-selects the display unit (seconds < 5 min, minutes < 3 h, hours < 1 day, days < 14 days; returns `""` for permanent/very-long effects). Seconds are shown with two decimals, the other units with one.
- **Console search**: On console, search is confirmed with Enter (keyCode 13) rather than live on change.
- **Footer visibility**: `btnSelect` / `btnSelectText` / `footerBackground` are visible only when the focused effect returns `CanRemove() == true`, giving a contextual "Remove" footer.
