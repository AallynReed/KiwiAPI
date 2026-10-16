# cornerstone.swf
> The Cornerstone window lets players manage their four Cornerstone plot slots — the personal building plots that follow a character in Trove. It displays each slot's current state (locked/purchasable, available to switch to, or currently active) and supports buying new slots, switching the active slot, renaming a slot, and erasing a slot's build.

**Document/main class:** `CornerstoneWindow` (extends `UIComponent`)
**SWF-specific classes:** 11 (excluding shared framework)

---

## Main class: `CornerstoneWindow`

`CornerstoneWindow` is the root UI component for the Cornerstone slot manager. The constructor registers two `addFrameScript` callbacks (frames 0 and 10), hides all four slot panels while they await their first render event, and attaches a one-shot `ENTER_FRAME` listener (`onRendered`) to reveal panels after they have painted. `configUI` registers all Iggy/ExternalInterface callbacks and, when running outside Iggy (dev mode), immediately sets a default selection and shows all panels. It also calls `setupTranslation()` (from `UIComponent`) and initialises the `winHeader` component via `__setProp_winHeader_Scene1_header_0`.

### Public methods

- `onRendered(e:Event) : void` — `ENTER_FRAME` handler; on its second tick it removes itself and reveals every panel that is not on the PC Buy frame, plus any Buy panel whose `TWP` button is disabled. A PC Buy panel with `TWP` still enabled stays hidden until `setPrice` shows it. `setPrice` re-arms this handler (resetting `frameCounter`) when `canAfford` is false.

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
- `OnBuy(index, buttonName)` — fired from a `TWC`/`TWP` click, controller A (`TWC`) or controller X (`TWP`, only while it is enabled) on a buy-state panel. `buttonName` is the pressed button's instance name, `"TWC"` (Credits) or `"TWP"` (Cubits); that literal is how the engine knows which currency was chosen.
- `OnSelect(index)` — fires when a Switch button is clicked or controller A is pressed on a switch-state panel.
- `OnErase(index)` — fires from a single click on the erase (trash) button, or controller X on a switch/selected panel; the SWF asks for no confirmation. The controller path is ignored while a rename is open.
- `OnRename(index, newName)` — sent by `submitRename`: on Enter, on a second click of the same rename button, when another panel's rename button is clicked mid-edit, or on a second `onControllerRename`. There is no focus-out handler.
- `OnSetRenamingState(isRenaming:Boolean)` — controller path only: `true` when `onControllerRename` opens an edit, `false` when it closes one or `onControllerRenameCancel` runs. The mouse rename path never sends it.
- `OnConfigured(numPanels)` — called from `onFrameRendered` only when `IsConsole()` is true (once per panel render pass, always with `NUM_PANELS`); never called on PC.
- `OnRequestClose()` — sent by `closeButton` (kiwi `WindowCloseButton`) together with the `Play_ui_window_close` sound. The console frame of the main timeline removes `closeButton`; the legend labels the east face button `$Close_ButtonLegend` instead.

**Translate keys:**
- `$Cornerstones_WinTitle` ("CORNERSTONES") — window header title (set as a component property, not a runtime call).
- `$Cornerstones_Purchase` ("Unlock Slot") — timeline text of `textSlotPurchase` on the Buy frames.
- `$Cornerstones_Selected` ("Currently Selected") — timeline text of an unnamed caption placed on the Selected frames.
- `$Rename_ButtonLegend` ("Rename") — button legend rename label (console only). `onFrameRendered` assigns it to `buttonLegend.renameText.textField`, but `ButtonLegend_12` has no child named `renameText`; the legend's Rename field already carries the key as timeline text.
- `$CreditsBuy_ButtonLegend` ("Buy ([img:Currency_TWC])"), `$CubitsBuy_ButtonLegend` ("Buy ([img:Currency_TWP])"), `$Select_ButtonLegend`, `$Erase_ButtonLegend`, `$Close_ButtonLegend` — timeline text of the console legend entries (see `ButtonLegend_12`).
- `$Switch` ("SWITCH") — default label for `switchBtn` in `cornerstone_entry_3` (set via component inspector properties).

`setupTranslation()` re-assigns every `$`-prefixed TextField in the display tree; `configUI` and every `onFrameRendered` pass call it, so captions placed by a state change go through it too.

**Visual effects:**
- `highlightPanel` applies an inner `GlowFilter` (colour `0xCCFF00`, blurX/Y = 2, strength = 100, HIGH quality) to the targeted panel. On NX it also shows `panel.highlight`.
- `unhighlightPanel` clears the filter array. On NX it hides `panel.highlight`.

---

## Other game-specific classes

### `Cornerstone_fla` timeline symbols

- **`cornerstone_list_2`** (symbol83) — container MC holding `cornerstone01`–`cornerstone04` slot sub-MCs. Frame 1 stops normally; frame 3 (Console layout) stops and sends each slot MC to its "Console" timeline label.

