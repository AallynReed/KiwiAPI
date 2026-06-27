# hotbar.swf

> The persistent bottom-of-screen HUD shown during gameplay in Trove. It renders the player's quick-slot items, health and energy globes, fuel/radiance gauge, and Bard-specific UI elements (song meter, melody level pips, QTE prompts). It switches between four distinct layout modes — adventure, build, pvp, and discovery — each with its own slot arrangement and globe configuration.

**Document/main class:** `Hotbar` (extends `UIComponent`)
**SWF-specific classes:** 24 (excluding framework)

---

## Main class: `Hotbar`

`Hotbar` is the root UIComponent that owns all four mode sub-MovieClips and delegates to them based on the active game mode. It is responsible for:

- Initialising all four mode layouts (`adventure`, `build`, `pvp`, `discovery`) at construction time, hiding all but the default (`adventure`).
- Configuring `Globe` instances per mode: standard square-mask fill for adventure/build/pvp; arc-mask fill with specific rotation/arcSize constants for discovery.
- Registering all `ExternalInterface` callbacks that the C++ game engine calls to drive the UI.
- Delegating drag-drop hit-testing to `SlotDragDropHelper`.

**Constructor flow:**

1. Sets `hotbarModes` array (`["adventure","build","pvp","discovery"]`).
2. Calls `addFrameScript(0, frame1)` which stops the timeline.
3. Iterates all modes, hides selectors, configures globe arc/mask parameters for discovery vs. normal modes.
4. Hides `pvp.spectateControls`; routes it to either `"Console"` or `"PC"` frame.
5. Hides `fuel`.
6. Registers `SlotDragDropHelper.registerDropCallback(onDrop)`.
7. If running in Iggy (`IggyFunctions.inIggy == true`): registers all `ExternalInterface` callbacks.
8. If NOT in Iggy (local test mode): switches to discovery mode, fills globes to 100%, queues test messages, and starts `ENTER_FRAME` for animated health drain simulation.

### Public methods

- `setSlotState(param1:int, param2:int) : void` — Configures a single slot at index `param1` within the current mode. Sets slot size (51 px square / 60 px circle), advances `equippedBorder` and `slotFrame` to `"square"` or `"circle"` labels, toggles `BG` and `goldFrame` visibility. `goldFrame` is hidden in discovery mode regardless of shape.

### Private methods (ExternalInterface callbacks)

- `setMode(param1:String) : *` — Shows the matching mode MovieClip, hides the rest. Recalculates how many slots are square vs. circle per mode:
  - adventure / pvp: slots 0–5 square, slots 6–7 circle.
  - discovery: slots 0–4 square, slots 5–7 circle.
  - build: all 8 circle (no square slots); globe frame goes to stop 2.
  Sets `globeFrame` stop for non-discovery modes.
- `toggleEquipQuickslot() : void` — Toggles visibility of the current mode's `selector` MovieClip.
- `moveSelector(param1:int) : void` — Repositions `selector.x` to align with slot `param1`.
- `confirmEquip(param1:int, param2:uint, param3:Number) : *` — Verifies a slot exists then calls `ExternalInterface.call("HOTBAR.DROP_ONTO_SLOT", ...)`.
- `adjustSlotForHotkeySize(param1:int, param2:Boolean) : void` — Nudges `hotkeyTextField` (and optionally the slot itself) by `_xTranslate` / `_yTranslate` offset constants (7/4 px) to compensate for a wide hotkey label. Tracks position state with `translatedHotkeyPosition` and `translatedSlotPosition` flags on the slot.
- `setBardSongState(param1:Boolean) : *` — Shows/hides `bardSongFilling`, `bardSongOutline`, and `QTE`; resets song and melody state when hiding.
- `setBardSongValue(param1:Number) : *` — Sets `bardSongFilling.scaleX` to `clamp(|current / max|, 0, 1)`.
- `setBardSongMax(param1:Number) : *` — Updates `bardSongMax`.
- `setBardMelodyLevel(param1:int) : *` — Shows Melody1–Melody11 sub-clips on `bardSongOutline` using fall-through switch (cumulative visibility up to the given level).
- `setBardQTE(param1:int) : *` — Shows one of four QTE sub-clips (`BasicAttack`, `Dodge`, `NimbleDance`, `SingingCrystal`) and calls `QTE.gotoAndPlay(0)` when a new positive QTE index is set.
- `setFuelType(param1:int) : *` — Delegates to `fuel.fuelType`.
- `setFuel(param1:int) : *` — Delegates to `fuel.amount`.
- `onDrop(x, y, ..., typeId, amount) : *` — Hit-tests all slots in current mode; on match calls `ExternalInterface.call("HOTBAR.DROP_ONTO_SLOT", slotIndex, typeId, amount)`.
- `localTestEnterFrame(param1:Event) : void` — Local-only test animation: slowly drains then refills the discovery health globe, with accelerated drain spikes near 80% and 60%.

