# cornerstone.swf
> The Cornerstone window lets players manage their four Cornerstone plot slots — the personal building plots that follow a character in Trove. It displays each slot's current state (locked/purchasable, available to switch to, or currently active) and supports buying new slots, switching the active slot, renaming a slot, and erasing a slot's build.

**Document/main class:** `CornerstoneWindow` (extends `UIComponent`)
**SWF-specific classes:** 11 (excluding shared framework)

---

## Main class: `CornerstoneWindow`

`CornerstoneWindow` is the root UI component for the Cornerstone slot manager. The constructor registers two `addFrameScript` callbacks (frames 0 and 10), hides all four slot panels while they await their first render event, and attaches a one-shot `ENTER_FRAME` listener (`onRendered`) to reveal panels after they have painted. `configUI` registers all Iggy/ExternalInterface callbacks and, when running outside Iggy (dev mode), immediately sets a default selection and shows all panels. It also calls `setupTranslation()` (from `UIComponent`) and initialises the `winHeader` component via `__setProp_winHeader_Scene1_header_0`.

### Public methods

- `onRendered(e:Event) : void` — one-shot `ENTER_FRAME` handler; after one frame it removes itself and reveals every panel whose current frame is not `FRAME_BUY` (unless the TWP button is disabled, in which case it defers again via a frameCounter restart). Ensures panels are fully rendered before becoming visible.

### Key fields

- `cornerstoneList : MovieClip` — container MC (a `cornerstone_list_2` symbol) holding the four slot sub-MCs `cornerstone01`–`cornerstone04`, each a `cornerstone_entry_3` symbol.
- `winHeader : WindowHeaderSmall` — kiwi window header; title set to `$Cornerstones_WinTitle`.
- `closeButton : WindowCloseButton` — standard kiwi close button.
- `buttonLegend : MovieClip` — console-only button-legend bar (a `ButtonLegend_12` symbol) that updates per highlighted panel state ("Buy", "Switch", "Selected").
- `currentlyEditing : TextField` — reference to the `textCornerstoneName` field that is currently in rename-edit mode; `null` when no rename is active.
- `cornerstoneName : String` — stores the name text before a controller-driven rename begins (used to restore on cancel).
- `switchAllowed : Boolean` — whether the Switch button is enabled globally (set by `setCanSwitchSlots`).
- `frameCounter : int` — used by `onRendered` / `setPrice` to defer visibility until after the first rendered frame.
- **Frame constants:**
  - `FRAME_BUY = 1` / `FRAME_SWITCH = 2` / `FRAME_SELECTED = 3` (PC)
  - `FRAME_BUY_CONSOLE = 4` / `FRAME_SWITCH_CONSOLE = 5` / `FRAME_SELECTED_CONSOLE = 6`
- `NUM_PANELS : int = 4` — number of cornerstone slot panels.

### Frame scripts / timeline

- **Frame 0 (`frame1`):** calls `stop()`.
- **Frame 10 (`frame11`):** calls `stop()`, then plays the console-layout variants of `background` and `cornerstoneList` (`gotoAndPlay("Console")`).

Each individual `cornerstone_entry_3` slot MC also carries its own frame scripts (see below).

### Runtime dependencies & integration

**ExternalInterface callbacks registered (Iggy → Flash):**
| Callback name | Handler |
|---|---|
| `enableCornerstone` | `enableCornerstone(index)` |
| `setName` | `setName(index, name)` |
| `setPrice` | `setPrice(slotId, twcPrice, twpPrice, canAfford)` |
| `setSelected` | `setSelected(index)` |
| `setCanSwitchSlots` | `setCanSwitchSlots(canSwitch)` |
| `onControllerInteractA` | `onControllerInteractA(index)` |
| `onControllerInteractX` | `onControllerInteractX(index)` |
| `onControllerInteractY` | `onControllerInteractY(index)` |
| `onControllerRename` | `onControllerRename(index)` |
| `onControllerRenameCancel` | `onControllerRenameCancel()` |
| `highlightPanel` | `highlightPanel(index)` |
| `unhighlightPanel` | `unhighlightPanel(index)` |

**ExternalInterface calls made (Flash → Iggy/game):**
- `OnBuy(index, buttonName)` — fired from `TWC`/`TWP` click or controller A/X on a buy-state panel; `buttonName` is `"TWC"` or `"TWP"` identifying which currency was used.
- `OnSelect(index)` — fires when a Switch button is clicked or controller A is pressed on a switch-state panel.
- `OnErase(index)` — fires from the erase (trash) button click or controller X on a non-buy panel.
- `OnRename(index, newName)` — submitted when the rename text field loses focus (Enter key or second click of rename button).
- `OnSetRenamingState(isRenaming:Boolean)` — notifies the engine when rename mode starts or ends (from controller flow).
- `OnConfigured(numPanels)` — called inside `onFrameRendered` on console, telling the engine all panels are configured.

