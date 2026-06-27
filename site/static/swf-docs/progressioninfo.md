# progressioninfo.swf
> The progression/crafting info panel shown when the player selects an upgrade node, a craftable item, or a progression choice. Displays a title, description, ingredient costs, rewards, an action button, and optionally an upgrade tree list. Used in multiple contexts: class gem unlocking, dragon upgrades, mastery progression, and club/cornerstone upgrades.

**Document/main class:** `ProgressionInfo` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 13

## Main class: `ProgressionInfo`

`ProgressionInfo` hosts a central info panel (`m_ui` MovieClip, embedded as `ProgressionInfo_fla.m_ui_3`) and a side banner (`Banner`) that displays art via two `ObjectPreview` instances (`sideBar` and `sideBarHeader`). Perspective projection is configured at `ADDED_TO_STAGE` (`fieldOfView = 40.616592°`, centre `(195, 534)`).

`configUI()` wires `m_actionBtn`, `m_title`, `m_description`, and `m_rewards` from inside `m_ui`, sets a click-sound event (`"Play_ui_forge_use"`) on the action button, adds a banner art container for the two `ObjectPreview` panels, and registers all Iggy callbacks. Initial highlight state is cleared.

The class tracks a `m_currentMode` (int 0–6) and a `m_currentFocus` chain for console D-pad navigation across ingredient slots and the action button. Ingredient `RequiredMaterial` clips are pooled in `m_ingredientList` and positioned in a horizontal row below the action button at runtime by `updateIngredientDisplay()`.

### Public methods
- `setTitle(text:String) : *` — sets `m_title.text`, resets ingredient selection, applies size-28 TextFormat, auto-resizes font via `KiwiTextUtil.resizeFont`, and vertically re-centres.
- `setDescription(text:String) : *` — sets `m_description.text`, applies size-18 TextFormat, auto-resizes.
- `setTitleAndDescription(title, desc) : *` — calls both of the above.
- `addIngredient(iconName, owned, needed, tooltipTitle, tooltipDesc) : *` — retrieves or creates a `RequiredMaterial` from the pool, calls `Setup()` on it, increments `m_numIngredientDisplay`, invalidates STATE.
- `clearIngredients() : *` — removes all `RequiredMaterial` children, clears the list, resets counter.
- `addReward(icon, name, ?, label) : *` — creates a `RewardSlot`, calls `init()`, adds it to the `StackList` inside `m_rewards.rewardList`.
- `clearRewards() : *` — clears the `StackList` in `m_rewards.rewardList`.
- `clearIngredientsAndRewards() : *` — calls both clears.
- `updateIngredientDisplay() : *` — lays out visible `RequiredMaterial` clips horizontally, spaced 60 px apart, centred around the action button. On NX alternates y by ±10 px per slot.
- `GainedFocus() : *` — re-highlights the current selected object (console).
- `LostFocus() : *` — unhighlights and fires `MOUSE_OUT`/`ROLL_OUT` on the selected object.
- `ResetFocus() : *` — calls `LostFocus`, resets focus state, calls `GainedFocus`.
- `moveSelection(dx, dy) : void` — navigates the focus sequence array; delegates horizontal movement to `moveCostsSelection` to cycle ingredient slots.
- `ClearAndSetHighlightSelection(target, highlight) : *` — hides all ingredient highlight clips and the action button highlight, then calls `HighlightUtil.highlightMovieClip` / `unhighlightMovieClip` on the target.

### Key fields
- `m_ui : MovieClip` — inner panel clip (`ProgressionInfo_fla.m_ui_3`), contains `m_actionBtn`, `m_title`, `m_description`, `m_rewards`.
- `m_actionBtn : LabelButton` — primary action button; label changes per mode.
- `m_title : TextField`, `m_description : TextField`.
- `m_rewards : MovieClip` — contains `rewardList:StackList` and `details:TextField`.
- `Banner : MovieClip` — side artwork container; `Banner.art` hosts `sideBar` and `sideBarHeader`.
- `sideBar : ObjectPreview(741, 2048)` — main side art texture.
- `sideBarHeader : ObjectPreview(741, 2048)` — side art header texture.
- `m_currentMode : int` — controls action button label and behaviour (see modes below).
- `m_ingredientList : Array` — pooled `RequiredMaterial` clips.
- `m_numIngredientDisplay : int` — count of currently visible ingredients.
- `m_selectedIngredient : int` — console D-pad cursor within ingredient row.
- `m_currentFocus / m_previousFocus : *` — console focus identifiers (-1 = unknown, 1 = costs).
- `m_currentFocusSequence : Array` — ordered list of focus zones for D-pad cycling.
- `opX / opY : Number` — `ObjectPreview` base dimensions (741, 2048) for resize scaling.

### Frame scripts / timeline
- Frame 1, 11, 21 — all `stop()`. Frames represent different visual states of the panel (e.g., different upgrade modes or visibility states).

### Runtime dependencies & integration
**Mode constants and action button labels:**
| Mode | Value | Button label key |
|------|-------|-----------------|
| UnlockMode | 0 | `$ModuleLoadout_UnlockBtn` |
| ChooseMode | 1 | `$Select_ButtonLegend` |
| UpgradeMode | 2 | `$Clubs_Upgrade` |
| DowngradeMode | 3 | `$Progression_DowngradeBtn` |
| Refund | 4 | `$Progression_Refund` |
| Reset / Mode_None | 5 / 6 | `$Progression_Reset` |

**ExternalInterface callbacks registered (Iggy):**
`setTitle`, `addReward`, `setDetails`, `setDescription`, `setTitleAndDescription`, `clearIngredients`, `clearRewards`, `clearIngredientsAndRewards`, `addIngredient`, `UIComponent.onStageResized`, `SetCurrentMode`, `GainedFocus`, `ResetFocus`, `LostFocus`, `moveSelection`, `requestAction`, `SetBackground`

