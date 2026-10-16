# activitytrackerui.swf

> The Activity Tracker is a multi-tab quest/objective panel that surfaces the player's active Trove pursuits across several categories: Events, Expertise (Golden Thread objectives), Adventures (club quests), Repeatable (star bar, tome, challenges, quests), and Badges. It appears as a persistent window the player opens to track daily/weekly progress and earned badge rewards.

**Document/main class:** `ActivityTrackerUI` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 19

---

## Main class: `ActivityTrackerUI`

`ActivityTrackerUI` is the root component for the entire window. It owns two `ScrollableTileView` panels — `categoryView` (the left-side tab list) and `contentView` (the right-side entry list) — and orchestrates all data arriving from the game engine via `ExternalInterface` callbacks.

On construction, `addFrameScript` hooks frames 0, 10, and 20 (each calls `stop()`), then `__setProp___id1__Scene1_header_0()` pre-configures the embedded `WindowHeaderSmall` with the translate key `$ActivityTracker`.

`configUI()` (called by the kiwi lifecycle on first render) does:
- Configures both scroll views (spacing, vertical step, scrollbar visibility).
- Calls `focusActivity(-1, 0)` to clear the initial focus state.
- Registers mouse and custom event listeners on both views.
- When `IggyFunctions.inIggy` is true, registers 33 `ExternalInterface` callbacks so the C++ game engine can push data into the Flash UI.
- When running outside Iggy (authoring preview), populates demo content using hard-coded `translate()` calls for every category.

Data is stored in `questsByCategory : Dictionary` keyed by category string, each value being another `Dictionary` keyed by entry ID. When the user switches categories, `selectCategory()` clears `contentView`, re-adds all entries for the new category, and calls `sortContent()`.

### Public methods (Iggy callbacks)