**Translate keys:**
- `$Cornerstones_WinTitle` — window header title (set as a component property, not a runtime call).
- `$Rename_ButtonLegend` — button legend rename label (console only, set in `onFrameRendered`).
- `$Switch` — default label for `switchBtn` in `cornerstone_entry_3` (set via component inspector properties).

**Visual effects:**
- `highlightPanel` applies an inner `GlowFilter` (colour `0xCCCC00`, blurX/Y = 2, strength = 100, HIGH quality) to the targeted panel. On NX it also shows `panel.highlight`.
- `unhighlightPanel` clears the filter array. On NX it hides `panel.highlight`.

---

## Other game-specific classes

### `Cornerstone_fla` timeline symbols

- **`cornerstone_list_2`** (symbol83) — container MC holding `cornerstone01`–`cornerstone04` slot sub-MCs. Frame 1 stops normally; frame 3 (Console layout) stops and sends each slot MC to its "Console" timeline label.

- **`cornerstone_entry_3`** (symbol82) — the per-slot panel MC. Has 6 frames (three PC states + three console states). Exposes `textCornerstoneName:TextField`, `renameBtn:btn_pencil`, `eraseBtn:btn_trash`, `TWC:btnGreenIcon_small`, `TWP:btnGreenIcon_small`, `switchBtn:btnGreenIcon_small`, `inputBG:MovieClip` (rename text background), and `highlight:MovieClip` (NX glow overlay). Frame scripts at 0 (Buy-PC), 1 (Switch-PC), 3 (Buy-Console), 4 (Switch-Console) initialise component inspector properties for `TWC`, `TWP`, and `switchBtn`, and drive sub-button console variants via `gotoAndPlay("Console")`.

- **`ButtonLegend_12`** (symbol90) — console button-legend bar with three keyed frames (0 = Buy, 10 = Switch, 20 = Selected), each stopped by a frame script. Navigated via `gotoAndStop("Buy" | "Switch" | "Selected")`.

### Top-level button skin classes (4 classes — all pure `LabelButton`/`BaseButton` skins)
`btnGreen` (symbol15), `btnGreen_small` (symbol25), `btnGreenIcon_small` (symbol60) — green label-button skins, four-state timeline (frames 10/20/30/40); `btn_pencil` (symbol45), `btn_trash` (symbol50) — icon-only `BaseButton` skins, same four-state pattern.

---

## Notable logic

- **Panel state machine:** Each slot MC is driven through a 3-state (PC) or 3-state (console) frame sequence by `enableCornerstone`, `setSelected`, and `setPrice`. The `onFrameRendered` callback (fired on `Event.RENDER`) wires the correct mouse listeners for whichever frame the MC just stopped on, avoiding stale listeners across state transitions.
- **Rename flow (mouse):** Clicking `renameBtn` calls `onRenameClicked`. On the first click it enables the text field for editing, shows `inputBG`, and focuses the field. A second click or Enter key calls `submitRename`, which emits `OnRename` and restores the field to read-only. If the user clicks a different panel's rename button while already editing, the previous rename is submitted first.
- **Rename flow (controller):** `onControllerRename` clears the field text before editing begins (contrast with the mouse path, which preserves it). Cancel via `onControllerRenameCancel` restores the pre-edit text from `cornerstoneName` and emits `OnSetRenamingState(false)`.
- **Buy panel visibility deferral:** `setPrice` hides a panel whose `TWP` button is disabled (player cannot afford it) and re-arms the `onRendered` frame loop so the panel remains hidden until the game explicitly re-enables it — preventing a flash of unaffordable content.
- **PC vs. console panel access:** `getMCByIndex(i)` returns `cornerstoneList["cornerstone0" + (i+1)]` (zero-indexed, one-suffixed). `getIndexByMC` extracts the last character of the parent's name and subtracts 1, enabling event-driven callbacks to resolve a panel index from any child's click event.
- **Two-currency purchase:** Each buy-state panel shows two buttons — `TWC` (Cubits, cheaper) and `TWP` (Trophy Points / premium currency, more expensive). Both fire `OnBuy` with their own `name` property so the engine knows which currency was chosen.
