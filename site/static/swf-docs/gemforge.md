# gemforge.swf
> The Gem Forge crafting window, opened at a Gem Forge station. Lets players drag gems into 12 inventory slots and one central upgrade slot, select which gem stat to augment/reroll/move, choose a booster/insurance item, review required crafting ingredients, and execute the upgrade.

**Document/main class:** `GemForge` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 14 (excluding slot-skin stubs and `dummy`)

---

## Main class: `GemForge`

Root window component. Manages 12 gem inventory slots (`gemSlot0`–`gemSlot11`), one upgrade target slot (`upgradeSlot`), 3 stat display rows, 5 booster/insurance item slots, and a required-ingredients panel. Constructor sets slot sizes (small=55px for slots at index % 3 != 2, large=77px for every 3rd slot, xlarge=100px for `upgradeSlot`), assigns colour frames (blue/yellow/red/opal cycling per group of 3 slots), initialises `requiredItems`, wires ingredient/insurance tooltip listeners, and on console builds a full `DirectionalMapping` graph for D-pad navigation. `configUI()` selects the correct augment button variant (PC vs. console), registers `SlotDragDropHelper.registerDropCallback`, and registers `ExternalInterface` callbacks.

### Public methods

- `previewUpgrade(gemName:String, level:int, powerRank:int, quality:int, rarity:int, isRepair:Boolean, canAugment:Boolean, needsRepair:Boolean) : void` — updates the upgrade slot's rarity/quality, clears and re-enables stat rows, updates `txtChooseStat` visibility, and sets `augmentButton.label` to one of `$RepairLabel`, `$GemForge_RerollStat`, `$GemForge_MoveStat`, or `$GemForge_AugmentStat` depending on mode. Drives booster slot frames (frame 1=repair mode, frame 2=augment mode).
- `addStat(name:String, value:String, roll:String, boost:int) : void` — appends a stat entry to `statList`, then refreshes all visible `GemStatRow` instances (text fields, boost pip visibility, roll-details HTML text). In REROLL_SLOT mode colours stats cyan (`0xC2FFFF`); otherwise green name / teal value.
- `addIngredient(icon:String, name:String, description:String, numHave:Number, numNeed:Number) : *` — appends to `ingredientList`, shows `requiredItems.requiredLabel`, and updates `reqText0`/`reqText1` with HTML colour-coded have/need counts (red `#FF3333` if insufficient, white if sufficient). Sets `haveIngredients` flag; enables `augmentButton` if `canAugment && haveIngredients`.
- `resetIngredients() : void` — clears `ingredientList` and hides all `reqImage`/`reqText` children.
- `clearForge() : void` — disables augment button, resets `selectedStat`, fires `ExternalInterface.call("OnSelectStat", -1)`, clears stat list, hides stats panel, resets upgrade slot rarity/quality to 0.
- `selectAugment(index:int) : void` — updates which booster/insurance slot has `equipped=true`, changes `augmentButton.label` to match the mode (augment/reroll/move).
- `setAugmentInfo(index:int, icon:String, name:String, description:String) : *` — sets `iconImage` and `slotImageSize=48` on the specified booster slot; stores name/description in `insuranceList` for tooltip display.
- `getCurrentSlotId() : int` — returns the `data` property of the currently console-selected element (slot index 0–12, stat row index 200–202, or booster index 100–104). Returns -1 if nothing selected.
- `moveSelection(dx:int, dy:int) : void` — console D-pad navigation; walks the `DirectionalMapping` graph, skipping invisible nodes recursively.
- `highlightSlot(focusUpgradeSlot:Boolean) : void` — forces highlight to `upgradeSlot` if param is true, otherwise re-highlights current selection.
- `activateSlot() : void` — activates the currently selected element: calls `Slot.activate()` for gem slots, `selectStat(rowIndex)` for stat rows, or `ExternalInterface.call("OnUpgradeSelected", index)` for booster slots.
- `moveGemToSlot(itemId:String, qty:int, type:int, slotIndex:int) : void` — fires `ExternalInterface.call("OnDropOntoSlot", itemId, qty, type, slotData)` for the target slot.

### Key fields

- `gemSlot0`–`gemSlot11 : Slot` — 12 gem inventory slots (data values 0–11). Slots at index % 3 == 2 are large (77px); others are small (55px). Colour frames cycle blue/yellow/red/opal.
- `upgradeSlot : Slot` — the central target gem slot (data=12, xlarge 100px, disabled by default).
- `stats : MovieClip` — container for `row0`/`row1`/`row2` (`GemStatRow` instances, rowIndex 200–202).
- `insuranceContainer : MovieClip` — container for `insuranceItem0`–`insuranceItem4` (each has a `.slot` `SlotBasic`; data values 100–104).
- `requiredItems : MovieClip` — ingredient display panel with `reqImage0`/`reqImage1`, `reqText0`/`reqText1`, `requiredLabel`, `instructions`.
- `augmentButton : LabelButton` — active button (either `pcAugmentButton` or `consoleAugmentButton` depending on platform).
- `pcAugmentButton / consoleAugmentButton : LabelButton` — platform-specific variants; only one is visible at runtime.
- `buttonSelect / trigger : MovieClip` — shown/hidden on console focus gain/loss.
- `buttonLegend : MovieClip` — controller button legend; repositioned via `PositioningButtons` on NX platform.
- `selectedStat : int` — rowIndex of the currently selected stat row (-1 if none); 200–202 range.
- `selectedAugment : *` — index of the active booster slot (0–4), or -1.
- `haveIngredients / canAugment : Boolean` — gate flags for augment button enable state.
- `ingredientList / insuranceList / statList : Array` — data caches for ingredient, booster, and stat entries.
- `currentSelection : MovieClip` — console-mode currently highlighted UI element.
- Constants: `REROLL_SLOT=3`, `MOVE_SLOT=4`, `STAT_DISPLAY_COUNT=3`, `BOOSTER_ITEM_COUNT=5`, `TOOLTIP_PADDING=10`.

