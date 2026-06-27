# displayarea.swf

> A console TV-safe-zone calibration overlay shown to players on console platforms. It presents a resizable safe-zone rectangle with Accept/Cancel buttons so the player can confirm the display area fits their screen.

**Document/main class:** `DisplayArea` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 3 game-specific + 2 asset wrappers

---

## Main class: `DisplayArea`

`DisplayArea` is the root document class. On construction it records the original dimensions and positions of `safeZone`, `bg`, `topRight`, and `bottomLeft`, then translates the Accept/Cancel button labels via `IggyFunctions.translate`. When running inside the Iggy runtime it registers a JavaScript callback `scaleSafeZone` so the game engine can drive the scaling at runtime.

### Public methods

- `scaleSafeZone(param1:Number) : void` — Scales `safeZone` and `bg` by the given factor (0–1) and repositions `topRight` and `bottomLeft` corner markers symmetrically, keeping the overlay centered on the original canvas.

### Key fields

| Field | Type | Role |
|---|---|---|
| `btnAccept` | `btnGreen` | Accept button; label set to `IggyFunctions.translate("$Accept")` |
| `btnCancel` | `btnGreen` | Cancel button; label set to `IggyFunctions.translate("$Cancel")` |
| `safeZone` | `MovieClip` | The inner rectangle representing the safe zone |
| `bg` | `MovieClip` | Background fill, scaled together with `safeZone` |
| `topRight` | `MovieClip` | Top-right corner marker, repositioned on scale |
| `bottomLeft` | `MovieClip` | Bottom-left corner marker, repositioned on scale |
| `textBoxes` | `MovieClip` | Text label group on the overlay |
| `originalWidth/Height` | `Number` | Stored initial dimensions for proportional scaling |

### Runtime dependencies & integration

- `IggyFunctions.translate("$Accept")`, `IggyFunctions.translate("$Cancel")` — localisation keys for button labels.
- `IggyFunctions.inIggy` guard — `scaleSafeZone` callback is only registered when running in the Iggy runtime.
- `ExternalInterface.addCallback("scaleSafeZone", ...)` — game engine calls this to push a new scale factor (e.g. after the player adjusts the slider).
- `this.setupTranslation()` — inherited UIComponent hook for any remaining locale setup.

---

## Other game-specific classes

- `btnGreen` — `LabelButton` subclass; embeds `/_assets/assets.swf#symbol19`. Eight-state timeline button (frames 10, 20, 30, 40, 50, 60, 70, 80) each with a `stop()` handler. Used for Accept and Cancel.
- `btn_console_dpad_north` — asset wrapper; embeds `/_assets/assets.swf#symbol3`. Plain `MovieClip`, no logic.
- `btn_console_dpad_south` — asset wrapper; embeds `/_assets/assets.swf#symbol6`. Plain `MovieClip`, no logic.

---

## Notable logic

- `scaleSafeZone` uses the formula `x = (1 - scale) * originalWidth * 0.5` to re-centre the scaled rectangle, ensuring the shrinkage is applied equally from all sides.
- The corner markers (`topRight`, `bottomLeft`) are moved in the opposite direction to the centre offset so they remain visually anchored at the corners of the original boundary.
