# contextmenu.swf
> A lightweight right-click context menu popup that displays a dynamic list of labelled options. It appears wherever the game engine triggers a context menu (e.g. right-clicking world objects or players) and fires `OnOptionSelected` back to the game when the player picks an entry.

**Document/main class:** `KiwiContextMenu` (extends `_kiwi.Core.UIComponent`) — [Embed symbol "symbol11"]
**SWF-specific classes:** 3

---

## Main class: `KiwiContextMenu`

Manages the root context-menu widget. On construction it registers two Iggy callbacks: `updatedScreenSize` and `addItem`. In non-Iggy (preview) mode `configUI` adds three hard-coded dummy items. The menu itself is rendered by the child `OptionsContainer` component.

### Public methods
- `addItem(label:String, disabled:Boolean = false) : void` — Appends one option row via `mc_options_container.addOption("", label, disabled)`.
- `clear() : void` — Removes all option rows from `mc_options_container`.
- `updatedScreenSize(w:int, h:int) : void` — Clamps the menu's `root.x`/`root.y` so it stays inside the given screen bounds; called by the engine after the menu is positioned.

### Key fields
- `items : Array` — Unused remnant; actual item list is owned by `OptionsContainer`.
- `mc_options_container : OptionsContainer` — The child component that holds and renders all option rows.

### Runtime dependencies & integration
- `IggyFunctions.inIggy` — gate for registering Iggy callbacks.
- `ExternalInterface.addCallback("updatedScreenSize", ...)` / `addCallback("addItem", ...)`.
- `ExternalInterface.call("OnOptionSelected", index)` — fired by `OptionsContainer` on click (unless an `onItemSelected` override is set).

---

## Other game-specific classes

- `Option` (extends `MovieClip`) — [Embed symbol4] — A single menu row; exposes `textField` (left column, used as label) and `textField2` (right column, used as shortcut hint). Pure display symbol instantiated by `OptionsContainer`.
- `OptionHighlight` (extends `MovieClip`) — [Embed symbol6] — The selection-highlight overlay; shown/hidden and repositioned by `OptionsContainer` as the pointer enters/leaves rows.

---

## Notable logic (`_kiwi.Controls.OptionsContainer`)

`OptionsContainer` [Embed symbol7] is the framework class that drives all option logic:

- **Adding options:** `addOption(col1, col2, disabled)` instantiates a new `Option`, stacks it below the previous one, wires `ROLL_OVER`/`ROLL_OUT`/`CLICK` mouse listeners, tracks `optionDisabled[]`, and grows `height` to match.
- **Highlight:** On `ROLL_OVER`, `selectedItem` index is set and `InvalidationType.SELECTED` is raised; `draw()` repositions `mc_selection_highlight` to the hovered row's `y + 2` and recolors both text fields to `Color_ItemEnabled_Highlight` (0x555555).
- **Click:** Calls `onItemSelected(index)` function if set; otherwise `ExternalInterface.call("OnOptionSelected", selectedItem)`.
- **Controller support:** `onItemControllerEnter(i)`, `onItemControllerLeave(i)`, `onItemControllerClick(i)` mirror the mouse events for gamepad/keyboard navigation.
- **Disabled items:** Items passed with `disabled=true` are colored `Color_ItemEnabled_Disabled` (0x888888) and cannot be selected.
- **Clear:** `clearOptions()` removes all `Option` children, splices both tracking arrays, resets `selectedItem = -1`, hides the highlight, and resets `height = 10`.
