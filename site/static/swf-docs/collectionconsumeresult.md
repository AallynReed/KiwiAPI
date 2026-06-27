# collectionconsumeresult.swf
> A small confirmation dialog shown after consuming a collection item, displaying the resulting item's name, a title string, an item icon slot, and an OK button. Appears immediately after the player uses (consumes) a collection box or similar item to reveal its contents.

**Document/main class:** `CollectionConsumeResult` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 1

## Main class: `CollectionConsumeResult`

Minimal dialog. Registers `SET_NAME` and `SET_TITLE` as `ExternalInterface` callbacks for the engine to populate text fields. The `slot` is sized to 75px on construction. The confirm button is pre-configured with `data = 1` (an integer token passed back in the `CONFIRM` call). Frame scripts stop at frames 1 and 11.

### Public methods
- `setName(name:String) : void` — Sets `nameTextField.text`.
- `setTitle(title:String) : void` — Sets `titleTextField.text`.
- `onConfirm(e:MouseEvent) : void` — Calls `ExternalInterface.call("CONFIRM", e.target.data)` — passes button data value (1) back to the engine.

### Key fields
- `titleTextField : TextField` — top label, set via `SET_TITLE`.
- `nameTextField : TextField` — item name, set via `SET_NAME`.
- `confirmButton : LabelButton` — labelled `$OK`, `data = 1`; clicking triggers `CONFIRM`.
- `slot : Slot` — 75×75 item icon display.
- `slotSize : int = 75` — slot dimension; passed to `slot.setSlotSize()` in constructor. Note: the slot's icon image is not set by Flash; the engine likely sets it via the `Slot` component's own `iconImage` property through Iggy.

### Frame scripts / timeline
- `frame1` — `stop()` (main display state).
- `frame11` — `stop()` (likely an animation end or alternate layout).

### Runtime dependencies & integration
- `ExternalInterface.addCallback("SET_NAME", setName)`, `("SET_TITLE", setTitle)`.
- `ExternalInterface.call("CONFIRM", data)` — `data` is always 1 (the button's `.data` property).
- Translate key: `$OK` — button label.
- `IggyFunctions.inIggy` — callbacks registered only in live Iggy context.

---

## Other game-specific classes

### `CollectionConsumeResult_fla.slotFrame_2` — Embed symbol29
Three-frame timeline symbol for the item slot border (3 frames = 3 rarity-state stops). Embedded from assets.

### `CollectionConsumeResult_fla.equipped_4`
Two-frame equipped-state indicator symbol (same pattern as in other SWFs).

### Asset wrappers (12 classes)
`rarity_frame_common_png`, `rarity_frame_uncommon_png`, `rarity_frame_rare_png`, `rarity_frame_epic_png`, `rarity_frame_legendary_png`, `rarity_frame_shadow_png`, `rarity_frame_relic_png`, `rarity_frame_resplendent_png`, `rarity_frame_radiant1_png`, `rarity_frame_stellar`, `BtnGreen`, `dummy` — bitmap/skin asset classes, no logic.

## Notable logic
- This is one of the smallest game-specific SWFs: a single main class with two text-field setters and one confirm callback. All complexity (item icon loading, rarity frame selection, quantity display) is handled by the `Slot` kiwi component internally once the engine sets `iconImage`.
- The `CONFIRM` call passes `e.target.data` (the button's data property = 1) rather than a hardcoded constant, suggesting this pattern supports multiple buttons with different data values in similar dialogs elsewhere.
