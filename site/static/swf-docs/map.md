# map.swf
> The in-game world map overlay, displaying a texture of the current zone with a dynamic legend that shows which point-of-interest categories are present. It appears when the player opens the map (M key or console equivalent) and optionally supports a zone-select / terraform mode for club worlds.

**Document/main class:** `Map` (extends `UIComponent`)
**SWF-specific classes:** 13 (+ 2 `Map_fla` timeline symbols)

---

## Main class: `Map`

`Map` is the root UIComponent for the world-map panel. On construction it registers several `ExternalInterface` callbacks so the game engine can push data in, hooks `MouseEvent` listeners for zone-select interaction, and triggers a one-frame `ENTER_FRAME` loop to run post-layout setup after all child symbols are initialised. The map texture is rendered via an `ObjectPreview` child loaded with the texture name `"_WorldMap"`. `_WorldMap` is an engine render target holding the whole picture, markers included: the player, other players, dungeons and the tether are painted into it by the engine at a fixed pixel size, so the SWF draws none of them and its icons appear only in the legend. The engine repaints it only while this window is open.

The constructor also sizes the legend's tether icon to 24×24 and hides both it and its label until the engine supplies a tether texture.

### Public methods

These twenty setters are not `ExternalInterface` callbacks: the engine assigns them as plain properties on the root, and they are the only way it reports which marker categories the current map has. Each only toggles `visible` on one icon and its caption; rows are re-stacked by `rebuildLegend()`. `you` and `player` have no setter (always listed), and the tether row is driven by `setTetherIcon()`.

- `set hasClubOfficer(param1:Boolean) : void` — shows/hides the club-officer icon+label pair in `mc_mapLegend` (caption "Expert").
- `set hasClubArchitect(param1:Boolean) : void` — shows/hides the club-architect icon+label pair (caption "Standard").
- `set hasClubMember(param1:Boolean) : void` — shows/hides the club-member icon+label pair (caption "Basic").
- `set hasClubNobuild(param1:Boolean) : void` — shows/hides the club-nobuild icon+label pair (caption "No Building").
- `set hasHub(param1:Boolean) : void` — shows/hides the hub icon+label pair.
- `set hasCleared(param1:Boolean) : void` — shows/hides the cleared icon+label pair.
- `set hasCornerstone(param1:Boolean) : void` — shows/hides the cornerstone icon+label pair.
- `set hasDungeon(param1:Boolean) : void` — shows/hides the dungeon icon+label pair.
- `set hasLargeDungeon(param1:Boolean) : void` — shows/hides the large-dungeon icon+label pair.
- `set has5StarDungeon(param1:Boolean) : void` — shows/hides the master/5-star dungeon icon+label pair.
- `set hasRecipeDungeon(param1:Boolean) : void` — shows/hides the recipe-dungeon icon+label pair.
- `set hasPVPArena(param1:Boolean) : void` — shows/hides the PvP arena icon+label pair.
- `set hasShrineUnity(param1:Boolean) : void` — shows/hides the shrine of unity icon+label pair (caption "Unity Shrine").
- `set hasVault01(param1:Boolean) : void` — shows/hides vault-type-01 icon+label pair. All three vault captions read "Clubit Union".
- `set hasVault02(param1:Boolean) : void` — shows/hides vault-type-02 icon+label pair.
- `set hasVault03(param1:Boolean) : void` — shows/hides vault-type-03 icon+label pair.
- `set hasAdventure01(param1:Boolean) : void` — shows/hides adventure-type-01 icon+label pair. All three adventure captions read "Rally of Heroes".
- `set hasAdventure02(param1:Boolean) : void` — shows/hides adventure-type-02 icon+label pair.
- `set hasAdventure03(param1:Boolean) : void` — shows/hides adventure-type-03 icon+label pair.
- `set hasOutpost(param1:Boolean) : void` — shows/hides the outpost icon+label pair (caption "Adventure Outpost").

### Key fields

