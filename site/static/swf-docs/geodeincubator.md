# geodeincubator.swf
> The Geode Incubator / Reliquary window lets players place eggs or reliquaries into one of three slots, apply Karma-based buff influences, and watch a hatching/opening reveal animation. It is opened from the Geode world's incubator or reliquary crafting stations.

**Document/main class:** `GeodeIncubator` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 3 (`GeodeIncubator`, `IncubationSlot`, `Equipped`) + 4 `GeodeIncubator_fla` timeline classes + ~15 skin/asset wrappers

---

## Main class: `GeodeIncubator`

`GeodeIncubator` orchestrates the three-slot incubator window. On construction it registers frame scripts (frames 1 and 11, both `stop()`), registers a drag-drop callback via `SlotDragDropHelper.registerDropCallback`, announces itself to the engine with `ExternalInterface.call("OnConfigured", ROTATION_SPEED)`, and then registers eight `ExternalInterface` callbacks. In non-Iggy preview mode it populates slots with dummy data. Console mode initialises `currentSelection` to `slot0` and makes it visually selected.

### Public / internal methods

- `setIncubatorType(mode : int) : void` — switches between `MODE_EGG (0)` and `MODE_RELIQUARY (1)`, propagating `currentMode` to all three `IncubationSlot` instances and updating the window title with `$prefabs_placeable_crafting_incubator_item_name` or `$prefabs_placeable_crafting_reliquary_item_name`.
- `updateSlot(slotIdx, eggTex, petTex, timeRemaining, maxTime, locked) : void` — calls `IncubationSlot.setTime`, sets textures and locked state. If a slot becomes empty (`timeRemaining <= 0` and `maxTime > 0`) and the current console selection is a `BaseButton`, it auto-moves selection to that slot.
- `setRewardText(slotIdx, text) : void` — sets `IncubationSlot.text` (the top-label string).
- `setInfluencer(slotIdx, buffed) : void` — shows/hides `buffedAnimation` on a slot and enables/disables `buffButton0`; toggles `slot` frame to `1` (active) or `2` (inactive).
- `setInfluenceInfo(buffIdx, iconPath, count, name, description) : void` — updates the static `buffInfo` array and refreshes all three slots' buff-button icons, counts, and enabled states.
- `activateSelection() : void` — console confirm: if selection is a `BaseButton`, fires `onBuffSelected`; otherwise enters the slot to trigger buff buttons, OK button, or `onSlotClicked`.
- `cancel() : void` — console back: if in a buff-button, returns selection to the parent slot; otherwise calls `ExternalInterface.call("OnRequestClose")`.
- `moveSelection(dx, dy) : void` — console D-pad: moves between slots (horizontal) or between buff buttons within a slot.

### Key fields

- `slot0`, `slot1`, `slot2 : IncubationSlot` — the three incubation slots.
- `winTitleTextField : TextField` — window title, set to a translate key by `setIncubatorType`.
- `currentMode : int` — `MODE_EGG` or `MODE_RELIQUARY`; shared down to each slot.
- `currentSelection : MovieClip` — the currently focused element (a slot or buff button); its `selectedState` child is toggled visible.
- `NUM_SLOTS : Number = 3` — iteration constant.
- `ROTATION_SPEED : Number = 0.1308996938995747` — radian/frame rotation speed reported to the engine on init (≈ 7.5°/frame).
- `static buffInfo : Array` — two-element array `[{name, description}, {name, description}]` holding the current influence tooltip data; read by `IncubationSlot.onShowBuffTooltip`.
- `__id0_ : WindowHeaderSmall` — the window's header bar (set disabled with empty title via component inspector).

### Frame scripts / timeline

- **Frame 1** (`frame1`): `stop()`.
- **Frame 11** (`frame11`): `stop()`.

### Runtime dependencies & integration

- `ExternalInterface.call("OnConfigured", rotationSpeed)` — sent on init so the engine can synchronise the 3-D egg spin speed.
- `ExternalInterface.addCallback("setIncubatorType", …)` — switches MODE_EGG / MODE_RELIQUARY.
- `ExternalInterface.addCallback("updateSlot", …)` — drives slot content and progress.
- `ExternalInterface.addCallback("setRewardText", …)` — sets the reward label on a slot.
- `ExternalInterface.addCallback("setInfluencer", …)` — marks a slot as currently buffed.
- `ExternalInterface.addCallback("setInfluenceInfo", …)` — pushes buff-button icon/count data.
- `ExternalInterface.addCallback("activateSelection", …)` — console confirm.
- `ExternalInterface.addCallback("cancel", …)` — console back / close.
- `ExternalInterface.addCallback("moveSelection", …)` — console D-pad.
- `ExternalInterface.call("OnRequestClose")` — fired when cancel pressed at top level.
- `ExternalInterface.call("OnDroppedIntoSlot", slotIdx, itemId, itemExtra)` — fired by drag-drop handler.
- `SlotDragDropHelper.registerDropCallback(onDrop)` — wires item drag from inventory into incubator slots.

