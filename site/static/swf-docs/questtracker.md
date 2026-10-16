# questtracker.swf
> The Quest Tracker is Trove's heads-up objective overlay, shown persistently during gameplay to display the current adventure's dungeons, the Golden Thread personal objective, the hourly challenge, badge progress, Delve depth/timer information, and transient event notifications. Entries are stacked vertically in a `StackList` and updated in real-time by the game engine.

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

- `updateBadge(badgeId, displayName, taskDesc, badgeImage, goal, current)` — creates or updates the single `BadgeEntry`; sets `badgeId`, `badgeImage`, and task progress via `setTaskProgress(taskDesc, goal, current)`. Goal comes before current here, unlike every other progress pair on this screen; a goal of `0` shows the task as plain `taskText` with no progress bar.
- `removeBadge()` — removes the `BadgeEntry` from the stack via `removeQuest(BADGE_ID)`.
- `updateQuest(questId, difficulty, name, progress, failed)` — creates or updates a `QuestEntry`; `complete = progress >= 1`. One row per dungeon on the player's current adventure, keyed by the engine's quest id — the only entry type that can appear more than once (every other type sits under a fixed sentinel key).
- `removeQuest(questId)` — removes the entry stored under the given key from both `entries` and `questsStackList`. Any key works, quest id or sentinel: `removeBadge`, `removeChallenge`, `removeGoldenThreadObjective` and `onAlertTimer` all route through it (`removeNotification` and `removeDelveInfo` remove their entries directly).
- `updateGoldenThreadObjective(name, desc, current, goal, rewardImage, rewardName, rewardQty, bonusQty, topIcon, bottomIcon, showTimer)` — creates or updates the `PersonalObjectiveEntry`; `name` is the title shown in `txt_questName` (the entry itself is always keyed `GOLDEN_ID`); manages dual-icon display (if `bottomIcon` is non-empty, shows it as `topIconImage` and `topIcon` as `bottomIconImage`); calls `setTimerVisibility`.
- `updateGoldenThreadObjectiveWithTimer(name, timerVisible, timerMs, desc, current, goal, rewardImage, rewardName, rewardQty, bonusQty, topIcon, bottomIcon)` — variant that also calls `setTimer(timerMs, timerVisible)` on the entry; `timerMs` is the time remaining in milliseconds (shown with hours).
- `setGoldenThreadComplete()` — sets `PersonalObjectiveEntry.progress = goal` to mark it done.
- `removeGoldenThreadObjective()` — delegates to `removeQuest(GOLDEN_ID)`.
- `updateChallenge(challengeName, description, level, current, goal, rewardName, rewardQty, rewardImage, bonusName, timerMs, dailyBonusName, dailyBonusQty, dailyBonusComplete, alertType)` — on first creation, if `alertType != ""` also creates an `AlertEntry` keyed `ALERT_ID`; creates the `ChallengeEntry`; sets all fields and `setTimer(timerMs)`. `bonusName` is the class-bonus line (`txt_bonus`, under the description); `timerMs` is the time left in milliseconds.
- `removeChallenge()` — delegates to `removeQuest(CHALLENGE_ID)`.
- `onAlertTimer(event:DataEvent)` — handler for `ALERT_TIMER` events from `AlertEntry`; calls `removeQuest(ALERT_ID)`.
- `addNotification(text)` — creates an `EventHighlight`, adds it to the stack, starts a `IggyTween` bouncing it in from x=-20 to x=10 (Bounce.easeOut, 2 sec). On tween finish calls `hideNotification`.
- `hideNotification()` — fades the `EventHighlight` out (alpha 1→0, 1 sec `IggyTween`); on finish calls `removeNotification`.
- `removeNotification()` — removes `EventHighlight` from the stack.
- `addDelveInfo(questName, title, depth)` — creates the `DelveEntry` on the first call (later calls only update `title`, `depth` and `questName`) and moves it to index 0 via `questsStackList.swapChildrenAt(0, numChildren-1)`.
- `updateDelveInfo(requirementIndex, description, status)` — calls `DelveEntry.setRequirement`.
- `updateDelveTime(timeLeftMs, warning)` — sets `DelveEntry.timer` and `DelveEntry.warning`. The value is the floor's time left in milliseconds (it feeds `TimeUtil.localizeTime`, whose unit table is in ms); it counts down and goes negative once the floor is over its time. No callback reports a floor's total time: the first value sent for a floor is its full allowance. The engine sends it repeatedly, many times per second.
- `removeDelveInfo()` — removes the `DelveEntry` from the stack.
- `resizeDelveInfo(newSize)` — trims `DelveEntry.m_requirements` to `newSize` elements.
- `clearDeltaliths()` — clears the `DelveEntry`'s `rewardImageContainer` children. The engine sends it on every delve floor.
- `addDeltalith(textureName)` — appends an `ObjectPreview(32,32)` to `DelveEntry.rewardImageContainer`, 33 px after the previous one. The argument is the deltalith's texture name, e.g. `questreward.placeable/deco/delve/deltalith_death_boon.unlocked.0`.

