# minigamescorecard.swf
> End-of-match scorecard window displayed after a Trove minigame concludes. Shows a ranked leaderboard of players with their scores, a set of up to four level-gated rewards available for that match, and a victory summary panel with earned rewards once a winner is declared.

**Document/main class:** `MiniGameScorecard` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 11 (main + 5 game-logic classes + 5 `MiniGameScorecard_fla` timeline symbols + button/asset helpers)

## Main class: `MiniGameScorecard`

Owns a `WindowHeaderSmall` (title `$Minigame_Header`), a header `MovieClip` with a score column label (`header.txt_score`), a `Rewards` component, and a `ScoreList`. On console, a `ButtonLegendClose` visibility is deferred until `onTargetFrame()` via `ENTER_FRAME`. Registers three `ExternalInterface` callbacks in `configUI`.

### Public methods

- `setVictory(level:int, bestRewardLevel:int) : void` — delegates to `Rewards.setVictory`; shows `ButtonLegendClose` on console.
- `setScoreType(type:int) : void` — sets `scoreList.scoreType`; swaps `header.txt_score` between `$MINIGAME_TIME` and `$MINIGAME_SCORE`.
- `setSortType(type:int) : void` — sets `scoreList.sortType` (ascending or descending).

### Key fields

- `rewards : Rewards` — sub-component managing up to 4 reward slots and the EOM earned panel.
- `scoreList : ScoreList` — scrollable stack of `ScoreListing` rows.
- `header : MovieClip` — header row containing `txt_score:TextField`.
- `ButtonLegendClose : MovieClip` — console button hint for closing; hidden until game result is received.
- `currentLevel : int` — cached victory level (−2 = not yet set).

### Frame scripts / timeline

- **frame 1 / 11 / 21** — `stop()` for PC, Console, and ConsoleLoc layouts respectively.

### Runtime dependencies & integration

- `ExternalInterface` callbacks: `setScoreType`, `setSortType`, `setVictory`.
- `IggyFunctions.inIggy` gate.
- `setupTranslation()` called from constructor.
- Translate keys: `$Minigame_Header`, `$MINIGAME_TIME`, `$MINIGAME_SCORE`.

---

## Other game-specific classes

### `ScoreList` (extends `_kiwi.Controls.StackList`) — Embed symbol107

Maintains an internal `scoreData:Array` of `ScoreData` objects and keeps a matching set of `ScoreListing` display children in sync. Sorting is done client-side with custom comparators; ties are broken by `scoreUpdatedTime` (earlier update wins in ascending; earlier update also wins in descending).

**Public methods:**
- `addPlayer(name:String) : void` — creates a `ScoreData`, pushes it, calls `updateRows()`.
- `removePlayer(name:String) : void` — splices by name, calls `updateRows()`.
- `setPlayerScore(name:String, score:Number, level:int) : void` — updates score and timestamps `scoreUpdatedTime`; calls `updateRows()` only if score changed.

**Key fields:**
- `scoreData : Array` — array of `ScoreData`; sorted in-place before every redraw.
- `_sortType : int` — `SORTING_ASCENDING` (0) or `SORTING_DESCENDING` (1, default).
- `_scoreType : int` — `SCORE_INTEGER` (0, default) or `SCORE_TIME` (1); controls `formatScore()` output.

**Score formatting:** integer scores displayed as plain integers; time scores formatted as `MM:SS` (milliseconds are computed but not displayed in the formatted string).

**ExternalInterface callbacks:** `addPlayer`, `removePlayer`, `setPlayerScore`. Test data injected when not in Iggy.

### `ScoreListing` (extends `MovieClip`) — Embed symbol36

Row display object: `txt_rank`, `txt_name`, `txt_score` TextFields; `levelIcon:MovieClip` (driven by `gotoAndStop(level + 1)`); `friendIcon:MovieClip`. Frame scripts at frames 2 and 3 (`stop()`).

### `ScoreData`

Plain value object: `name:String`, `score:Number`, `level:int`, `scoreUpdatedTime:Number`. No display logic.

### `Rewards` (extends `_kiwi.Core.UIComponent`) — Embed symbol151

