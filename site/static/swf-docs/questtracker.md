# questtracker.swf
> The Quest Tracker is Trove's heads-up objective overlay, shown persistently during gameplay to display active quests, the Golden Thread personal objective, weekly challenges, badge progress, Delve depth/timer information, and transient event notifications. Entries are stacked vertically in a `StackList` and updated in real-time by the game engine.

**Document/main class:** `QuestTracker` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 13

---

## Main class: `QuestTracker`

`QuestTracker` is the root document class. It owns a single `StackList` (`questsStackList`, item spacing 12 px) and a `Dictionary` (`entries`) keyed by entry-type sentinel strings. The game sends all data via `ExternalInterface` callbacks. On console, the tracker starts hidden (`visible = false`) and waits for `onTargetFrame()` before showing and calling `setupTranslation()`.

A perspective projection is applied to the root on `ADDED_TO_STAGE`: `fieldOfView = 36.656964`, `projectionCenter = new Point(275, 200)`.

### Sentinel constants (entry type keys)

| Constant | Value |
|---|---|
| `GOLDEN_ID` | `"_POBJECTIVE_"` |
| `BADGE_ID` | `"_BADGE_"` |
| `CHALLENGE_ID` | `"_CHALLENGE_"` |
| `ALERT_ID` | `"_ALERT_"` |
| `NOTIFICATION_ID` | `"_NOTIFICATION_"` |
| `DELVE_ID` | `"_DELVE_"` |
| `ALERT_TIMER` *(public static)* | `"_ALERT_TIMER_"` |

### Public methods (all also registered as ExternalInterface callbacks)

- `updateBadge(badgeId, displayName, taskDesc, badgeImage, current, goal)` — creates or updates the single `BadgeEntry`; sets `badgeId`, `badgeImage`, and task progress.
- `removeBadge()` — removes the `BadgeEntry` from the stack.
- `updateQuest(questId, difficulty, name, progress, failed)` — creates or updates a `QuestEntry`; `complete = progress >= 1`.
- `removeQuest(questId)` — removes the entry with the given key from both `entries` and `questsStackList`.
- `updateGoldenThreadObjective(id, desc, current, goal, rewardImage, rewardName, rewardQty, bonusQty, topIcon, bottomIcon, showTimer)` — creates or updates the `PersonalObjectiveEntry`; manages dual-icon display (if `bottomIcon` is non-empty, shows it as `topIconImage` and `topIcon` as `bottomIconImage`); calls `setTimerVisibility`.
- `updateGoldenThreadObjectiveWithTimer(id, timerVisible, timerMs, desc, current, goal, rewardImage, rewardName, rewardQty, bonusQty, topIcon, bottomIcon)` — variant that also calls `setTimer(timerMs, timerVisible)` on the entry.
- `setGoldenThreadComplete()` — sets `PersonalObjectiveEntry.progress = goal` to mark it done.
- `removeGoldenThreadObjective()` — delegates to `removeQuest(GOLDEN_ID)`.
- `updateChallenge(challengeName, description, level, current, goal, rewardName, rewardQty, rewardImage, bonusName, timerMs, dailyBonusName, dailyBonusQty, dailyBonusComplete, alertType)` — on first creation, if `alertType != ""` also creates an `AlertEntry` keyed `ALERT_ID`; creates the `ChallengeEntry`; sets all fields and `setTimer(timerMs)`.
- `removeChallenge()` — delegates to `removeQuest(CHALLENGE_ID)`.
- `onAlertTimer(event:DataEvent)` — handler for `ALERT_TIMER` events from `AlertEntry`; calls `removeQuest(ALERT_ID)`.
- `addNotification(text)` — creates an `EventHighlight`, adds it to the stack, starts a `IggyTween` bouncing it in from x=-20 to x=10 (Bounce.easeOut, 2 sec). On tween finish calls `hideNotification`.
- `hideNotification()` — fades the `EventHighlight` out (alpha 1→0, 1 sec `IggyTween`); on finish calls `removeNotification`.
- `removeNotification()` — removes `EventHighlight` from the stack.
- `addDelveInfo(questName, title, depth)` — creates a `DelveEntry` and inserts it at index 0 (swap to top) via `questsStackList.swapChildrenAt(0, numChildren-1)`.
- `updateDelveInfo(requirementIndex, description, status)` — calls `DelveEntry.setRequirement`.
- `updateDelveTime(timerValue, warning)` — sets `DelveEntry.timer` and `DelveEntry.warning`.
- `removeDelveInfo()` — removes the `DelveEntry` from the stack.
- `resizeDelveInfo(newSize)` — trims `DelveEntry.m_requirements` to `newSize` elements.
- `clearDeltaliths()` — clears the `DelveEntry`'s `rewardImageContainer` children.
- `addDeltalith(textureName)` — appends an `ObjectPreview(32,32)` to `DelveEntry.rewardImageContainer`.