- `textureImage : ObjectPreview` — holds the `"_WorldMap"` texture rendered inside `textureContainer`; created lazily in `setMapTexture()`.
- `closeButton : MovieClip` — the window's close button, sprite 28 (`_kiwi.Controls.WindowCloseButton`: a click posts the `Play_ui_window_close` sound event and calls `OnRequestClose`). Placed on the PC frame only; nothing in the SWF hides it, and `enableZoneSelect()` sets it visible on PC.
- `instructions : MovieClip` — overlay panel with a `textField`; hidden by default, shown via `setInstructions()`; on console it plays the `"Console"` label on frame 11.
- `textureContainer : MovieClip` — parent clip that hosts the map texture; used as the hit-test target for click/mouse-move zone select.
- `mc_mapLegend : MovieClip` — contains all icon/label pairs for the legend; its `legend` sub-clip height is resized dynamically by `rebuildLegend()`. Includes `tetherIcon : ExternalArt` and its `tetherTextField` label. On the PC frame it is sprite 167, a single unlabelled frame; frame 11 replaces it with the console legend `map_legend_console_43` (sprite 203).
- `m_troveHub : MovieClip` — Trove Hub panel; starts hidden (`visible = false`).
- `m_geodeHub : MovieClip` — Geode Hub panel, the Geode-world counterpart to `m_troveHub`. Unlike `m_troveHub` it is *not* hidden by the constructor, so its default visibility comes from the timeline. It has no dedicated class — it is an untyped `MovieClip` stage instance driven by the engine. Sprite 68 holds a backdrop shape and eight unnamed text fields captioned from `$WorldMap_Geode_*` keys (Sunseeker Landing, Sunseeker Square, Reliquary Research, Crystallogy Center, Companion Ranch, Crafter's Commons, Mining Facility, and "Battle Mode" from `$WorldMap_Geode_BomberRoyale`) — no `LabelButton`s.
- `mapHeader : WindowHeaderSmall` — title bar; title key `"$WorldMap_Header"` ("WORLD MAP"); disabled (non-interactive).
- `legendIcons : Array` — ordered list of 23 icon `MovieClip` refs built once on the first `ENTER_FRAME`: you, player, tether, hub, cornerstone, cleared, dungeon, largeDungeon, recipeDungeon, clubArchitect, clubOfficer, clubMember, clubNobuild, pvpArena, shrineUnity, vault01–03, adventure01–03, outpost, masterDungeon.
- `legendTextFields : Array` — corresponding `TextField` refs, index-matched to `legendIcons`.
- `zoneSelectEnabled : Boolean` — gating flag; when `true`, mouse/stick/button-A events are forwarded to the engine.
- `FrameRightMargin : int = 2`, `FrameBottomMargin : int = 8` — pixel padding applied when the engine notifies a texture resize.
- `DEFAULT_ICON_X : int = 7`, `DEFAULT_TEXT_X : int = 37`, `DEFAULT_ROW_PADDING : int = 6`, `DEFAULT_ROW_INCREMENT : int = 30` — layout constants for legend row positioning.

### Frame scripts / timeline

- **frame 1** (`frame1`) — `stop()`. Default/PC layout frame.
- **frame 11** (`frame11`) — `stop()` then `this.instructions.gotoAndPlay("Console")`. Console layout frame; triggered when `IsConsole()` is true and the ENTER_FRAME loop reaches the target frame. On the timeline it swaps the backdrop, removes the PC legend (sprite 167) and `closeButton`, and places the console legend (sprite 203) as `mc_mapLegend`. Also hides `btn_selectzone` and `btn_TerraformZone` initially (re-shown later if `enableZoneSelect()` is called).

### Runtime dependencies & integration

- **ExternalInterface callbacks registered (IggyFunctions.inIggy only):**
  - `"notifyTextureResized"` → `notifyTextureResized(width:int, height:int)` — the engine reports the pixel size it has rendered `_WorldMap` at. Calls `setMapTexture()`, then sets the root's `width`/`height` to the reported size plus the `textureContainer` offset and the frame margins. The first `setMapTexture()` call (from the constructor's own `notifyTextureResized()` call) creates the `ObjectPreview`; later calls `resize()` it from the root's pre-update size, which changes only the bitmap's drawn size, never the texture's resolution.
  - `"rebuildLegend"` → `rebuildLegend()` — repositions all visible legend icon+text pairs vertically; resizes `mc_mapLegend.legend` height. While zone select is on it first sends `mc_mapLegend` to its `"Terraform"` label.
  - `"setInstructions"` → `setInstructions(text:String)` — sets `instructions.textField.text` and makes the overlay visible.
  - `"enableZoneSelect"` → `enableZoneSelect()` — activates zone-select mode; shows appropriate UI for PC (`closeButton`) or console (`btn_selectzone`, `btn_TerraformZone`).
  - `"onMapStickChanged"` → `onMapStickChanged(x, y):Boolean` — treats `(x, y)` as a stage point: while zone select is on and the point is over `textureContainer`, converts it to local texture coords, calls `ExternalInterface.call("OnMapMouseMoved", ...)` and returns `true`; otherwise returns `false`.
  - `"onMapButtonA"` → `onMapButtonA(x, y)` — maps console A-button press (a stage point, hit-tested the same way) to `ExternalInterface.call("OnMapClicked", ...)`.
  - `"setTetherIcon"` → `setTetherIcon(name:String)` — the engine sends the texture name of the tether's icon, not a flag. Sets `mc_mapLegend.tetherIcon.textureName`; shows or hides both the icon *and* its `tetherTextField` label depending on whether the name is non-empty.
- **ExternalInterface calls (out):**
  - `"OnConsoleFrameEntered"` — notifies engine when the console frame is reached.
  - `"OnMapClicked"(x, y, w, h)` — fired on mouse click or console button-A within the texture bounds, with coords local to `textureContainer` and its current width and height. Only while zone select is on.
  - `"OnMapMouseMoved"(x, y, w, h)` — fired on mouse move or stick change within the texture bounds, same arguments. Only while zone select is on.
  - `"OnRequestClose"` — sent by `closeButton` (`WindowCloseButton`) on click, PC frame only.
