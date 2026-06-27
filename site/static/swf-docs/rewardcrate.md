# rewardcrate.swf

Reward Crate (lockbox) opening window that appears when a player opens a loot crate or lockbox in Trove. It shows the item being opened, a scrollable list of possible reward items, an optional augment slot, a karma progress meter, and buttons to open, claim, or apply an augment. The window supports both PC (mouse-driven) and console (gamepad focus/activate) input paths.

**Document/main class:** `RewardCrate` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 17 (excluding asset wrappers and `org.flintparticles` library)

---

## Main class: `RewardCrate`

The root UIComponent for the reward crate window. It initialises the UI layout, registers all `ExternalInterface` callbacks so the game engine can push data in, and fires `ExternalInterface.call(...)` back to the engine when the player acts.

### Constructor / lifecycle

- `super()` → `addFrameScript(9, frame10; 19, frame20)` — two named frame labels (frames 10 and 20 both `stop()`), likely the "open" and "content/results" states of the timeline.
- Creates `ExplodingProgressBar` sized to `karmaMeter.karmaMask` dimensions, positions it at `(72, 540)`.
- Attaches click listeners to `claimBtn`, `openBtn`, `applyBtn`, and `rewardList`.
- Calls `__setProp_*` helpers (kiwi component-inspector property injection for `karmaMeter` and the window header).
- Adds a `FRAME_CONSTRUCTED` listener (`__setProp_handler`) to re-apply button props on frame changes.

`configUI()` (called by kiwi framework after construction):
- If running in Iggy: registers `SlotDragDropHelper.registerDropCallback(onDrop)` and the full set of `ExternalInterface.addCallback(...)` entries (see Runtime dependencies).
- Configures `rewardList` (no vertical scrollbar, no centering, spacing `1,0,1,1`).
- On non-console: sets button labels from translate keys, hides all three action buttons initially.
- Hides `progressBar`, `karmaMeter`, `rewardList`, `rewardsTextField`, augment slot frame highlight, current-item slot frame highlight.
- Calls `setRewardSize(0,0,"",0)`, `setCrateItem("",0)`, `setAugmentItem("",0)`, `setCrateKarma(0,0)` to blank out the UI.
- Calls `setupTranslation()` (inherited from UIComponent).

### Public methods

- `addReward(iconPath:String, name:String, qty:Number, rarity:Number, index:Number, frameIndex:Number) : *` — reuses or creates a `RewardRow` in `rewardList` at `rewardIndex`, calls `RewardRow.init(...)`, increments `rewardIndex`. Frame index controls the visual rarity frame on the row.
- `setRewardSize(count:Number, maxRewards:int, rarityLabel:String, rarityColor:Number) : void` — primary state-switch: if `count <= 0` resets to "pre-open" mode (shows `openBtn`, hides reward list, augment frame, rarity text); if `count > 0` switches to "post-open/results" mode (shows `claimBtn` + `applyBtn`, reward list, augment slot, rarity label). Updates `rewardsTextField` with `$RewardCrate_SelectReward` / `$RewardCrate_SelectRewards` pluralised and `{0}`-substituted with `maxRewards`. Updates `buttonLegend.selectBtn.textField` with either `$OpenCrate_ButtonLegend` or `$Select_ButtonLegend`.
- `clearResults() : void` — calls `rewardList.clear()`.
- `rewardClicked(e:MouseEvent) : void` — resolves clicked target's parent as a `MovieClip` and delegates to `selectReward`.
- `selectRewardIndex(index:int) : void` — selects a `RewardRow` by index from `rewardList`.
- `setRewardFocus(index:int) : void` — console gamepad focus tracking: clears the slot-frame highlight on the previously focused slot (augment, current-item, or a reward row), then lights up the newly focused one. Iterates all reward rows to set `slot.slotFrame.visible`.
- `selectReward(mc:MovieClip) : void` — toggles selection on a `RewardRow`; enforces `maxRewards` cap by evicting the oldest selection (FIFO via `selectedRewards.shift()`). Fires `ExternalInterface.call("OnSelectRewards", getSelectedRewards())`.
- `onClaimRewards(e:MouseEvent) : void` — fires `ExternalInterface.call("OnClaimRewards", getSelectedRewards())`.
- `getSelectedRewards() : int` — returns a bitmask: bit N set if the Nth reward row is selected.
- `onOpenCrate(e:MouseEvent) : void` — fires `ExternalInterface.call("OnOpenCrate")`.
- `onApplyAugment(e:MouseEvent) : void` — fires `ExternalInterface.call("OnApplyAugment")`.

