# mainmenu.swf
> The persistent in-game main menu bar displayed during gameplay. It provides a collapsible dropdown of navigation options, a hub shortcut button, and a patron upsell icon. The game drives it entirely via `ExternalInterface` callbacks.

**Document/main class:** `MainMenu` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 6 (excluding framework)

---

## Main class: `MainMenu`

`MainMenu` is the root UIComponent for the main menu overlay. Its constructor wires up click listeners on three child MovieClips (`btn_main_menu`, `btn_hub`, `patronIcon`) and registers all Iggy-facing callbacks via `ExternalInterface.addCallback`. When not running inside Iggy it seeds the dropdown with three dummy options for layout testing. After construction it calls the component-inspector helper `__setProp_mc_dropdown_menu_Scene1_dropdown_0` to initialise the dropdown's properties from Flash IDE inspector data.

`configUI()` (called by the framework on first render) hides the dropdown by default.

`onStageResized()` fires `ExternalInterface.call("OnConfigured", NUM_SELECTIONS)` on console builds, letting the game know the layout is ready and how many slots exist.

### Public methods

- `showDropDown(param1:Boolean) : void` — shows or hides the dropdown; delegates to the `menuVisible` setter.
- `addOption(key:String, label:String) : uint` — adds a row to `mc_options_container` and resizes `mc_dropdown_menu` to fit; returns the new option's index.
- `clearOptions() : void` — removes all rows from `mc_options_container`.
- `showPatronUpsell(visible:Boolean) : void` — shows/hides `patronIcon`.
- `setShowMenuHotkey(hotkey:String) : void` — writes a hotkey hint string into the menu-button's `textField`.
- `set menuVisible(v:Boolean) : void` — toggles `mc_dropdown_menu.visible` and `mc_options_container.visible`; plays the `"down"` or `"up"` label animation on `btn_main_menu`.
- `get menuVisible() : Boolean` — returns `mc_dropdown_menu.visible`.
- `set showGlow(v:Boolean) : void` / `get showGlow() : Boolean` — controls `btn_main_menu.btnMenuOuterGlow.visible`.

### Key fields

- `NUM_SELECTIONS : int = 6` — total option-slot count reported to the game on console resize.
- `btn_hub : MovieClip` — hub-navigation button instance.
- `btn_main_menu : MovieClip` — the trigger button; has a `textField` for hotkey text and a `btnMenuOuterGlow` child.
- `mc_options_container : OptionsContainer` — framework list that holds individual `Option` rows.
- `mc_dropdown_menu : MovieClip` — the dropdown panel whose height is recalculated each time an option is added.
- `patronIcon : MovieClip` — patron upsell icon (hidden unless `showPatronUpsell(true)` is called).

### Frame scripts / timeline

- `__setProp_mc_dropdown_menu_Scene1_dropdown_0()` — sets inspector defaults on `mc_dropdown_menu`: `data=""`, `enabled=false`, `label="LabelButton"`, `toggle=false`, `visible=true`.

### Runtime dependencies & integration

**ExternalInterface callbacks registered (Iggy → Flash):**
| Callback | Method |
|---|---|
| `showDropDown` | `showDropDown` |
| `addOption` | `addOption` |
| `clearOptions` | `clearOptions` |
| `setShowMenuHotkey` | `setShowMenuHotkey` |
| `showPatronUpsell` | `showPatronUpsell` |
| `activateSelection` | `onActivateSelection` → `mc_options_container.onItemControllerEnter` |
| `deactivateSelection` | `onDeactivateSelection` → `mc_options_container.onItemControllerLeave` |
| `select` | `onSelect` → `mc_options_container.onItemControllerClick` |

**ExternalInterface calls (Flash → game):**
- `"OnDropTrigger"` — fired when the user clicks the menu trigger button and the menu opens.
- `"OnHubClicked"` — fired when `btn_hub` is clicked.
- `"OnPatronClicked"` — fired when `patronIcon` is clicked.
- `"OnConfigured"(NUM_SELECTIONS)` — fired on stage resize on console builds.

**Events listened:**
- `MouseEvent.CLICK` on `btn_main_menu`, `btn_hub`, `patronIcon`.

---

## Other game-specific classes

- `Option` — `[Embed symbol="symbol25"]` MovieClip with two `TextField` children (`textField`, `textField2`); used as individual menu rows in `OptionsContainer`.
- `OptionHighlight` — `[Embed symbol="symbol27"]` plain MovieClip; used as the hover/selection highlight overlay in option rows.
- `btnMainMenu` — `[Embed symbol="symbol10"]` `BaseButton` subclass; 4-state button (frames 10/20/30/40 stop) for the main trigger; hosts `textField` (hotkey label) and `btnMenuOuterGlow` child.
- `btnHub` — `[Embed symbol="symbol21"]` `BaseButton` subclass; 4-state stop-on-frame button for the hub shortcut.
- `patron_button` — `[Embed symbol="symbol15"]` `BaseButton` subclass; 4-state button for the patron upsell icon slot.

---

## Notable logic

- **Dynamic dropdown height** — every call to `addOption` recalculates `mc_dropdown_menu.height` as `(mc_options_container.y − mc_dropdown_menu.y + mc_options_container.height + 5)` so the panel always fits its content exactly.
- **Console detection** — `IsConsole()` is checked inside `onStageResized`; on non-console builds the `OnConfigured` call is skipped entirely.
- **Iggy guard** — all `ExternalInterface` registration and calling is guarded by `IggyFunctions.inIggy` so the SWF degrades gracefully in a standalone Flash Player.