- **Closing on movement:** the engine closes this window when the player moves, unless `/mapautoclose off` is set (stored as the client setting `KeepMapOpenWhileMoving`). Nothing in the SWF handles movement.
- **`setupTranslation()`** — inherited from `UIComponent`; applies localisation to text elements.
- **`IsConsole()`** — inherited from `UIComponent`; true when the platform the engine reports through `GetPlatform` is `xbone` or `ps4`. Controls console-specific layout branches (header scaling, legend button visibility, frame navigation).
- **`onTargetFrame()`** — inherited from `UIComponent`; returns true when the playhead is on the frame corresponding to the current platform, used to defer `setLegendArrays()` and `rebuildLegend()`.

---

## Other game-specific classes

- `TroveHub` (extends `UIComponent`, embeds `symbol50`) — hub-area overlay showing `LabelButton` entries for Statue, Club, Event, Character, Quest, Crafting, Leaderboard, and Battle sections; labels set from `$WorldMap_Trove_*` translate keys via `IggyFunctions.translate()`.
- `ExternalArt` (extends `ObjectPreview`, embeds `symbol29`) — generic dynamic art placeholder used for the tether icon in the legend.
- `btn_console_analog_top_left` (extends `MovieClip`, embeds `symbol13`) — analog-stick button glyph from the shared console button set, pulled into this SWF's library for the console legend.
- `dummy` (extends `BitmapData`, embeds `1_dummy.png`) — 48×48 placeholder bitmap asset.
- **`Map_fla/instructionsContainer_14`** (extends `MovieClip`, embeds `symbol74`) — two-frame symbol (frame 1 = PC, frame 11 = Console) containing a single `textField`; used as the `instructions` clip.
- **`Map_fla/map_legend_console_43`** (extends `MovieClip`, embeds `symbol203`) — the console `mc_mapLegend`, placed on frame 11 in place of the PC legend (sprite 167). Two stop frames, labelled `"Map"` (1) and `"Terraform"` (11), holding the same legend icon sprites, label `TextField`s, the `tetherIcon : ExternalArt` and `tetherTextField`, and the `legend` resize clip. Its button glyphs: `btn_closemap` (east button, `$Close_ButtonLegend` "Close") and an unnamed LT/RT row (`$WorldMap_ButtonLegend_Zoom` "Zoom In/Out") from frame 1; `btn_TerraformZone` (south button, "Terraform Zone") and `btn_selectzone` (`btn_console_analog_top_left` glyph, "Select Zone") exist only on the `"Terraform"` frame. Its `clubMemberTextField` holds the malformed text `$WorldMap_Legend_Mem$SlashCommand_ZoneRestrict_Arg_ModifyLow`, which is not a key.
- `battle_label`, `character_label`, `club_label`, `crafting_label`, `event_label`, `leaderboard_label`, `quest_label`, `statue_label` — 8 named `LabelButton` subclasses (each embeds a distinct symbol), used as the interactive labels inside `TroveHub`.

---

## Notable logic

- **Dynamic legend layout:** `rebuildLegend()` iterates `legendIcons` in a fixed order; for each visible icon it places it at `(DEFAULT_ICON_X, currentY)` and its matching `TextField` at `(DEFAULT_TEXT_X, currentY)`, advances `currentY` by `max(textHeight, 30)`, then resizes `mc_mapLegend.legend.height` to wrap the content. This means the legend automatically collapses empty rows.
- **Terraform / zone-select mode:** when the engine calls `enableZoneSelect()`, the map begins forwarding local-coordinate click/move events back to the engine via `ExternalInterface`. On console the stick and button-A handlers replicate mouse behaviour. The next `rebuildLegend()` sends `mc_mapLegend` to its `"Terraform"` label; only the console legend (sprite 203) has that label, where it adds the Terraform Zone and Select Zone glyphs. The PC legend (sprite 167) is a single unlabelled frame.
- **Club row captions:** the four club rows are captioned with the club zone-restriction level names — `clubArchitectTextField` `$SlashCommand_ZoneRestrict_Arg_ModifyStandard` ("Standard"), `clubOfficerTextField` `…_ModifyHigh` ("Expert"), `clubMemberTextField` `…_ModifyLow` ("Basic") — and `clubNobuildTextField` `$WorldMap_Legend_NoBuilding` ("No Building"). `ui.binfab` also carries `$WorldMap_Legend_Architect`, `_Officer` and `_Member` ("Architect", "Officer", "Member"), which no text field in this SWF uses.
- **Platform branching (console vs PC):** nearly every flow-control decision tests `IsConsole()`. The header text field is manually re-centred on console because `scaleX` is overridden. The map legend hides zone-select buttons on console until `enableZoneSelect()` is explicitly called.
- **Texture sizing:** the engine's only sizing input is `notifyTextureResized()`. The component reads its own width/height in the constructor (for its first `notifyTextureResized()` call) and in `setMapTexture()` (to size the preview), and writes them in `notifyTextureResized()`.
