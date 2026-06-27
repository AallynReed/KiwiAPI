# healthmeter.swf

> Displays a boss (Titan) health bar during encounters in Trove. The UI shows the Titan's name and a horizontally-scaled mask representing current health as a fraction of maximum health. It is driven entirely by C++ game callbacks registered through the Iggy/ExternalInterface bridge.

**Document/main class:** `HealthMeter` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 1

## Main class: `HealthMeter`

`HealthMeter` is the document class and the only game-specific class in this SWF. It extends `UIComponent` and overrides `configUI()` to register three ExternalInterface callbacks that the game engine calls at runtime. There is no self-contained initialization logic beyond callback registration; all state is pushed from the host application.

### Public methods

- `configUI() : void` *(override, protected)* — Called by the UIComponent lifecycle after the component is added to the stage. Registers the three ExternalInterface callbacks when running inside Iggy (`IggyFunctions.inIggy == true`).

### Key fields

- `maxHp : Number` — Stores the maximum health value set by `setMaxHealth`; used as the divisor in `setHealth` to compute the fill ratio.
- `fillingMaskMC : MovieClip` — Timeline symbol that acts as a horizontal mask for the health bar graphic. Its `scaleX` is set to `currentHp / maxHp` (range 0–1) to represent the fill level.
- `txtBossName : TextField` — Displays the Titan/boss name string received via `setTitanName`.

### Runtime dependencies & integration

- **`IggyFunctions.inIggy`** — Guard flag; callbacks are only registered when running inside the Iggy runtime.
- **ExternalInterface callbacks registered:**
  - `"setMaxHealth"` → `setHealth(Number)` — sets `maxHp`.
  - `"setHealth"` → `setHealth(Number)` — sets `fillingMaskMC.scaleX = value / maxHp`.
  - `"setTitanName"` → `setTitanName(String)` — sets `txtBossName.text`.
- No events dispatched outward; no translate keys; no timers.

## Other game-specific classes

None beyond `HealthMeter`.

## Notable logic

- Health fraction is computed as a simple linear scale: `scaleX = currentHp / maxHp`. No clamping is applied in code, so the game engine is expected to send values in the valid range.
- `setMaxHealth` must be called before `setHealth` to avoid division by zero or an undefined `maxHp`.