### Key fields

- `currentMode : String` — Tracks which of the four modes is active (default: `"adventure"`).
- `hotbarModes : Array` — Ordered list `["adventure","build","pvp","discovery"]` used for iteration.
- `adventure, build, pvp, discovery : MovieClip` — The four mode-layout sub-clips (instances of their respective `Hotbar_fla/*Mode_*` classes).
- `fuel : Fuel` — The fuel/radiance gauge component (initially hidden).
- `bardSongOutline : MovieClip` — Outline container holding Melody1–Melody11 sub-clips.
- `bardSongFilling : MovieClip` — Horizontally scaled fill bar for bard song charge; `scaleX` driven by `setBardSongValue`.
- `bardSongMax : Number` — Maximum song charge value (default 50).
- `bardSongCurrent : Number` — Current song charge value.
- `bardMelodyLevelCurrent : int` — Current melody level (0–11).
- `QTE : MovieClip` — Container for Quick-Time Event prompts (maps to `Hotbar_fla.QTE_61`); holds `BasicAttack`, `Dodge`, `NimbleDance`, `SingingCrystal` sub-clips.
- `currentQTE : int` — Last QTE index set; deduplicates redundant updates (default -1 = none).
- `messageQueue : MessageQueue` — The in-game notification queue component.
- `_xTranslate / _yTranslate : Number` — Pixel offsets (7, 4) for hotkey label repositioning.
- `degen : Boolean` — Direction flag for local test health-drain animation.

### Frame scripts / timeline

- `frame1()` — Calls `stop()`.

### Runtime dependencies & integration

**Iggy / ExternalInterface callbacks registered (C++ → Flash):**

| Callback name | Handler |
|---|---|
| `setMode` | `setMode(String)` |
| `addMessage` | `messageQueue.addMessage` |
| `TOGGLE_EQUIP_QUICKSLOT` | `toggleEquipQuickslot()` |
| `MOVE_SELECTOR` | `moveSelector(int)` |
| `CONFIRM_EQUIP` | `confirmEquip(int, uint, Number)` |
| `ADJUST_SLOT_FOR_HOTKEY_SIZE` | `adjustSlotForHotkeySize(int, Boolean)` |
| `SET_BARD_SONG_STATE` | `setBardSongState(Boolean)` |
| `SET_BARD_SONG_VALUE` | `setBardSongValue(Number)` |
| `SET_BARD_SONG_MAX` | `setBardSongMax(Number)` |
| `SET_BARD_MELODY_LEVEL` | `setBardMelodyLevel(int)` |
| `SET_BARD_QTE` | `setBardQTE(int)` |
| `SET_FUEL_TYPE` | `setFuelType(int)` |
| `SET_FUEL` | `setFuel(int)` |

**ExternalInterface calls (Flash → C++):**

| Call | Triggered by |
|---|---|
| `HOTBAR.DROP_ONTO_SLOT(slotIndex, typeId, amount)` | Drag-drop hit (`onDrop`) or `confirmEquip` |

**Events listened:**
- `Event.ENTER_FRAME` on self — local test mode only.
- `Event.ENTER_FRAME` on each `Globe` instance — Console layout, deferred hide of `amountText`.

**Framework hooks:**
- `SlotDragDropHelper.registerDropCallback(onDrop)` — receives global drag-drop events.
- `IggyFunctions.inIggy` — runtime flag used to branch between Iggy and local test paths.
- `IsConsole()` — global function used in `Hotbar` (for spectateControls frame), `Globe` (ENTER_FRAME branch), and `slot`/mode classes (Console timeline branch).

---

## Other game-specific classes

