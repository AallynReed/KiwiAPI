# royalescorecard.swf
> The end-of-match (and mid-match) scoreboard for Trove's Bomber Royale mode. Displays a ranked list of all players with stats (kills, damage, power-ups, time alive), reward tier icons, a winner dance panel, and action buttons (Exit, Play Again, Spectate). The card has three modes — alive, dead, and match-over — and updates in real time as players are eliminated.

**Document/main class:** `RoyaleScoreCard` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 4 (excluding framework/asset wrappers)

## Main class: `RoyaleScoreCard`

Pre-allocates 20 `RoyalePlayerListing` instances split between a `StackList` (used while alive, fixed positions) and a `ScrollableTileView` (used when dead/over, scrollable). Exposes all data entry via `ExternalInterface` callbacks. On construction, button labels are set from translate keys. In preview mode (non-Iggy) seeds 20 players and sets mode 2 (over).

### Public methods
- `setMode(modeIndex:int, publicQueue:Boolean) : void` — Maps index to "alive"/"dead"/"over", calls `gotoAndStop`, shows/hides scroll list, action buttons, and console button legend. In MODE_OVER repositions elements (scroll list x=314, exit btn x=680, etc.).
- `addPlayer(name, isAlive, isViewer, rank, timeAlive, kills, powerups, damage, firstBlood) : *` — Routes to the correct listing object by rank; in dead/over mode auto-scrolls the viewer's row into view if rank > 10. Updates `rewardToHighlight` for the viewer and calls `rewardsContainer.update()`.
- `setRewardInfo(blood, bronze, silverCount, goldCount, silverSlots, bronzeSlots) : void` — Populates `rewardQuantities[frame]` and builds `rewardPlacements[]` array mapping each rank to a reward tier frame index.
- `showDancer(textureName, winnerName, clubName) : void` — Fills the `dancePanel` with the winner's character texture and name; a second call adds a second winner ("name + name") and clears the club name if they differ.
- `setPlayerInQueue(inQueue:Boolean) : void` — Toggles `btnPlay` label between `$PVP_QUEUE_ENTER_QUEUE` and `$PVP_QUEUE_LEAVE_QUEUE`.
- `onScrollList(delta:Number) : void` — Scrolls the player list by `delta * 30 / verticalStep` pixels (controller scroll support).

### Key fields
- `currentMode : String` (static) — `"alive"`, `"dead"`, or `"over"`.
- `rewardQuantities : Array` (static) — indexed by `RoyaleRewards.FRAME_*` constants, holds counts per tier.
- `rewardPlacements : Array` (static) — per-rank reward tier: index 0 = gold, then silver slots, then bronze slots.
- `rewardToHighlight : int` (static) — the reward tier to animate for the local viewer.
- `viewerFirstBlood : Boolean` (static) — whether the viewer got first blood (controls blood reward animation).
- `playerStackList : StackList` — 20 fixed-position `RoyalePlayerListing` children, used in alive mode.
- `playerScrollList : ScrollableTileView` — 20 scrollable listings, used in dead/over modes.
- `rewardsContainer : RoyaleRewards` — the reward-icon strip at the top.
- `dancePanel : MovieClip` — winner showcase panel (contains `art`, `art2` ArtClips, `winnerNameText`, `winnerClubNameText`).
- `btnExit / btnPlay / btnSpectate : LabelButton` — post-match action buttons.
- `buttonLegend : MovieClip` — console button hints panel (contains `btnX`, `btnXTextField`, `buttonY`, `buttonYTextField`).
- `PENUMBRA : int = 6` — y-offset applied to dead players' rows to create visual separation.

### Frame scripts / timeline
- Frame labels: `"alive"` (frame 1), `"dead"` (frame 11), `"over"` (frame 21), each with a `stop()`.