- `addCategory(id:String, label:String) : void` — Creates a `CategoryEntry` toggle-button and registers a new `Dictionary` bucket in `questsByCategory`.
- `addHeader(category:String, activityId:int, title:String, desc:String, emptyDesc:String, sortOrder:int, hidden:Boolean) : void` — Adds or updates an `ActivityHeader` section divider inside a category. `hidden` is the inverse of the header's Always Show box (`showBtn.checked = !hidden`); `sortOrder` places the header in the content sort.
- `selectCategory(id:String) : int` — Switches the visible content pane to the named category; toggles the checked state of the old/new `CategoryEntry`; hides the first header's `divider`; returns the category's index. An empty id selects the first category whose `sizeOfCategory` is above zero, falling back to the last category. A click on a category tab calls this locally; no outbound call is made.
- `selectCategoryIndex(index:int) : void` — Simulates a click on a category by index.
- `updateQuest(id, difficulty, name, progress, failed)` — Upserts a `QuestEntry` in the REPEATABLE bucket. `difficulty` is an index 0–3 (clamped) onto the `"1star"`, `"2star"`, `"3star"`, `"5star"` icon frames; `progress >= 1` marks the quest complete.
- `updateActivity(category, id, name, desc, progress, goal, rewardImg, rewardName, rewardQty, source, canRemove, sortOrder, secondarySort, topIcon, hasTimer)` — Upserts a `PersonalObjectiveEntry`.
- `updateGoldenThreadObjective(...)` — Thin wrapper calling `updateActivity` with category = EXPERTISE.
- `updateActivityWithTimer(category, id, name, hasTimer, timerMs, desc, progress, goal, rewardImg, rewardName, rewardQty, source, canRemove, sortOrder, secondarySort, topIcon)` — Like `updateActivity` but also calls `entry.setTimer()`. Note the different argument order: the timer pair follows `name`. `timerMs` is milliseconds remaining; the entry's `CountdownTimer` runs it down locally and shows hours.
- `updateGoldenThreadObjectiveWithTimer(...)` — Wrapper for the timer variant in EXPERTISE.
- `updateChallenge(name, desc, level, progress, goal, rewardName, rewardQty, rewardImg, bonusName, timerMs, dailyBonusName, dailyBonusQty, dailyBonusComplete)` — Upserts a `ChallengeEntry`. The engine sends no id: the first argument is the challenge's display name, and the entry is always stored under the fixed key `"challenges"` in REPEATABLE (the key `removeChallenge` deletes).
- `updateBadge(id, rank, name, desc, texture, maxVal, currentVal, sortOrder)` — Upserts a `BadgeEntry` in the BADGES bucket; wires up rollover listeners.
- `addBadgeReward(badgeId, rewardId, texture, quantity)` — Appends a reward slot to an existing `BadgeEntry` carrousel. A `rewardId` the badge already holds is ignored, so the same reward can be re-sent without duplicating.
- `clearBadges()` — Removes all `BadgeEntry` items from contentView and clears the BADGES dictionary.
- `selectCurrentBadge() / deselectCurrentBadge()` — Finds the focused `BadgeEntry` and fires synthetic ROLL_OVER / ROLL_OUT on its `badgeImageContainer`.
- `badgeRewardHighlightForward() / badgeRewardHighlightBackwards()` — Delegates to `BadgeEntry.highlightRewardForward/Backward()` on the focused entry.
- `setProgressBar(type:int, sortOrder, filledPct, rewardAmt, rewardImg, autoUseTexture)` — Creates (if needed) one of three progress bars: `StarProgressBar` (0), `TomeProgressBar` (1), `AutoUseTomeProgressBar` (2), and updates its fill. Bars are keyed `"starbar"`, `"tomebar"`, `"autousebar"` in REPEATABLE; the type index is the only handle the engine gives a bar. The auto-use bar also receives an `ObjectPreview` child with the `autoUseTexture` icon, which is applied only when that bar is first created.
- `removeActivity(category, id) : void` — Removes a single entry from a category's dictionary and from the content view, then calls `updateHeader()`.
- `removeGoldenThreadObjective()` — Calls `removeActivity(EXPERTISE, "goldenThread")`.
- `removeChallenge()` — Calls `removeActivity(REPEATABLE, "challenges")`.
- `removeQuest(id)` — Calls `removeActivity(REPEATABLE, id)`.
- `focusCategory(index, direction)` — Highlights a category item's background, scrolls the category view if the item is off-screen. Calls `buttonLegend.gotoAndStop("category")`.
- `focusActivity(index, direction)` — Sets `currentFocus` on content items, scrolls contentView if needed. Fires badge rollover events when focus moves to/from a `BadgeEntry`. Calls `buttonLegend.gotoAndStop("activity")`.
- `toggleHideActivityType(activityIndex)` — Walks backwards from an item index to the nearest `ActivityHeader` with a `showBtn` and clicks it.
- `selectActivityIndex(index)` — Fires a `TRACK_ACTIVITY` event for the `PersonalObjectiveEntry` at that content index.
- `removeActivityIndex(index)` — Fires a `REMOVE_ACTIVITY` event for the entry at that index.
- `setTrackedActivity(category, id)` — Clears `isTracked` on the old tracked entry and sets it on the new one.
- `getCategoryId() : String` — Returns `currentCategory`. Registered under two names, `getCategoryId` and `getHeaderIndex`, so the engine's `getHeaderIndex` call also receives the category id string. The class's own `getHeaderIndex() : int` (index of the first `ActivityHeader` in `contentView`) is never registered.
- `getCategoryCount() / getActivityCount() : int` — Item counts of the two scroll views.
- `sizeOfCategory(id) : int` — Counts non-header, non-progress-bar entries in a category (everything stored, not only what is on screen).
- Index arguments (`focusActivity`, `selectActivityIndex`, `removeActivityIndex`, `toggleHideActivityType`) and `getActivityCount` address positions in `contentView`, i.e. the sorted list of the open category only. `selectCategoryIndex`, `focusCategory` and `getCategoryCount` use `addCategory` order.
- `updateHeader() : void` — Iterates visible items; sets `showEmpty = true` on any `ActivityHeader` that is last or is immediately followed by another header (i.e., has no entries beneath it).

### Key fields

- `categoryView : ScrollableTileView` — Left panel; holds `CategoryEntry` toggle buttons.
- `contentView : ScrollableTileView` — Right panel; holds all entry types mixed together, sorted on every update.
- `buttonLegend : MovieClip` (timeline symbol `ButtonLegend_23`) — Controller-button hint strip; frames `"category"` and `"activity"` show appropriate button hints.
- `__id1_ : WindowHeaderSmall` — The window title bar, pre-configured with title key `$ActivityTracker` (English text "ADVENTURES"), set to disabled (non-interactive).
- `autoUseIcon : ObjectPreview` — 30×30 icon injected into the `AutoUseTomeProgressBar` barIcon.
- `questsByCategory : Dictionary` — Top-level keyed by category string; each value is a `Dictionary<id, DisplayObject>`.
- `categories : Array` — Ordered list of category ID strings (insertion order = tab order).
- `currentCategory : String` — Currently visible category.
- `trackedCategory / trackedActivity : String` — Track the single activity that has `isTracked = true`.
- `validateSort : Boolean` — Dirty flag; triggers `sortContent()` on the next `draw()` pass when a new item is added.

