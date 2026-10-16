# charsheet.swf
> The Character Sheet window in Trove, opened from the main HUD. It shows the player's equipped gear slots, class abilities, stats, name/title, patron status, XP/mastery progress, and houses four tabs: Character, Gems, Rewards (Mastery), and PVP.

**Document/main class:** `CharSheet` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 17 (excluding asset wrappers and framework files)

---

## Main class: `CharSheet`

`CharSheet` is the root document class for the character sheet window. The constructor registers three frame scripts (frames 1, 11, 21), initialises all gear `Slot` instances with sizes, assigns numeric slot IDs (positive on console, negative for cosmetics on PC), populates the tab array, wires the mastery and PvP reward data sources, and — when running inside Iggy — registers all `ExternalInterface` callbacks that the game engine calls to push data in.

On console, an `ENTER_FRAME` listener delays full initialisation until `onTargetFrame()` is true, then sets up navigation arrays and calls `onSlotEnter(1)`.

### Public methods

- `rewardLevel : Number` (get/set) — Returns `metaLevel` or `battleLevel` depending on which rewards panel is visible; setter routes the value to the appropriate field.
- `onEffectsClick(e:MouseEvent = null) : void` — Calls `ExternalInterface.call("OpenEffectsViewer")`.
- `ConfirmPressed() : *` — Console confirm handler; delegates to Trove/Geode mastery button click or `masteryRewards.ConfirmPressed()`.
- `setName(name:String) : void` — Sets `header.title`.
- `addStat(id:int, label:String, value:String, desc:String, dimmed:Boolean = false) : *` — Creates a `StatRow` and appends it to `statList1` or `statList2` (overflows after 20 rows); tracks it in `rows` Dictionary by id.
- `updateStat(id:int, value:String, desc:String, dimmed:Boolean = false) : *` — Looks up the `StatRow` in `rows` and updates value/description; sets color to `RED` if `dimmed`, else `WHITE`.
- `setClassInfo(className:String, classLevel:Number, paragon:Number, totalLevel:Number, isTrial:Boolean, shieldFrame:int, wingsFrame:int) : void` — Formats `$CharSheet_LevelClass` string from the class level, appends `$ClassNameTrialSuffix` if trial, writes `totalLevel` to `totalLevelTextField`, updates power-rank icon frames. The third argument is the class's Paragon total, which the stock screen does not display.
- `setAbility(slot:uint, iconImage:String, displayName:String, description:String, type:String, isActive:Boolean) : void` — Populates one of the five `ArtClip` ability icons; slot 4 (sub-class) also hides `noSubClassAnim`. `displayName` is the ability's own name; `type` is a label for the slot (`SubClass` on the sub-class slot, whatever the character), unused by the stock screen. `isActive` picks frame 2 of `abilityFrameN`.
- `lockSubClassAbility() : void` — Hides `noSubClassAnim` and disables `abilityFrame5`.
- `resetSubClassAbility() : void` — Shows `noSubClassAnim` and re-enables `abilityFrame5`.
- `addNameColor(label:String, id:int) : void` — Adds an item to `m_nameColorComboBox`.
- `selectNameColor(id:int, color:uint) : *` — Selects matching item in combo box; sets `header.winTitleTextField.textColor`.
- `HandleDirectionInput(dir:int) : *` — Console directional navigation for the name-color combo box.
- `setMetaRewardSection(trackId:int, sectionId:int, label:String, desc:String, expandable:Boolean, collapsed:Boolean) : void` — Calls `masteryRewards.rewardsView.addSection(...)` and invalidates DATA.
- `ChangeMetaTrack(dir:int) : *` — Switches between Trove and Geode mastery tracks; handles both section-selected and top-level highlight states.
- `setSelectedTrack(track:int) : void` — Syncs `m_TroveMasteryBtn`/`m_GeodeMasteryBtn` checked state and mastery icon frame ("trove"/"geode"). The engine owns the selection: the buttons send `OnMasteryButton(0|1)` and the engine switches the track it reports.
- `clearMastery() : void` — Clears `masteryRewards.rewardsView` and unhighlights both mastery buttons.
- `setupMetaRewards(section:int, row:int) : void` — Marks reward view dirty, calls `masteryRewards.setupMetaRewards`, then calls `ExternalInterface.call("ShowMasteryTooltip", ...)`.
- `setupPvPRewards(count:int, level:int) : void` — Calls `pvpRewards.setupRewards(count)` and sets `rewardLevel`.
- `unlockReward(index:int) : void` — Finds the `RewardTile` at `index` in the visible rewards panel and calls `setLocked(false)`.
- `setMetaLevel(isPvP:Boolean, level:Number, xpPercent:Number) : void` — Updates `levelTextField` using `$CharSheet_BattleHeader` or `$CharSheet_MasteryHeader`, resizes font, sets `xpBar.width`. `xpPercent` is a 0–1 fraction toward the next rank. For mastery the engine reports only the currently selected track (Trove or Geode), and re-sends the current rank on refreshes.
- `enableTab(tabId:int, visible:Boolean) : void` — Shows/hides a tab and invalidates STATE so tab Y positions restack.
- `onSlotEnter(slotIndex:int) : void` — Console hover highlight: shows tooltip for gear slot or applies glow/label-button/ArtClip highlight for informational items.
- `onSlotLeave(slotIndex:int) : void` — Reverses `onSlotEnter` highlight.
- `onSelectionActivated(index:int) : void` — Console confirm on a selection: calls `SLOT.ACTIVATE`, `OnShowStats`, `showSubClassSelector`, or `OpenTitlesSelector` via `ExternalInterface`.
- `scrollBarTranslate(delta:Number) : void` — Scrolls the active reward list (mastery via `HandleStickChange`, PvP via direct scrollV increment); also manages console mastery-track highlight.
- `showBoosterLegend(showBoosters:Boolean) : void` — Switches `buttonLegend` frame to "Boosters"/"Gems" (with "Loc" suffix for non-EN/ZH locales).