### Frame scripts / timeline

- Frame 1: `stop()` — PC layout.
- Frame 11: `stop()` — Console layout.
- Frame 21: `stop()` — NX/Switch layout.

### Runtime dependencies & integration

- `ExternalInterface.addCallback` registrations: `previewUpgrade`, `addStat`, `addIngredient`, `resetIngredients`, `clearForge`, `selectAugment`, `setAugmentInfo`, `getCurrentSlotId`, `fuseGems`, `moveSelection`, `highlightSlot`, `activateSlot`, `moveGemToSlot`, `LoseFocus`, `GainFocus`.
- Outbound `ExternalInterface.call`: `OnUpgradeRequest()`, `OnSelectStat(statIndex)`, `OnDropOntoSlot(itemId, qty, type, slotData)`, `OnUpgradeSelected(boosterIndex)`, `UIComponent.OnShowTooltip(x, y, name, desc)`, `UIComponent.OnHideTooltip()`.
- `SlotDragDropHelper.registerDropCallback` — receives drag-drop events from the shared drag system; `onDrop` hit-tests against all gem slots and the upgrade slot.
- `IggyFunctions.translate` — `$GemInstructions_empty`, `$Forge_Gems`, `$Level_X`, `$NoRarityPowerRank`, `$GemPreviewName{n}`, `$GemPreviewState{n}`.
- `DirectionalMapping` — console D-pad navigation graph; manually defined adjacency between all 13 gem slots, 3 stat rows, and 5 booster items.
- `GlowFilter` — applied to booster/insurance items on console highlight (color `0xCCCC00`, blurX/Y=2, strength=100).
- `PositioningButtons` — NX-specific helper to reposition `buttonLegend` relative to `backBanner`.

---

## Other game-specific classes

- `GemStatRow` (extends `BaseButton`, embeds `symbol47`) — stat display row. Fields: `statName`, `statValue`, `rollDetails` (TextFields), `selectedIndicator`, `boost0`–`boost4` (pip MovieClips), `rowIndex`. `selectStat` setter shows/hides `selectedIndicator`. `toggleHighlight` applies a hover glow. `pulsing` setter animates the indicator via `IggyTween` (console "choose a stat" prompt). 6 frame stops (1, 10, 11, 20, 30, 40).
- `GemForge_fla` timeline clips:
  - `stats_45` (symbol163) — `row0`/`row1`/`row2` GemStatRow container.
  - `insurance_8` (symbol153) — `insuranceItem0`–`insuranceItem4` container; frame 11 sends items 0–3 to "Console" frame.
  - `required_items_16` (symbol161) — ingredient panel with `requiredLabel`, `reqText0/1`, `reqImage0/1/2`, `instructions`. Two frames.
  - `power_rank_49` (symbol168) — `powerRankValue:TextField`. Two frames.
  - `color_frame_large_20` (symbol98) — 4-frame gem colour state indicator (blue/yellow/red/opal).
  - `triggerLegend_54` (symbol173) — `textField:TextField`, two frames (PC/console).
  - `equipped_39` (symbol57) — 2-frame equipped toggle.
  - `bannerTop_2` / `bannerBottom_5` (symbols 139/142) — animated frame banners (15-frame animations).
  - `animatedHighlightLarge_24` / `animatedHiglight_32` (symbols 103/87) — animated selection highlight clips (6-frame loops).
  - `qualityPips_43` (symbol71) — quality pip display, stops at frame 1.
  - `slotFrameLarge_23`, `slotFrame_31`, `slot_frame_small_29`, `slotFrameXL_36` — slot frame border variants.
- Asset wrappers (9): `Equipped` (symbol13), `SlotBackground` (symbol123), `SlotBackgroundLocked` (symbol10), `SlotFrameHigh` (symbol8), `SlotFrameMedium` (symbol5), `SlotFrameNormal` (symbol2), `image` (ArtClip, symbol106), `art` (ArtClip, symbol64), `slot`/`slot_large`/`slot_xlarge` (Slot subclasses), `btnGreen`/`btnGreenWide` (LabelButton, 4-state), `dummy` (BitmapData placeholder).

---

## Notable logic

- **Slot size and colour assignment:** gem slots are sized small (55px) or large (77px) based on `index % 3 == 2`. Colour (`slotColor` child MovieClip label) cycles in groups of 3: 0–2 = blue, 3–5 = yellow, 6–8 = red, 9–11 = opal.
- **Ingredient colour coding:** `addIngredient` renders `numHave/numNeed` in HTML; insufficient quantities appear in red (`#FF3333`), sufficient in white. The augment button is only enabled when all ingredients are sufficient AND `canAugment` is true.
- **Augment mode button labels:** `selectAugment` and `previewUpgrade` cooperate to set the correct label. Repair takes priority; then REROLL_SLOT (index 3) = reroll; MOVE_SLOT (index 4) = move; otherwise = augment.
- **Console D-pad graph:** the adjacency graph is fully hardcoded in the constructor. Navigation skips invisible nodes recursively. `highlightSelection` shows a slot tooltip at `root.width + 10` for gem slots, toggles `GemStatRow.toggleHighlight`, or applies a `GlowFilter` for booster items.
- **Drag-drop integration:** `onDrop` uses `hitTestPoint` against slots 0–8 and the upgrade slot (notably slots 9–11 are not in the drag-drop hit-test list, likely intentional design).