### Key fields

- `questsStackList : StackList` — the single vertical container; item spacing 12 px.
- `entries : Dictionary` — maps sentinel strings to their live entry instances.
- `notificationTween : IggyTween` — holds the current bounce-in / fade-out tween for the `EventHighlight`.

### Frame scripts / timeline

- **frame 0** — `stop()` (PC mode).
- **frame 10** — `stop()` (Console mode; tracker already made visible via `onEnterFrame` loop at this point).

### Runtime dependencies & integration

- All `ExternalInterface.addCallback` calls are guarded by `IggyFunctions.inIggy`.
- Callbacks registered: `updateBadge`, `removeBadge`, `updateQuest`, `removeQuest`, `updateGoldenThreadObjective`, `removeGoldenThreadObjective`, `updateChallenge`, `removeChallenge`, `setGoldenThreadComplete`, `addNotification`, `addDelveInfo`, `updateDelveInfo`, `updateDelveTime`, `removeDelveInfo`, `resizeDelveInfo`, `clearDeltaliths`, `addDeltalith`, `updateGoldenThreadObjectiveWithTimer`.
- `IggyTween` used for notification slide-in (Bounce.easeOut) and fade-out animations.
- `fl.transitions.easing.Bounce` imported directly for the notification tween.
- `draw()` override: when `InvalidationType.DATA` is pending, calls `questsStackList.RefreshLayout()`.
- `__setPerspectiveProjection_` listener on `ADDED_TO_STAGE` sets a fixed perspective projection on `root`.
- On console (`IsConsole()`): starts invisible, uses `ENTER_FRAME` loop checking `onTargetFrame()` before showing and calling `setupTranslation()`.
- In editor (`!inIggy`): populates example entries for layout preview.

---

## Other game-specific classes

- `QuestEntry` — [Embed symbol18] Single active quest row: `txt_questName:TextField`, `questDifficultyIcon:MovieClip` (frames: `"1star"`, `"2star"`, `"3star"`, `"5star"`, `"complete"`, `"failed"`), focus background. `sortOrder=51`. Background height auto-fits to text height.
- `ChallengeEntry` — [Embed symbol95] Weekly challenge row with name, description, level badge, progress (`"current/goal"`), countdown `timer:CountdownTimer`, clock animation, reward image (`ObjectPreview 32×32`), reward quantity/name, bonus text, daily-bonus section, and daily-bonus-complete tick. `sortOrder=11`. When timer < 60 s remaining, shows `clockAnim`. `$Challenges_TimeUp` shown when expired. 3-frame platform variant.
- `PersonalObjectiveEntry` — [Embed symbol46] Golden Thread (personal objective) row: `txt_questName`, `txt_description`, `txt_progress` (shown only when `goal > 1`), reward preview (`ObjectPreview 32×32`), dual icon previews (top/bottom, 64×64), `CountdownTimer`, `objectiveCompleted` MC. Supports `trackBtn:LabelButton` (dispatches `TRACK_ACTIVITY` DataEvent) and `removeBtn:BaseButton` (dispatches `REMOVE_ACTIVITY`). Translates button labels `$ActivityTrackerTrackButton` / `$ActivityTrackerTrackedButton`. `sortOrder=1`. 3-frame platform variant.
- `BadgeEntry` — [Embed symbol109] Badge progress row: 64×64 badge image, `BadgeProgressBar` or plain `taskText`, reward carousel (`rewardsCarrousel` MC with up to 5 reward slots `reward00`…`reward04`, left/right arrow buttons). Dispatches `REWARD_ROLLOVER` / `REWARD_ROLLOUT` DataEvents with position data for tooltip placement. `KiwiTextUtil.resizeFont` applied to name and task text. `sortOrder=1`.
- `AlertEntry` — [Embed symbol168] Transient challenge-alert pop-up. Plays a `"tweenIn"` animation on construction, then a `"tweenOut"` after the timer (`DEFAULT_TIMER=7000 ms` or `RAMPAGE_TIMER=10000 ms`, polled every 500 ms). On timer complete dispatches `QuestTracker.ALERT_TIMER` event. Height is fixed at 71. `iconContainer` frames by `alertType` string; falls back to `"default"` if the label does not exist.
- `DelveEntry` — [Embed symbol61] Delve dungeon-run tracker: title, depth, timed counter (`timerField` turns red when `warning=true`), requirement list (`descField` with per-requirement colour coding: white=normal, grey=met, red=failed), and a `rewardImageContainer` of Deltalith `ObjectPreview` images. `TimeUtil.localizeTime` used for timer display. 3-frame platform variant.
- `EventHighlight` — [Embed symbol50] Transient notification banner: `content:MovieClip` wraps `txt_description` and `background`. Description auto-sizes; background height fits text. Animated in/out by `IggyTween` in `QuestTracker`.
- `SimpleProgressBar` — No embed; base progress-bar component. Masks `fillingMaskMC` width proportionally to `currentValue / maxValue`.
- `BadgeProgressBar` — [Embed symbol106] Extends `SimpleProgressBar`; adds `progressText:TextField` (shows `"current/maxValue"`) and `taskText:TextField`. Overrides `maxValue`/`currentValue` setters to update `progressText`.