- **`cornerstone_entry_3`** (symbol82) — the per-slot panel MC. Has 6 frames (three PC states + three console states). Exposes `textCornerstoneName:TextField`, `textSlotPurchase:TextField` (Buy-frame caption, `$Cornerstones_Purchase`), `renameBtn:btn_pencil`, `eraseBtn:btn_trash`, `TWC:btnGreenIcon_small` (Credits buy button, left), `TWP:btnGreenIcon_small` (Cubits buy button, right), `switchBtn:btnGreenIcon_small`, `inputBG:MovieClip` (rename text background), and `highlight:MovieClip` (NX glow overlay). Frame scripts at 0 (Buy-PC), 1 (Switch-PC), 3 (Buy-Console), 4 (Switch-Console) initialise component inspector properties for `TWC`, `TWP`, and `switchBtn`, and drive sub-button console variants via `gotoAndPlay("Console")`.
  - The currency icons are not part of the buttons. Both Buy frames (1 and 4) place an unnamed `DefineShape2` (character 70, depth 70) whose two bitmap fills are 24×24 `DefineBitsLossless2` images: the prismatic Credits cube (character 69) just left of `TWC` and the gold Cubits cube (character 68) just left of `TWP`. Being unnamed, it appears in no decompiled class.

- **`ButtonLegend_12`** (symbol90) — console button-legend bar with three keyed frames (0 = Buy, 10 = Switch, 20 = Selected), each stopped by a frame script. Navigated via `gotoAndStop("Buy" | "Switch" | "Selected")`. Its glyphs are `btn_console_*` classes from the imported `SharedLibraryController`, each beside an unnamed text field: Buy shows south `$CreditsBuy_ButtonLegend`, west `$CubitsBuy_ButtonLegend`, east `$Close_ButtonLegend`; Switch shows south `$Select_ButtonLegend`, west `$Erase_ButtonLegend`, north `$Rename_ButtonLegend`, east Close; Selected drops the south entry. This matches the pad callbacks: `onControllerInteractA` is south, `X` west, `Y` north.

### Top-level button skin classes (5 classes — all pure `LabelButton`/`BaseButton` skins)
`btnGreen` (symbol15), `btnGreen_small` (symbol25), `btnGreenIcon_small` (symbol60) — green label-button skins, four-state timeline (frames 10/20/30/40); `btn_pencil` (symbol45), `btn_trash` (symbol50) — icon-only `BaseButton` skins, same four-state pattern.

---

## Notable logic

- **Panel state machine:** Each slot MC has three states, on PC frames 1–3 or console frames 4–6. A slot starts on Buy; `enableCornerstone` moves it to Switch (an owned slot) and `setSelected` to Selected. Each touches only the slot it names. `setPrice` fills the prices of a slot on its Buy frame and never changes frame. The `onFrameRendered` callback (fired on `Event.RENDER`) wires the correct mouse listeners for whichever frame the MC just stopped on, avoiding stale listeners across state transitions.
- **Rename flow (mouse):** Clicking `renameBtn` calls `onRenameClicked`. On the first click it enables the text field for editing, shows `inputBG`, and focuses the field. A second click or Enter key calls `submitRename`, which emits `OnRename` and restores the field to read-only. If the user clicks a different panel's rename button while already editing, the previous rename is submitted first. Enter (`KEY_UP`, keyCode 13) is the only key handled, and the mouse path has no cancel: Escape is the game's own close-all-UI shortcut and never reaches the SWF.
- **Rename flow (controller):** `onControllerRename` is a toggle. Opening clears the field text before editing begins (contrast with the mouse path, which preserves it); a second call on the open edit submits it through `submitRename`. Cancel via `onControllerRenameCancel` restores the pre-edit text from `cornerstoneName` and emits `OnSetRenamingState(false)`.
- **Buy panel visibility:** On PC a Buy panel stays hidden until `setPrice` arrives for it. With `canAfford` true it is shown at once. With `canAfford` false, `setPrice` disables `TWP` and re-arms `onRendered`, which shows the panel two frames later with `TWP` greyed out.
- **PC vs. console panel access:** `getMCByIndex(i)` returns `cornerstoneList["cornerstone0" + (i+1)]` (zero-indexed, one-suffixed). `getIndexByMC` extracts the last character of the parent's name and subtracts 1, enabling event-driven callbacks to resolve a panel index from any child's click event.
- **Two-currency purchase:** Each buy-state panel shows two buttons — `TWC` for Credits (`$Currency_TWC_Title`, the premium currency bought with cash) and `TWP` for Cubits (`$Currency_TWP_Title`, the standard currency earned in play). Both fire `OnBuy` with their own `name` property so the engine knows which currency was chosen. `setPrice`'s `canAfford` gates only `TWP`: when false, `TWP.enabled = false` (greyed, not clickable, and skipped by controller X). The SWF never disables `TWC`.
