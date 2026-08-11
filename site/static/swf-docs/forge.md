# forge.swf
> The Forge crafting window, opened at Forge stations in Trove. Allows players to upgrade equipment by dragging items into a central slot, selecting an upgrade type (standard, chaos, pearl/enchanted, tome, particle), viewing required ingredients, comparing before/after stats, and confirming the upgrade. Supports four station types with different forge-mode sub-menus.

**Document/main class:** `Forge2` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 6 (main class + 5 `Forge_fla` timeline symbols + 2 asset helpers)

## Main class: `Forge2`

Large, stateful crafting UI. On PC, `ConstructForgeMenu()` is called immediately from the constructor; on console, it is deferred via `ENTER_FRAME` until `onTargetFrame()`. `ConfigureForgeMenu()` wires all mouse events, registers drag-drop, and sets up per-equipment-type `ArtClip` slots. Station type (standard / chaos / pearl-enchanted / tome+particle) is set by `SET_STATION_TYPE` and drives which forge options and icons are shown.

### Public methods

- `setStationType(type:int) : void` — sets `stationType`, calls `applyStationType()` on PC; on console deferred to post-configure.
- `setForgeType(index:int) : void` — cycles within `forgeOptions` array; calls `ExternalInterface.call("OnSelectUpgradeType", upgradeTypeCode)`.
- `addIngredient(texName, name, desc, numHave, numNeed) : void` — adds ingredient data to `ingredientList`, updates `requiredItems.ingredient_N` display (count text coloured red if insufficient), repositions slots to centre 1–3 ingredients, fires `ExternalInterface.call("OnConfigured", ...)`.
- `reset() : void` — clears `ingredientList`, disables `btn_upgrade`, hides all ingredient slots, clears before/after text.
- `setEquipmentImage(slotId, iconImage, starLevel, itemName, rarity) : void` — assigns an icon to one of the four equipment preview slots (hat/weapon/face/ring), increments `slotsEquipped`.
- `transferFromEquipment(x, y) : void` — hit-tests equipment previews at a coordinate and triggers an internal drop if found.
- `onPrevUpgradeType / onNextUpgradeType(event) : void` — cycles `currentForgeType` within `forgeOptions` with wrap-around.
- `getItemContainingPoint(point) : MovieClip` — returns `currentForge` if it hit-tests the given point; used for drag-drop targeting.

### Key fields

- `forgeType : String` — current forge mode string: `"standard"`, `"chaos"`, `"pearl"`, `"tome"`, `"particle"`.
- `stationType : int` — index into `stationTypes` / `stationNames` arrays (0=standard, 1=chaos, 3=pearl/enchanted).
- `forgeOptions : Array` — sub-modes available for this station type, e.g. `["pearl","tome","particle"]`.
- `stationTypes : Array` — `["standard","chaos",null,"pearl"]`.
- `stationNames : Array` — `["$Forge_Header_Forge","$Forge_Header_ChaosForge",null,"$Forge_Header_EnchantedForge"]`.
- `btn_upgrade : LabelButton` — craft button; label set by `SET_BUTTON_TEXT` callback; click fires `UPGRADE_REQUEST`.
- `lockButton : LabelButton` — lock/unlock button; shown via `SHOW_LOCK_BUTTON`; click fires `LOCK_REQUEST`.
- `currentForge : MovieClip` (symbol `currentForge_7`) — centre slot containing `artClip:image`, `forgeBG`, `forgeTypeIcon`, `btn_prev`, `btn_next`.
- `requiredItems : MovieClip` (symbol `requiredItems_16`) — holds up to 3 `ingredient_N` sub-clips and a `starLevel` clip.
- `statComparisonLeft / statComparisonRight : StackList` — before/after stat columns populated by `ADD_STAT_COMPARE`.
- `beforeTxt / afterTxt : TextField` — text preview for upgrades that don't use stat rows (tome level display, particle aura names, chaos warnings).
- `ingredientList : Array` — array of ingredient data objects `{slotTextureName, name, description, numHave, numNeed, txtColor}`.
- `selections : Array` — ordered list of focusable elements for console navigation (forgeBG, 3 ingredient slots, btn_upgrade, 4 equipment slots).
- `timer : Timer` — 500ms delay before showing ingredient tooltip on console hover.
- `equipmentTypes : Array` — `["item_hat","item_weapon","item_face","item_ring"]`.
- `numChaosSlots : int` — number of stats to keep on right side in chaos forge preview (default 2).
- `MAX_NUM_STATS : int` — configurable cap on stat rows (default 4, set via `setMaxStats`).

### Frame scripts / timeline

- **frame 1** (`stop()`) — PC layout.
- **frame 11** (`stop()`) — Console; sends `BG`, `header`, `requiredItems`, `btn_upgrade`, `currentForge` to `"Console"` label.
- **frame 21** (`stop()`) — Console + localisation; same children directed to `"Console"`.

### Runtime dependencies & integration