---

## Other game-specific classes

- `IncubationSlot` (extends `UIComponent`, embeds `assets.swf#symbol180`) — the central game-logic class for each slot. Manages egg/pet `ObjectPreview` textures, a progress bar (`progressBar.karmaMask.scaleX`), top-label text with auto-shrink via `KiwiTextUtil.resizeFont`, two buff buttons (`buffButton0/1`), an OK reset button, and an `openAnimation1/2` reveal sequence. Key behaviors:
  - `setTime(timeRemaining, maxTime)` — fills progress bar, detects "ready" state, shows open animations, posts sound `Play_ui_incubator_ready_to_hatch` via `ExternalInterface.call("POST_SOUND_EVENT", …)`.
  - `set eggTexture` / `set petTexture` — on `petTexture` arriving non-empty while `openAnimation1` is visible, triggers the full `IggyTween`-based reveal sequence: egg brightens → speed ramps (`ExternalInterface.call("OnAnimationChanged", slotId, speed)`) → cross-fade egg→pet → fade animations out → show OK button.
  - `set locked` — toggles button labels `$Store_Locked`, `$Incubator_AddEgg`, `$Incubator_AddReliquary`.
  - Buff buttons fire `ExternalInterface.call("OnBuffClicked", slotId, buffIdx)` and show/hide tooltips via `ExternalInterface.call("UIComponent.OnShowTooltip", …)` / `UIComponent.OnHideTooltip`.
  - Pulse animation (`IggyTween` loop on `glowPulse`) active in RELIQUARY mode while egg is loaded.
  - IggyTween used extensively: `pulseAnimation`, `animation1`–`animation3`, `accelerate`.
  - Translate keys: `$Incubator_ReadyToHatch`, `$Incubator_ReadyToOpen`, `$Store_Locked`, `$Incubator_AddEgg`, `$Incubator_AddReliquary`, `$OK`.

### GeodeIncubator_fla timeline symbols

- `equipped_50` (embeds `assets.swf#symbol18`) — two-frame stop MovieClip (equipped indicator).
- `icon_MC_7` — icon MovieClip (no additional AS logic beyond base MovieClip).
- `ButtonLegend_34` (embeds `assets.swf#symbol204`) — console button-legend bar with four directional button refs (`btnNorth/South/East/West` of types `btn_console_*`) and matching text fields; 5-frame timeline for different legend states.
- `slotFrameLarge_48` — large slot frame graphic (no logic).

### Asset wrappers (no logic)

~15 classes: `SlotBackground`, `SlotBackgroundLocked`, `SlotFrameHigh`, `SlotFrameMedium`, `SlotFrameNormal`, `slot_large`, `art`, `dummy`, `Equipped`, `Default_Button`, `btnGreen`, `btnGreenIcon_small`, `btnArrowLeft/Right/First/Last`, `AddUnlockButton`, `PreviewContainer`, `buff_slot`, `buff_slot_new`, `meter_karma`, `btn_console_north/south/east/west`, `btn_XBOne_A/B/X/Y` — pure graphic symbols or trivial wrappers with no game logic.

---

## Notable logic

- **Rotation speed constant**: `ROTATION_SPEED = Math.PI/24 ≈ 0.1309` rad/frame is reported to the engine immediately on init, letting the C++ renderer sync a spinning egg preview.
- **Reveal tween chain**: `IncubationSlot` executes a four-stage `IggyTween` sequence (egg brightness → speed acceleration → cross-fade → normalise) when `petTexture` is set while open animations are playing. Sound cues (`Play_ui_incubator_common` / `Play_ui_incubator_uncommon`) distinguish buffed vs unbuffed hatches.
- **Buff info static cache**: `GeodeIncubator.buffInfo` is a static array shared across all slots so `IncubationSlot.onShowBuffTooltip` can look up names/descriptions without holding a reference to the parent.
- **Progress bar**: `IncubationSlot.setTime` scales `progressBar.karmaMask.scaleX` proportionally to `timeRemaining / maxTime` (0 → 1).
