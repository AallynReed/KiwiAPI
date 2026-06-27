# moduleloadout.swf
> The Module Loadout panel in Trove where a player equips and swaps active modules, passive modules, and reliquaries on their character, and views/swaps their active companion. It is opened from the character sheet and supports both PC and console navigation.

**Document/main class:** `ModuleLoadout` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 9 game-logic classes (plus 5 `ModuleLoadout_fla` timeline symbols and ~35 asset-wrapper/skin classes)

---

## Main class: `ModuleLoadout`

`ModuleLoadout` is the root UIComponent. The constructor calls `addFrameScript(0→frame1, 11→frame12)`, then calls `__setProp_swapCompanion_Scene1_swapBtn_0()` to configure the companion swap button's translate label, and registers a `FRAME_CONSTRUCTED` listener to re-apply timeline component inspector properties when the frame changes.

`configUI()` is the main initialisation:
- Listens for `CLICK` on `m_acceptButton` and `swapCompanion`.
- Listens for `CLICK` on `m_moduleList["closeBtn"]` to close the swap module window.
- Hides `m_moduleList` and sets `companionIcon.data = 4` (companion type default).
- Registers all `ExternalInterface` callbacks when `IggyFunctions.inIggy` is true.

The `draw()` override (triggered by `STYLES` invalidation) re-positions companion stat labels, compressing leading to fit all stats above the swap companion button.

### Public methods (also registered as ExternalInterface callbacks in Iggy)
- `setCompanionName(name:String)` — shows and sets `companionName` TextField; invalidates STYLES.
- `addCompanionStat(text:String)` — appends a `StatText` instance above the companion swap button; invalidates STYLES for layout.
- `clearCompanionStats()` — hides all `StatText` instances.
- `addModule(iconImage, itemImage, tooltipTitle, tooltipDesc, rarity:int, hotkey:String, showSwap:Boolean, itemIndex:int)` — adds a `ModuleItem` (active module slot); positions it vertically at `moduleListYPosition + n * moduleListSpacing`; adjusts description text alignment based on whether an icon image is present.
- `clearModules()` — hides all `ModuleItem` instances.
- `addModuleListItem(moduleIndex, iconImage, abilityImage, name, desc, level, rarity:int, isUnlocked:Boolean, hotkey:String, rarityColor:uint)` — adds a `ModuleSelectButton` to the swap-module popup list (`m_moduleList`), laid out in a 2-column grid.
- `clearSelectModuleList()` — hides all `ModuleSelectButton` instances and resets console cursor.
- `addPassiveModule(iconImage, name, desc, tooltip, rarity:int, color:uint)` — adds a `ModulePassive` to a 2-column grid on the right side.
- `clearPassiveModules()` — hides all `ModulePassive` instances.
- `addReliquary(index, iconImage, name, desc, xp:Number, maxXp:Number, isLocked:Boolean, rarityIndex:int)` — adds a `MegaItem` (reliquary slot); positioned at `reliquaryListYPosition + n * reliquaryListSpacing`.
- `clearReqliquaries()` — hides all `MegaItem` instances.
- `moveSelection(dx:int, dy:int)` — console D-pad navigation across 4 selection modes (0=active modules, 1=passive modules, 2=reliquaries, 3=companion swap button).
- `activateCurrentSelection()` — triggers click on whichever console-selected element is active (module swap, reliquary swap, accept, companion swap, or a `ModuleSelectButton`).

