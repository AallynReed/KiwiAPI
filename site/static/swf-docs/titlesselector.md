# titlesselector.swf
> The Titles selector window where players browse, filter, favourite, and equip name titles (prefixes and suffixes) for their character. It includes a live preview of the player's formatted name with the currently equipped titles and a name colour picker.

**Document/main class:** `TitlesSelector` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 6

## Main class: `TitlesSelector`

`TitlesSelector` is the root UIComponent for the titles panel. It owns search/filter controls, display-setting checkboxes, a name-colour `KiwiComboBox`, a live `PlayerNamePreview` TextField, a count label, and a `titles` MovieClip that contains the inner `TitlesView`. Frame scripts at frames 1 and 11 both call `stop()`.

`configUI()` registers all Iggy callbacks and wires event listeners:
- `SearchBox.btnClearText` CLICK → clears search text and re-sorts
- `SearchBox.searchText` CHANGE (PC) or KEY_DOWN/Enter (console) → `sortList()`
- `PrefixCheckBox`, `SuffixCheckBox`, `FavCheckBox` checkbox CLICK → `sortList()`
- `ShowPlayerNames`, `ShowNameAndPowerRank`, `DisplayClubNames` checkbox CHANGE → respective `OnShow*` callbacks
- `m_nameColorComboBox` CHANGE → `OnSelectNameColor`, updates `titleView` colour
- Mouse wheel on the panel → forwarded to `titleView.MouseWheel()`
- All `Highlighted` child clips are initially hidden

`sortList()` calls `titleView.Sort(trimmedSearch, prefix, suffix, fav)` which re-renders the tile grid.

### Public methods
- `addTitle(id, localizedName, tags, location, isFav, isNew, description, isEquipped) : void` — delegates to `titleView.AddTitle()`, updates `titleCount` with translated `$Titles_Count` string and 12pt format.
- `updateTitle(id, isFav, isNew, isEquipped) : void` — delegates to `titleView.UpdateTitle()`.
- `updatePlayerName(name, prefix, suffix) : void` — assembles `PlayerNamePreview` text as `"<prefix> <name><suffix>"` with smart spacing/punctuation handling.
- `removeTitle(id) : void` — delegates to `titleView.RemoveTitle()`, updates count.
- `Fav() : void` — delegates to `titleView.Favorite()` (console favourite button).
- `ShowInfo() : void` — delegates to `titleView.ShowInfo()` (console info button).
- `Select() : void` — branches on `dPadSelectionY`: ≥0 → `titleView.Select()`; -1 → search row (activate search input or toggle checkbox); -2 → settings row (toggle DisplayClubNames or open/close ComboBox); -3 → `ShowNameAndPowerRank` toggle; -4 → `ShowPlayerNames` toggle.
- `MoveSelection(dx, dy) : void` — console D-pad navigation across a virtual 2-D grid: rows ≥0 are the title tiles (delegated to `titleView.MoveSelection`), row -1 is the search/filter bar (SearchBox at x=0, checkboxes at x=1–3), row -2 is the settings row (DisplayClubNames at x=0, colour dropdown at x=1), row -3 is ShowNameAndPowerRank, row -4 is ShowPlayerNames. Shows/hides `Highlighted` MovieClip on the focused control; when at row -1/x=0 focuses the search TextField.
- `ClearEquipedTitles() : void` — delegates to `titleView.ClearEquipedTitles()`.
- `HandleDirectionInput(dir:int) : *` — moves `m_nameColorComboBox.selectedIndex` when the ComboBox is open (used by console D-pad).
- `addNameColor(label:String, data:int) : void` — adds an item to `m_nameColorComboBox`.
- `selectNameColor(colorId:int, colorValue:uint) : *` — finds matching combo item, sets `PlayerNamePreview.textColor`, and propagates to `titleView.SetColor` + `UpdateEquiped`.
- `SetSettings(showNamePowerRank:Boolean, showPlayerNames:Boolean, displayClubNames:Boolean) : void` — sets initial checkbox states.
- `onMouseWheel(event:MouseEvent) : void` — forwards to `titleView.MouseWheel`.

### Key fields
- `titles : MovieClip` — container; `titles.titleView` is the `TitlesView` instance.
- `titleView : TitlesView` — the 3-column tiled title grid (private, set in `configUI`).
- `FavCheckBox, PrefixCheckBox, SuffixCheckBox : MovieClip` — search filter checkboxes (each has a `.checkbox` Kiwi Checkbox child and a `.Highlighted` clip).
- `SearchBox : MovieClip` — contains `searchText:TextField` and `btnClearText:MovieClip`.
- `ShowPlayerNames, ShowNameAndPowerRank, DisplayClubNames : MovieClip` — display setting checkboxes.
- `PlayerNamePreview : TextField` — live formatted name preview.
- `titleCount : TextField` — shows total visible title count.
- `m_nameColorComboBox : KiwiComboBox` — name colour picker.
- `dropdownHighlighted : MovieClip` — highlight overlay for the combo box (console).
- `favoriteButton, infoButton : MovieClip`; `favoriteButtonText, infoButtonText : TextField` — console action buttons; visibility tracks whether a tile row is selected (`dPadSelectionY >= 0`).
- `dPadSelectionX / dPadSelectionY : int` — current D-pad cursor position.
- `inComboBox : Boolean` — whether the colour ComboBox is open in console mode.
- `searchOptions : Vector.<MovieClip>` — `[SearchBox, PrefixCheckBox, SuffixCheckBox, FavCheckBox]` — the search bar row for D-pad navigation.
- `HighlightedMovieClip : MovieClip` — the currently visible highlight clip.

### Frame scripts / timeline
- Frame 1, 11 — both `stop()`. Two visual states (PC layout vs. console layout implied).

