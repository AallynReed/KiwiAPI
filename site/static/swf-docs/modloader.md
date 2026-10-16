# modloader.swf
> A window UI for browsing and managing installed game mods. Shows a scrollable list of mods on the left and a detail panel with title, author, notes, warnings, and a rendered preview on the right, along with Enable/Disable actions. Opened from the MODS button of the escape menu (`escapewindow.swf`, `$EscapeMenu_ModLoader`).

**Document/main class:** `ModLoader` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 5 (excluding framework/asset wrappers)

## Main class: `ModLoader`

Constructs by pulling sub-references from child `MovieClip` containers (`modListPane`, `authorInfoBox`, `modViewerWindow`) rather than using direct public vars for everything. Creates a `ModListDataSource` in `configUI`, wires it to the `ModList`, and registers `ExternalInterface` callbacks for all data entry points. When not in Iggy, seeds two dummy mods for preview.

### Public methods
- `setModDetails(modDesignation:String) : void` — The engine's answer to `OnSelected`, and the only place the author/title/notes/warning fields and the button label are written. Looks up the `Mod` by designation in the data source; fills in author, title, notes, warning text; sets `modActionButton` label to `$ModUI_Enable` or `$ModUI_Disable` and disables it when `mod.blockedBy` is non-empty.
- `setModPreview(modPreviewPath:String) : void` — Sent by the engine with `setModDetails` after `OnSelected`; an empty path means the mod has no preview. If path is non-empty, sets `modPreviewPlaceholder.iconImage`, requests `.blueprint` previews as a square of the frame's smaller side and everything else at natural size (`resize(-1,-1)`), and registers the `onImageLoaded` callback. If empty, hides placeholder and shows `previewUnavailable` (`$Preview_Unavailable`).
- `setAllModsDisabled() : void` — Sent when mod loading is switched off for the client. Shows `allModsDisabled` text field, hides the list pane, viewer and author panels.
- `setNoModsLoaded() : void` — Shows `noModsLoaded` text field, hides panels.

### Key fields
- `modList : ModList` — virtualized scrollable list of mod entries, pulled from `modListPane.modList`.
- `dataSource : ModListDataSource` — provides `Mod` objects; receives `addMod`, `removeMod`, `updateMod`, `associateFileWithMod`, `clear` via `ExternalInterface`.
- `modActionButton : LabelButton` — Enable/Disable toggle button; click calls `OnEnable` (if `mod.disabled`) or `OnDisable` with the designation of the row at `modList.curDataIndex`. The row is looked up among the pooled on-screen rows, so a click does nothing while the selected row is scrolled out of that range.
- `modPreviewPlaceholder : ArtClip` — shows the mod's preview, which is an object preview rather than a picture file (the SWF's own non-Iggy preview data, `Test #1`/`Test #2`, uses `.prefab` paths); positioned and scaled in `onImageLoaded`.
- `winHeader : WindowHeaderSmall` — titled `$ModUI_ModsTitle`.
- `allModsDisabled / noModsLoaded : TextField` — state banners, initially hidden. Their timeline text is `$ModUI_AllModsDisabled` ("Mod Loading is Disabled\nPlease check your settings." — the English string spells the break as a literal `\n` escape, not a newline) and `$ModUI_NoModsLoaded` ("You don't have any mods yet!", followed by where to place `.tmod` files).

### Runtime dependencies & integration
- `ExternalInterface` callbacks registered: `setModDetails`, `setModPreview`, `setAllModsDisabled`, `setNoModsLoaded` (here), `clear`, `addMod`, `associateFileWithMod`, `updateMod`, `removeMod` (`ModListDataSource`) and `selectIndex` (`ModList`); all only when `IggyFunctions.inIggy`.
- Load order: one `addMod` per mod, then one `associateFileWithMod` per file of that mod. The engine sends every file path; `Mod.Warnings()` is what cuts the display to two.
- `ExternalInterface.call("OnSelected", designation)` — sent by `ModListItem.setSelected(true)`; the engine answers with `setModDetails(designation)` and `setModPreview(path)`.
- `ExternalInterface.call("OnEnable", designation)` / `OnDisable` — enable/disable a mod; the engine re-sends the selection after each one.
- Preview handshake: `iconImage` hands the path to the `ArtClip`'s `ObjectPreview` as its texture name, which calls `UIComponent.CheckTextureExists(path)`; the engine renders the object and answers `objectPreviewReady(path)`, which binds the bitmap and runs `onImageLoaded`.
- `IggyFunctions.translate` keys: `$ModUI_AuthorPrefix`, `$ModUI_NotesPrefix`, `$ModUI_Enable`, `$ModUI_Disable`, `$ModUI_ModsTitle`. `$ModUI_WarningsPrefix` ("Warnings: ") and `$ModUI_ModsTitle_AllModsDisabled` ("MODS (Disabled)") ship in `ui.binfab` but are not used by this SWF.
- Preview image scaling logic in `onImageLoaded` fits the image inside the frame rectangle, letter-boxing as needed. Blueprint previews are additionally constrained square.

---

## Other game-specific classes