### Key fields

- `maxRewards : Number` — maximum number of rewards the player may select simultaneously.
- `rewardIndex : Number` — write cursor into `rewardList` during `addReward` calls.
- `focusIndex : Number` (init `-1`) — currently gamepad-focused slot index; used by `setRewardFocus`.
- `selectedRewards : Array` — ordered list of currently selected `RewardRow` instances (FIFO cap at `maxRewards`).
- `claimBtn : LabelButton` — "Claim" button; label key `$Claim`.
- `openBtn : LabelButton` — "Open" button; label key `$Lockbox_OpenButton`.
- `applyBtn : LabelButton` — "Apply" button; label key `$RewardCrate_Apply`.
- `karmaMeter : MovieClip` — contains `karmaMask` (a mask clip whose `scaleX` is set to karma fill fraction) and tooltip properties.
- `augmentItemFrame : MovieClip` — two-frame clip: frame 1 = hidden (no augment), frame 2 = augment mode visible.
- `rewardList : ScrollableTileView` — tile-view container holding `RewardRow` instances.
- `progressBar : ExplodingProgressBar` — particle-explosion bar overlaid on `karmaMeter`; shows animated confetti when karma fills.
- `rewardsTextField : TextField` — shows pluralised "select N reward(s)" instruction.
- `descTextField : TextField` — item description, set via `setDescription`.
- `rarityTextField : TextField` — rarity label string with colour set per reward.
- `slot_currentItem : Slot` — shows the item being opened; `data = -11`.
- `slot_augment : Slot` — the augment ingredient slot; `data = -10`.
- `buttonLegend : MovieClip` — console button-legend overlay (child clips `selectBtn` and `buttonLegendClose`).
- `__id0_ : WindowHeader` — window title header; title key `$RewardCrate_WinTitle`.

### Frame scripts / timeline

- Frame 10 (`frame10`): `stop()` — "open/pre-result" rest state (openBtn frame range 1–10).
- Frame 20 (`frame20`): `stop()` — "content/post-result" rest state (claimBtn + applyBtn frame range 1–10 within that segment).
- `__setProp_handler` on `FRAME_CONSTRUCTED`: re-applies button component-inspector properties for the current frame, gating on frame ranges 1–10.

### Runtime dependencies & integration

**ExternalInterface callbacks registered (game → Flash):**

| Callback | Method |
|---|---|
| `setCrateItem(iconPath, qty)` | Sets `slot_currentItem` icon and quantity; clears results if icon changes. |
| `setAugmentItem(iconPath, qty)` | Sets `slot_augment` icon and quantity. |
| `addReward(icon, name, qty, rarity, index, frameIndex)` | Appends or reuses a `RewardRow`. |
| `setCrateKarma(current, max)` | Sets `karmaMeter.karmaMask.scaleX = current/max`; triggers `ExplodingProgressBar.initParticles("KarmaParticle", 3)` on the penultimate step; calls `progressBar.start()` on reset-to-zero when partially full. |
| `setRewardSize(count, max, rarityLabel, rarityColor)` | Main state switch (pre/post open). |
| `selectRewardIndex(index)` | Programmatically selects a reward row. |
| `setDescription(text)` | Sets `descTextField.text`. |
| `setRewardFocus(index)` | Console gamepad focus. |
| `getSelectedRewards()` | Returns bitmask of selected rows. |
| `ACTIVATE_SLOT(index)` | Activates (calls `.activate()`) the slot at `index`; maps negative indices to `slot_augment` (`data -10`) or `slot_currentItem` (`data -11`). |