### Runtime dependencies & integration
**ExternalInterface callbacks registered (Iggy):**
`addTitle`, `updateTitle`, `updatePlayerName`, `RemoveTitle`, `Sort`, `SetSettings`, `AddNameColor`, `SelectNameColor`, `MoveDropdown` (→ `HandleDirectionInput`), `ClearSelectedTitles` (→ `ClearEquipedTitles`), `MoveSelection`, `Select`, `Favorite` (→ `Fav`), `Info` (→ `ShowInfo`)

**ExternalInterface calls fired:**
- `ActivateSearchInput` — console, when search box is selected via D-pad
- `OnSelectNameColor(data:int)` — when colour combo changes
- `OnShowNamePowerRank` — when `ShowPlayerNames` checkbox changes (note: the event handlers appear inverted by name but the callbacks are wired to the correct checkboxes)
- `OnShowPlayerNames` — when `ShowNameAndPowerRank` checkbox changes
- `OnShowClubNames` — when `DisplayClubNames` checkbox changes

**translate keys:** `$Titles_PrefixOnly`, `$Titles_SuffixOnly`, `$Titles_FavoriteOnly`, `$Settings_ShowPlayerNames`, `$Settings_ShowOwnNameplate`, `$Settings_DisplayClubName`, `$Titles_Count`

---

## Other game-specific classes

### `TitlesView` (extends `_kiwi.Controls.ScrollableTileView`) — [Embed symbol111]
The 3-column tiled grid of `Title` cells. Constants: `TILES_PER_ROW = 3`, spacing (H=1, V=6, margin=1). Maintains `_titles:Vector.<Title>` and `selectedIndex:int` (-1 = none). Key operations:
- `AddTitle(...)` — creates a `Title` item, calls `SetupTitle`, adds to tile view, auto-selects index 0 if first.
- `Sort(search, prefix, suffix, fav)` — filters `_titles` by `MatchesState`, then sorts by tag-intersection score against each other (most overlapping tags come first), then re-adds matching items to the view.
- `MoveSelection(dx, dy)` — converts 1-D `selectedIndex` into row/column, clamps movement to grid bounds, scrolls selected tile into view.
- `UpdateEquiped()` — re-applies `SetEquipped` on all equipped titles with the current name colour.
- `SetColor(color)` — stores `nameColor`; 0 → default white (0xFFFFFF).
- `GetTotalRows()` — `ceil(numItems / 3)`, used by `TitlesSelector` to know when D-pad exits the tile area.

### `Title` (extends `_kiwi.Core.UIComponent`) — [Embed symbol26]
A single title tile. Stores: `_titleId`, `_localizedTitle`, `_tags:Vector.<String>`, `_location:int` (1=prefix, 2=suffix, 3=both), `_favorite:Boolean`, `_description:String`, `_new:Boolean`, `_equiped:Boolean`, `_highlighted:Boolean`. Visual children: `localizedTitle:TextField`, `location:TextField`, `NewTag:MovieClip`, `favorite:Checkbox`, `infoButton:MovieClip`, `background:MovieClip`, `Equiped:MovieClip`, `Highlighted:MovieClip`.

`SetupTitle` populates all fields, translates title via `IggyFunctions.translate(localizedTitle)`, sets location string from `$Titles_Prefix/Suffix/Both` keys, and calls `SetColorData` (brightens equipped indicator by 1.5× per channel).

Events fired: `OnSelect(titleId)`, `POST_SOUND_EVENT("Play_ui_button_select")`, `OnFavorited(titleId)`, `OnSeen(titleId)` (on roll-over), `ShowInfo(x, y, titleId)` / `HideInfo` (on info button hover or console toggle).

`MatchesState(search, prefix, suffix, fav)` — filters by location type and favourite state first, then checks `_localizedTitle.toLowerCase().indexOf(search)`, full tag match, comma-split tags, space-split tags.

Two timeline frames (1 and 2), both `stop()`.

### `FavoriteCheckBox` (extends `_kiwi.Controls.Checkbox`) — [Embed symbol20]
Dynamic subclass of `Checkbox` with four animation keyframe stops (frames 10, 20, 30, 40) for its checked/unchecked up/over/down states. No additional logic.

### `TitlesSelector_fla/` timeline symbols (2 classes)
- `CheckBox_4` — checkbox symbol at frame 4.
- `btnInfo2_58` — info button symbol at frame 58.

### Asset wrappers (22 classes)
Scrollbar skins: `ScrollArrowDown_*` (4), `ScrollArrowUp_*` (4), `ScrollThumb_*` (3), `ScrollTrack_skin`, `ScrollBar_thumbIcon`, `focusRectSkin`. List/ComboBox skins: `List_skin`, `CellRenderer_*` (6 states), `ComboBox_*` (4 states). Misc: `metaIcon0`, `BtnGreen`, `btnClearText`.

---

## Notable logic
- **Tag-based sorting:** After filtering, `TitlesView.Sort` ranks results by how many of their tags overlap with the tags of other results in the set — a rudimentary "most related to search" heuristic.
- **D-pad grid with settings rows:** `MoveSelection` treats rows -4 through -1 as non-tile rows above the grid, each with their own x-column layout. The `favoriteButton` and `infoButton` are hidden when the cursor is in any settings row.
- **Name preview assembly:** `updatePlayerName` handles punctuation edge cases — checks whether the last character of the prefix or first character of the suffix is in a set of punctuation chars before adding spaces.
- **Colour propagation:** When name colour changes via the combo box, `SetColor` is called on `TitlesView`, which re-renders the `Equiped` indicator on every currently equipped title.
- **ComboBox console navigation:** When `inComboBox` is true, `MoveSelection` delegates to `HandleDirectionInput` instead, routing D-pad movement to cycle `selectedIndex` within the colour dropdown.
