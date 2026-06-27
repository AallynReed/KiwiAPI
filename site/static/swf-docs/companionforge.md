# companionforge.swf

> The Companion Forge is the UI panel for upgrading companion items in Trove. It displays the currently-forged item (name, rarity, star rating), lists required crafting ingredients, shows available skill upgrades in a scrollable list, and presents one or more upgrade buttons the player can activate to spend resources and level up the companion.

**Document/main class:** `CompanionForge` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 13

---

## Main class: `CompanionForge`

`CompanionForge` is the root UIComponent for the entire panel. It owns every sub-widget, wires all `ExternalInterface` callbacks the game engine calls, and forwards user actions back via `ExternalInterface.call`. It also handles console/NX-specific controller navigation across three selection zones (upgrade buttons, ingredient slots, skill list).

### Key fields

| Field | Type | Role |
|---|---|---|
| `m_header` | `MovieClip` | Window header bar; title set via `setHeaderName()` |
| `m_levelUpAnim` | `MovieClip` | Level-up celebration animation (hidden until triggered) |
| `m_maxLevelAnim` | `MovieClip` | Max-level celebration animation (hidden until triggered) |
| `m_upgradeBtn` | `LabelButton` | Primary upgrade button (always present; label `$Forge_Button_Upgrade`, sound `Play_ui_forge_use`) |
| `m_upgradeBtn1/2/3` | `LabelButton` | Additional upgrade buttons for multi-choice forge mode (hidden by default) |
| `currentItemTitle` | `TextField` | Displays item name; initially `"$Forge_Legend_Select"` |
| `currentItemRarity` | `TextField` | Displays rarity text; rainbowified on rarity 8 |
| `backBanner` | `MovieClip` | Full-height background banner; resizes to `stageHeight × 1.5` |
| `m_buttonLegend` | `MovieClip` | Console button-legend overlay (shown/hidden on focus) |
| `skillsView` | `SkillsView` | Scrollable `DynamicRowView` listing skill upgrades |
| `frameCircle` | `MovieClip` | Circle frame around the item preview; contains a `rarity` sub-MovieClip driven by `gotoAndStop(rarityIndex)` |
| `skillsHighlight` | `MovieClip` | Highlight overlay for the skill list (NX controller nav) |
| `m_itemPreview` | `MovieClip` | Item art preview; mouse-over/out triggers `ShowTooltip`/`HideTooltip` calls |
| `skillDataSource` | `SkillsRowExternalDataSource` | Data adapter wired to `skillsView` |
| `m_starList` | `Vector.<MovieClip>` | Dynamically grown list of `togglestar` instances showing star rating |
| `m_ingredientList` | `Array` | Dynamically grown pool of `RequiredMaterial` instances |
| `m_quickLinkList` | `Array` | Dynamically grown pool of `QuickLinkItem` instances (up to 8, two columns) |
| `m_selectedUpgrade` | `int` | Currently highlighted multi-upgrade index (0–2) |
| `m_selectMode` | `int` | Controller nav zone: 0 = upgrade button, 1 = ingredients, 2 = skill list |
| `m_upgradeForge` | `Boolean` | True when multi-choice upgrade mode is active |
| `m_upgradeButtonIndex` | `int` | Which of the multi-choice buttons is highlighted |
| `m_numberOfUpgradeButtons` | `int` | Count of active upgrade buttons (2 or 3) |
| `adjustHeightMultiplier` | `Number` | Used when vertically aligning the header title (default 0) |
| `adjustHeightAdditive` | `Number` | Additive offset for header title alignment (default 21) |

Layout constants: `QUICKLINK_LEFTX = 16.65`, `QUICKLINK_RIGHTX = 310`, `QUICKLINK_Y = 208.5`, `QUICKLINK_VERTICAL_PADDING = 3`, `SKILL_OFFSET = 76.2`, `SKILL_PADDING = 6`, `ITEM_OFFSET = 90.38`, `INGREDIENT_WIDTH = 60`, `SCROLL_BUFFER = 3`.

### Frame scripts / timeline

The main class timeline has three labelled stops registered in the constructor:

| Frame index | Script |
|---|---|
| 0 (frame 1) | `stop()` |
| 10 (frame 11) | `stop()` |
| 20 (frame 21) | `stop()` |

A `FRAME_CONSTRUCTED` listener (`__setProp_handler`) applies `m_header` component-inspector properties conditionally for frames 1–20 and 21–30, covering two header-state ranges. The 3D projection is set once on `ADDED_TO_STAGE`: field-of-view 40.6°, projection centre `(195, 534)`.

### Runtime dependencies & integration

**ExternalInterface callbacks registered (game engine → Flash):**