### Key fields
- `m_acceptButton : LabelButton` — "Accept" button; translate key `$Accept`; fires `AcceptClicked` via ExternalInterface.
- `m_moduleList : MovieClip` — container for the swap-module popup; holds `closeBtn` and the dynamically added `ModuleSelectButton` children; initially hidden.
- `swapCompanion : MovieClip` (configured as `LabelButton`) — translate key `$ModuleLoadout_SwapBtnCompanion`; fires `OnSwapCompanion`.
- `companionIcon : ArtClip` — renders the companion's portrait; `data = 4` on init.
- `companionName : TextField` — companion name label.
- `companionRarity : TextField` — companion rarity label; positioned below `companionName` in `draw()`.
- `unequippedCompanionText : TextField` — shown when no companion is equipped.
- `petBackground : MovieClip` — visual background for the companion panel.
- `passiveModules : Array` — pool of `ModulePassive` instances.
- `moduleSelectList : Array` — pool of `ModuleSelectButton` instances (in the swap popup).
- `moduleButtons : Array` — pool of `ModuleItem` instances.
- `companionStats : Array` — pool of `StatText` instances.
- `reliquariesList : Array` — pool of `MegaItem` instances.
- `currentSelection : MovieClip` — currently console-highlighted element.
- `currentSelectedModule : int` — index of the highlighted item in `moduleSelectList` (−1 = none).
- `m_selectionMode : int` — 0=active modules, 1=passive modules, 2=reliquaries, 3=companion.
- Layout constants: `moduleListXPosition=108.45`, `moduleListYPosition=72.55`, `moduleListSpacing=127.45`, `reliquaryListXPosition=110`, `reliquaryListYPosition=361.35`, `reliquaryListSpacing=125.15`, `passiveModuleListXPosition=596`, `passiveModuleListYPosition=77`, `passiveModuleListXSpacing=151.95`, `passiveModuleListYSpacing=127`, `NUM_PASSIVE_COLUMNS=2`, `companionStatXPosition=671.85`, `companionSkillPadding=25.6`, `companionRarityPadding=−4`, `moduleSelectListXPosition=23.75`, `moduleSelectListYPosition=58.45`, `moduleSelectListXSpacing=455.15`, `moduleSelectListYSpacing=184.55`, `passiveModuleImageSize=74.5`.

### Frame scripts / timeline
- `frame1` — `stop()` (PC layout).
- `frame12` — `stop()` (console layout; no additional child navigation).
- `__setProp_handler` on `FRAME_CONSTRUCTED` — re-applies `m_acceptButton` component inspector props when the frame changes (translate key `$Accept`).

### Runtime dependencies & integration
- **IggyFunctions.inIggy** — gates ExternalInterface callback registration.
- **ExternalInterface.call** (outgoing): `AcceptClicked`, `CloseSwapModule`, `OnSwapCompanion`, `OpenSwapModule(itemIndex)`, `OnSwapReliquary(index)`, `SelectedModule(moduleIndex)`.
- **IsConsole()** — adjusts passive module tooltip style and auto-selects first item in module lists.
- **HighlightUtil** (`_kiwi.Util.HighlightUtil`) — `highlightMovieClip` / `unhighlightMovieClip` used for console focus; `ModuleSelectButton.selectHighlight` used instead for select items.
- **Slot** (`_kiwi.Controls.Slot`) — `setSize`, `setSlotSize`, `setRarityScale`, `UITooltipTitle/Description`, `rarity`, `iconImage`, `clickFeedback` properties.
- **KiwiTextUtil.rainbowify** — applied to `moduleLevel` text when rarity == 6 (stellar/radiant).
- **fl.core.InvalidationType.STYLES** — triggers companion stat layout recalculation in `draw()`.

---

## Other game-specific classes