### Frame scripts / timeline

- Frame 0 → `stop()`
- Frame 10 → `stop()`
- Frame 20 → `stop()`

These three stop-frames are the platform-skin frame labels `"PC"` (0), `"Console"` (10) and `"ConsoleLoc"` (20).

### Runtime dependencies & integration

**Iggy / ExternalInterface calls (outbound — Flash → C++):**
- `OnTrackActivity(activityId)` — fired when the user clicks the Track button on a `PersonalObjectiveEntry`.
- `OnRemoveActivity(activityId)` — fired when the user clicks the Remove button.
- `OnHideCategory(activityId, alwaysShow)` — fired from `ActivityHeader.onShowClicked()` when the Always Show checkbox is clicked (only for headers with an `activityId`). Despite the name, the engine reads the second argument as the new Always Show state, not a hide flag: `true` keeps the section shown, `false` hides it. The game stores this setting; vanilla sends `!showBtn.checked`.
- `OnBadgeRollOver(badgeId, badgeRank, x, y)` — fired on badge image mouseover (tooltip trigger). `x, y` is the window's top-right corner (`root.x + root.width`, `root.y`), not the pointer; the engine grows the tooltip right and down from that point, so it opens beside the window.
- `OnBadgeRollOut()` — fired on badge image mouseout.
- `OnBadgeRewardRollOver(rewardId, x, y)` — bubbled from `BadgeEntry`; fired on reward icon rollover. `x, y` is the stage position of the reward slot's right edge and top.
- `OnBadgeRewardRollOut()` — bubbled from `BadgeEntry`.
- `POST_SOUND_EVENT("Play_ui_starbar_reward")` — called by `ProgressBar.newProgressPercent` setter when a bar reaches 100 %.
- `UIComponent.OnShowTooltip(x, y, name, desc)` / `UIComponent.OnHideTooltip()` — called by `ProgressBar` on hover/rollout, anchored at the pointer's stage position and passing the raw `$…Tooltip_Title` / `$…Tooltip_Description` keys.

**translate() keys observed:**
`$ActivityTracker`, `$ActivityEventsTitle`, `$ActivityEventsDesc`, `$ActivityExpertiseTitle`, `$ActivityExpertiseDesc`, `$ActivityClubAdventuresTitle`, `$ActivityClubAdventuresDesc`, `$ActivityStarBarTitle`, `$ActivityStarBarDesc`, `$ActivityTomeTitle`, `$ActivityTomeDesc`, `$ActivityChallengeTitle`, `$ActivityChallengeDesc`, `$ActivityQuestTitle`, `$ActivityQuestDesc`, `$ActivityTrackerAlwaysShow`, `$ActivityTrackerTrackButton`, `$ActivityTrackerTrackedButton`, `$StarBar_Tooltip_Title`, `$StarBar_Tooltip_Description`, `$TomeBar_Tooltip_Title`, `$TomeBar_Tooltip_Description`, `$AutoUseBar_Tooltip_Title`, `$AutoUseBar_Tooltip_Description`, `$Challenges_TimeUp`

**Content sort:** `sortContent()` sorts all items in `contentView` by `sortOrder` (primary), then `secondarySortOrder` (secondary), with `ActivityHeader` items winning ties at equal primary sort order. Each header's sortOrder comes from `addHeader` (the non-Iggy preview uses 0, 10, 20, 30), and entries are interleaved between them.

---

## Other game-specific classes

### Entry types

- `ActivityHeader` — Section divider row within a category [Embed symbol190]. Extends `UIComponent`. Has a `Checkbox` (`showBtn`) to toggle sub-section visibility; clicking it calls `OnHideCategory` via ExternalInterface. Displays a title (`txt_name`) and a description that switches between `_description` (when items are present) and `_descriptionEmpty` (when the section is empty, controlled by `showEmpty`). Uses `IsConsole()` to choose `htmlText` vs `text` rendering. The background auto-sizes to the text height in `fitBackgroundToText()`. `currentFocus` setter drives a `"on"/"off"` background frame.

