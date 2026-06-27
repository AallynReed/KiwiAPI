# compass.swf
> The in-game compass HUD bar displayed at the top of the screen while exploring. It renders icons for nearby dungeons, lairs, quests, players, and delve rooms at horizontal positions representing their real-world compass bearing, and shows a countdown widget for Mega Dungeon boss encounters.

**Document/main class:** `Compass` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 32 (2 logic classes + 2 timeline logic classes + 28 icon/asset wrappers)

---

## Main class: `Compass`

Manages the compass bar layout and all icon pools. On construction it translates the 8 cardinal/intercardinal direction strings via `IggyFunctions.translate`, applies a mask to the directional strip, and calls `useDiscoveryMode(false, 0)`. In `configUI()` it registers ~12 `ExternalInterface` callbacks for the Iggy/Scaleform JS bridge. All icon types are managed as object pools (arrays of recycled, visibility-toggled MovieClips).

### Public methods

- `useDiscoveryMode(isDiscovery:Boolean, maxDist:Number) : void` — toggles between Adventure world (normal) and Geode/Discovery world. Swaps anchor MovieClips (`topLeftAnchor`/`bottomRightAnchor` vs. `topLeftAnchor2`/`bottomRightAnchor2`), shows/hides `adventureBG`/`discoveryBG`, and uses `GeodeDirection` vs. `AdventureDirection` for heading labels. Sets `maxPlayerDistance` for player dot alpha scaling.
- `updateCornerstone(fraction:Number) : void` — moves or creates the `Cornerstone` diamond icon at the given 0–1 horizontal fraction.
- `clearDungeons() : void` — hides all pooled icons (called before each compass refresh cycle).
- `addNewIcon(fraction:Number, type:int, level:uint) : MovieClip` — core factory. Selects the correct pool array by `type` constant, recycles a hidden instance or instantiates a new one via `getDefinitionByName`, and positions it via `GetIconHorizontalFromLocation`. The `level` param (0=flat, 1=up, 2=down) selects the appropriate directional variant class.
- `addFlag(position:Number, flagType:int) : void` — places a `Flag` icon; `gotoAndStop(flagType+1)` selects the flag colour frame.
- `setDelveRooms(position:Number, clearedMask:Number) : void` — lays out all delve room icons at evenly-spaced fractions derived from `position`; the last room gets a `DelveBoss` icon. `clearedMask` is a bitmask of which rooms are cleared.
- `addQuest(position:Number, questType:Number) : void` — adds `MainQuest`, `GeodeQuest`, or `AdventureQuest` icon (type offset maps to `MAIN_QUEST`=101, `GEODE_QUEST`=102, `ADVENTURE_QUEST`=103).
- `addPlayer(fraction:Number, distance:Number, isFriend:Boolean, isDead:Boolean, isPlayer:Boolean) : void` — adds a `NearbyPlayer` dot. Selects friend/player/dead frame, scales alpha by `(distance²/maxPlayerDistance)`.
- `setHeading(rightEdge:Number, leftEdge:Number) : void` — computes which of the 8 cardinal labels fall within the visible arc `[leftEdge..rightEdge]` and places `AdventureDirection`/`GeodeDirection` instances at their fractional positions. Labels are pre-translated strings from `directionText`.
- `setLocation(x:Number, y:Number, z:Number, visible:Boolean) : void` — shows/hides `locationText` with coordinate values and toggles `locationBackground`.

### Key fields

- `directionText : Array` — 8 translated cardinal/intercardinal heading strings (`$CompassHeading_S` … `$CompassHeading_SE`).
- `topLeftAnchor / bottomRightAnchor` — Adventure world pixel bounds for icon positioning.
- `topLeftAnchor2 / bottomRightAnchor2` — Discovery/Geode world pixel bounds.
- `directionalAnchor / directionalMask` — strip container and its mask clip.
- `iconContainer : MovieClip` — parent for all pooled icon instances.
- `megaDungeonProgress : MegaDungeonProgress` — embedded boss-wave HUD widget.
- `discoveryMode : Boolean` — tracks current world mode.
- `maxPlayerDistance : Number` — denominator for player dot alpha falloff.
- Per-type pool arrays: `dungeons`, `dungeonsUp`, `dungeonsDown`, `lairs`, `lairsUp`, `lairsDown`, `recipeLairs`, `conquestLairs`, `crowdLairs`, `outposts`, `megaDungeons`, `directions`, `flags`, `players`, `delveBoss`, `delveRooms`, `mainQuests`, `geodeQuests`, `adventureQuests`.

### Runtime dependencies & integration

