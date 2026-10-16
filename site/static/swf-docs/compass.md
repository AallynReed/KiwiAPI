# compass.swf
> The in-game compass HUD bar displayed at the top of the screen while exploring. It renders icons for nearby dungeons, lairs, quests, players, and delve rooms at horizontal positions representing their real-world compass bearing, and shows a countdown widget for Mega Dungeon boss encounters.

**Document/main class:** `Compass` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 32 (2 logic classes + 2 timeline logic classes + 28 icon/asset wrappers)

---

## Main class: `Compass`

Manages the compass bar layout and all icon pools. On construction it translates the 8 cardinal/intercardinal direction strings via `IggyFunctions.translate`, applies a mask to the directional strip, and calls `useDiscoveryMode(false, 0)`. In `configUI()` it registers 12 `ExternalInterface` callbacks for the Iggy/Scaleform JS bridge. All icon types are managed as object pools (arrays of recycled, visibility-toggled MovieClips).

### Public methods

- `useDiscoveryMode(isDiscovery:Boolean, maxDist:Number) : void` — toggles between Adventure world (normal) and Geode/Discovery world. Swaps anchor MovieClips (`topLeftAnchor`/`bottomRightAnchor` vs. `topLeftAnchor2`/`bottomRightAnchor2`), shows/hides `adventureBG`/`discoveryBG`, and uses `GeodeDirection` vs. `AdventureDirection` for heading labels. Sets `maxPlayerDistance` for player dot alpha scaling. Also removes every heading label and every delve-track icon (`directions`, `delveRooms`, `delveBoss`) from the display.
- `updateCornerstone(fraction:Number) : void` — moves or creates the `Cornerstone` diamond icon at the given 0–1 horizontal fraction; a fraction of 0 or below, or 1 or above, removes it.
- `clearDungeons() : void` — hides the pooled markers before each refresh: 22 pools, covering dungeons, lairs, flags, players and quests. `delveRooms` and `delveBoss` are not among them; only `setDelveRooms` and `useDiscoveryMode` clear the delve track.
- `addNewIcon(fraction:Number, type:int, level:uint) : MovieClip` — core factory. Selects the correct pool array by `type` constant, recycles a hidden instance or instantiates a new one via `getDefinitionByName`, and positions it via `GetIconHorizontalFromLocation`. The `level` param is the marker's height relative to the player (0 same level, 1 above, 2 below) and selects the directional variant class; conquest and crowd lairs ignore it and use one pool each. These three values are all the engine sends for a marker: no distance crosses the bridge. `setDelveRooms`, `addPlayer` and `addQuest` also call it internally with kinds 98/99, 100 and 101–103.
- `addFlag(flagType:int, fraction:Number) : void` — places a `Flag` icon; `gotoAndStop(flagType+1)` selects the flag colour frame.
- `setDelveRooms(roomCount:Number, clearedMask:Number) : void` — lays out `roomCount` `DelveRoom` icons, room `i` at fraction `i / (roomCount - 1)` and stretched to `(stripWidth / roomCount - 1) / 30` with `scaleX`; the last room also gets a `DelveBoss` icon at fraction 1. Bit `i` of `clearedMask` puts room `i` on its cleared frame (2); it is set when a room is cleared, not when it is reached. A one-room delve divides by zero.
- `addQuest(fraction:Number, questType:Number) : void` — adds `MainQuest`, `GeodeQuest`, or `AdventureQuest` icon (`questType` 1–3 plus 100 maps to `MAIN_QUEST`=101, `GEODE_QUEST`=102, `ADVENTURE_QUEST`=103; 0 or less adds nothing).
- `addPlayer(fraction:Number, squaredDistance:Number, isFriend:Boolean, isAlive:Boolean, isSelf:Boolean) : void` — adds a `NearbyPlayer` dot. Frame is `"friend"` or `"player"` when `isAlive`, otherwise `"dead"`; the local player (`isSelf`) is included and drawn at 1.25× scale. The second argument is already the squared horizontal distance in blocks, and alpha is `(maxPlayerDistance² − squaredDistance) / maxPlayerDistance²`. The engine clears and re-sends every player each frame.
- `setHeading(leftAngle:Number, rightAngle:Number) : void` — the two ends of the camera's visible arc in radians, each in `[0, 2π)`: the first is the angle at the strip's left anchor (fraction 0), the second at the right anchor (fraction 1). When the first is smaller (the arc crosses south) it gets `2π` added. Computes which of the 8 cardinal labels fall within the arc (padded by 15 %) and places `AdventureDirection`/`GeodeDirection` instances at their fractional positions. Labels are pre-translated strings from `directionText`. Sent every frame.
- `setLocation(x:Number, y:Number, z:Number, visible:Boolean) : void` — shows/hides `locationText` with integer-truncated coordinate values and toggles `locationBackground`. It is the only position the screen receives; before the player has a position the game reports `0, 0, 0`.

