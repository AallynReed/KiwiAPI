# pvpinstructionsui.swf
> A simple modal dialog presenting the Bomber Royale instructions: six pictures, each with a one-line tip, under a BOMBER ROYALE header. Its "OK" button (and, on the PC frame, a window close button) closes the window and notifies the game.

**Document/main class:** `PVPInstructionsUI` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 5 (1 main class + 4 button skin variants; no `_fla` symbols)

---

## Main class: `PVPInstructionsUI`

`PVPInstructionsUI` is a thin UIComponent that manages a single interactive element: `okButton` (a `LabelButton`). The constructor adds frame scripts for frames 1 and 10 (both `stop()`), attaches a click listener to `okButton`, and applies component-inspector defaults via `__setProp_okButton_Scene1_button_0`.

The panel content is authored on the timeline and nothing in code fills it: a header sprite (chid 80) whose field carries `$PVP_battleroyale_header` ("BOMBER ROYALE"), one shape (chid 75) holding all six pictures, six tip `DefineEditText`s (chid 63–68) carrying the `$PVP_battleroyale_insructions_*` keys, and an unnamed `_kiwi.Controls.WindowCloseButton` (chid 36) on the PC frames. The class registers no callbacks of its own (only the inherited `UIComponent.*` ones), so the screen receives no data from the engine.

### Public methods

There are no public methods beyond the inherited UIComponent API. The only outward communication happens through the `okButton` click handler and the window close button, which sends the same pair of calls itself.

### Key fields

- `okButton : LabelButton` — the OK control. Its label is set to the translate key `"$ok"` by the component-inspector helper; no shipped language file defines `$ok` (the key the game ships is `$OK`). Two root instances carry this name: a `btnGreenIcon_small` (chid 56) at depth 1 and a `btnGreenIcon_medium` (chid 46) at depth 54, bottom centre; the field can bind only one of them.

### Frame scripts / timeline

- **Frame 1** — `stop()`. Labelled `PC`; the panel's initial visible state.
- **Frame 10** — `stop()`. Labelled `Console`; identical to frame 1 except that the window close button is removed.

### Runtime dependencies & integration

**ExternalInterface calls (Flash → game):**
- `ExternalInterface.call("POST_SOUND_EVENT", "Play_ui_window_close")` — plays the window-close sound effect on OK click.
- `ExternalInterface.call("OnRequestClose")` — notifies the game to dismiss the instructions panel.

**translate keys:**
- `"$ok"` — set as the `okButton` label via the component-inspector default; resolved at runtime by the framework's localisation layer.
- Baked into the timeline text fields: `$PVP_battleroyale_header` and the six tips `$PVP_battleroyale_insructions_heal`, `_powerups`, `_instantheal`, `_aim`, `_victory`, `_arena` (the keys are spelled "insructions").

**Events listened:**
- `MouseEvent.CLICK` on `okButton`.

---

## Other game-specific classes

All four are `LabelButton` subclasses embedding different button-art symbols from `/_assets/assets.swf`. Each registers 4-state stop-on-frame scripts (frames 10, 20, 30, 40 → `stop()`), the standard Kiwi button state pattern.

- `btnGreen` — `[Embed symbol="symbol15"]` full-size green label button.
- `btnGreen_small` — `[Embed symbol="symbol25"]` smaller green label button.
- `btnGreenIcon_medium` — `[Embed symbol="symbol46"]` medium green button with icon slot.
- `btnGreenIcon_small` — `[Embed symbol="symbol56"]` small green button with icon slot.

`btnGreenIcon_medium` and `btnGreenIcon_small` are the two `okButton` instances; `btnGreen` and `btnGreen_small` are not placed on the root timeline.

---

## Notable logic

- The UI is intentionally minimal: all text and imagery is authored in the SWF and the game sends it no data. The Flash side only handles the dismiss interaction.
- Sound playback is driven by posting a named sound event string to the game engine rather than using Flash's built-in sound system.