- `PersonalObjectiveEntry` — General personal objective / Golden Thread row [Embed symbol100]. Extends `UIComponent`. Holds three `ObjectPreview` slots (reward, topIcon, bottomIcon), a `CountdownTimer`, a track button (`trackBtn : LabelButton`) that dispatches `TRACK_ACTIVITY`, and an optional remove button (`removeBtn`) that dispatches `REMOVE_ACTIVITY`. The track button reads `$ActivityTrackerTrackButton` ("Show"), and `$ActivityTrackerTrackedButton` ("Shown") disabled once `isTracked`. Progress is shown as `n/goal` when goal > 1. The `objectiveCompleted` MovieClip replaces the description when `_progress == _goal` (so a 0/0 objective with no counter also reads as complete) or when the description is empty. The `bottomIconImage` and `bonusQuantity` setters exist but `ActivityTrackerUI` never sets them. Supports a countdown timer (`setTimer` / `setTimerVisibility`). Dispatches `DataEvent(TRACK_ACTIVITY)` and `DataEvent(REMOVE_ACTIVITY)` on button clicks, bubbling to `ActivityTrackerUI`.

- `ChallengeEntry` — Daily/weekly challenge row [Embed symbol132]. Extends `UIComponent`. Shows challenge name, level (drives `badge` MovieClip frame 1–5), progress text, reward info, bonus reward text, daily bonus with a completion checkmark MovieClip, and a `CountdownTimer`. The clock animation (`clockAnim`) starts playing when the timer drops below 60 seconds; it stops on timer expiry. Calls `setupTranslation()`.

- `QuestEntry` — Quest row for the REPEATABLE category [Embed symbol53]. Extends `UIComponent`. Displays quest name and a difficulty icon (`questDifficultyIcon`) with frames `"1star"`, `"2star"`, `"3star"`, `"5star"`, `"complete"`, `"failed"`; the engine's difficulty index 0–3 maps to the four star frames, so index 3 is five stars. Background auto-sizes to the text height. No Iggy calls.

- `BadgeEntry` — Badge entry with a 5-slot reward carrousel [Embed symbol170]. Extends `UIComponent`. Contains a 64×64 badge icon (`badgeImageContainer`), a `BadgeProgressBar` (shown when `maxValue > 0`) or plain `taskText`, and a `rewardsCarrousel` MovieClip with up to 5 `reward0N` slots and left/right arrow buttons. Rewards are stored in `_rewards : Array` of Dictionaries with keys `ID`, `texture`, `quantity`. Navigation wraps circularly via `_leftmostShownReward` / `_rightmostShownReward`. Dispatches `DataEvent(REWARD_ROLLOVER)` and `DataEvent(REWARD_ROLLOUT)` on reward icon hover, which `ActivityTrackerUI` forwards to `OnBadgeRewardRollOver/Out`. Font auto-shrinks via `KiwiTextUtil.resizeFont`.

### Progress bars

- `ProgressBar` — Base class (not embedded directly). Masks `fillingMaskMC` and `newProgMaskMC` to represent filled vs. current-total fill. Plays a `rewardAnimation` (with sound event) when `newProgressPercent` reaches 1.0. Hover/rollout calls `UIComponent.OnShowTooltip` / `OnHideTooltip`.
- `StarProgressBar` — `ProgressBar` subclass [Embed symbol39]; used for the star bar (sortOrder index 0).
- `TomeProgressBar` — `ProgressBar` subclass [Embed symbol20]; used for the tome XP bar (sortOrder index 1).
- `AutoUseTomeProgressBar` — `ProgressBar` subclass [Embed symbol175]; same as TomeProgressBar but hosts an `ObjectPreview` `autoUseIcon` inside `barIcon.autoUseDisplay`.
- `SimpleProgressBar` — Simpler base (no animation, no sound); extends `UIComponent`. Masks `fillingMaskMC` as `currentValue/maxValue * maskMaxWidth`.
- `BadgeProgressBar` — Extends `SimpleProgressBar` [Embed symbol151]. Adds `progressText` (`currentValue/maxValue`) and `taskText` (description label); overrides `maxValue` and `currentValue` setters to update the progress text.

### Tab / scroll views

- `CategoryEntry` — Extends `_kiwi.Controls.LabelButton` [Embed symbol145]. Used as a toggle-button tab for each category in `categoryView`. Has four frame-script stops (frames 0, 9, 19, 29 — up/over/down/disabled states).
- `ContentView` — Extends `ScrollableTileView` [Embed symbol244]. No added logic; just a named embedded symbol for the content area.
- `CategoryView` — Extends `ScrollableTileView` [Embed symbol243]. Adds three platform stop-frames (0, 10, 20).

### Button symbol wrappers (top-level, extend kiwi buttons)