### Key fields

- `directionText : Array` — 8 translated cardinal/intercardinal heading strings (`$CompassHeading_S` … `$CompassHeading_SE`, English "S" … "SE").
- `topLeftAnchor / bottomRightAnchor` — Adventure world pixel bounds for icon positioning.
- `topLeftAnchor2 / bottomRightAnchor2` — Discovery/Geode world pixel bounds.
- `directionalAnchor / directionalMask` — strip container and its mask clip.
- `iconContainer : MovieClip` — parent for the pooled dungeon, lair, quest and delve-room icons. `NearbyPlayer`, `DelveBoss`, `Flag` and `Cornerstone` are added to the root instead, so they draw above it.
- `megaDungeonProgress : MegaDungeonProgress` — embedded boss-wave HUD widget, placed at (273, 108), below the 544×52 stage; `locationText` sits at y −5.5, above it. The game does not clip the screen to its stage, so both still draw.
- `discoveryMode : Boolean` — tracks current world mode.
- `maxPlayerDistance : Number` — player fade distance from `useDiscoveryMode`; squared in `addPlayer` to compare against the squared distance the engine sends.
- Per-type pool arrays: `dungeons`, `dungeonsUp`, `dungeonsDown`, `lairs`, `lairsUp`, `lairsDown`, `recipeLairs`, `conquestLairs`, `crowdLairs`, `outposts`, `megaDungeons`, `directions`, `flags`, `players`, `delveBoss`, `delveRooms`, `mainQuests`, `geodeQuests`, `adventureQuests`.

### Runtime dependencies & integration

- `ExternalInterface.addCallback` registrations: `useDiscoveryMode`, `updateCornerstone`, `clearDungeons`, `setHeading`, `setLocation`, `addFlag`, `addPlayer`, `addQuest`, `setDelveRooms`, `addNewIcon`, `megaDungeonProgress.show`, `megaDungeonProgress.hide`. The game client's compass callback table also names a `ShowLocation`, which this SWF never registers.
- Outbound: none. `Compass` and `MegaDungeonProgress` make no `ExternalInterface.call` of their own; only the shared framework does.
- `IggyFunctions.translate` — called at construction for all 8 direction strings.
- `getDefinitionByName` — used in `addNewIcon` to instantiate icon classes by string name when the pool is empty.
- Dungeon type constants: `DUNGEON_LARGE=1`, `DUNGEON_LAIR=2`, `DUNGEON_RECIPE_LAIR=3`, `DUNGEON_CONQUEST_LAIR=4`, `DUNGEON_CROWD_LAIR=5`, `DUNGEON_OUTPOST_LAIR=6`, `DUNGEON_MEGA=7`, `DELVE_ROOM=98`, `DELVE_BOSS=99`, `NEARBY_PLAYER=100`, `MAIN_QUEST=101`, `GEODE_QUEST=102`, `ADVENTURE_QUEST=103`.
- translate keys used: `$CompassHeading_S`, `$CompassHeading_SW`, `$CompassHeading_W`, `$CompassHeading_NW`, `$CompassHeading_N`, `$CompassHeading_NE`, `$CompassHeading_E`, `$CompassHeading_SE`.

---

## Other game-specific classes

### `GeodeDirection` (embeds `symbol10`)
Dual-label direction tick for Geode worlds. Has `whiteText`, `blueText`, `whiteTick`, `blueTick` children. The `distance` setter switches between white (near-centre, `< 40px`) and blue (outer) rendering, fading blue alpha as `(216 - distance) / 216`.

### `MegaDungeonProgress` (embeds `symbol146`)
Self-contained animated HUD strip for Mega Dungeon boss waves. Shown/hidden via `show(questState, remainingSeconds, waveNum, ...skullStates)` / `hide()`; this is everything the engine sends about the encounter (no dungeon name or tier). `remainingSeconds` arrives as whole seconds, re-sent several times a second (`-1` keeps the previous value; a value above 0 starts `animateIn`). `waveNum` above 1 shows `$Quest_FiveStarWave` ("Wave: {0}"). Each skull state is an index into `["inactive","active","complete","failed","hidden"]`; skulls without a state get `"hidden"`. `questState` 2 is complete and 3 is failed, either of which plays `animateOut`. Drives a `CountdownTimer` child (`textContainer`) per-frame, animates up to 5 skull icons (`skull0`–`skull4` inside `skullContainer`), and plays timeline labels `active` → `animateOut`. On completion or failure shows `bannerComplete`+shine effects or `bannerFailed`. The symbol is 167 frames: `start` (1, empty), `animateIn` (11), `active` (31), `animateOut` (41–167).

