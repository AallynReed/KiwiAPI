# shadowtower.swf
> The Shadow Tower window, opened when interacting with a Shadow Tower portal. Lets players select a floor (boss), choose a difficulty (Normal/Hard/Ultra), review key costs, and enter the tower, with a second tab showing a leaderboard of completion times per boss and difficulty.

**Document/main class:** `ShadowTower` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 10 (excluding skin/scroll stubs)

---

## Main class: `ShadowTower`

Root component managing two tabs (Portal and Leaderboard), up to 6 floor buttons, difficulty selection, boss art display, key-cost panel, and a reset timer. Floors are selected via `node` button MovieClips inside a `floors` container. `configUI()` wires floor click listeners (with optional console tooltip offsets), tab click listeners, and registers `ExternalInterface` callbacks. Has three timeline frames (1, 11, 21), all stopped — frame 11 is the console layout.

### Public methods

- `setDefaultDifficulty(index:int, only:Boolean) : void` — sets the difficulty through `setDifficultyHelper(…, false)`, so a change still sends `OnDifficultySelected` back. When the second argument is true the index is the only difficulty this player has: the other two difficulty clips on `createPortal` are hidden. The engine uses this to refuse a pick: after `OnDifficultySelected` for a difficulty the player cannot open, it answers `setDefaultDifficulty(0, true)` and the selection snaps back.
- `setFloor(bossName:String, bossPortrait:String, accessible:Boolean, defeated:Boolean, reward:String, score:int, fluxCost:int, bestTime:int, powerRank:int) : void` — populates one floor button at `loadedFloors` index; builds a localised tooltip from translated strings; sets `completedIcon` visibility and enabled state on the `node` button; stores boss data and calls `setCurrentFloor` for floor 0 on first load. `bossName` is a translation key (translated on the not-defeated branch only); `bossPortrait` is an object-preview target; `fluxCost` fills `$ShadowTower_LevelRequirement` ("It costs {0} Flux to open this portal."); `bestTime` is seconds, shown to two places with `$TimeUnit_Seconds`; `powerRank` fills `$ShadowTower_LootLevelRequirement` ("You must be Power Rank {0} or higher to gain loot.") and is per floor, appended only when above 1. `reward` and `score` go into `{3}`/`{2}` of `$ShadowTower_DefeatedLevel` ("You defeated {0} in {1} this week!"), which has neither, so they never show.
- `setCurrentFloor(index:int) : void` — sets `selectedIcon` visibility on the active floor node, updates boss art (`bossArt.iconImage`, `setImageSize(392, 220)`), and calls `createPortal.setPortalHeading()` and `leaderboards.setLeaderBoardHeading()` with boss name and current difficulty string.
- `showBossIcons(normal:String, hard:String, ultra:String) : void` — sets `iconImage` on each difficulty slot in `leaderboards` (`normal`, `hard`, `ultra` `SlotBasic` children).
- `updateMaterials(icon:String, cost:int, shadowKeys:int, lunarKeys:int, eclipseKeys:int) : void` — sets `createPortal.artClip.iconImage`, `txt_cost` text, and shows/hides key count fields in `keysPanel` with amounts for shadow/lunar/eclipse keys, i.e. the Normal, Hard and Ultra keys (the Hard and Ultra key items are named "Moon Key" and "Eclipse Key"). Only the cost arrives with an icon; no key icon is sent.
- `addLeaderboardEntry(name:String, time:Number, isFriend:Boolean) : void` — creates a `Listing` row, fills rank/name/time fields, handles tie-rank display, colours friend entries green (`0x66FF33`), marks rank-1 row white on "active" frame.
- `addPlayerLeaderboardEntry(name:String, time:Number, rank:String) : void` — fills the player-specific row at the bottom of the leaderboard panel.
- `canEnter(enabled:Boolean) : void` — enables/disables `createPortal.btn_enter`.
- `setTimeUntilReset(seconds:Number) : void` — formats via `TimeUtil.formatCountdown` and fills `createPortal.txt_timeRemaining`.
- `moveHighlight(column:int, delta:int) : void` — console D-pad navigation. `column < 1` moves floor selection (up/down through `node` buttons); `column == 1` moves difficulty selection (cycles through normal/hard/ultra). Manages `lastHightlight` (`selectedIcon` on floor nodes vs. `selectionGlow` on difficulty buttons).
- `switchTabs() : void` — toggles between Portal and Leaderboard tab MovieClips. When switching back to Portal if `displayDirty` is true, calls `ExternalInterface.call("OnDifficultySelected", currentDifficulty)`.
- `setLeaderboardSize(n:int) : void` — resets leaderboard entry counter and clears the `rowView`.
- `scrollBarTranslate(value:Number) : void` — updates leaderboard scrollbar position.

