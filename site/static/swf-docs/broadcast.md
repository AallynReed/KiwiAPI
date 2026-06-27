# broadcast.swf

> A minimal HUD overlay that displays two lines of broadcast text in Trove: one system-driven message and one player-driven message. It appears whenever the game needs to push a short announcement or player notification onto the screen.

**Document/main class:** `Broadcast` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 1

---

## Main class: `Broadcast`

`Broadcast` is the sole game-specific class in this SWF. It exposes two `TextField` instances placed on the timeline — `systemTextField` for server/system messages and `playerTextField` for messages targeted at the local player. The constructor initialises both fields to empty strings and registers a single ExternalInterface callback so the Iggy/C++ host can push player messages at runtime.

### Public methods

- `set systemMessage(param1:String) : void` — writes `param1` directly into `systemTextField.text`. Intended for server-side system announcements.
- `setPlayerMessage(param1:String, param2:uint) : void` — writes `param1` into `playerTextField.text` and applies `param2` as the text colour (`textColor`). Registered as an ExternalInterface callback named `"setPlayerMessage"`.

### Key fields

- `systemTextField : TextField` — timeline symbol reference; displays system/server broadcast messages.
- `playerTextField : TextField` — timeline symbol reference; displays player-targeted messages with a runtime-configurable colour.

### Frame scripts / timeline

No `addFrameScript` calls or labelled frame logic are present. The two `TextField` instances are embedded directly on the timeline.

### Runtime dependencies & integration

- **ExternalInterface callback:** `"setPlayerMessage"` → `this.setPlayerMessage` — the game engine calls this from C++/Iggy to push a coloured player message into the UI.
- **System message path:** `systemMessage` setter is called from ActionScript (no ExternalInterface wrapper visible here); likely driven by an Iggy event or a parent controller invoking the setter directly.
- **No translate() keys** observed.
- **No timers or event listeners** beyond UIComponent internals.

---

## Notable logic

- The split between `systemTextField` and `playerTextField` suggests two separate broadcast channels: one for global/system announcements (set via AS setter) and one for player-personalised messages (set via ExternalInterface, with explicit colour control).
- The colour parameter in `setPlayerMessage` (`uint`) allows the host to colour-code messages per context (e.g. green for loot, red for alerts) without any AS-side logic.
- There are no framework-package (`_kiwi/`, `fl/`) classes that add game-specific behaviour; all Kiwi files present are shared UIComponent infrastructure.