- `arrowBtnLeft` — Extends `BaseButton` [Embed symbol166]; left-arrow for badge reward carrousel.
- `arrowBtnRight` — Extends `BaseButton` [Embed symbol161]; right-arrow for badge reward carrousel.
- `btnGreenIcon_small` — Extends `LabelButton` [Embed symbol83]; used as the Track button.
- `btnKick` — Extends `BaseButton` [Embed symbol92]; purpose is to kick/remove entries (used as remove button).

### Timeline symbol classes (`ActivityTrackerUI_fla` package)

- `ButtonLegend_23` [symbol261] — Controller button-hint strip MovieClip; contains `buttonLegendRemove`, `buttonLegendSelect`, `buttonLegendToggle`, `buttonLegendClose` sub-clips. Frame labels `"category"` (frame 0) and `"activity"` (frame 10).
- `AdventureFrameBackground_44` [symbol56] — Background graphic for adventure/club frames; two-state stop-frame.
- `badgeRewardImage_47` [symbol156] — Single badge reward slot; holds `rewardPreview : MovieClip` and `quantityText : TextField`.
- `badge_58` [symbol117] — 5-frame badge-level icon (frames 1–5 = rank 1–5 visuals).
- `clock_animation_57` [symbol109] — Clock spinning animation; frame 2 is the start of the "loop" label, frame 31 loops back via `gotoAndPlay("loop")`.
- `entryBackground_66` [symbol22] — Reusable entry background shape; two platform stop-frames.
- `header_background_30` [symbol3] — Section header background shape; two platform stop-frames.
- `quest_difficulty_icon_67` [symbol52] — 6-state difficulty icon (frames 0, 10, 20, 30, 40, 50 = 1star, 2star, 3star, complete, failed, 5star).
- `reward_animation_69` [symbol35] — 47-frame reward pop animation; frame 47 calls `halt()` to stop.

### Scroll-bar skin asset wrappers (trivial — 9 classes)

`ScrollArrowDown_disabledSkin`, `ScrollArrowDown_downSkin`, `ScrollArrowDown_overSkin`, `ScrollArrowDown_upSkin`, `ScrollArrowUp_disabledSkin`, `ScrollArrowUp_downSkin`, `ScrollArrowUp_overSkin`, `ScrollArrowUp_upSkin`, `ScrollThumb_upSkin`, `ScrollThumb_downSkin`, `ScrollThumb_overSkin`, `ScrollTrack_skin`, `ScrollBar_thumbIcon`, `focusRectSkin` — pure `MovieClip` embeds; no logic.

---

## Notable logic

- **Category data isolation:** Each category is a separate `Dictionary` inside `questsByCategory`. Switching categories fully clears and re-populates `contentView`, so the scroll position resets. Headers, progress bars, and entries all share the same flat Dictionary per category, distinguished at runtime by `instanceof` checks.

- **Sort order scheme:** `ActivityHeader` nodes act as section anchors at the sortOrder the engine passes to `addHeader`. Entry types sit at values in between: `updateQuest` and `updateChallenge` carry no sort order, so `QuestEntry` always sorts at its default 51 and `ChallengeEntry` at 11; `PersonalObjectiveEntry` (default 1) takes `sortOrder`/`secondarySortOrder` from `updateActivity`, and the Golden Thread wrappers pass 0/0; progress bars take theirs from `setProgressBar`. The sort function places headers before any peer entry with the same sortOrder, and uses `secondarySortOrder` for stable sub-ordering.

- **Console vs. PC rendering path:** Several `draw()` methods call `IsConsole()` (from `IggyFunctions`) and `onTargetFrame()` to choose between `htmlText` (console, with rich text mark-up from the engine) and plain `text` (PC). On console they also perform dynamic height adjustment via `adjustForDescriptionHeight` / `resizeDueToHeaderText` to cope with variable text flow.

- **Badge carrousel wrapping:** The carrousel uses signed integer offsets (`_leftmostShownReward`, `_rightmostShownReward`) that can go negative, mapping to the tail of `_rewards` array via modulo arithmetic, giving infinite left-scroll wrap-around.

- **Progress bar animation gating:** `ProgressBar.newProgressPercent` setter only triggers the fill-complete reward animation once per cycle (`waitingOnAnimation` guard). The animation's last frame resets `filledPercent` to 0 and clears the flag, allowing the next cycle to animate again.

- **buttonLegend state:** `focusCategory` / `focusActivity` drive `buttonLegend.gotoAndStop("category" | "activity")` so the controller hint strip shows the correct bindings for whichever panel has focus.