### Key fields

- `leaderboards : ShadowTowerLeaderboard` — the leaderboard tab panel.
- `createPortal : ShadowTowerCreatePortal` — the portal entry tab panel.
- `floors : MovieClip` — container holding `floor_0`–`floor_5` as `node` (BaseButton) instances. Each floor's tooltip is set through the button's `UITooltipTitle` / `UITooltipDescription`.
- `keysPanel : MovieClip` — shows shadow/lunar/eclipse key counts. On its `PC` frame it carries one key icon (bitmap character 170, the only key art in the SWF), one name field (`lunarKeyText`, holding `$prefabs_item_key_delvekey_01_name`, "Delve Shadowkey") and the three amount fields; its `Console` frame adds `shadowKeyText` / `lunarKeyText` / `eclipseKeyText` labelled `$ShadowKey`, `$LunarKey` and `$EclipseKey`, keys defined in no shipped language file.
- `currentDifficulty : int` — index into `difficulties` array (`["normal","hard","ultra"]`).
- `difficultyString : String` — translated difficulty label for heading display.
- `keyType : String` — translated key type name for tooltip.
- `currentFloor / loadedFloors : int` — track which floor is selected and how many have been set.
- `bossData : Array` — cached per-floor data (name, portrait, accessible, defeated, etc.).
- `lastHightlight : MovieClip` — the currently console-highlighted element.
- `displayDirty : Boolean` — true when difficulty changed while on Leaderboard tab; triggers `OnDifficultySelected` on tab switch.

### Frame scripts / timeline

- Frame 1: `stop()` — PC layout.
- Frame 11: `stop()` — Console layout (fires when the SWF is running on a controller-based platform).
- Frame 21: `stop()` — third layout variant (NX/Switch).
- Stage 762×600 at 30 fps, 30 frames.

### Runtime dependencies & integration

- `ExternalInterface.addCallback` registrations: `setLeaderboardSize`, `setDefaultDifficulty`, `setFloor`, `showBossIcons`, `updateMaterials`, `addLeaderboardEntry`, `addPlayerLeaderboardEntry`, `canEnter`, `setTimeUntilReset`, `moveHighlight`, `switchTabs`, `scrollBarTranslate`.
- Outbound `ExternalInterface.call`: `OnDifficultySelected(index)`, `OnFloorSelected(floor)`, `OnAccept()`, `OnLobbyClicked()`, `GetLeaderboard(floor, difficulty)`.
- `IggyFunctions.translate` — used for difficulty strings, key type names, and floor tooltip text construction.
- `TimeUtil.formatCountdown` / `_kiwi.Util.FormatCountdownResult` — formats the reset countdown.
- Translate keys include: `$ShadowTower_LeaderboardHeading1`, `$ShadowTower_LeaderboardHeading2`, `$ShadowTower_PortalHeading`, `$Open`. `setDifficultyHelper` also translates `$ShadowTower_KeyNormal` / `_KeyHard` / `_KeyUltra` into `keyType`, but `keyType` only fills a `{1}` that `$ShadowTower_LevelRequirement` does not contain, so it never shows (and `$ShadowTower_KeyUltra` reads "Moon Key", the same as `_KeyHard`).
- `showToolTip` / `hideToolTip` (`SLOT.POINTER_ENTER`, `TOOLTIP.SHOW`, `TOOLTIP.HIDE`) are never attached to any listener, so those calls are never made from this class.

---

## Other game-specific classes

