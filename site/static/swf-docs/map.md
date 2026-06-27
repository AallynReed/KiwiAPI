# map.swf
> The in-game world map overlay, displaying a texture of the current zone with a dynamic legend that shows which point-of-interest categories are present. It appears when the player opens the map (M key or console equivalent) and optionally supports a zone-select / terraform mode for club worlds.

**Document/main class:** `Map` (extends `UIComponent`)
**SWF-specific classes:** 8

---

## Main class: `Map`

`Map` is the root UIComponent for the world-map panel. On construction it registers several `ExternalInterface` callbacks so the game engine can push data in, hooks `MouseEvent` listeners for zone-select interaction, and triggers a one-frame `ENTER_FRAME` loop to run post-layout setup after all child symbols are initialised. The map texture is rendered via an `ObjectPreview` child loaded with the texture name `"_WorldMap"`.

### Public methods

- `set hasClubOfficer(param1:Boolean) : void` — shows/hides the club-officer icon+label pair in `mc_mapLegend`.
- `set hasClubArchitect(param1:Boolean) : void` — shows/hides the club-architect icon+label pair.
- `set hasClubMember(param1:Boolean) : void` — shows/hides the club-member icon+label pair.
- `set hasClubNobuild(param1:Boolean) : void` — shows/hides the club-nobuild icon+label pair.
- `set hasHub(param1:Boolean) : void` — shows/hides the hub icon+label pair.
- `set hasCleared(param1:Boolean) : void` — shows/hides the cleared icon+label pair.
- `set hasCornerstone(param1:Boolean) : void` — shows/hides the cornerstone icon+label pair.
- `set hasDungeon(param1:Boolean) : void` — shows/hides the dungeon icon+label pair.
- `set hasLargeDungeon(param1:Boolean) : void` — shows/hides the large-dungeon icon+label pair.
- `set has5StarDungeon(param1:Boolean) : void` — shows/hides the master/5-star dungeon icon+label pair.
- `set hasRecipeDungeon(param1:Boolean) : void` — shows/hides the recipe-dungeon icon+label pair.
- `set hasPVPArena(param1:Boolean) : void` — shows/hides the PvP arena icon+label pair.
- `set hasShrineUnity(param1:Boolean) : void` — shows/hides the shrine of unity icon+label pair.
- `set hasVault01(param1:Boolean) : void` — shows/hides vault-type-01 icon+label pair.
- `set hasVault02(param1:Boolean) : void` — shows/hides vault-type-02 icon+label pair.
- `set hasVault03(param1:Boolean) : void` — shows/hides vault-type-03 icon+label pair.
- `set hasAdventure01(param1:Boolean) : void` — shows/hides adventure-type-01 icon+label pair.
- `set hasAdventure02(param1:Boolean) : void` — shows/hides adventure-type-02 icon+label pair.
- `set hasAdventure03(param1:Boolean) : void` — shows/hides adventure-type-03 icon+label pair.
- `set hasOutpost(param1:Boolean) : void` — shows/hides the outpost icon+label pair.

### Key fields

- `textureImage : ObjectPreview` — holds the `"_WorldMap"` texture rendered inside `textureContainer`; created lazily in `setMapTexture()`.
- `closeButton : MovieClip` — shown on PC when zone-select mode is active.
- `instructions : MovieClip` — overlay panel with a `textField`; hidden by default, shown via `setInstructions()`; on console it plays the `"Console"` label on frame 11.
- `textureContainer : MovieClip` — parent clip that hosts the map texture; used as the hit-test target for click/mouse-move zone select.
- `mc_mapLegend : MovieClip` — contains all icon/label pairs for the legend; its `legend` sub-clip height is resized dynamically by `rebuildLegend()`.
- `m_troveHub : MovieClip` — Trove Hub panel; starts hidden (`visible = false`).
- `mapHeader : WindowHeaderSmall` — title bar; title key `"$WorldMap_Header"`; disabled (non-interactive).
- `legendIcons : Array` — ordered list of icon `MovieClip` refs built once on the first `ENTER_FRAME`.
- `legendTextFields : Array` — corresponding `TextField` refs, index-matched to `legendIcons`.
- `zoneSelectEnabled : Boolean` — gating flag; when `true`, mouse/stick/button-A events are forwarded to the engine.
- `FrameRightMargin : int = 2`, `FrameBottomMargin : int = 8` — pixel padding applied when the engine notifies a texture resize.
- `DEFAULT_ICON_X : int = 7`, `DEFAULT_TEXT_X : int = 37`, `DEFAULT_ROW_PADDING : int = 6`, `DEFAULT_ROW_INCREMENT : int = 30` — layout constants for legend row positioning.

### Frame scripts / timeline