### Top-level game classes

- `Globe` (extends `UIComponent`, embed symbol77) — Health or energy fill widget. Supports two display modes: square-mask (vertical fill using `square_mask` height), and arc-mask (using `ArcMask` for the circular discovery UI). Features IggyTween-driven pulse animation below a configurable `lowPercentThreshold` (using `ColorMatrixFilter` brightness overlay), and an optional "chunk" drain animation (darkens content during deferred arc retraction over 5 seconds). Draws a glowing highlight line at the arc's leading edge using `Graphics` API and `GlowFilter`. Uses `KiwiTextUtil.addDigitDelimiters` for numeric amount display. Two `IggyTween` instances (`pulseAnimation`, `chunkAnimation`) are managed internally.

- `Fuel` (extends `UIComponent`, embed symbol143) — Container for the fuel gauge. Currently only handles `fuelType == 15` (Radiance type), delegating to a nested `radiance : MovieClip` sub-component. Hidden when fuel type is 0. Exposes `fuelType` and `amount` setters.

- `Radiance` (extends `UIComponent`, embed symbol142) — Displays the Radiance resource bar. Contains a `bar` MovieClip (frame-stepped by integer amount) and seven directional arrow MovieClips (`arrow1`–`arrow7`). `UpdateArrows` animates pairs of arrows (arrow2/3 at threshold 10, arrow4/5 at 17, arrow6/7 at 24) by playing/stopping based on whether the value crosses those thresholds.

- `slot` (extends `_kiwi.Controls.Slot`, embed symbol116) — Standard quickslot widget. Frame 1 stops; frame 11 (Console branch) stops and tells `quantityBadge` to play from `"Console"`.

- `slotDiscovery` (extends `_kiwi.Controls.Slot`, embed symbol122) — Identical in structure to `slot`; used for the circular discovery slots (0–4 in discovery mode).

- `messageQueue` (extends `_kiwi.Controls.MessageQueue`, embed symbol46) — Asset-embed wrapper for the notification queue; no additional logic.

- `meterHealthMC` (extends `Globe`, embed symbol32) — Asset-embed wrapper for the health globe symbol; no additional logic.

- `meterEnergy` (extends `Globe`, embed symbol37) — Asset-embed wrapper for the energy globe symbol; no additional logic.

- `FuelGauge` (extends `MovieClip`, embed symbol39) — Bare asset-embed wrapper; no logic.

- `HomeButton` (extends `UIComponent`, embed symbol44) — Bare asset-embed wrapper; no logic.

- `hotbar_bg` (extends `UIComponent`, embed symbol49) — Bare asset-embed wrapper for the background graphic; no logic.

### Hotbar_fla timeline symbol classes

- `adventureMode_45` (embed symbol170) — Adventure mode layout MovieClip. Holds `slot_0`–`slot_7` (`slot`), `health`, `energy`, `selector`. Console branch (frames 11 and 20) triggers `gotoAndPlay("Console")` on all child slots and globes.

- `buildMode_43` (embed symbol166) — Build mode layout. Same slot/globe structure as adventure. Console branch (frame 12) triggers `gotoAndPlay("Console")` on children.

- `pvpMode_33` (embed symbol165) — PVP mode layout. Holds `slot_0`–`slot_5`, `health`, `energy`, `selector`, and `spectateControls` (a MovieClip routed to `"PC"` or `"Console"` frame by `Hotbar`). Two timeline frames (PC / Console).

- `discoveryMode_9` (embed symbol147) — Discovery mode layout. Slots 0–4 are `slotDiscovery`; slots 5–7 are standard `slot`. Contains `health`, `energy`, `selector`. All slots initialised with `clickFeedback = false`.

- `globeWithMask_36` (embed symbol164) — Wrapper MovieClip holding a `Globe` child reference. Console frame (frame 11) triggers `globe.gotoAndPlay("Console")`.

- `QTE_61` (embed symbol223) — QTE prompt container. Holds `BasicAttack`, `Dodge`, `NimbleDance`, `SingingCrystal` sub-clips. Plays a 48-frame animation when a QTE starts; frame 48 stops playback.

- `slotFrame_11` (embed symbol82) — Visual frame around a slot; three-state timeline: frames 1/11/21 each stop (likely square, circle, and a third variant).

