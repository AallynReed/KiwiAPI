# shortcuttray.swf
> The shortcut tray is the persistent HUD toolbar that gives players quick access to core UI panels (Store, Character, Collections, Inventory, Map, Class Select, Marketplace, Leaderboard, Chat). It adapts its layout and visible buttons depending on whether the client is running on PC or console (D-Pad mode).

**Document/main class:** `ShortcutTray` (extends `UIComponent`)
**SWF-specific classes:** 11

## Main class: `ShortcutTray`

`ShortcutTray` extends `_kiwi.Core.UIComponent` and is the root document class for the SWF. Its constructor wires up nine `BaseButton` references into an ordered `buttonsById` array (indices 0–8 correspond to the button constants defined below) and sets four of those buttons' initial component-inspector properties (store, character, collections, inventory). It registers `addFrameScript` handlers at frames 2 and 11 (both issue `stop()`), corresponding to the PC and Console timeline labels.

`configUI()` (called by the framework after the component is added to the stage) iterates over `buttonsById`, assigns each button's `data` property to its array index, sets `checked = false`, puts the button in the "up" state, and attaches a `MouseEvent.CLICK` listener pointing to `onOption`.

Platform adaptation is handled in `onStageResized`: when `IsConsole()` returns true the timeline jumps to the "Console" label, three buttons are repositioned to specific D-Pad pixel coordinates, and six buttons (store, collections, classChanger, marketplace, leaderboard, chat) are hidden by setting `alpha = 0`. On PC the timeline plays the "PC" label with all buttons visible.

### Public methods
- `ShortcutTray()` — constructor; populates `buttonsById`, calls four `__setProp_*` helpers to initialise component-inspector settings.

### Key fields
- `storeButton : BaseButton` — button at index `OPTION_STORE` (0); opens the Store panel.
- `characterButton : BaseButton` — index `OPTION_CHARACTER` (1); opens Character sheet.
- `collectionsButton : BaseButton` — index `OPTION_COLLECTIONS` (2); opens Collections.
- `inventoryButton : BaseButton` — index `OPTION_INVENTORY` (3); opens Inventory.
- `mapButton : BaseButton` — index `OPTION_MAP` (4); opens the Map.
- `classChangerButton : BaseButton` — index `OPTION_CLASSSELECT` (5); opens Class Select.
- `marketplaceButton : BaseButton` — index 6; opens the Marketplace.
- `leaderboardButton : BaseButton` — index 7; opens Leaderboard.
- `chatButton : BaseButton` — index 8; opens Chat.
- `buttonsById : Array` — ordered array of the nine buttons above; used to resolve button identity to option ID in `getOptionId`.
- Constants `DPAD_WEST_X/Y`, `DPAD_EAST_X/Y`, `DPAD_NORTH_X/Y` — pixel offsets for repositioning three buttons in console D-Pad layout.

### Frame scripts / timeline
- `frame2()` — calls `stop()`. Corresponds to the end of the PC layout label.
- `frame11()` — calls `stop()`. Corresponds to the end of the Console layout label.

### Runtime dependencies & integration
- `ExternalInterface.call("OnOptionClicked", optionId)` — fired when any button is clicked; passes the integer index (0–8) to the game engine.
- `IsConsole()` — global Iggy helper consulted in `onStageResized` to select the platform layout.
- All buttons are configured with `toggle = true`; the game engine is responsible for setting their checked state externally.
- `__setProp_*` methods apply Flash component-inspector defaults (enabled, toggle, visible) for four buttons; the remaining five (marketplace, leaderboard, chat, classChanger, map) rely on symbol defaults.

## Other game-specific classes

**Button symbol classes** (top-level, each embeds an `_assets/assets.swf` symbol and extends `BaseButton` with four frame stops at up/over/down/disabled states): `storeBtn` (symbol61), `characterBtn` (symbol55), `collectionBtn` (symbol49), `inventoryBtn` (symbol43), `mapBtn` (symbol37), `classBtn` (symbol31), `marketplaceButn` (symbol25), `leaderboardButton` (symbol19), `chatBtn` (symbol13), `settingsBtn` (symbol7, uses slightly different frame spacing: 1/12/23/34).

**Timeline symbol class** — `ShortcutTray_fla/btn_Console_DPAD_10` (symbol66, extends `MovieClip`): the console D-Pad button graphic with 8 frames and stop scripts at frames 1 and 8.

## Notable logic
- **Console vs. PC branching**: `onStageResized` is the sole branch point. On console, only three buttons remain interactive (character → east, inventory → west, map → north); all others are invisible (`alpha = 0`) but still present in the display list.
- **Option dispatch**: clicking any button calls `ExternalInterface.call("OnOptionClicked", id)` with a zero-based integer. The game engine maps these IDs to the actual panel open logic. There is no local navigation or state management beyond the button's toggle state.
- **No data push from engine**: the engine does not register callbacks into this SWF — communication is entirely outbound (click → `OnOptionClicked`).