### Runtime dependencies & integration
- `ExternalInterface` callbacks: `setRewardInfo`, `setMode`, `addPlayer`, `showDancer`, `setPlayerInQueue`, `onScrollList`.
- `ExternalInterface.call`: `OnRequestClose`, `OnPlayAgain`, `OnSpectate`.
- Translate keys: `$Scorecard_ExitGame`, `$Scorecard_NewGame`, `$PVP_battleroyale_spectate`, `$PVP_QUEUE_ENTER_QUEUE`, `$PVP_QUEUE_LEAVE_QUEUE`.
- `IsConsole()` — switches between mouse buttons and `buttonLegend` controller hints.
- Static state on `RoyaleScoreCard` is read by both `RoyalePlayerListing` and `RoyaleRewards`, creating a shared-state coupling across the three classes.

---

## Other game-specific classes

### `RoyalePlayerListing` (extends `MovieClip`) — Embed symbol32
Individual player row. Fields: `placeText`, `playerNameText`, `timeAliveText`, `killCountText`, `damageText`, `powerupsText`, `iconDead`, `rewardIcon`, `rewardIconBlood`. Frame labels: "alive", "dead", "playeralive", "playerdead" (composite of `isViewer` + `isAlive` strings). `updateData(...)` fills all text fields and applies colour: viewer = white (0xFFFFFF), alive non-viewer = 0xC6ABDD, dead = 0x778E7E. Time alive is formatted via `_kiwi.Util.TimeUtil.formatTime(minutes, seconds, false)`. `rewardIcon` goes to the frame from `rewardPlacements[rank]` and is visible when dead or in MODE_OVER. `rewardIconBlood` is shown when `firstBlood` is true (stops at frame 4). `height` is overridden to subtract 2px (row gap).

### `RoyaleRewards` (extends `_kiwi.Core.UIComponent`) — Embed symbol90
Strip of 4 reward icons (`reward0`–`reward3`) displayed above the player list. Icon order is fixed: blood (4), bronze (1), silver (2), gold (3). `update()` iterates reward slots, sets each icon's inner `icon` clip to the appropriate frame, writes the quantity text, and shows/hides the `animation` clip (pulsing glow): blood animates when viewer has first blood; others animate when their frame matches `rewardToHighlight`. Shifts x to 311 in MODE_OVER. Calls `ExternalInterface.call("RequestRewards")` on construction.

### `PreviewContainer` (extends `_kiwi.Core.ObjectPreview`) — Embed symbol63
Trivial subclass used as a typed handle for the object preview widget in the scorecard (likely the character model viewer). No added logic.

### `RoyaleScoreCard_fla.rewardIconLarge_42` — Embed symbol85
Timeline symbol for the large reward icon (4 frames: bronze/silver/gold/blood), each `stop()`.

### `RoyaleScoreCard_fla.rewardIcon_50` — Embed symbol15
Timeline symbol for the small per-row reward icon (same 4-frame structure).

### Asset wrappers
`btnGreen`, `btnGreenIcon_small`, `btnGreenIcon_medium` — button skin assets, no logic. Standard scroll skins (12 classes).

## Notable logic
- **Static shared state**: `RoyaleScoreCard.currentMode`, `rewardQuantities`, `rewardPlacements`, `rewardToHighlight`, and `viewerFirstBlood` are all static, allowing `RoyalePlayerListing` and `RoyaleRewards` (which receive no constructor arguments) to read the current state directly without callbacks.
- **Dual list architecture**: The `StackList` (alive) keeps all 20 rows at fixed positions without scrolling; the `ScrollableTileView` (dead/over) enables scrolling as the board fills up. `hasAutoScrolled` ensures the view scrolls to the viewer's row exactly once on first render.
- **Reward placement algorithm**: Gold is always rank 0. The `silverCount` and `bronzeCount` parameters from `setRewardInfo` fill subsequent positions, so the reward tiers follow the match's configured prize structure.
- **Dance panel dual-winner**: `showDancer` is called once per winner. The first call sets `art.textureName`; the second call sets `art2.textureName`, shifts `art` left by 150px instead of 75px, and concatenates names. Club name is blanked if winners are from different clubs.