**ExternalInterface calls (Flash → game):**

| Call | Trigger |
|---|---|
| `OnOpenCrate()` | openBtn clicked |
| `OnClaimRewards(bitmask)` | claimBtn clicked |
| `OnApplyAugment()` | applyBtn clicked |
| `OnSelectRewards(bitmask)` | any reward row toggled |
| `OnDropIntoWindow(icon, flags, qty, slotData)` | drag-drop into `slot_currentItem` or `slot_augment` |

**Iggy / translate keys used:**

- `$RewardCrate_WinTitle` — window header title
- `$RewardCrate_Apply` — applyBtn label
- `$Claim` — claimBtn label
- `$Lockbox_OpenButton` — openBtn label
- `$RewardCrate_SelectReward` / `$RewardCrate_SelectRewards` — reward count prompt (singular/plural, `{0}` substitution)
- `$OpenCrate_ButtonLegend` — console button legend pre-open
- `$Select_ButtonLegend` — console button legend post-open
- `$LockboxTooltip_KarmaDesc` — karma meter tooltip body
- `$GemTooltip_Karma_Bar_Title` — karma meter tooltip title

**Drag-drop:** `SlotDragDropHelper.registerDropCallback(onDrop)` — hit-tests drag point against `slot_currentItem` and `slot_augment`; fires `OnDropIntoWindow` with the matching slot's `data` value.

---

## Other game-specific classes

### Top-level

- `RewardRow` — `UIComponent`, embeds `symbol61`. One row in the reward list. Fields: `slot : Slot` (icon + rarity frame + selection highlight), `nameTextField : TextField`, `bg : MovieClip` (visible = selected), `frame : MovieClip` (`gotoAndStop(frameIndex + 1)` for rarity frame). `init(icon, name, qty, rarity, index, frameIndex)` sets slot properties and hides selection. On console, defers `setUpItem()` (name/alignment) via `ENTER_FRAME` until `onTargetFrame()` is true. `selected` getter/setter toggles `bg.visible`. Frame scripts on 10 and 20 (`stop()`).

- `ExplodingProgressBar` — `MovieClip`. Particle-burst animation overlay for the karma meter fill event. Uses the `org.flintparticles` 2D library (Emitter2D + DisplayObjectRenderer). Constructor takes `(width, height)` of the progress bar. `initParticles(prefix, count)` dynamically loads `KarmaParticle0..N` classes by name via `getDefinitionByName`, creates a grid of Particle2D instances covering the bar width, builds a sequence of `GravityWell` + `Explosion` actions staged at even intervals. `start()` makes renderer visible, starts emitter and a `Timer` (500ms / 20 explosions cadence) that pops one action per tick. Constants: `NUM_EXPLOSIONS=20`, `EXPLOSION_POWER=2`, `EXPLOSION_EXPANSION_RATE=180`.

- `meter_karma` — `UIComponent`, embeds `symbol136` from `assets.swf`. The MovieClip instance used as `karmaMeter` on the stage; contains `karmaMask` child.

- `KarmaParticle0` / `KarmaParticle1` / `KarmaParticle2` — `MovieClip` embeds (`symbol29`, `symbol27`, `symbol25`). Three sprite variants loaded dynamically by `ExplodingProgressBar.initParticles("KarmaParticle", 3)`.

- `btnGreen` — `LabelButton`, embeds `symbol99`. Green action button with four timeline states (frames 10, 20, 30, 40 — typical up/over/down/disabled). Used for `claimBtn`, `openBtn`, `applyBtn` on the stage.

- `btnGreenIcon_small` — `LabelButton`, embeds `symbol89`. Smaller green button with eight timeline states (frames 10–80, 10 each). Likely the console button-legend button variant.

