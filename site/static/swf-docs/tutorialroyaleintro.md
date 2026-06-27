# tutorialroyaleintro.swf
> A mode-selection interstitial shown to players entering a Bomber Royale tutorial for the first time, presenting two choices: play the Adventure tutorial or jump directly into Bomber Royale. Supports both mouse/click and console controller navigation.

**Document/main class:** `TutorialRoyaleIntro` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 1 game-logic class + 3 `_fla` timeline symbols

## Main class: `TutorialRoyaleIntro`

Simple two-button selection screen. Wires `buttonAdventure` and `buttonRoyale` (`BaseButton`) to click handlers and registers `ExternalInterface` callbacks for controller navigation. On console, auto-fires a `ROLL_OVER` on `buttonAdventure` to show the initial highlighted state. Frame scripts stop at frames 1 and 10.

### Public methods
*(All private or internal; public surface is via ExternalInterface callbacks only.)*

### Key fields
- `buttonAdventure : BaseButton` — left choice, fires `RequestAdventure` on click.
- `buttonRoyale : BaseButton` — right choice, fires `RequestBomberRoyale` on click.
- `currentSelection : int` — tracks which button (0=adventure, 1=royale) is highlighted for controller activation; default 0.

### Frame scripts / timeline
- `frame1` — `stop()` (main displayed state).
- `frame10` — `stop()` (alternate/outro state, purpose unclear without full timeline).

### Runtime dependencies & integration
- `ExternalInterface.addCallback("activateSelection", ...)` — called by game engine when the player presses the confirm button on console; triggers the currently highlighted choice.
- `ExternalInterface.addCallback("highlightSelection", index)` — moves highlight to adventure (0) or royale (1) by dispatching synthetic `ROLL_OVER`/`MOUSE_UP` events on the buttons.
- `ExternalInterface.call("RequestAdventure")` — notifies the game the player chose the adventure tutorial.
- `ExternalInterface.call("RequestBomberRoyale")` — notifies the game the player chose Bomber Royale.
- `IsConsole()` — triggers initial hover highlight on console.

---

## Other game-specific classes

### `TutorialRoyaleIntro_fla.slotFrameLarge_25` — Embed symbol24
Timeline symbol for a large item slot frame with 4 stopping frames (rarity tiers: common/uncommon/rare/epic or similar). Used in the intro's item display.

### `TutorialRoyaleIntro_fla.slotFrame_14` / `equipped_16`
Two additional `_fla` timeline symbols (not read in detail): `slotFrame_14` is likely a standard slot border, `equipped_16` a two-frame equipped-state indicator similar to counterparts in other SWFs.

### Asset wrappers (13 classes)
`rarity_frame_common_png`, `rarity_frame_uncommon_png`, `rarity_frame_rare_png`, `rarity_frame_epic_png`, `rarity_frame_legendary_png`, `rarity_frame_shadow_png`, `rarity_frame_relic_png`, `rarity_frame_resplendent_png`, `rarity_frame_radiant1_png`, `rarity_frame_stellar`, `backgroundFrame`, `slot_large`, `btnGreen`, `btnGreen_small`, `btnGreenIcon_small` — bitmap/shape asset classes, no logic.

`dummy` — 52x52 BitmapData placeholder.

## Notable logic
- **Controller navigation by synthetic events**: Rather than a separate highlight state machine, `highlightSelection` dispatches `MouseEvent.ROLL_OVER` / `MouseEvent.MOUSE_UP` on the `BaseButton` instances directly. This reuses the button's own hover visual state for controller focus feedback without duplicating animation logic.
- **`activateSelection` pattern**: A common Trove pattern where the game engine manages D-pad focus externally and sends a single "activate" signal; the Flash UI then routes it to the correct action based on locally tracked `currentSelection`.