| Callback name | Method | Description |
|---|---|---|
| `setHeaderName(name)` | `setHeaderName()` | Sets window title text, auto-resizes font |
| `setItemName(name)` | `setItemName()` | Updates item name field, resets ingredient/skill selection indices |
| `setStarCount(total, filled)` | `setStarCount()` | Creates/positions `togglestar` instances; `filled` controls how many show as lit |
| `setRarity(index)` | `setRarity()` | Drives `frameCircle.rarity.gotoAndStop(index)`; index 8 triggers rainbow text effect |
| `setupSkillStats(numSkills, activeSkillIndex)` | `setupSkillStats()` | Clears and repopulates `skillsView` with a single section; auto-shows/hides scrollbar; scrolls to active skill |
| `clearIngredients()` | `clearIngredients()` | Hides all ingredient slots (pool preserved) |
| `addIngredient(icon, owned, needed, name, desc)` | `addIngredient()` | Activates/creates a `RequiredMaterial` slot scaled to 0.7, triggers layout invalidation |
| `clearQuickLinks()` | `clearQuickLinks()` | Hides all quick-link items, resets `IndexReference = -1` |
| `addQuickLink(index, icon, label, desc)` | `addQuickLink()` | Activates/creates a `QuickLinkItem`; first four go in left column, next four in right column |
| `GainedFocus()` | `GainedFocus()` | Shows button legend, re-highlights selection (NX: calls `PositioningButtons`) |
| `LostFocus()` | `LostFocus()` | Hides button legend, removes highlight |
| `ActivateSelection()` | `ActivateSelection()` | Fires `requestUpgrade` — same as clicking the upgrade button |
| `moveSelection(dx, dy)` | `moveSelection()` | D-pad navigation across the three selection zones |
| `setupUpgradeForge(numButtons)` | `setupUpgradeForgeButtons()` | Enables multi-choice mode; wires `MOUSE_OVER` on extra buttons to call `SetSelectedUpgrade` |

**ExternalInterface calls (Flash → game engine):**

| Call | Trigger |
|---|---|
| `UPGRADE_REQUEST` | Upgrade button clicked (any of the four buttons) |
| `SetSelectedUpgrade(index)` | Mouse-over on one of the multi-choice upgrade buttons; also on controller navigation between them |
| `OnDropOntoSlot(itemType, itemId, quantity)` | Item dragged and dropped onto `frameCircle` hit area |
| `ShowTooltip(x, y)` | Mouse-over on `m_itemPreview` |
| `HideTooltip()` | Mouse-out on `m_itemPreview` |
| `TOOLTIP.HIDE` | Controller navigation leaves ingredient or skill zone |
| `QuickLinkClicked(index)` | User clicks a `QuickLinkItem` (delegated from that class) |

**Drag-and-drop:** `SlotDragDropHelper.registerDropCallback(onDrop)` — accepts drops anywhere on the component; only forwards to engine if the drop point hits `frameCircle`.

**Translate keys observed:** `$Forge_Legend_Select` (item title placeholder), `$Forge_Button_Upgrade` (all upgrade buttons), `$Clubs_Upgrade` (console confirm-legend label).

**Sound event:** `Play_ui_forge_use` — assigned as `clickSoundEvent` on all four upgrade buttons.

---

## Other game-specific classes

### Top-level game classes

- `SkillsView` — extends `_kiwi.Controls.DynamicRowView` [Embed symbol137]. Scrollable list for skill rows. Overrides `setRowData` to optionally force font size (`m_ForceFontSize`) or left-align (`m_ForceAlignLeft`) across all visible rows via `editVisibleRows`. Its embedded `vScrollbar` targets the named clip `rewardsView`.

- `SkillRow` — extends `_kiwi.Controls.DynamicRowViewRow` [Embed symbol39]. One row in the skill list. Fields: `skillName:TextField`, `m_upgradeTag:MovieClip`, `levelUp:MovieClip`. `setData()` applies three colour states — active (0xE92768, gold-ish), offered (0xC6FA06, green-ish), or locked (0x185129, dark green) — and shows/hides the `levelUp` animation and `m_upgradeTag` label. `SetTextPosition()` adjusts `skillName.y` based on line count (single-line: y=3, multi-line: y=−5).

- `SkillsRowData` — plain data struct (no framework base). Fields: `skillString:String`, `skillisActive:Boolean`, `skillisOffered:Boolean`, `tagLabel:String`, `playAnim:Boolean`. `clear()` resets all fields.

- `SkillsRowExternalDataSource` — extends `_kiwi.Controls.DynamicRowViewExternalDataSource`. Holds one reusable `SkillsRowData rowData` instance; `getData()` calls `rowView.setRowData()` then clears `rowData` for reuse.

- `RequiredMaterial` — extends `_kiwi.Core.UIComponent` [Embed symbol45]. Single ingredient slot. Fields: `txt_cost:TextField`, `artClip:ArtClip`. `Setup(icon, owned, needed, name, desc)` loads the icon into `artClip` at 80×80 px, sets tooltip strings, and triggers a data-invalidation redraw. `draw()` renders `owned/needed` (truncates owned to `*` if > 9999); text is white when met, red when short.