- `slot_large` — `Slot`, embeds `symbol63`. Larger slot variant used for `slot_currentItem` and `slot_augment` (size set to 55 via `setSlotSize`).

- `rarity_frame_stellar` — `BitmapData`, embeds `/_assets/3_rarity_frame_stellar.png`. One non-`_png`-suffixed bitmap (54×54 default size).

### RewardCrate_fla timeline symbols

- `forgeBG_chaos_6` — `MovieClip`, `symbol146`, 34-frame animation (stops on frame 34). The animated chaos/forge background graphic behind the item slot.
- `framecopy_7` — `MovieClip`, `symbol150`, 78-frame animation (stops on frame 78). Animated decorative frame copy.
- `item_frame_5` — `MovieClip`, `symbol147`, 2-frame clip (frames 1 and 2, both stop). Toggle between frame states for the item slot surround.
- `bg_claimed_4` — `MovieClip`, `symbol50`, frames 1 and 11 (both stop). "Claimed" background state indicator for reward rows.
- `slotFrameXL_33` — `MovieClip`, `symbol34`, 2-frame clip (stop on each). XL slot frame overlay (two states: unselected/selected).
- `equipped_37` — `MovieClip`, `symbol36`, 2-frame clip. "Equipped" badge indicator on a slot.
- `qualityPips_42` — `MovieClip`, `symbol45`, stops on frame 1. Quality star/pip indicator row.
- `itemRowBG_50` — `MovieClip`, `symbol54`, frames 1 and 11. Row background for item list entries (two visual states).
- `itemRowRarity_51` — `MovieClip`, `symbol58`, 3-frame clip (stop on each). Rarity-tier colour band for reward rows (3 visual states).
- `ButtonLegend_44` — `MovieClip`, `symbol170`, stops on frame 1. Console button legend bar; child clips `selectBtn` (holds `textField`) and `buttonLegendClose`.

### Asset wrappers (not detailed individually)

- **23 rarity frame PNG/bitmap wrappers** (`rarity_frame_*_png`, `rarity_frame_*_over_png`) covering rarities: common, uncommon, rare, epic, legendary, relic, shadow, crystal, stellar, radiant1, resplendent — normal and hover states.
- **9 ScrollBar skin classes** (`ScrollArrowDown_*`, `ScrollArrowUp_*`, `ScrollThumb_*`, `ScrollTrack_skin`, `ScrollBar_thumbIcon`, `focusRectSkin`) — standard kiwi scroll component visual skins.

---

## Notable logic

- **Reward selection bitmask:** `getSelectedRewards()` builds its result as `int` with bitwise OR (`1 << rowIndex`), so the game engine receives a compact bitmask rather than an array. This limits the reward list to 31 items in practice.
- **FIFO selection cap:** when the player selects more than `maxRewards` items, the oldest selection is automatically deselected by `selectedRewards.shift()`, keeping the array length at most `maxRewards`.
- **Karma fill-burst timing:** `setCrateKarma` checks `param1 == param2 - 1` (penultimate karma step) to pre-load particles, and checks `param1 == 0 && param2 > 0 && scaleX > 0 && scaleX < 1` (reset after partial fill) to fire the burst — so the animation triggers on a karma-meter wraparound/reset rather than on initial fill.
- **Console vs PC branch:** `IsConsole()` (global function, likely from IggyFunctions) gates button visibility, drag-drop and ExternalInterface registration (all disabled on console), and the `ENTER_FRAME`-deferred text setup in `RewardRow`.
- **Particle class lookup:** `ExplodingProgressBar` uses `flash.utils.getDefinitionByName("KarmaParticle" + i)` to load particle classes by convention, making the particle count configurable without hard-coded class references.
- **Slot data sentinel values:** `slot_currentItem.data = -11` and `slot_augment.data = -10` let `activateSlot` and `setRewardFocus` distinguish the two special slots from the indexed reward rows using only the integer slot-data field.