### Key fields

- `questsStackList : StackList` — the single vertical container, placed on the root timeline at (11, 5) on the 350 × 400 stage; item spacing 12 px.
- `entries : Dictionary` — maps sentinel strings to their live entry instances.
- `notificationTween : IggyTween` — holds the current bounce-in / fade-out tween for the `EventHighlight`.

### Frame scripts / timeline

- **frame 0** (label `PC`) — `stop()` (PC mode).
- **frame 10** (label `Console`) — `stop()` (Console mode; tracker already made visible via `onEnterFrame` loop at this point).

### Runtime dependencies & integration

- All `ExternalInterface.addCallback` calls are guarded by `IggyFunctions.inIggy`.
- Callbacks registered: `updateBadge`, `removeBadge`, `updateQuest`, `removeQuest`, `updateGoldenThreadObjective`, `removeGoldenThreadObjective`, `updateChallenge`, `removeChallenge`, `setGoldenThreadComplete`, `addNotification`, `addDelveInfo`, `updateDelveInfo`, `updateDelveTime`, `removeDelveInfo`, `resizeDelveInfo`, `clearDeltaliths`, `addDeltalith`, `updateGoldenThreadObjectiveWithTimer`.
- `IggyTween` used for notification slide-in (Bounce.easeOut) and fade-out animations.
- `fl.transitions.easing.Bounce` imported directly for the notification tween.
- `draw()` override: when `InvalidationType.DATA` is pending, calls `questsStackList.RefreshLayout()`.
- `__setPerspectiveProjection_` listener on `ADDED_TO_STAGE` sets a fixed perspective projection on `root`.
- On console (`IsConsole()`): starts invisible, uses `ENTER_FRAME` loop checking `onTargetFrame()` before showing and calling `setupTranslation()`.
- In editor (`!inIggy`): populates example entries for layout preview.
- No outbound calls from the SWF's own classes: none of them uses `ExternalInterface.call`. The only outbound traffic is the shared framework's — each `ObjectPreview` calls `UIComponent.CheckTextureExists` for its texture (and registers `objectPreviewReady`), and `UIComponent` makes its platform queries.
- The tracker reports no size; the engine places it. It is stacked directly under `starbar.swf`, at whatever height the star bar reports through `NOTIFY_RESIZED`, with both hung off the same right edge — so this 350 px stage starts 24 px inside the star bar's 374 px strip. The engine's box for the tracker is 350 px wide regardless of the SWF's declared stage, and drawing past it is not clipped.
- The 18 callbacks are the whole engine contract; no public field is engine-written. `sortOrder` / `secondarySortOrder`, `badgeRank`, `displayType` and `isComplete` are never read in this SWF (`StackList` keeps arrival order and has no sort), and `activityId` only by the track/remove button handlers, whose buttons this SWF does not place. The entry classes share their names with `activitytrackerui.swf`, which does sort on `sortOrder` and use `activityId` / `badgeRank`.