### Key fields

- `rows : Dictionary` — Maps stat id (int) → `StatRow` instance for `updateStat` lookups.
- `gearSlotArray : Array` — Ordered list of all 18 gear `Slot` instances used for console navigation. No callback carries gear: the engine fills these slots by writing into the timeline instances by name (`gearHat`, `gearWeapon`, `skin`, …), so the instance names are part of the contract.
- `powerRankTextField`, `m_totalMastery : TextField` — Written by the engine directly, not through a callback: the power-rank figure, and the total mastery rank shown on the Rewards tab (the engine also rewrites its formatting). The stock code only positions, shows and hides them.
- `informationalSelections : Array` — Secondary navigation targets (patronBadge, powerRankIcon, totalLevelTextField, stat/effects buttons, ability icons, editNameColor).
- `tabs : Array`, `tabOrigin : Array` — Tab MovieClips and their original Y positions; recompacted when tabs are hidden.
- `masteryRewards : MetaRewards`, `pvpRewards : PvPRewards` — Sub-component MovieClips embedded from `_assets/assets.swf`.
- `masteryDataSource : MetaRewardRowExternalDataSource`, `pvpDataSource : PvPRewardRowExternalDataSource` — Pull data via Iggy external calls (`GetMetaRowData`/`GetPvPRowData`). The engine answers inside the call by writing the row's fields into the source's shared public `rowData`, which `getData` copies into the view and then clears, one row at a time.
- `patronStatus : Boolean`, `patronOnTitle/Content : String` — Patron badge display and tooltip strings (translated from `$CharSheet_PatronActive`, `$CharSheet_PatronBonuses`).
- `level, xpPercent, metaLevel, battleLevel : Number` — Track current level display state.
- `m_currentTrack, m_highlightedTrack : int` — Active and console-highlighted mastery track (0 = Trove, 1 = Geode).
- `xpBarMaxWidth : Number` — Cached full width of `xpBar`; used to scale bar by `xpPercent`.

### Frame scripts / timeline

- **frame 1** (`frame1`, label `PC`) — `stop()`. Default stopped state.
- **frame 11** (`frame11`, label `Console`) — `stop()`. Second stopped state (console variant).
- **frame 21** (`frame21`, label `ConsoleLoc`) — `stop()`; calls `patronStatsBubble.gotoAndStop("ConsoleLoc")`. Console localised patron bubble.

