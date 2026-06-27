# modloader.swf
> A window UI for browsing and managing installed game mods. Shows a scrollable list of mods on the left and a detail panel with title, author, notes, warnings, and an image preview on the right, along with Enable/Disable actions. Appears when the player opens the Mod Manager from the settings or launcher.

**Document/main class:** `ModLoader` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 5 (excluding framework/asset wrappers)

## Main class: `ModLoader`

Constructs by pulling sub-references from child `MovieClip` containers (`modListPane`, `authorInfoBox`, `modViewerWindow`) rather than using direct public vars for everything. Creates a `ModListDataSource` in `configUI`, wires it to the `ModList`, and registers `ExternalInterface` callbacks for all data entry points. When not in Iggy, seeds two dummy mods for preview.

### Public methods
- `setModDetails(modDesignation:String) : void` — Looks up the `Mod` by designation in the data source; fills in author, title, notes, warning text; sets `modActionButton` label to `$ModUI_Enable` or `$ModUI_Disable` and disables it when `mod.blockedBy` is non-empty.
- `setModPreview(modPreviewPath:String) : void` — If path is non-empty, sets `modPreviewPlaceholder.iconImage` and sizes the preview differently for `.blueprint` vs other asset types; registers `onImageLoaded` callback. If empty, hides placeholder and shows "unavailable" text.
- `setAllModsDisabled() : void` — Shows `allModsDisabled` text field, hides the list pane, viewer and author panels.
- `setNoModsLoaded() : void` — Shows `noModsLoaded` text field, hides panels.

### Key fields
- `modList : ModList` — virtualized scrollable list of mod entries, pulled from `modListPane.modList`.
- `dataSource : ModListDataSource` — provides `Mod` objects; receives `addMod`, `removeMod`, `updateMod`, `associateFileWithMod`, `clear` via `ExternalInterface`.
- `modActionButton : LabelButton` — Enable/Disable toggle button; click calls `OnEnable` or `OnDisable` with the mod's designation string.
- `modPreviewPlaceholder : ArtClip` — loads and displays the mod's preview image; positioned and scaled in `onImageLoaded`.
- `winHeader : WindowHeaderSmall` — titled `$ModUI_ModsTitle`.
- `allModsDisabled / noModsLoaded : TextField` — state banners, initially hidden.

### Runtime dependencies & integration
- `ExternalInterface` callbacks registered: `setModDetails`, `setModPreview`, `setAllModsDisabled`, `setNoModsLoaded`.
- `ExternalInterface.call("OnEnable", designation)` / `OnDisable` — enable/disable a mod.
- `IggyFunctions.translate` keys: `$ModUI_AuthorPrefix`, `$ModUI_NotesPrefix`, `$ModUI_Enable`, `$ModUI_Disable`, `$ModUI_ModsTitle`.
- Preview image scaling logic in `onImageLoaded` fits the image inside the frame rectangle, letter-boxing as needed. Blueprint previews are additionally constrained square.

---

## Other game-specific classes

### `ModListDataSource` (extends `EventDispatcher`)
Manages an array of `Mod` objects. All mutations are exposed via `ExternalInterface`:
- `addMod(title, author, notes, previewPath, blockedByMod, blocksOtherMods, disabled, isRestartRequired, modSource)` — appends a new `Mod` and fires `DataChangeEvent.ADD`.
- `associateFileWithMod(title, author, filePath, blocked)` — calls `mod.AddFile()` to register a file as blocked or unblocked.
- `updateMod(title, author, blockedByMod, blocksOtherMods, disabled, isRestartRequired)` — patches an existing mod and fires `DataChangeEvent.CHANGE`.
- `removeMod(title, author)` — removes by title+author match and fires `PRE_DATA_CHANGE REMOVE`.
- `getItemDataByDesignation(designation)` — looks up a `Mod` by its composite key (`modSource-title-author`).

### `Mod`
Data model for a single mod. Key fields: `author`, `title`, `notes`, `previewPath`, `blockedBy:String` (name of the blocking mod), `blocksOther:Boolean`, `disabled:Boolean`, `restartRequired:Boolean`, `modSource:String` (e.g. "Steam"), `unblockedFiles:Array`, `blockedFiles:Array`. `Designation()` returns `"modSource-title-author"`. `AddFile(filename, blocked)` appends to the appropriate array. `Warnings()` builds a human-readable warning string using translate keys `$ModUI_List_BlockedStatus`, `$ModUI_List_BlockingStatus`, `$ModUI_List_EnabledStatus`, `$ModUI_List_DisabledStatus`, `$ModUI_List_BlockedFiles_Footer` (with `{0}` substitution for counts), showing up to 2 filenames inline.

### `ModList` (extends `_kiwi.Controls.ScrollableView`) — Embed symbol87
Virtualized 16-item pooled list of `ModListItem` rows. Selection is tracked via `curDataIndex`. On item click, deselects the previous item and selects the clicked one via `setSelected(true/false)`. `ExternalInterface` callback `selectIndex(index)` allows programmatic selection. Items are alternately coloured. Exposes `enableMod`/`disableMod` (call `EnableMod`/`DisableMod` externally) though these are not wired to a button directly in this class.

### `ModListItem` (extends `_kiwi.Core.UIComponent`) — Embed symbol13
Individual mod row. Displays `modAuthorTextField`, `modTitleTextField`, `modStatusTextField`, and a `modListItemCaution` warning icon. `setData(mod:Mod)` stores the mod and invalidates. On `draw`: fills text fields; status text uses translate keys `$ModUI_List_EnabledStatus`, `$ModUI_List_DisabledStatus`, `$ModUI_List_EnabledNeedRestartStatus`, `$ModUI_List_DisabledNeedRestartStatus`; status colour is green (0x60C000) for enabled, red (0xFF0000) if blocked, grey (0x839343) if disabled-unblocked; caution icon visible when `blockedBy.length > 0 || blocksOther`. `setSelected(true)` calls `ExternalInterface.call("OnSelected", designation)` and plays frame "Selected".

### Asset wrappers
`dummy` (BitmapData, 52x52 placeholder PNG embed), `btnGreen`, `btnGreen_small`, `btnGreenIcon_small` — skin/button asset classes, no logic. Plus standard scroll-bar skins (13 classes).

## Notable logic
- **Mod designation key**: `modSource + "-" + title + "-" + author` — used consistently across add/remove/select/enable/disable calls to unambiguously identify mods when multiple mods may share a title.
- **Conflict visualization**: `modListItemCaution` icon lights up both when a mod is blocked by another AND when it blocks others (`blocksOther`), alerting the user to any conflict participant.
- **Image preview fitting**: `onImageLoaded` computes `scaleX`/`scaleY` ratios and picks the smaller one to maintain aspect ratio, then centers the result within the frame rectangle (with a 4px/8px/16px inset margin).
- **Restart-required state**: Stored on the `Mod` and surfaced in the list item status text — the game sets this flag when a mod change requires a restart to take effect.
