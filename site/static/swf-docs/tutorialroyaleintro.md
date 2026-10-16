# tutorialroyaleintro.swf
> A mode-selection interstitial shown to players entering a Bomber Royale tutorial for the first time, presenting two choices: play the Adventure tutorial or jump directly into Bomber Royale. Supports both mouse/click and console controller navigation.

**Document/main class:** `TutorialRoyaleIntro` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 1 game-logic class + 3 `_fla` timeline symbols

## Main class: `TutorialRoyaleIntro`

Simple two-button selection screen. Wires `buttonAdventure` and `buttonRoyale` (`BaseButton`) to click handlers and registers `ExternalInterface` callbacks for controller navigation. On console, auto-fires a `ROLL_OVER` on `buttonAdventure` to show the initial highlighted state. Frame scripts stop at frames 1 and 10. Each button is a whole card with its picture and text baked into the symbol; nothing in code sets their content.

### Public methods
*(All private or internal; public surface is via ExternalInterface callbacks only.)*

### Key fields
- `buttonAdventure : BaseButton` — left choice (class `buttonAdventure`, symbol106), fires `RequestAdventure` on click. The card shows a map-and-sword picture, `$tutorial_popup_adventure_title` ("GO ADVENTURE") and `$tutorial_popup_adventure_text`.
- `buttonRoyale : BaseButton` — right choice (class `buttonRoyale`, symbol114), fires `RequestBomberRoyale` on click. The card shows a three-bomb picture, a `$tutorial_popup_advanced` ("ADVANCED") ribbon, `$tutorial_popup_bomberroyale_title` ("BOMBER ROYALE") and `$tutorial_popup_bomberroyale_text`.
- `currentSelection : int` — tracks which button (0=adventure, 1=royale) is highlighted for controller activation; default 0. Only `highlightSelection` changes it — mouse hover does not — so `activateSelection` fires the last controller highlight.

### Frame scripts / timeline
- `frame1` — `stop()`; labelled `PC`. The root holds `backgroundFrame`, `buttonRoyale` and `buttonAdventure`.
- `frame10` — `stop()`; labelled `Console`. Reshapes `backgroundFrame` and adds two unnamed console-only children (an empty text field, chid 118, and a panel sprite, chid 121).

### Runtime dependencies & integration
- `ExternalInterface.addCallback("activateSelection", ...)` — called by game engine when the player presses the confirm button on console; triggers the currently highlighted choice.
- `ExternalInterface.addCallback("highlightSelection", index)` — moves highlight to adventure (0) or royale (1) by dispatching synthetic `ROLL_OVER`/`MOUSE_UP` events on the buttons.
- `ExternalInterface.call("RequestAdventure")` — notifies the game the player chose the adventure tutorial.
- `ExternalInterface.call("RequestBomberRoyale")` — notifies the game the player chose Bomber Royale.
- `IsConsole()` — triggers initial hover highlight on console.

---

## Other game-specific classes

### `TutorialRoyaleIntro_fla.slotFrameLarge_25` — Embed symbol24
Timeline symbol for a large item slot frame with 4 stopping frames labelled `circle`, `square`, `largeCircle` and `highlight`. It is the `slotFrame` child of `slot_large`, which is not placed anywhere on this screen.

### `TutorialRoyaleIntro_fla.slotFrame_14` / `equipped_16`
Two additional `_fla` timeline symbols: `slotFrame_14` (symbol48) is the `slotFrame` child of `_kiwi.Controls.Slot`, and `equipped_16` (symbol28, two frames) is the `equippedBorder` child of both slot symbols. No slot is placed on this screen, so all three are unused shared assets here.

### Asset wrappers (13 classes)
`rarity_frame_common_png`, `rarity_frame_uncommon_png`, `rarity_frame_rare_png`, `rarity_frame_epic_png`, `rarity_frame_legendary_png`, `rarity_frame_shadow_png`, `rarity_frame_relic_png`, `rarity_frame_resplendent_png`, `rarity_frame_radiant1_png`, `rarity_frame_stellar`, `backgroundFrame`, `slot_large`, `btnGreen`, `btnGreen_small`, `btnGreenIcon_small` — bitmap/shape asset classes, no logic.

`dummy` — 52x52 BitmapData placeholder.

## Notable logic
- **Controller navigation by synthetic events**: Rather than a separate highlight state machine, `highlightSelection` dispatches `MouseEvent.ROLL_OVER` / `MouseEvent.MOUSE_UP` on the `BaseButton` instances directly. This reuses the button's own hover visual state for controller focus feedback without duplicating animation logic.
- **`activateSelection` pattern**: A common Trove pattern where the game engine manages D-pad focus externally and sends a single "activate" signal; the Flash UI then routes it to the correct action based on locally tracked `currentSelection`.