### Runtime dependencies & integration

**ExternalInterface callbacks registered (Iggy receives from engine):**
`addNameColor`, `selectNameColor`, `MoveDropdown`, `addStat`, `updateStat`, `setName`, `setClassInfo`, `setAbility`, `lockSubClassAbility`, `resetSubClassAbility`, `setTitle`, `setPatronStatus`, `showTab`, `enableTab`, `setupPvPRewards`, `setupMetaRewards`, `setMetaRewardSection`, `unlockReward`, `setMetaLevel`, `clearMastery`, `setSelectedTrack`, `ChangeMetaTrack`, `masteryRewards.CancelPressed`, `masteryRewards.ConfirmPressed`, `onSlotEnter`, `onSlotLeave`, `onSelectionActivated`, `onStatsClick`, `onEffectsClick`, `scrollBarTranslate`, `showBoosterLegend`, `UIComponent.onStageResized`

**ExternalInterface calls (Flash → engine):**
`OnMasteryButton` (0 = Trove, 1 = Geode), `showSubClassSelector`, `OpenTitlesSelector`, `OnSelectNameColor`, `OnShowTab` (0 = Character, 1 = Gems, 2 = Rewards, 3 = PvP), `POST_SOUND_EVENT`, `ShowMasteryTooltip`, `ShowPowerRankTooltip`, `ShowPowerRankBreakdown`, `HideTooltip`, `OpenEffectsViewer`, `CloseWindow`, `CheckEmotesCount`, `OnDropIntoWindow`, `SLOT.ACTIVATE`, `OnShowStats`, `showColorNameComboBox`, `SetMenuInformation` (console init), `PositioningButtons` (NX only)

**Translate keys:** `$CharSheet_PatronActive`, `$CharSheet_PatronBonuses`, `$CharSheet_LevelClass`, `$ClassNameTrialSuffix`, `$CharSheet_BattleHeader`, `$CharSheet_MasteryHeader`, `$CharSheet_Stats`, `$CharSheet_Effects`, `$CharSheet_Trove`, `$CharSheet_Geode`

**Drag-and-drop:** `SlotDragDropHelper.registerDropCallback(onDrop)` — hit-tests dropped items against a subset of gear slots (`gearCart`, `gearFace`, `gearHat`, `gearMount`, `gearPet`, `gearRing`, `gearWeapon`, `gearEdible`, `appearanceWeapon`, `appearanceHat`, `appearanceFace`, `skin`, `gearPVPBanner`, in that order) and calls `OnDropIntoWindow(sourceEntityId, sourceComponent, sourceSlotId, slotName, extra)`, reporting the target by its instance name. The three `appearance*` slots are declared but not placed on the live timeline.

**Locale branching:** `buttonLegend` frame names get "Loc" suffix for non-EN/non-ZH locales. `setButtonLegendFrameByTabId` uses a 150 ms `Timer` to trigger NX button repositioning after frame change.

**Stage resize:** Overrides `onStageResized` to scale `backBanner.height` proportionally.

---

## Other game-specific classes

