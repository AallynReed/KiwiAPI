# dpsui.swf

> A minimal combat overlay that displays two live text fields — current DPS and cumulative damage dealt — updated by the game engine via ExternalInterface callbacks. Appears during or after combat encounters in Trove.

**Document/main class:** `DPSUI` (extends `MovieClip`)
**SWF-specific classes:** 1

---

## Main class: `DPSUI`

Extremely minimal: the constructor makes both text fields visible and registers two ExternalInterface callbacks when running inside the Iggy runtime. There is no layout logic, no state tracking beyond what the text fields hold, and no timer. Each setter requests a redraw via a deferred `Event.RENDER` listener.

### Public methods

- `setDpsText(param1:String, param2:uint = 0xFFFFFF) : void` — sets `dpsField.text` and `dpsField.textColor`, ensures the field is visible, then schedules `RequestRedraw`.
- `setDamageText(param1:String, param2:uint = 0xFFFFFF) : void` — same as above but targets `damageField`.

### Key fields

- `dpsField : TextField` — displays the DPS value string sent by the engine.
- `damageField : TextField` — displays the total damage value string sent by the engine.

### Runtime dependencies & integration

- **`IggyFunctions.inIggy`** — gates ExternalInterface registration; in non-Iggy mode the fields are visible but never populated.
- **ExternalInterface callbacks registered**: `setDpsText`, `setDamageText`.
- **ExternalInterface call made**: `RequestRedraw` — fired via a one-shot `Event.RENDER` listener after each text update, removed immediately after firing.

---

## Notable logic

- Both text fields start visible (`visible = true`) in the constructor unconditionally; the setters also force `visible = true`, so there is no hide/show mechanism — visibility is fully controlled by the engine positioning or removing the SWF from the stage.
- Color defaults to white (`0xFFFFFF`) for both setters, but the engine can pass any 24-bit color.
- No formatting, font, or layout code is present in the ActionScript — all styling must be embedded in the SWF timeline/symbol properties.
