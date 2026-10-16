# healthmeter.swf

> Displays a boss (Titan) health bar during encounters in Trove. The UI shows the Titan's name and a horizontally-scaled mask representing current health as a fraction of maximum health. It is driven entirely by C++ game callbacks registered through the Iggy/ExternalInterface bridge.

**Document/main class:** `HealthMeter` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 1

## Main class: `HealthMeter`

`HealthMeter` is the document class and the only game-specific class in this SWF. It extends `UIComponent` and overrides `configUI()` to register three ExternalInterface callbacks that the game engine calls at runtime. There is no self-contained initialization logic beyond callback registration; all state is pushed from the host application.

### Public methods

- `configUI() : void` *(override, protected)* — Called by the UIComponent lifecycle after the component is added to the stage. Registers the three ExternalInterface callbacks when running inside Iggy (`IggyFunctions.inIggy == true`).

### Key fields

- `maxHp : Number` — Stores the maximum health value set by `setMaxHealth`; used as the divisor in `setHealth` to compute the fill ratio. Values are raw health points; a titan's maximum runs into the billions.
- `fillingMaskMC : MovieClip` — Despite the name, not the fill: a solid rectangle (chid 5) placed as a clipping layer (clip depth 15) over the red fill, which is an unnamed nine-slice bitmap clip (chid 8) at depth 5. Its `scaleX` is set to `currentHp / maxHp` (range 0–1) to represent the fill level.
- `txtBossName : TextField` — Displays the Titan/boss name string received via `setTitanName` (Open Sans, placeholder text "Spike Walker").

### Runtime dependencies & integration

- **`IggyFunctions.inIggy`** — Guard flag; callbacks are only registered when running inside the Iggy runtime.
- **ExternalInterface callbacks registered:**
  - `"setMaxHealth"` → `setMaxHealth(Number)` — sets `maxHp`.
  - `"setHealth"` → `setHealth(Number)` — sets `fillingMaskMC.scaleX = value / maxHp`. In a crowded fight the engine sends it several times a frame, mostly repeating the value already shown.
  - `"setTitanName"` → `setTitanName(String)` — sets `txtBossName.text`.
- No events dispatched outward; no translate keys; no timers.

## Other game-specific classes

None beyond `HealthMeter`. The SWF is a single 425×48 frame at 24 fps: a frame bitmap clip at depth 1, `fillingMaskMC` at depth 3, the fill at depth 5 and `txtBossName` at depth 17.

## Notable logic

- Health fraction is computed as a simple linear scale: `scaleX = currentHp / maxHp`. No clamping is applied in code, so the game engine is expected to send values in the valid range.
- `setMaxHealth` must be called before `setHealth` to avoid division by zero or an undefined `maxHp`.