- `ExternalInterface.addCallback` registrations: `useDiscoveryMode`, `updateCornerstone`, `clearDungeons`, `setHeading`, `setLocation`, `addFlag`, `addPlayer`, `addQuest`, `setDelveRooms`, `addNewIcon`, `megaDungeonProgress.show`, `megaDungeonProgress.hide`.
- `IggyFunctions.translate` — called at construction for all 8 direction strings.
- `getDefinitionByName` — used in `addNewIcon` to instantiate icon classes by string name when the pool is empty.
- Dungeon type constants: `DUNGEON_LARGE=1`, `DUNGEON_LAIR=2`, `DUNGEON_RECIPE_LAIR=3`, `DUNGEON_CONQUEST_LAIR=4`, `DUNGEON_CROWD_LAIR=5`, `DUNGEON_OUTPOST_LAIR=6`, `DUNGEON_MEGA=7`, `DELVE_ROOM=98`, `DELVE_BOSS=99`, `NEARBY_PLAYER=100`, `MAIN_QUEST=101`, `GEODE_QUEST=102`, `ADVENTURE_QUEST=103`.
- translate keys used: `$CompassHeading_S`, `$CompassHeading_SW`, `$CompassHeading_W`, `$CompassHeading_NW`, `$CompassHeading_N`, `$CompassHeading_NE`, `$CompassHeading_E`, `$CompassHeading_SE`.

---

## Other game-specific classes

### `GeodeDirection` (embeds `symbol10`)
Dual-label direction tick for Geode worlds. Has `whiteText`, `blueText`, `whiteTick`, `blueTick` children. The `distance` setter switches between white (near-centre, `< 40px`) and blue (outer) rendering, fading blue alpha as `(216 - distance) / 216`.

### `MegaDungeonProgress` (embeds `symbol146`)
Self-contained animated HUD strip for Mega Dungeon boss waves. Shown/hidden via `show(questState, remainingSeconds, waveNum, ...skullStates)` / `hide()`. Drives a `CountdownTimer` child (`textContainer`) per-frame, animates up to 5 skull icons (`skull0`–`skull4` inside `skullContainer`) to labels `inactive/active/complete/failed/hidden`, and plays timeline labels `active` → `animateOut`. On completion or failure shows `bannerComplete`+shine effects or `bannerFailed`.

### `timerMC` (embeds `symbol103`)
Thin `CountdownTimer` subclass used as the timer display inside `MegaDungeonProgress`.

### `compass_fla/_5StarDungeonSkullMC_9` (symbol111)
4-label skull MovieClip with stops at frames 1, 16, 30, 46 (`inactive`, `active`, `complete`, `failed`). Child of `MegaDungeonProgress.skullContainer`.

### `compass_fla/_5StarDungeonSkullEyesGlowMC_10` (symbol109)
2-stop eye-glow effect clip (frames 8 and 20); nested inside the skull MC.

### Icon/asset wrapper classes (28 classes)
All embed from `/_assets/assets.swf` and have no logic beyond `super()` or simple `stop()` frame scripts. Grouped by type:
- **Dungeon icons (flat/up/down):** `LargeDungeon` (symbol54), `LargeDungeon_Up` (symbol48), `LargeDungeon_Down` (symbol51), `Dungeon` (symbol66), `Dungeon_Up` (symbol60), `Dungeon_Down` (symbol63), `MegaDungeon` (symbol78), `MegaDungeon_Up` (symbol72), `MegaDungeon_Down` (symbol75), `Outpost` (symbol42), `Outpost_Up` (symbol36), `Outpost_Down` (symbol39), `RecipeLair` (symbol33), `RecipeLair_Up` (symbol27), `RecipeLair_Down` (symbol30), `ConquestDungeon` (symbol96), `CrowdLair` (symbol91)
- **Delve icons:** `DelveRoom` (symbol83, 2-frame stop), `DelveBoss` (symbol88)
- **Quest icons:** `MainQuest` (symbol45), `AdventureQuest` (symbol69), `GeodeQuest` (symbol57)
- **Other:** `Cornerstone` (symbol94), `Flag` (symbol24, multi-frame by flag type), `NearbyPlayer` (symbol17, frames: friend/player/dead), `AdventureDirection` (symbol98, has `textfield:TextField`), `locationBackground` (symbol149)

---

## Notable logic

- **Object pool pattern:** every icon type has a backing array. `addNewIcon` walks the pool to find a hidden instance before creating a new one, preventing GC churn during frequent compass refreshes.
- **Vertical level encoding:** `addNewIcon` `param3` value 0 = flat, 1 = going up, 2 = going down; this selects between the three directional variants (e.g. `LargeDungeon` / `LargeDungeon_Up` / `LargeDungeon_Down`).
- **Delve room bitmask:** `setDelveRooms` param2 is a bitmask of cleared rooms; individual bits control the `DelveRoom` cleared state per room index.
- **Discovery mode coordinate system:** `maxPlayerDistance` is supplied alongside the discovery mode flag; player dot alpha = `1 - (distance² / maxPlayerDistance)` giving a quadratic falloff.
- **Test/preview mode:** when `IggyFunctions.inIggy` is false, `configUI` calls `addNewIcon(0.25, DUNGEON_LARGE, 0)` and `megaDungeonProgress.show(1, 59, 0, 1, 2)` to populate visible test content.