### `ModListDataSource` (extends `EventDispatcher`)
Manages an array of `Mod` objects. All mutations are exposed via `ExternalInterface`:
- `clear()` — empties the array and fires `DataChangeEvent.DATA_CHANGE` with `REMOVE_ALL`.
- `addMod(title, author, notes, previewPath, blockedByMod, blocksOtherMods, disabled, isRestartRequired, modSource)` — appends a new `Mod` and fires `DataChangeEvent.ADD`. Always appended, so the array stays in the order the engine sent the mods.
- `associateFileWithMod(title, author, filePath, blocked)` — finds the first mod with that title+author (the call carries no `modSource`) and calls `mod.AddFile()` to register the file as blocked or unblocked; dropped if no such mod has been added yet.
- `updateMod(title, author, blockedByMod, blocksOtherMods, disabled, isRestartRequired)` — patches the first mod with that title+author and fires `DataChangeEvent.CHANGE`.
- `removeMod(title, author)` — removes by title+author match and fires `PRE_DATA_CHANGE REMOVE`.
- `getItemDataByDesignation(designation)` — looks up a `Mod` by its composite key (`modSource-title-author`).

### `Mod`
Data model for a single mod. Key fields: `author`, `title`, `notes`, `previewPath`, `blockedBy:String` (name of the blocking mod), `blocksOther:Boolean`, `disabled:Boolean`, `restartRequired:Boolean`, `modSource:String` (the non-Iggy preview data uses "Steam"), `unblockedFiles:Array`, `blockedFiles:Array`. `Designation()` returns `"modSource-title-author"`. `AddFile(filename, blocked)` appends to the appropriate array. `Warnings()` builds a human-readable warning string using translate keys `$ModUI_List_BlockedStatus` ("Blocked by {0}:", `{0}` = `blockedBy`), `$ModUI_List_BlockingStatus`, `$ModUI_List_EnabledStatus`, `$ModUI_List_DisabledStatus`, `$ModUI_List_BlockedFiles_Footer` ("+ {0} more files"), showing up to 2 filenames inline: the blocked files when `blockedBy` is set, otherwise the unblocked ones, with the rest counted by the same footer key in both cases.

### `ModList` (extends `_kiwi.Controls.ScrollableView`) — Embed symbol87
Virtualized 16-item pooled list of `ModListItem` rows. Selection is tracked via `curDataIndex`. On item click, deselects the previous item and selects the clicked one via `setSelected(true/false)`. `ExternalInterface` callback `selectIndex(index)` allows programmatic selection; `index` is a position in the data source's array (the order the engine sent `addMod`), not an on-screen row. It does not scroll (the private `setScrollLocation` is never called), so a row outside the pooled range is highlighted, and reported with `OnSelected`, only once it is scrolled into view. `relayoutItems` sets `alternateColor` on every other row, but `ModListItem.draw` never reads it, so rows are not drawn differently. Exposes `enableMod`/`disableMod` (call `EnableMod`/`DisableMod` externally), but nothing in the SWF calls them; the action button uses `OnEnable`/`OnDisable`.

### `ModListItem` (extends `_kiwi.Core.UIComponent`) — Embed symbol13
Individual mod row. Displays `modAuthorTextField`, `modTitleTextField`, `modStatusTextField`, and a `modListItemCaution` warning icon. `setData(mod:Mod)` stores the mod and invalidates. On `draw`: fills text fields; status text uses translate keys `$ModUI_List_EnabledStatus`, `$ModUI_List_DisabledStatus`, `$ModUI_List_EnabledNeedRestartStatus`, `$ModUI_List_DisabledNeedRestartStatus`; status colour is green (0x60DE00) whenever enabled (the `blocksOther` branch sets the same green), red (0xFF0000) when disabled with a non-empty `blockedBy`, grey (0x838383) when disabled otherwise; caution icon visible when `blockedBy.length > 0 || blocksOther`. `setSelected(true)` calls `ExternalInterface.call("OnSelected", designation)` and goes to frame label "Selected"; `setSelected(false)` goes to "unselected".

### Asset wrappers
`dummy` (BitmapData, 52x52 placeholder PNG embed), `btnGreen`, `btnGreen_small`, `btnGreenIcon_small` — skin/button asset classes, no logic. Plus standard scroll-bar skins (13 classes).

## Notable logic
- **Mod designation key**: `modSource + "-" + title + "-" + author` — the handle in `OnSelected`, `OnEnable`/`OnDisable` and `setModDetails`. Only `addMod` carries `modSource`; `updateMod`, `removeMod` and `associateFileWithMod` arrive with title and author alone and act on the first match, so those three calls cannot tell apart two mods that share a title and author.
- **`OnSelected` repeats**: `ModList` calls `setSelected(true)` on whichever row holds `curDataIndex` each time rows are drawn from the pool — when the selected row scrolls back into view, and after every `addMod`, `removeMod` or `clear`, which return all rows to the pool — so the engine receives `OnSelected` more than once per selection. `curDataIndex` is never adjusted when the array changes, so after a `removeMod` at or before it the next `OnSelected` names whichever mod now holds that index.
- **Conflict visualization**: `modListItemCaution` icon lights up both when a mod is blocked by another AND when it blocks others (`blocksOther`), alerting the user to any conflict participant.
- **Image preview fitting**: `onImageLoaded` computes `scaleX`/`scaleY` ratios and picks the smaller one to maintain aspect ratio, then centers the result within the frame rectangle (inset 4 px left/right and 8 px top/bottom). Only a preview larger than that rectangle is scaled; a smaller one is centered at 1:1.
- **Restart-required state**: Stored on the `Mod` and surfaced in the list item status text — the game sets this flag when a mod change requires a restart to take effect.