- `qualityPips_12` (embed symbol89) — Item quality pip display; stops on frame 1.

- `equipped_14` (embed symbol97) — Two-frame clip (frames 1 and 2 both stop) representing equipped/unequipped border state.

- `slotQuantity_22` (embed symbol113) — Quantity badge containing a `TextField`; two-frame PC/Console layout.

- `globeFrame_42` (embed symbol75) — Globe decorative frame; two-frame stop (likely normal/build variants toggled by `Hotbar.setMode`).

- `radiance_bar_3` (embed symbol134) — Radiance bar fill clip; stops on frame 1.

- `radiance_arrow_anim_6` (embed symbol141) — Single radiance arrow animation; plays 8 frames then stops.

- `spectateControls_35` (embed symbol161) — Spectate control overlay for PVP; two-frame PC/Console layout.

- `bg_fill_large_39` (embed symbol59) — Globe background fill animation with halting frames at 10, 20, 30 (three animated variants).

- `edgeBar_40` (embed symbol66) — Globe liquid-edge animation with identical halt structure to `bg_fill_large_39` (frames 10, 20, 30).

### Asset wrappers (bitmap/shape embeds — not individually detailed)

9 classes: `rarity_frame_common_png`, `rarity_frame_uncommon_png`, `rarity_frame_rare_png`, `rarity_frame_epic_png`, `rarity_frame_legendary_png`, `rarity_frame_relic_png`, `rarity_frame_shadow_png`, `rarity_frame_resplendent_png`, `rarity_frame_radiant1_png` — all extend `BitmapData`, embedding `/_assets/*.png`. Also: `rarity_frame_stellar` (extends `BitmapData`, embeds `/_assets/10_rarity_frame_stellar.png`), `dummy` (extends `BitmapData`, embeds `/_assets/99_dummy.png`, 52×52). Radiance projectile symbols: `radiance_projectile_4`, `radiance_projectile_a`, `radiance_projectile_b`, `radiance_projectile_c`, `radiance_projectile_d` — all extend `MovieClip`, bare embed wrappers. Total asset-wrapper classes: 16.

---

## Notable logic

- **Mode switching and slot shape:** `setMode` drives both layout visibility and slot shape assignment. Adventure/PVP use 6 square + 2 circle slots; discovery uses 5 square + 3 circle; build uses 0 square + 8 circle. The `goldFrame` overlay is explicitly suppressed in discovery mode.

- **Arc-mask globes (discovery mode):** The discovery health globe uses `arcSize = 348.5°` (nearly full circle), rotation `−106.5°`, and fixed inner/outer radii of 38–54 px. The energy globe uses a narrower arc of `145°`, rotation `−287°`, and tapered radii (70.5–75.8 px outer, 54.5–75.4 px inner). These are hardcoded in Hotbar's constructor using degree-to-radian conversion.

- **Chunk drain animation:** When health or energy drops by more than 5% in one update (`CHUNK_THRESHOLD`) and `useChunkAnimations` is true (discovery health only), the old fill level is retained as a darkened "ghost" that animates down to the new level over 5 seconds at 2 units/second via `IggyTween`. During this, `content.filters` is set to a dark `ColorMatrixFilter (−160 brightness)`.

- **Low-health pulse:** When `percent < lowPercentThreshold`, the globe overlays a brightening filter (`+200 brightness`) and starts an `IggyTween` ping-pong (`pulseIn`/`pulseOut`) cycling at 0.2 s per leg until the percentage rises above the threshold again.

- **Bard melody level:** `setBardMelodyLevel` uses an intentional fall-through switch (no `break` statements) so setting level 5 makes Melody1–Melody5 all visible simultaneously.

- **Hotkey label nudge:** `adjustSlotForHotkeySize` distinguishes between square slots (BG visible) — which only shift the text field — and circle slots — which shift both the slot container and the text field in opposite directions, effectively centering the label differently for each shape.

- **Console vs PC branching:** `IsConsole()` is called at multiple levels. `spectateControls` switches its layout frame on init. Mode MovieClips have a dedicated "Console" timeline label that `gotoAndPlay`s child globes and slots into their Console variants. `Globe` additionally uses `ENTER_FRAME` on Console to defer hiding `amountText` until the Console layout frame has rendered.
