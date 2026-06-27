# pvpinstructionsui.swf
> A simple modal dialog shown before a PvP match begins, presenting the rules or instructions for the game mode. It has a single "OK" button that closes the window and notifies the game.

**Document/main class:** `PVPInstructionsUI` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 5 (1 main class + 4 button skin variants; no `_fla` symbols)

---

## Main class: `PVPInstructionsUI`

`PVPInstructionsUI` is a thin UIComponent that manages a single interactive element: `okButton` (a `LabelButton`). The constructor adds frame scripts for frames 1 and 10 (both `stop()`), attaches a click listener to `okButton`, and applies component-inspector defaults via `__setProp_okButton_Scene1_button_0`.

The panel content (rules text, images) is assumed to be authored on the timeline; no dynamic content loading is done in code.

### Public methods

There are no public methods beyond the inherited UIComponent API. The only outward communication happens through the `okButton` click handler.

### Key fields

- `okButton : LabelButton` — the sole interactive control. Its label is set to the translate key `"$ok"` by the component-inspector helper.

### Frame scripts / timeline

- **Frame 1** — `stop()`. The panel's initial visible state.
- **Frame 10** — `stop()`. A second labelled state (likely an animation end-frame or alternate layout); the game controls which frame is shown.

### Runtime dependencies & integration

**ExternalInterface calls (Flash → game):**
- `ExternalInterface.call("POST_SOUND_EVENT", "Play_ui_window_close")` — plays the window-close sound effect on OK click.
- `ExternalInterface.call("OnRequestClose")` — notifies the game to dismiss the instructions panel.

**translate key:**
- `"$ok"` — set as the `okButton` label via the component-inspector default; resolved at runtime by the framework's localisation layer.

**Events listened:**
- `MouseEvent.CLICK` on `okButton`.

---

## Other game-specific classes

All four are `LabelButton` subclasses embedding different button-art symbols from `/_assets/assets.swf`. Each registers 4-state stop-on-frame scripts (frames 10, 20, 30, 40 → `stop()`), the standard Kiwi button state pattern.

- `btnGreen` — `[Embed symbol="symbol15"]` full-size green label button.
- `btnGreen_small` — `[Embed symbol="symbol25"]` smaller green label button.
- `btnGreenIcon_medium` — `[Embed symbol="symbol46"]` medium green button with icon slot.
- `btnGreenIcon_small` — `[Embed symbol="symbol56"]` small green button with icon slot.

Only `btnGreen` (or a compatible LabelButton) is used by `PVPInstructionsUI` for `okButton`; the others are embedded in the SWF for potential timeline use.

---

## Notable logic

- The UI is intentionally minimal: the game is responsible for populating any text or imagery on the timeline. The Flash side only handles the dismiss interaction.
- Sound playback is driven by posting a named sound event string to the game engine rather than using Flash's built-in sound system.