---

## Other game-specific classes

- `QuestEntry` — [Embed symbol18] Single adventure-dungeon row: `txt_questName:TextField`, `questDifficultyIcon:MovieClip` (frames: `"1star"`, `"2star"`, `"3star"`, `"5star"`, `"complete"`, `"failed"`; `difficulty` 0–3 picks the first four, clamped), focus background. `sortOrder=51`. Background height auto-fits to text height.
- `ChallengeEntry` — [Embed symbol95] Hourly challenge row with name, description, level badge, progress (`"current/goal"`), countdown `timer:CountdownTimer`, clock animation, reward image (`ObjectPreview 32×32`), reward quantity/name, class-bonus text (`txt_bonus`), daily-bonus section, and daily-bonus-complete tick. `sortOrder=11`. When timer < 60 s remaining, shows `clockAnim`. `$Challenges_TimeUp` shown when expired. 3-frame platform variant.
- `PersonalObjectiveEntry` — [Embed symbol46] Golden Thread (personal objective) row: `txt_questName`, `txt_description`, `txt_progress` (shown only when `goal > 1`), reward preview (`ObjectPreview 32×32`), dual icon previews (top/bottom, 64×64), `CountdownTimer`, `objectiveCompleted` MC. Supports `trackBtn:LabelButton` (dispatches `TRACK_ACTIVITY` DataEvent) and `removeBtn:BaseButton` (dispatches `REMOVE_ACTIVITY`). Translates button labels `$ActivityTrackerTrackButton` / `$ActivityTrackerTrackedButton`. This SWF's symbol46 places neither button (nor `txt_source`) and nothing here listens for the events; they are used by the same class in `activitytrackerui.swf`. `sortOrder=1`. 3-frame platform variant.
- `BadgeEntry` — [Embed symbol109] Badge progress row: 64×64 badge image, `BadgeProgressBar` or plain `taskText`, reward carousel (`rewardsCarrousel` MC with up to 5 reward slots `reward00`…`reward04`, left/right arrow buttons). Dispatches `REWARD_ROLLOVER` / `REWARD_ROLLOUT` DataEvents with position data for tooltip placement. In this SWF the carousel is inert: symbol109 has no `rewardsCarrousel` child and nothing calls `addReward`. `KiwiTextUtil.resizeFont` applied to name and task text. `sortOrder=1`.
- `AlertEntry` — [Embed symbol168] Transient challenge-alert pop-up: title `$HourlyChallengeTitle` ("Hourly Challenge") over the challenge name. Plays a `"tweenIn"` animation on construction, then a `"tweenOut"` after the timer (`DEFAULT_TIMER=7000 ms` or `RAMPAGE_TIMER=10000 ms`, polled every 500 ms). On timer complete dispatches `QuestTracker.ALERT_TIMER` event. Height is fixed at 71. `iconContainer` frames by `alertType` string; falls back to `"default"` if the label does not exist (its labels are `new`, `default`, `rampage`). Its `icon` child is sent to the `alertType` frame too: `quest`, `rampage`, `minigameportal`, `pvp`, `monster`, `amberine`, `lazul`, `th'lan`, `gabbro`.
- `DelveEntry` — [Embed symbol61] Delve dungeon-run tracker: title, depth, timed counter (`timerField` turns red when `warning=true`, is hidden at 0, and shows negative time as `-` before the absolute value), requirement list (`descField` with per-requirement colour coding by status: 0 white=normal, 1 grey=met, 2 red=failed; `completeField` shows once every requirement is met), and a `rewardImageContainer` of Deltalith `ObjectPreview` images. `TimeUtil.localizeTime` used for timer display. 3-frame platform variant.
- `EventHighlight` — [Embed symbol50] Transient notification banner: `content:MovieClip` wraps `txt_description` and `background`. Description auto-sizes; background height fits text. Animated in/out by `IggyTween` in `QuestTracker`.
- `SimpleProgressBar` — No embed; base progress-bar component. Masks `fillingMaskMC` width proportionally to `currentValue / maxValue`.
- `BadgeProgressBar` — [Embed symbol106] Extends `SimpleProgressBar`; adds `progressText:TextField` (shows `"current/maxValue"`) and `taskText:TextField`. Overrides `maxValue`/`currentValue` setters to update `progressText`.