- `ModuleItem` — Embed symbol125; extends `UIComponent`; displays one active module slot with `image` (ArtClip for the module ability icon), `itemImage` (Slot for the item render), `swapButton` (LabelButton, label `$ModuleLoadout_SwapBtn`), `moduleName` and `description` TextFields; `onSwapClicked` calls `ExternalInterface.call("OpenSwapModule", itemIndex)`.
- `ModulePassive` — Embed symbol106; extends `UIComponent`; read-only display of a passive module with `itemImage` (Slot), `moduleName`, `moduleLevel`, `moduleDescription` TextFields; no interaction logic.
- `MegaItem` — Embed symbol146; extends `UIComponent`; reliquary slot with `itemImage` (Slot), `swapBtn` (LabelButton, label `$ModuleLoadout_SwapBtn` / `$ModuleLoadout_UnlockBtn` depending on locked state), `xpBar` MovieClip (fill + highlight sub-clips), `reliquaryXP` TextField, `moduleName` TextField, `lockedImage`/`locked`/`unequipped`/`m_emptySlot` visibility states; `SetData` computes XP bar fill as `fill.scaleX = clamp(xp/maxXp, 0, 1)`; calls `ExternalInterface.call("OnSwapReliquary", m_index)`.
- `ModuleSelectButton` — Embed symbol104; extends `UIComponent`; shown in the swap-module popup; 4 frame labels (Default, hover, down, Locked) plus console variants (frames 1, 10, 21, 32); `SetData` sets `itemRender` (Slot), `abilityIcon` (ArtClip), name/level/description TextFields, hotkey MovieClip (console uses `htmlText`), and locked state; `onClick` fires `ExternalInterface.call("SelectedModule", moduleIndex)` unless locked; `KiwiTextUtil.rainbowify` applied to `moduleLevel` when rarity == 6.
- `StatText` — Embed symbol44; trivial `MovieClip` with a single `textField:TextField`; used for companion stat lines.
- `PreviewContainer` — Embed symbol201; extends `ArtClip`; thin wrapper for companion/item preview rendering (no additional logic).
- `Image` — Embed symbol86; extends `ArtClip`; generic image/art display wrapper (no additional logic).
- `ModuleCloseBtn` — Embed symbol189; extends `_kiwi.Controls.BaseButton`; 4-state button (frames 10/20/30/40 stops) used as the close button on `m_moduleList`.

**ModuleLoadout_fla timeline symbols (5):**
- `equipped_18` (symbol59) — 2-frame MC (frames 1, 2 stop) for equipped indicator overlay.
- `equipedLarge_16` (symbol56) — animated MC; frame 1 stops, frame 61 loops to `"Pulse"` label (pulse animation for equipped large slot).
- `slotFrameLarge_15` (symbol53) — 3-frame slot frame (stops at 1, 2, 3) for large slot rarity states.
- `qualityPips_25` (symbol79) — single-frame stop; quality pip indicator graphic.
- `abilityFrame_35` (symbol122) — frame 1 stop; holds `highlight:MovieClip` and `hotkeyText:TextField` used by active module slots.

**Asset wrappers (~35 classes):** `rarity_frame_*_png` / `rarity_frame_*_large_png` / `rarity_frame_*_over_png` / `rarity_frame_*_large_over_png` (all rarity tiers: common, uncommon, rare, epic, legendary, relic, resplendent, radiant1, shadow, stellar — full and large variants with normal and hover states), `slot_large`, `itemSlot`, `reliquarySlotEmpty`, `moduleCloseButton`, `btnGreen_small`, `btnGreenIcon_small`, `btnGreenIcon_medium`, `BtnGreen`, `rarity_frame_stellar_large`, `dummy`.

---

## Notable logic

- **Object pool pattern**: `ModuleItem`, `ModulePassive`, `MegaItem`, `ModuleSelectButton`, and `StatText` are all pooled — `add*` methods reuse existing hidden instances before creating new ones, and `clear*` hides them without destroying.
- **Console selection grid**: `moveSelection` manually encodes a grid layout — active modules (column 0, mode 0) link right to passive modules (column 1, mode 1), which link down to companion swap (mode 3); reliquaries (mode 2) sit below active modules. When the swap popup (`m_moduleList`) is visible, selection is redirected entirely to the `moduleSelectList` 2-column grid.
- **Companion stat layout**: `draw()` iterates reducing line leading (0–5 px) until all stat TextFields fit above the `swapCompanion` button's y position.
- **Hotkey rendering**: on console the `hotkey["hotkeyText"]` uses `htmlText` with filters cleared; on PC it uses plain `text` and the hotkey flag is visible.
- **MegaItem lock state**: when `isLocked=true` the swap button label switches to `$ModuleLoadout_UnlockBtn`, the XP bar / item image hide, and a lock graphic appears.