- **frame 1** (`frame1`) — `stop()`. Default/PC layout frame.
- **frame 11** (`frame11`) — `stop()` then `this.instructions.gotoAndPlay("Console")`. Console layout frame; triggered when `IsConsole()` is true and the ENTER_FRAME loop reaches the target frame. Also hides `btn_selectzone` and `btn_TerraformZone` initially (re-shown later if `enableZoneSelect()` is called).

### Runtime dependencies & integration

- **ExternalInterface callbacks registered (IggyFunctions.inIggy only):**
  - `"notifyTextureResized"` → `notifyTextureResized(width:int, height:int)` — resizes the `ObjectPreview` texture and resizes the component to fit.
  - `"rebuildLegend"` → `rebuildLegend()` — repositions all visible legend icon+text pairs vertically; resizes `mc_mapLegend.legend` height.
  - `"setInstructions"` → `setInstructions(text:String)` — sets `instructions.textField.text` and makes the overlay visible.
  - `"enableZoneSelect"` → `enableZoneSelect()` — activates zone-select mode; shows appropriate UI for PC (`closeButton`) or console (`btn_selectzone`, `btn_TerraformZone`).
  - `"onMapStickChanged"` → `onMapStickChanged(x, y):Boolean` — translates analog stick position to local texture coords and calls `ExternalInterface.call("OnMapMouseMoved", ...)`.
  - `"onMapButtonA"` → `onMapButtonA(x, y)` — maps console A-button press to `ExternalInterface.call("OnMapClicked", ...)`.
  - `"setTetherIcon"` → `setTetherIcon(name:String)` — sets `mc_mapLegend.tetherIcon.textureName` and toggles visibility.
- **ExternalInterface calls (out):**
  - `"OnConsoleFrameEntered"` — notifies engine when the console frame is reached.
  - `"OnMapClicked"(x, y, w, h)` — fired on mouse click or console button-A within the texture bounds, with local coords and texture dimensions.
  - `"OnMapMouseMoved"(x, y, w, h)` — fired on mouse move or stick change within the texture bounds.
- **`setupTranslation()`** — inherited from `UIComponent`; applies localisation to text elements.
- **`IsConsole()`** — global function; controls console-specific layout branches (header scaling, legend button visibility, frame navigation).
- **`onTargetFrame()`** — inherited from `UIComponent`; returns true when the playhead is on the frame corresponding to the current platform, used to defer `setLegendArrays()` and `rebuildLegend()`.

---

## Other game-specific classes

- `TroveHub` (extends `UIComponent`, embeds `symbol49`) — hub-area overlay showing `LabelButton` entries for Statue, Club, Event, Character, Quest, Crafting, Leaderboard, and Battle sections; labels set from `$WorldMap_Trove_*` translate keys via `IggyFunctions.translate()`.
- `ExternalArt` (extends `ObjectPreview`, embeds `symbol29`) — generic dynamic art placeholder used for the tether icon in the legend.
- `dummy` (extends `BitmapData`, embeds `1_dummy.png`) — 48×48 placeholder bitmap asset.
- **`Map_fla/instructionsContainer_14`** (extends `MovieClip`, embeds `symbol73`) — two-frame symbol (frame 1 = PC, frame 11 = Console) containing a single `textField`; used as the `instructions` clip.
- **`Map_fla/map_legend_console_43`** (extends `MovieClip`, embeds `symbol202`) — two-frame symbol holding all legend icon `MovieClip`s, label `TextField`s, the `tetherIcon : ExternalArt`, console buttons (`btn_selectzone`, `btn_TerraformZone`, `btn_closemap`), and the `legend` resize clip.
- `battle_label`, `character_label`, `club_label`, `crafting_label`, `event_label`, `leaderboard_label`, `quest_label`, `statue_label` — 8 named `LabelButton` subclasses (each embeds a distinct symbol), used as the interactive labels inside `TroveHub`.

---

## Notable logic

- **Dynamic legend layout:** `rebuildLegend()` iterates `legendIcons` in a fixed order; for each visible icon it places it at `(DEFAULT_ICON_X, currentY)` and its matching `TextField` at `(DEFAULT_TEXT_X, currentY)`, advances `currentY` by `max(textHeight, 30)`, then resizes `mc_mapLegend.legend.height` to wrap the content. This means the legend automatically collapses empty rows.
- **Terraform / zone-select mode:** when the engine calls `enableZoneSelect()`, the legend navigates to the `"Terraform"` frame label (changing its appearance) and the map begins forwarding local-coordinate click/move events back to the engine via `ExternalInterface`. On console the stick and button-A handlers replicate mouse behaviour.
- **Platform branching (console vs PC):** nearly every flow-control decision tests `IsConsole()`. The header text field is manually re-centred on console because `scaleX` is overridden. The map legend hides zone-select buttons on console until `enableZoneSelect()` is explicitly called.
- **Texture sizing:** the engine drives all sizing via `notifyTextureResized()`; the component never reads its own width/height for layout — it only writes them.