Manages two child panels: `rewardsAvailable` (4 `RewardInfo` slots shown pre-victory) and `rewardsEarned` (EOM panel shown post-victory). On console, fires `ExternalInterface.call("OnConfigured")` after first `ENTER_FRAME`.

**Key methods:**
- `setReward(slot, scoreThreshold, qty, ghosted, iconImage)` — populates one `RewardInfo` slot (1–4); sets `artClip.iconImage`, `txt_level`, `txt_score`, `txt_quantity`, ghosting, and check-mark visibility.
- `setVictoryReward(slot, qty, ghosted, iconImage)` — populates the corresponding earned slot in `rewardsEarned.rewards`.
- `setVictory(level, bestRewardLevel)` — switches visibility to `rewardsEarned`, fills `txt_level` / `txt_nextLevel` texts, sets `levelIcon` frame, wires `btnExit` click to `ExternalInterface.call("OnRequestClose")`.

**Translate keys:** `$Minigame_Level` (replace `{0}`), `$Minigame_Level_Earned` (replace `{0}`), `$Minigame_No_Level_Earned`, `$Minigame_Level_Next` (replace `{0}`).

### `RewardInfo` (extends `MovieClip`) — Embed symbol116

Single reward display row: `artClip:ArtClip`, `txt_score:TextField`, `txt_level:TextField`, `txt_quantity:TextField`, `check:MovieClip`, `levelIcon:MovieClip`.

### `MiniGameScorecard_fla` timeline symbols

| Class | Symbol | Role |
|---|---|---|
| `rewardInfoEOM_4` | symbol148 | EOM earned-rewards panel; holds 4× `artClip`/`txt_quantity`/`check` triplets, `levelIcon`, `txt_level`, `txt_nextLevel`, `txt_reward`, and `btnExit:btnGreenIcon_small` (label `$Scorecard_ExitGame`). |
| `iconLVLReward_11` | symbol108 | Multi-frame clip for level-badge icon in reward slots; `stop()` on frame 1. |
| `equipped_23` | symbol49 | Two-frame clip (equipped / unequipped indicator). |
| `scoreChange_35` | symbol30 | Single-frame clip; score-change animation placeholder. |
| `YouIcon_36` | symbol35 | Three-state clip (frames 1/10/19) marking the local player in the score list. |
| `levelIconMC_8` | symbol (no explicit Embed seen) | Level badge used in the score rows (referenced via `ScoreListing.levelIcon`). |
| `iconLVL_34` | — | Additional level icon variant. |
| `slotFrame_21` / `slotFrameLarge_32` | — | Slot frame decorators for reward slots. |

### Button/asset helpers (no game logic)

- `btnGreen` (extends `LabelButton`) — Embed symbol81; green button, four-state frame stops.
- `btnGreen_small` (extends `LabelButton`) — Embed symbol91; small variant.
- `btnGreenIcon_small` (extends `LabelButton`) — Embed symbol126; icon+label small button (used as Exit button in EOM panel).
- `slot_large` (extends `_kiwi.Controls.Slot`) — Embed symbol64; large item slot graphic.
- `dummy` (extends `BitmapData`) — Embed `53_dummy.png`; placeholder bitmap for unloaded art.
- `rarity_frame_*` (×9 PNG/non-PNG wrappers) — rarity tier border graphics.
- `rarity_frame_stellar` — stellar rarity border (no `_png` suffix, pure MovieClip).

## Notable logic

- Score sort uses a stable-by-time comparator: if two players share the same score, the one whose score was set earlier wins in descending sort (i.e. first to achieve the score ranks higher).
- `KiwiTextUtil.resizeFont` and `KiwiTextUtil.alignTextFieldVertically` are called on the EOM panel's `txt_level` field to fit long localised strings.
- The `Rewards` component references child objects by name string (`"reward1"` … `"reward4"`, `"artClip1"` … `"artClip4"`, `"check1"` … `"check4"`, `"txt_quantity1"` … `"txt_quantity4"`) via `getChildByName`, so the FLA timeline naming is load-bearing.