### `QuestTracker_fla` timeline symbols (6 classes)
- `AlertIconGrp_4` — icon group MC for the alert entry.
- `new_timer_bar_5` — animated timer bar symbol.
- `alertIconsImageGrp_11` — alert icon image group.
- `default_timer_bar_18` — default timer bar style.
- `rampage_timer_bar_20` — rampage-specific timer bar style.
- `clock_animation_34` — ticking clock animation used in `ChallengeEntry`.
- `badge_35` — animated badge level indicator for `ChallengeEntry` (`gotoAndStop(level+1)`).
- `quest_difficulty_icon_45` — difficulty star icon used in `QuestEntry`.

### Asset-wrapper symbols (1 class)
`image` — bare image/bitmap container symbol.

---

## Notable logic

- **Delve always at top**: When `addDelveInfo` creates a `DelveEntry`, it immediately swaps it to position 0 in `questsStackList` so the Delve tracker always appears above all other entries.
- **Dual-icon logic in Golden Thread**: When `updateGoldenThreadObjective` receives a non-empty `bottomIcon`, the visual "top" slot gets `bottomIcon` and the visual "bottom" slot gets `topIcon` — swapped from the parameter names, allowing the server to pass a secondary icon that overrides the primary display position.
- **Alert auto-dismiss**: `AlertEntry` is self-timing — it starts its own `Timer` in the constructor and fires `QuestTracker.ALERT_TIMER` when done. `QuestTracker.onAlertTimer` then calls `removeQuest(ALERT_ID)` to clean it up.
- **Notification tween chain**: `addNotification` → bounce-in `IggyTween` → `motionFinishCallback = hideNotification` → fade-out `IggyTween` → `motionFinishCallback = removeNotification`. No explicit timer; the animation drives the lifecycle.
- **Perspective projection**: The 3D perspective projection applied to `root` (`fieldOfView=36.656964`, `projectionCenter=(275,200)`) suggests the tracker may use 3D transforms or depth effects on some entry animations.
- **Console show-defer**: On console the entire tracker is hidden until `onTargetFrame()` returns true (i.e. the SWF has reached the target platform frame), at which point it becomes visible and calls `setupTranslation()`. This prevents a flash of unstyled content before the console-specific fonts/layout are loaded.
- **Challenge + Alert pairing**: On the first call to `updateChallenge`, if `alertType != ""`, an `AlertEntry` is created alongside the `ChallengeEntry` to show a pop-up announcement. Subsequent calls update the existing `ChallengeEntry` without creating a new alert.
