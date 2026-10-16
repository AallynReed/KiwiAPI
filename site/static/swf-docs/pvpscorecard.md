# pvpscorecard.swf
> End-of-match scorecard displayed after a PVP battle in Trove. Shows both teams' player scores (kills, deaths, captures, returns), a victory/defeat header, team names and aggregate scores, a PVP meta-level progress bar, the match's bronze/silver/gold/first-blood reward counts, and buttons to exit or play again.

**Document/main class:** `PVPScoreCard` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 8

---

## Main class: `PVPScoreCard`

`PVPScoreCard` is the root document class. Its constructor registers frame scripts for frames 0, 10, and 20, hides the header and victory banner until a winner is declared, sets up the PVP meter mask, and calls `setupTranslation()`. On console, it also initializes `teamScoresUpdated` tracking and registers an `ENTER_FRAME` listener that waits until the component reaches its target frame before calling `showLegend(false)` and then (if a victor has already been set) calling `setVictor`. In `configUI()` all game callbacks are registered via `ExternalInterface.addCallback`; in the Flash IDE preview path a set of dummy players and metadata are added for testing.

### Public methods

- `reset() : void` — clears all player rows from both team lists (or resets `teamScoresUpdated` counters on console).
- `setVictor(param1:int) : void` — wires the Exit/Play-Again button click listeners, makes the header visible, hides `footerMask`, and shows the button legend.
- `hasVictor() : Boolean` — returns `true` when `winningTeam != -1`.
- `showLegend(param1:Boolean) : void` — toggles visibility of `playAgainButton`, `playAgainTextField`, and `ButtonLegendClose`.
- `setPVPMetaData(oldLevel, newLevel, oldPercent, newPercent, battleBoxes, battleFactor) : void` — animates the PVP meter `maskShape.scaleX` from `oldPercent` to `newPercent` using two sequential `IggyTween` calls (with optional level rollover); updates `txtMetalevel` and `txtBattleBoxes`.
- `setRewardInfo(blood:int, bronze:int, silver:int, gold:int, p5:int, p6:int) : void` — stores the four reward quantities in the static `rewardQuantities` (indexed by the `PVPRewards.FRAME_*` constants) and refreshes `rewardsContainer`. The fifth and sixth arguments are accepted but not read.
- `setViewerRewardTier(tier:int, firstBlood:Boolean) : void` — stores the viewer's reward tier (a `PVPRewards.FRAME_*` value) in `rewardToHighlight` and the first-blood flag in `viewerFirstBlood`, then refreshes `rewardsContainer`.
- `addTeamScore(team:int, color:uint, score:int) : void` — sets the appropriate `teamScoreN` TextField color (color is passed as `color >> 8`) and text.
- `overrideColumnHeaders(...rest) : void` — writes up to 4 strings into the `columnN` TextFields on both `teamHeader1` and `teamHeader2`.
- `addPlayerScore(team, isLocal, name, c0, c1, c2, c3) : void` — creates or updates a `ScoreListing` row in the appropriate team list. On console in update mode it finds an existing row by username and repositions it. When `isLocal` is true, repositions team header/score/list MovieClips for both teams, navigates `teamName` and `teamHeader` clips to the correct frame, refreshes `rewardsContainer`, and queues an `Event.RENDER` handler.
- `exitClicked(e:MouseEvent) : void` — calls `ExternalInterface.call("OnRequestClose")`.
- `playAgainClicked(e:MouseEvent) : void` — calls `ExternalInterface.call("OnPlayAgain")`.
- `onRender(e:Event) : void` — recalculates x-position of both team score TextFields based on team-name label width; calls `setupTranslation()`.

### Key fields

- `header : MovieClip` — victory/defeat banner; hidden until `setVictor` is called.
- `teamName1 / teamName2 : MovieClip` — team name clips (`headerFrame_2`); framed by team index. Both start on frame 1, which reads YOUR TEAM, until the local player's row arrives; then `teamName1` goes to frame `team` and `teamName2` to frame `team + 2`.
- `teamHeader1 / teamHeader2 : MovieClip` — column header clips (contain `column0`–`column3` TextFields via `headerClip_23`).
- `teamList1 / teamList2 : MovieClip` — containers that hold `ScoreListing` rows as children, stacked at `y = 32 * index`.
- `teamScore1 / teamScore2 : TextField` — aggregate score displays.
- `pvpMeter : MovieClip` — contains `fill` and `maskShape`; mask applied in constructor. Animated by `setPVPMetaData`.
- `txtMetalevel : TextField` — current PVP meta level.
- `txtBattleBoxes : TextField` — battle box count (format: `n/factor`).
- `btnExit : LabelButton` — label `"$Scorecard_ExitGame"`; listener attached when a victor is set.
- `btnPlay : LabelButton` — label `"$Scorecard_NewGame"`; listener attached when a victor is set.
- `footerMask : MovieClip` — masks the footer area until game ends.
- `winningTeam : int` — starts at -1; set by `setVictor`.
- `allPlayersAdded : Boolean` — set by `initialScoresAdded()`; switches console update path.
- `teamScoresUpdated : Array` — per-team row counters used for console re-ordering.
- `TWEEN_TIME : Number = 1` — total seconds for PVP meter fill animation.
- `rewardsContainer : PVPRewards` — the reward strip, a root child placed just above `teamName2`.
- `rewardQuantities : Array` (static) — reward counts indexed by `PVPRewards.FRAME_*`.
- `rewardToHighlight : int` (static, default -1) — the viewer's reward tier.
- `viewerFirstBlood : Boolean` (static) — whether the viewer earned first blood.