**ExternalInterface callbacks registered:** `SET_BUTTON_TEXT`, `addUpgrade`, `clearUpgrades`, `ADD_STAT_COMPARE`, `RESET_STAT_COMPARE`, `ADD_AURA_COMPARE`, `setSelected`, `setNumChaosSlots`, `setMaxStats`, `addIngredient`, `RESET_INGREDIENTS`, `setForgedItem`, `SHOW_LOCK_BUTTON`, `ENABLE_LOCK_BUTTON`, `setEquipmentImage`, `SET_STATION_TYPE`, `transferFromEquipment`, `onPrevUpgradeType`, `onNextUpgradeType`, `highlightSelection`, `unhighlightSelection`, `activateSelection`.

**ExternalInterface calls out:** `OnConfigured(numIngredients, slotsPerRow, numRows)`, `OnSelectUpgradeType(typeCode)`, `OnSetForgeFromEquipped(slotId)`, `OnDropIntoWindow(texName, rarity, qty)`, `UIComponent.OnShowTooltip(x, y, name, desc)`, `UIComponent.OnHideTooltip()`, `UPGRADE_REQUEST`, `LOCK_REQUEST`, `FORGE.REQUEST_CLOSE`, `POST_SOUND_EVENT("Play_ui_forge_use")`, `POST_SOUND_EVENT("Play_ui_window_drop_interactive")`, `POST_SOUND_EVENT("Play_ui_window_click_item")`.

**Drag-drop:** `SlotDragDropHelper.registerDropCallback(onItemDropped)` — receives external drag drops; equipment preview items use native `startDrag`/`stopDrag` with `MouseEvent.MOUSE_UP` completion.

**Visual effects:** `DropShadowFilter` + `GlowFilter` applied to `currentForgeTitle` for rarity ≥ 7 (shadow=4 relic/radiant). `GlowFilter` (yellow) applied to highlighted selection.

**Translate keys:** `$Forge_Header_Forge`, `$Forge_Header_ChaosForge`, `$Forge_Header_EnchantedForge`, `$Forge_Button_Upgrade`, `$Forge_Add_Stat`, `$Forge_Boost_Stat`, `$Forge_Cannot_Boost_Stats`, `$Forge_Required_Level`, `$Forge_Required_Level_Max`, `$Forge_Cannot_Add_Particles`, `$Forge_Cannot_Chaos`, `$Forge_Legend_Set`.

**Stage resize:** `onStageResized` scales `backBanner.height = stageHeight/scaleY * 1.5`.

---

## Other game-specific classes

### `Forge_fla.currentForge_7` (extends `MovieClip`) — Embed symbol142
Centre forge slot panel. Children: `artClip:image` (the item being forged), `forgeBG:MovieClip` (driven by `gotoAndStop(forgeType)`), `forgeTypeIcon:MovieClip`, `btn_prev`, `btn_next`. Four-frame stop scripts (PC/console layout variants).

### `Forge_fla.requiredItems_16` (extends `MovieClip`) — Embed symbol158
Ingredient display tray with `ingredient_0/1/2:MovieClip` and `starLevel:MovieClip`. Sends ingredient sub-clips to `"Console"` label on frame 11.

### `Forge_fla.btn_skip_11` (extends `MovieClip`) — Embed symbol127
Four-state button clip (up/over/down/disabled) used as the skip/prev/next navigation button.

### `Forge_fla.bannerTop_2` / `Forge_fla.bannerBottom_5` / `Forge_fla.forgeBG_8` / `Forge_fla.forgeBG_chaos_10` / `Forge_fla.mainBG_6` / `Forge_fla.arrow_chaos_22` / `Forge_fla.forgeType_12` / `Forge_fla.starLevel_17` / `Forge_fla.itemSlot_24` / `Forge_fla.requiredItem_19`
Timeline symbol classes for background panels, decorative arrows, forge-type icons, star-level indicators, and item slot frames. All are pure layout/animation clips with `stop()` frame scripts; no game logic.

### Asset helpers

- `image` (extends `_kiwi.Controls.ArtClip`) — Embed symbol78; the item-art display in the centre forge slot (`currentForge.artClip`).
- `btnGreen` (extends `LabelButton`) — Embed symbol76; green action button with four-state frame stops.
- `rarity_frame_*` (×12 PNG/asset wrappers) — rarity border graphics for ingredient slots.
- `dummy` — not present in this SWF (contrast with minigamescorecard).

## Notable logic

- **Stat comparison columns:** `ADD_STAT_COMPARE(isLeft, statName, statValue, color)` adds a `StatRow` to either `statComparisonLeft` (current stats) or `statComparisonRight` (projected stats). For chaos forge, the right column is computed in `setChaosForgeRightStatRows`: keeps `numChaosSlots` existing stats and pads the rest with `"???"` rows to show randomisation.
- **Aura/particle preview:** `ADD_AURA_COMPARE(oldAura, newAura)` stores strings in `beforeTxt`/`afterTxt` using HTML `<font size=18>`.
- **Tome preview:** shows current required level vs. reduced level in `beforeTxt`/`afterTxt` as HTML `<font size=14>`.
- **Ingredient display:** amounts > 5 digits shown as `"*"` to avoid overflow. Insufficient amounts shown in `#FF3333` red.
- **Console tooltip timer:** a 500ms `Timer` fires `showTooltip` after highlighting an ingredient slot; unhighlighting cancels the timer and calls `UIComponent.OnHideTooltip`.
- **`IsNX()` branch:** NX Switch-specific layout call to `PositioningButtons` for repositioning `buttonLegend`.