- `ShadowTowerLeaderboard` (extends `UIComponent`, embeds `symbol132`) — leaderboard panel. Has `normal`/`hard`/`ultra` `SlotBasic` difficulty filter buttons (click dispatches `DataEvent(SET_DIFFICULTY)`), `rowView:RowView` for scrollable `Listing` rows, rank/name/time header labels. `addLeaderboardEntry` creates `Listing` instances and handles ties and friend colouring.
- `ShadowTowerCreatePortal` (extends `UIComponent`) — portal entry panel. Has `normal`/`hard`/`ultra` difficulty toggle MovieClips (with `selectionGlow` child), `bossNameText`, `bossArt:ArtClip`, `btn_enter:LabelButton`, `txt_timeRemaining`, `artClip` for material cost icon. Difficulty clicks dispatch `DataEvent(SET_DIFFICULTY)`; `btn_enter` click calls `ExternalInterface.call("OnAccept")`.
- `createPortal` (extends `ShadowTowerCreatePortal`, embeds `symbol161`) — concrete SWF-symbol binding for `ShadowTowerCreatePortal`; sets `btn_enter` component defaults.
- `Listing` (extends `MovieClip`, embeds `symbol11`) — leaderboard row. Fields: `friendIcon`, `txt_userName`, `txt_time`, `txt_rank`. Two frame-label states (frames 2 and 3, both `stop()`).
- `node` (extends `BaseButton`, embeds `symbol61`) — floor selection button. 4-state (frames 1, 10, 20, 30). Expected children: `selectedIcon`, `completedIcon`.
- `btnGreenWide` (extends `LabelButton`, embeds `symbol31`) — wide green action button. 4-state.
- `Image` (extends `ArtClip`, embeds `symbol143`) — generic art display slot.
- `Equipped` (extends `MovieClip`, embeds `symbol34`) — equipped/selected badge stub.
- `dummy` — 100×100 `BitmapData` placeholder.
- `ShadowTower_fla` timeline clips (5): `btn_toggle_7` (symbol154, difficulty toggle with `selectionGlow`, 2 frames), `tab_39` (symbol197, tab button with `textField`, 2 frames), `headerFrame_40` (symbol202), `keyHeaderFrame_4` (symbol182), `keysPanel_2` (symbol189, 3×(amount+text) TextFields for shadow/lunar/eclipse keys, 2 frames).
- Skin stubs (18): scroll arrow/thumb/track skins, `focusRectSkin`, `SlotBackground`, `SlotBackgroundLocked`, `SlotFrameHigh`, `SlotFrameNormal`, `SlotFrameMedium` — all trivial MovieClip embeds.

---

## Notable logic

- **Difficulty dirty flag:** changing difficulty while on the Leaderboard tab sets `displayDirty=true` and sends only `GetLeaderboard(floor, difficulty)`. The game engine's `OnDifficultySelected` is only called when the user switches back to the Portal tab, preventing spurious server requests during leaderboard browsing. Any difficulty change also runs `resetFloors()`, and the engine re-sends the floors (`setFloor`) only in answer to `OnDifficultySelected`, so until the switch back the floor buttons carry no data.
- **Player row:** the viewer's own result arrives separately through `addPlayerLeaderboardEntry(name, time, rank)` into fixed fields under the list; on a board the player is not on, the engine sends no rank for it.
- **Leaderboard tie-rank handling:** `addLeaderboardEntry` tracks the previous entry's time and rank; if the new time matches, the rank display is blanked out to show a visual tie.
- **Console D-pad navigation:** `moveHighlight` uses two separate interaction domains. `column < 1` navigates the floor `node` buttons (delta ±1 index). `column == 1` cycles difficulty toggle buttons, managing `selectionGlow` visibility.
- **Key panel visibility:** individual key type rows in `keysPanel` are shown or hidden based on whether the game passes a non-zero count for shadow/lunar/eclipse keys, allowing the panel to collapse unused rows.
- **Test/preview mode:** when not in Iggy, `configUI` seeds 4 floors with dummy boss names/portraits and 15 leaderboard entries to allow offline layout inspection.