- `MetaRewards` (extends `UIComponent`) — [Embed symbol260] Hosts a `RowView` for mastery/PvP reward tiles. Manages two-level console selection (section mode / row mode) via `HandleStickChange`; `ConfirmPressed`/`CancelPressed` toggle modes and can call `ExternalInterface.call("CloseWindow")`. Drives scroll position calculation based on section heights.
- `PvPRewards` (extends `MetaRewards`) — [Embed symbol258] Thin subclass; adds frame 2 stop script.
- `RowView` (extends `DynamicRowView`) — [Embed symbol259] Virtualised reward tile list. `setupRewards` configures for mastery (section-based) or PvP (flat single section). `UpdateSelectedObject` highlights tiles and triggers `ShowMasteryRewardTooltip`. `onHeadingClick` collapses all other sections when one opens.
- `RewardTile` (extends `DynamicRowViewRow`) — [Embed symbol109] Single reward row: icon `Slot`, `RewardLevel` badge, description `TextField`. `setLocked` applies `GhostedFilter`. Tooltip calls `ShowMasteryRewardTooltip`/`HideTooltip` on hover.
- `RewardLevel` (extends `UIComponent`) — [Embed symbol106] Tiny badge showing the integer mastery level required. `setLevel(n)` writes to `levelText`.
- `MetaRewardSection` (extends `DynamicRowViewSection`) — [Embed symbol133] Section header for mastery reward rows; shows/hides a `lockedIcon` child via `expandable` setter and a `highlight` overlay.
- `MetaRewardRowData` — Plain data-transfer object: `level`, `description`, `textureName`, `locked`, `hideReward`.
- `MetaRewardRowExternalDataSource` (extends `DynamicRowViewExternalDataSource`) — Pulls row data via Iggy external function `GetMetaRowData` / `GetMetaSectionData`.
- `PvPRewardRowExternalDataSource` (extends `DynamicRowViewExternalDataSource`) — Pulls row data via Iggy external function `GetPvPRowData`.
- `MasteryBtn` (extends `LabelButton`) — [Embed symbol212] Multi-state button (9 frame-stop states at frames 10, 20, 30, 40, 50, 60, 70, 80, 90).
- `SubClassAbilityFrame` (extends `BaseButton`) — [Embed symbol282] Four-state button frame for the sub-class ability slot (frames 1, 11, 21, 31).
- `PvPRowView` (extends `RowView`) — [Embed symbol250] Thin subclass; re-applies vScrollbar component inspector settings.
- `Image` — Thin asset-wrapper clip (not detailed further).
- `dummy` — Placeholder class, no logic.

**Asset-wrapper skin classes (16 total):** `CellRenderer_*Skin` (7 states), `ComboBox_*Skin` (4 states), `ScrollArrow*_*Skin` (8), `ScrollThumb_*Skin` (3), `ScrollTrack_skin`, `List_skin`, `focusRectSkin`, `slot_large`, `rewardBGPVP`, `btnArrowLeft`, `btnGreen_small`, `btn_console_analog_top_left`, `btn_pencil`.

**Rarity-frame bitmap assets (44 total):** `rarity_frame_<tier>_png` and `rarity_frame_<tier>_large_png` / `*_over_png` for tiers: common, uncommon, rare, epic, legendary, relic, crystal, mystic, radiant1, resplendent, shadow, stellar (some tiers also have non-png MovieClip variants: `rarity_frame_stellar`, `rarity_frame_stellar_large`).

**CharSheet_fla timeline symbols (21):** `bannerTop_2`, `bannerBottom_5`, `innerBackground_6`, `slotFrame_10`, `equiped_11` (×2 variants), `equipped_13`, `slotFrameLarge_23`, `equipedLarge_24`, `abilityFrame_29`, `btn_legend_41`, `tabHeader_mastery_72`, `tabHeader_char_73`, `tabHeader_pvp_74`, `tabHeader_gems_75`, `masteryIconMedium_80`, `statScreen2_103`, `patronpassicon_105`, `statScreen_26`, `CollapsedIcon_108`, `subcategoryHeader_107`.

---

## Notable logic

- **Tab visibility reflow:** `enableTab` hides/shows tabs and invalidates STATE; the `draw()` override re-assigns Y positions from `tabOrigin` skipping invisible tabs, keeping them visually contiguous.
- **Dual stat columns:** `addStat` places up to 20 rows in `statList1`, then overflows to `statList2`; `statList2.x` is pinned to `statList1.x` in `draw()`.
- **Console vs PC slot IDs:** Gear slot `.data` values differ in sign convention (console uses 1-based positive, PC uses 0 for skin and negative for cosmetics). The engine reads these IDs back via `SLOT.ACTIVATE`.
- **Mastery scroll auto-position:** `UpdateScrollBar` computes pixel offset through section/row heights to keep the selected item visible without a standard scroll event.
- **Button legend locale branching:** NX platforms additionally call `PositioningButtons` after a 150 ms timer, triggered by tab changes.