**ExternalInterface calls fired by `requestAction`:**
- `UNLOCK_REQUEST` (UnlockMode)
- `UPGRADE_REQUEST` (UpgradeMode)
- `DOWNGRADE_REQUEST` (DowngradeMode)
- `REFUND_REQUEST` (Refund or Reset mode)

**Stage resize:** `onStageResized` scales the whole component (`scaleX = scaleY = scale`), repositions `Banner` relative to `m_ui` accounting for scale offset, resizes both `ObjectPreview`s, and re-lays out all ingredient clips.

**Perspective projection:** On `ADDED_TO_STAGE`, sets `root.transform.perspectiveProjection.fieldOfView = 40.616592` and `projectionCenter = (195, 534)`.

---

## Other game-specific classes

### `RequiredMaterial` (extends `_kiwi.Core.UIComponent`) — [Embed symbol29]
Single ingredient slot showing an icon and `owned/needed` text. `Setup(iconName, owned, needed, tooltipTitle, tooltipDesc)` sets the `ArtClip` image and tooltip properties. `draw()` formats `txt_cost` as `"owned/needed"` (abbreviates owned to `"*"` if > 9999); text is white when sufficient, red (0xFF0000) when not. Two frame scripts (frames 1 and 11, both stop).

### `RewardSlot` (extends `MovieClip`) — [Embed symbol23]
Displays a single reward: icon (`ObjectPreview` 45×45 inside `art`), `_name:TextField`, `_amount:TextField`. `init(iconName, name, amount)` sets all three; hides `art` if iconName is empty. Uses `KiwiTextUtil.resizeFont` with max size 14, min 2 lines.

### `ProgressionUpgradeView` (extends `_kiwi.Controls.DynamicRowView`) — [Embed symbol42]
A scrollable row list for choose-mode upgrade trees. Tracks `m_selectedRow:ProgressionUpgradeRow`. On row becoming visible, adds a click listener. Click fires `ExternalInterface.call("OnChoiceSelected", upgradeTree, upgradeNode)` and toggles highlight visibility.

### `ProgressionUpgradeRow` (extends `_kiwi.Controls.DynamicRowViewRow`) — [Embed symbol53]
A single choice row. Fields: `m_upgradeTree:String`, `m_upgradeNode:String`, `m_title:TextField`, `m_description:TextField`, `m_activeIndicator:MovieClip`, `m_inactiveIndicator:MovieClip`, `highlight:MovieClip`. `purchasable` setter applies a 0.5× grey `ColorTransform` when false. Mouse-over/out fire `ShowTooltip(tree, node, x, y)` / `HideTooltip`.

### `ProgressionUpgradeSection` (extends `_kiwi.Controls.DynamicRowViewSection`) — [Embed symbol48]
Section header row for the upgrade tree view. No additional logic.

### `ProgressionUpgradeRowData`
Data transfer object: `m_upgradeTree`, `m_upgradeNode`, `m_name`, `m_waitingForRequirements:Array`, `m_waitingForCosts:Array`, `m_purchasable:Boolean`. `clear()` resets all fields.

### `QuickLinkItem` (extends `MovieClip`) — [Embed symbol37]
A clickable shortcut item with `textField:TextField`, `imageContainer:MovieClip`, and an `ArtClip` image. `IndexReference:int` is the link ID. Click fires `ExternalInterface.call("QuickLinkClicked", IndexReference)`.

### `dummy` (extends `BitmapData`)
Embedded asset: `/_assets/63_dummy.png` (48×48). Placeholder bitmap — no logic.

### `ProgressionInfo_fla/` timeline symbols (2 classes)
- `m_ui_3` — [Embed symbol178] the inner panel MovieClip containing `m_actionBtn:btnGreen`, `m_title:TextField`, `m_description:TextField`, `m_rewards:MovieClip`. Configures action button defaults (`label="$Clubs_Upgrade"`, `enabled=false`).
- `itemSlot_60` — item slot symbol at frame 60.

### Asset wrappers (18 classes)
Rarity frame PNGs (9): `rarity_frame_common_png`, `rarity_frame_uncommon_png`, `rarity_frame_rare_png`, `rarity_frame_epic_png`, `rarity_frame_legendary_png`, `rarity_frame_stellar_png`, `rarity_frame_radiant1_png`, `rarity_frame_relic_png`, `rarity_frame_resplendent_png`, `rarity_frame_shadow_png`. Scrollbar skins (9): `ScrollArrowDown_*`, `ScrollArrowUp_*`, `ScrollThumb_*`, `ScrollTrack_skin`, `ScrollBar_thumbIcon`, `focusRectSkin`. Button: `btnGreen`. `image` (bitmap embed).

---

## Notable logic
- **Ingredient pool reuse:** `m_ingredientList` is never shrunk; `clearIngredients` hides and removes children but keeps the array for the next `addIngredient` call, avoiding allocation churn during repeated panel updates.
- **Font auto-resize:** `KiwiTextUtil.resizeFont` is called after setting title (max 28pt) and description (max 18pt) text to prevent overflow.
- **Dual side art:** `sideBar` and `sideBarHeader` are separate `ObjectPreview` instances in `Banner.art`, allowing the game to independently set main and header textures via `SetBackground(mainTex, headerTex)`.
- **Multi-context mode:** A single SWF serves unlock, choose (upgrade tree picker), upgrade, downgrade, refund, and reset workflows — all driven by `SetCurrentMode` and the corresponding `*_REQUEST` callback on button press.
