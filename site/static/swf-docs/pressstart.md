# pressstart.swf

> The Trove console title / main-menu screen displayed after boot, showing the account name and a navigable list of options (Play, Credits, News, Switch Profile). It handles platform-specific visibility (NX/Durango) and drives a loading animation on the decorative cube.

**Document/main class:** `PressStart` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 4 (1 logic-bearing, 3 trivial asset wrappers)

---

## Main class: `PressStart`

`PressStart` is the root document class. Its constructor registers Iggy callbacks, initialises state, uses `listenForFrame` to defer building the selectable-item list until the timeline reaches `targetFrame`, and stops the decorative `cube` clip. The component has a two-frame timeline (frames 1 and 11, both `stop()`).

### Public methods

- `setAccountName(param1:String) : *` — writes the player's account name into `accountNameText`.
- `setLatestNewsStatus(param1:Boolean) : *` — colours the News entry yellow (`Colors.LIGHT_YELLOW`) if there is unread news, or white otherwise. Safe to call before `setUpSelectableItems` runs (guarded by `newsIndex != -1`).
- `setHighlight(param1:int) : void` — moves the visual highlight (and yellow tint) to the given selection index; called by the engine for controller D-pad navigation.

### Key fields

| Field | Type | Role |
|---|---|---|
| `accountNameText` | `TextField` | Displays the logged-in account name |
| `centerTextPlay` | `MovieClip` | "Play" menu entry clip |
| `centerTextCredits` | `MovieClip` | "Credits" menu entry clip |
| `centerTextProfile` | `MovieClip` | "Switch Profile" entry (Durango only) |
| `centerTextNews` | `MovieClip` | "News" entry (hidden on NX) |
| `cube` | `MovieClip` | Decorative spinning cube; plays on `triggerLoadingAnimation` |
| `btnBackground` | `MovieClip` | Background graphic |
| `currentSelection` | `int` | Index of currently highlighted item (default 0) |
| `selectableItems` | `Array` | Ordered list of menu-entry clips, built at `targetFrame` |
| `newNews` | `Boolean` | Whether there is unread news (affects News entry colour) |
| `newsIndex` | `int` | Position of the News entry in `selectableItems` (-1 until built) |

### Frame scripts / timeline

- `frame1` — `stop()` (initial hold frame).
- `frame11` — `stop()` (second state frame, likely a transition end).
- `listenForFrame(this, targetFrame, setUpSelectableItems)` — deferred init: builds `selectableItems` array after the timeline reaches `targetFrame`.

### Runtime dependencies & integration

**`ExternalInterface.addCallback` registrations:**
- `SET_ACCOUNT_NAME` → `setAccountName`
- `SET_LATEST_NEWS_STATUS` → `setLatestNewsStatus`
- `setHighlight` → `setHighlight`
- `triggerLoadingAnimation` → starts `cube.play()`

**Platform guards:**
- `IsNX()` — hides `centerTextProfile` and `centerTextNews` entirely on Nintendo Switch.
- `_platform == PLATFORM_DURANGO && !IsNX()` — adds `centerTextProfile` to the selectable list on Xbox (Durango).

**Colour constants used:** `Colors.YELLOW` (highlighted item), `Colors.WHITE` (normal), `Colors.LIGHT_YELLOW` (news indicator).

---

## Other game-specific classes

- `Play` — `UIComponent` subclass; embeds `/_assets/assets.swf#symbol15`. No logic beyond constructor.
- `Credits` — `UIComponent` subclass; embeds `/_assets/assets.swf#symbol13`. No logic beyond constructor.
- `SwitchProfile` — `UIComponent` subclass; embeds `/_assets/assets.swf#symbol11`. No logic beyond constructor.

---

## Notable logic

- `setUpSelectableItems` runs after timeline reaches `targetFrame`, ensuring the named child clips exist. The News entry index (`newsIndex`) is captured at build time so `setLatestNewsStatus` can colour it without iterating the whole array.
- Highlight state is purely visual: each entry clip is expected to have child clips named `highlight` (alpha toggled), `button_console_south` (alpha toggled), and `textField` (colour toggled).
- `triggerLoadingAnimation` simply calls `cube.play()` — the animation runs until its own timeline ends, presumably looping or landing on a hold frame.