### Frame scripts / timeline

- **Frame 0 (`frame1`)** — `stop()`. PC layout.
- **Frame 10 (`frame11`)** — `stop()`. Console layout: tells `teamHeader1/2`, `header`, `teamName1/2`, `btnExit`, `btnPlay` to `gotoAndPlay("Console")`.
- **Frame 20 (`frame21`)** — `stop()`. Same console child redirections as frame 10 (second console variant).

### Runtime dependencies & integration

- **Iggy callbacks registered:** `reset`, `setVictor`, `setPVPMetaData`, `addTeamScore`, `overrideColumnHeaders`, `addPlayerScore`, `hasVictor`, `initialScoresAdded`, `setRewardInfo`, `setViewerRewardTier`.
- **ExternalInterface calls out:** `OnRequestClose`, `OnPlayAgain`, `RequestRewards` (from the `PVPRewards` constructor).
- **IggyTween:** used in `setPVPMetaData` for the progress-bar fill animation.
- **translate keys:** `"$Scorecard_ExitGame"`, `"$Scorecard_NewGame"` (set on buttons via `__setProp_*`).
- `setupTranslation()` called on construction and re-render.
- `IsConsole()` checked in constructor, `addPlayerScore`, `reset`, `setVictor`.

---

## Other game-specific classes

- `ScoreListing` — Embed symbol18; represents one player row. Constructor accepts (name, c0–c3 strings, isLocal). `setScoreText` sets `txt_kills`, `txt_deaths`, `txt_captures`, `txt_returns`; if `isLocal` is true, sets all fields to white (#FFFFFF). Two timeline frames (0 and 10) for PC/console layout.
- `PVPScoreCard_fla/headerFrame_2` — Embed symbol101; the team-name clip placed as `teamName1` and `teamName2`; contains `txt_label`; frames 1–5 are labelled `PC`, 6–10 `Console`, stops at frames 5 and 10. PC frames: 1 `$Scorecard_YourTeamHeaderName` ("YOUR TEAM") in red, 2 the same in blue, 3 `$Scorecard_EnemyNameTitle` ("ENEMY TEAM") in blue, 4 the same in red.
- `PVPScoreCard_fla/headerVictoryDefeat_21` — Embed symbol114; the `header` victory/defeat strip with `txt_label`; frames 1–5 `PC`, 6–10 `Console`, stops at frames 5 and 10. The local-player branch of `addPlayerScore` sends it to frame 1 (`$Scorecard_Victory`) when the local team is `winningTeam`, otherwise frame 2 (`$Scorecard_Defeat`).
- `PVPScoreCard_fla/headerClip_23` — Embed symbol130; four-column header row with `column0`–`column3` TextFields; four stopping frames (one per team × layout variant).
- `PVPRewards` (extends `UIComponent`) — Embed symbol85; the reward strip. Holds four slots `reward0`–`reward3`, each with an `icon` (`rewardIconLarge_19`), a `rewardCount` TextField, an `animation` clip and an `art` `PreviewContainer`. Frame constants: `FRAME_BRONZE = 1`, `FRAME_SILVER = 2`, `FRAME_GOLD = 3`, `FRAME_BLOOD = 4`; slots are shown in the order blood, bronze, silver, gold. `update()` sends each `icon` to its tier frame, writes the matching `rewardQuantities` entry into `rewardCount`, and shows `animation` on the slot whose tier equals `rewardToHighlight` (on the blood slot, when `viewerFirstBlood` is true). The constructor calls `update()` and then `ExternalInterface.call("RequestRewards")`.
- `PVPScoreCard_fla/rewardIconLarge_19` — Embed symbol80; four-frame reward icon (each frame `stop()`), framed by `PVPRewards.update()`.
- `PreviewContainer` (extends `_kiwi.Core.ObjectPreview`) — Embed symbol51; the `art` child of each reward slot. No code in this SWF sets it: the engine writes its `textureName` by path, `rewardsContainer.reward{0}.art`, where `{0}` is the slot index in `iconFrameOrder` order (0 blood, 1 bronze, 2 silver, 3 gold).
- **Asset wrappers:** `btnGreen`, `btnGreenIcon_small` — 2 button skin symbols.

---

## Notable logic

- **Score positioning:** `getScorePosition` computes the x of each team-score TextField by reading the team-name label's `textWidth`, centering the score under the team name. This is deferred to `Event.RENDER` so Flash has laid out the text first.
- **PVP meter rollover:** `setPVPMetaData` detects a level rollover (`newPercent <= oldPercent`), splits the animation at the `1.0` boundary, resets `maskShape.scaleX` to 0 and updates `txtMetalevel` mid-tween via a closure.
- **Console update path:** after `initialScoresAdded()` is called, `addPlayerScore` switches to finding and repositioning existing `ScoreListing` children (keyed by `txt_userName`) rather than appending new ones, enabling live score updates during a match.
- **Reward strip (October 2026 build):** the engine answers `RequestRewards` with `setRewardInfo` and marks the viewer's own result with `setViewerRewardTier`; every call re-runs `PVPRewards.update()`, as does the local-player branch of `addPlayerScore`.
- **Color encoding quirk:** `addTeamScore` shifts the incoming color right by 8 bits (`param2 >> 8`) before applying it to `textColor` — the game passes color as a shifted 32-bit value.