- `QuickLinkItem` — extends `MovieClip` [Embed symbol53]. Shortcut link slot. Fields: `textField:TextField`, `image:ArtClip` (resolved from `imageContainer.artClip`), `IndexReference:int`. On click calls `ExternalInterface.call("QuickLinkClicked", IndexReference)`.

- `togglestar` — extends `MovieClip` [Embed symbol18]. Two-state star icon with a `filled:MovieClip` child; visibility of `filled` controlled by `setStarCount()` in main class.

- `image` — extends `_kiwi.Controls.ArtClip` [Embed symbol40]. Thin asset-wrapper subclass used as the item-slot art clip inside `itemSlot_72`.

- `btnGreen` — extends `_kiwi.Controls.LabelButton` [Embed symbol163]. Green-styled button with 8 frame stops (frames 10, 20, 30, 40, 50, 60, 70, 80) covering button states (up, over, down, disabled, and toggle variants).

- `dummy` — extends `BitmapData` [Embed `58_dummy.png`]. 48×48 placeholder bitmap.

### `CompanionForge_fla` timeline symbols

- `bannerTop_2` — MovieClip [Embed symbol173]. Animated top banner; plays to frame 15 then stops.
- `bannerBottom_5` — MovieClip [Embed symbol176]. Animated bottom banner; plays to frame 15 then stops.
- `currentForge_7` — MovieClip [Embed symbol219]. The circular item-preview frame containing `SelectTxt:TextField` and `rarity:MovieClip`. Four frame stops (frames 1, 10, 20, 31) for different display states.
- `FrameRarities_8` — MovieClip [Embed symbol215]. Rarity frame art strip; stops on frame 1 (driven externally by `gotoAndStop(rarityIndex)`).
- `itemSlot_72` — MovieClip [Embed symbol50]. Two-frame slot (frame 1 = empty, frame 2 = filled); contains `artClip:image`.
- `skillOn_Anim_75` — MovieClip [Embed symbol33]. Skill-activated flash animation; plays to frame 15 then stops.

### Asset wrapper classes (pure image embeds, no logic)

9 rarity frame PNGs: `rarity_frame_common_png`, `rarity_frame_uncommon_png`, `rarity_frame_rare_png`, `rarity_frame_epic_png`, `rarity_frame_legendary_png`, `rarity_frame_relic_png`, `rarity_frame_resplendent_png`, `rarity_frame_shadow_png`, `rarity_frame_stellar_png`, `rarity_frame_mystic_png`, `rarity_frame_radiant1_png`.

ScrollBar UI skin classes (11): `ScrollArrowDown_disabledSkin`, `ScrollArrowDown_downSkin`, `ScrollArrowDown_overSkin`, `ScrollArrowDown_upSkin`, `ScrollArrowUp_disabledSkin`, `ScrollArrowUp_downSkin`, `ScrollArrowUp_overSkin`, `ScrollArrowUp_upSkin`, `ScrollThumb_downSkin`, `ScrollThumb_overSkin`, `ScrollThumb_upSkin`, `ScrollTrack_skin`, `focusRectSkin`.

---

## Notable logic

- **Ingredient layout:** Computed in `draw()` on `STATE` invalidation. Ingredients are centred horizontally below the header. On NX, odd-indexed ingredients are nudged +10 px vertically and even-indexed −10 px, creating a staggered layout.

- **Quick-link layout:** Up to 8 slots arranged in two fixed columns (x = 16.65 left, x = 310 right) starting at y = 208.5, spaced by `height + 3 px`. Items are pooled and reused across `clearQuickLinks`/`addQuickLink` calls.

- **Multi-choice upgrade mode:** When `setupUpgradeForge(n)` is called (n = 2 or 3), extra upgrade buttons become visible and wired. Controller left/right navigation cycles `m_upgradeButtonIndex` and immediately fires `SetSelectedUpgrade(index)` to keep the engine in sync. Mouse hover also triggers `SetSelectedUpgrade`.

- **Rarity rainbow effect:** `setRarity(8)` calls `KiwiTextUtil.rainbowify(currentItemRarity)` — the only use of the rainbow text utility in this SWF, reserved for the highest rarity tier (rarity index 8).

- **Font sizing:** `KiwiTextUtil.resizeFont()` is called after every text assignment to the header title and item name to prevent overflow. `SkillRow` also calls it per-row.

- **3D perspective:** Set once on `ADDED_TO_STAGE` — field-of-view 40.6°, projection centre at `(195, 534)` — likely to give a subtle 3D depth effect to the forge panel.

- **Star pool growth:** `setStarCount(total, filled)` only ever grows `m_starList` (never shrinks); extra stars beyond `total` are hidden via `visible = false`.