### `timerMC` (embeds `symbol103`)
Thin `CountdownTimer` subclass used as the timer display inside `MegaDungeonProgress`.

### `compass_fla/_5StarDungeonSkullMC_9` (symbol111)
4-label skull MovieClip with stops at frames 1, 16, 30, 46 (`inactive`, `active`, `complete`, `hidden`). `active` is the skull with green eyes, `complete` the skull under a red cross. There is no `failed` label, although `MegaDungeonProgress.states` names one at index 3, so a skull sent state 3 would hit a missing label. Child of `MegaDungeonProgress.skullContainer`.

### `compass_fla/_5StarDungeonSkullEyesGlowMC_10` (symbol109)
2-stop eye-glow effect clip (frames 8 and 20); nested inside the skull MC.

### Icon/asset wrapper classes (28 classes)
All embed from `/_assets/assets.swf` and have no logic beyond `super()` or simple `stop()` frame scripts. Grouped by type:
- **Dungeon icons (flat/up/down):** `LargeDungeon` (symbol54), `LargeDungeon_Up` (symbol48), `LargeDungeon_Down` (symbol51), `Dungeon` (symbol66), `Dungeon_Up` (symbol60), `Dungeon_Down` (symbol63), `MegaDungeon` (symbol78), `MegaDungeon_Up` (symbol72), `MegaDungeon_Down` (symbol75), `Outpost` (symbol42), `Outpost_Up` (symbol36), `Outpost_Down` (symbol39), `RecipeLair` (symbol33), `RecipeLair_Up` (symbol27), `RecipeLair_Down` (symbol30), `ConquestDungeon` (symbol96), `CrowdLair` (symbol91)
- **Delve icons:** `DelveRoom` (symbol83, 2-frame stop: a 30 px wide rectangle in the same blue at two opacities, frame 1 not cleared and frame 2 cleared), `DelveBoss` (symbol88)
- **Quest icons:** `MainQuest` (symbol45), `AdventureQuest` (symbol69), `GeodeQuest` (symbol57)
- **Other:** `Cornerstone` (symbol94), `Flag` (symbol24, multi-frame by flag type), `NearbyPlayer` (symbol17, frames: friend/player/dead), `AdventureDirection` (symbol98, has `textfield:TextField`), `locationBackground` (symbol149)

---

## Notable logic

- **Object pool pattern:** every icon type has a backing array. `addNewIcon` walks the pool to find a hidden instance before creating a new one, preventing GC churn during frequent compass refreshes.
- **Vertical level encoding:** `addNewIcon` `param3` value 0 = same level, 1 = above the player, 2 = below; this selects between the three directional variants (e.g. `LargeDungeon` / `LargeDungeon_Up` / `LargeDungeon_Down`). Because it is relative height, the same marker changes variant as the player climbs or descends.
- **Strip fractions:** every marker fraction maps 0 to the left anchor and 1 to the right anchor. Outside the visible arc the engine clamps the fraction to exactly 0.0 or 1.0, so an off-screen marker carries only "off the left" or "off the right"; `updateCornerstone` drops its icon at those values.
- **Heading angles:** `directionText` is walked from `2π` downwards in steps of `π/4`, so S is 0 (`2π`), E `π/2`, N `π`, W `3π/2`. In world terms the engine's angle is `atan2(x, z)` with +X east and +Z south (north is −Z).
- **Delve room bitmask:** `setDelveRooms` param2 is a bitmask of cleared rooms; individual bits control the `DelveRoom` cleared state per room index, and the boss icon takes the last room's bit. In a delve, `addPlayer`'s fraction is the player's position along this room track rather than a bearing.
- **Discovery mode coordinate system:** `maxPlayerDistance` is supplied alongside the discovery mode flag; player dot alpha = `1 - squaredDistance / maxPlayerDistance²`, a linear falloff in squared distance (the engine sends the distance already squared).
- **Test/preview mode:** when `IggyFunctions.inIggy` is false, `configUI` calls `addNewIcon(0.25, DUNGEON_LARGE, 0)` and `megaDungeonProgress.show(1, 59, 0, 1, 2)` to populate visible test content.