### `QuestTracker_fla` timeline symbols (8 classes)
- `AlertIconGrp_4` — icon group MC for the alert entry (`AlertEntry.iconContainer`); frames `new`, `default`, `rampage`.
- `new_timer_bar_5` — animated timer bar symbol, on `AlertIconGrp_4`'s `new` frame.
- `alertIconsImageGrp_11` — alert icon image group (`iconContainer.icon`), one frame per alert type.
- `default_timer_bar_18` — default timer bar style, on `AlertIconGrp_4`'s `default` frame.
- `rampage_timer_bar_20` — rampage-specific timer bar style, on `AlertIconGrp_4`'s `rampage` frame.
- `clock_animation_34` — ticking clock animation used in `ChallengeEntry`.
- `badge_35` — animated badge level indicator for `ChallengeEntry` (`gotoAndStop(level+1)`).
- `quest_difficulty_icon_45` — difficulty star icon used in `QuestEntry`.

### Asset-wrapper symbols (1 class)
`image` — bare image/bitmap container symbol.

---

## Notable logic

- **Delve always at top**: When `addDelveInfo` creates a `DelveEntry`, it immediately swaps it to position 0 in `questsStackList` so the Delve tracker always appears above all other entries. `StackList.swapChildrenAt` exchanges the two entries, so whatever was first moves to the bottom; everything else keeps arrival order.
- **Dual-icon logic in Golden Thread**: Both update callbacks end with two texture names. With only the first set, it is the top icon and there is no bottom icon; with both set, the second is the top icon and the first sits under it.
- **Deltalith names**: The prefab path inside an `addDeltalith` texture name also keys that deltalith's in-world sign text — `placeable/deco/delve/deltalith_death_boon` gives `$prefabs_placeable_deco_delve_deltalith_death_boon_interactable_sign_title` ("Death Boon") and the matching `_sign_content` key; `prefabs_placeable_deco_delve.binfab` holds 77 such pairs. The stock tracker shows only the 32×32 icon, with no name or tooltip.
- **Alert auto-dismiss**: `AlertEntry` is self-timing — it starts its own `Timer` in the constructor and fires `QuestTracker.ALERT_TIMER` when done. `QuestTracker.onAlertTimer` then calls `removeQuest(ALERT_ID)` to clean it up.
- **Notification tween chain**: `addNotification` → bounce-in `IggyTween` → `motionFinishCallback = hideNotification` → fade-out `IggyTween` → `motionFinishCallback = removeNotification`. No explicit timer; the animation drives the lifecycle.
- **Perspective projection**: The 3D perspective projection applied to `root` (`fieldOfView=36.656964`, `projectionCenter=(275,200)`) suggests the tracker may use 3D transforms or depth effects on some entry animations.
- **Console show-defer**: On console the entire tracker is hidden until `onTargetFrame()` returns true (i.e. the SWF has reached the target platform frame), at which point it becomes visible and calls `setupTranslation()`. This prevents a flash of unstyled content before the console-specific fonts/layout are loaded.
- **Challenge + Alert pairing**: On the first call to `updateChallenge`, if `alertType != ""`, an `AlertEntry` is created alongside the `ChallengeEntry` to show a pop-up announcement. Subsequent calls update the existing `ChallengeEntry` without creating a new alert.
