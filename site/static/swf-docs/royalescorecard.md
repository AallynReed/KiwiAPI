# royalescorecard.swf
> The end-of-match (and mid-match) scoreboard for Trove's Bomber Royale mode. Displays a ranked list of all players with stats (kills, damage, power-ups, time alive), reward tier icons, a winner dance panel, the player's PvP lootbox progress at match end, and action buttons (Exit, Play Again, Spectate). The card has three modes — alive, dead, and match-over — and updates in real time as players are eliminated.

**Document/main class:** `RoyaleScoreCard` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 4 (excluding framework/asset wrappers)

## Main class: `RoyaleScoreCard`

Pre-allocates 20 `RoyalePlayerListing` instances split between a `StackList` (used while alive, fixed positions) and a `ScrollableTileView` (used when dead/over, scrollable). Exposes all data entry via `ExternalInterface` callbacks. On construction, button labels are set from translate keys. In preview mode (non-Iggy) seeds 20 players and sets mode 2 (over).

### Public methods
- `setMode(modeIndex:int, publicQueue:Boolean) : void` — Maps index to "alive"/"dead"/"over", calls `gotoAndStop`, shows/hides scroll list, action buttons, and console button legend. In MODE_OVER repositions elements (scroll list x=314, exit btn x=680, play btn x=866, button legend x=430) and shows `pvpLootboxRoyale`; in the other modes it is hidden.
- `addPlayer(name, isAlive, isViewer, rank, timeAlive, kills, powerups, damage, firstBlood) : *` — Routes to the correct listing object by rank; in dead/over mode auto-scrolls the viewer's row into view if rank > 10. Updates `rewardToHighlight` for the viewer and calls `rewardsContainer.update()`.
- `setRewardInfo(blood, bronze, silverCount, goldCount, silverSlots, bronzeSlots) : void` — Populates `rewardQuantities[frame]` and builds `rewardPlacements[]` array mapping each rank to a reward tier frame index.
- `showDancer(textureName, winnerName, clubName) : void` — Fills the `dancePanel` with the winner's character texture and name; a second call adds a second winner ("name + name") and clears the club name if they differ.
- `setPlayerInQueue(inQueue:Boolean) : void` — Toggles `btnPlay` label between `$PVP_QUEUE_ENTER_QUEUE` and `$PVP_QUEUE_LEAVE_QUEUE`.
- `setPVPMetaDataForBR(oldLevel, newLevel, oldPercent, newPercent, battleBoxes, battleFactor) : void` — fills `pvpLootboxRoyale`: writes `oldLevel` to `txtMetalevel` and `battleBoxes/battleFactor` to `txtBattleBoxes`, then tweens `pvpMeter.maskShape.scaleX` from `oldPercent` to `newPercent` over `TWEEN_TIME` with two sequential `IggyTween`s. When `newPercent <= oldPercent` the first leg runs to 1.0, the second restarts from 0, and `txtMetalevel` switches to `newLevel` between the legs. Same animation as `pvpscorecard.swf`'s `setPVPMetaData`.
- `onScrollList(delta:Number) : void` — Scrolls the player list by `delta * 30 / verticalStep` pixels (controller scroll support).
- `dancePanelVisible : Boolean` (getter) — read by the engine; returns whether `dancePanel` exists, which is only on the `over` frame.

### Key fields
- `currentMode : String` (static) — `"alive"`, `"dead"`, or `"over"`.
- `rewardQuantities : Array` (static) — indexed by `RoyaleRewards.FRAME_*` constants, holds counts per tier.
- `rewardPlacements : Array` (static) — per-rank reward tier: index 0 = gold, then silver slots, then bronze slots.
- `rewardToHighlight : int` (static) — the reward tier to animate for the local viewer.
- `viewerFirstBlood : Boolean` (static) — whether the viewer got first blood (controls blood reward animation).
- `playerStackList : StackList` — 20 fixed-position `RoyalePlayerListing` children, used in alive mode.
- `playerScrollList : ScrollableTileView` — 20 scrollable listings, used in dead/over modes.
- `rewardsContainer : RoyaleRewards` — the reward-icon strip at the top. Placed from the `dead` frame on, so it is constructed (and sends `RequestRewards`) only when the timeline first leaves `alive`.
- `dancePanel : MovieClip` — winner showcase panel, placed only on the `over` frame (in the depth `playerStackList` vacates); contains `art`, `art2` (`PreviewContainer`s), `winnerNameText`, `winnerClubNameText`.
- `btnExit / btnPlay / btnSpectate : LabelButton` — post-match action buttons.
- `buttonLegend : MovieClip` — console button hints panel (contains `btnX`, `btnXTextField`, `buttonY`, `buttonYTextField`).
- `pvpLootboxRoyale : MovieClip` — PvP lootbox meter panel (contains `pvpMeter` with `fill` + `maskShape`, `txtMetalevel`, `txtBattleBoxes`); the mask is applied in the constructor, and the panel is visible only in MODE_OVER.
- `TWEEN_TIME : Number = 1` — total seconds for the lootbox meter animation.
- `PENUMBRA : int = 6` — y-offset applied to dead players' rows to create visual separation.

### Frame scripts / timeline
- Frame labels: `"alive"` (frame 1, which also carries the label `"Not used BUT do not delete this needs a frame 1"`), `"dead"` (frame 11), `"over"` (frame 21), each with a `stop()`. `playerStackList` is removed at `dead`, `rewardsContainer` is added at `dead`, and `dancePanel` is added at `over`.

### Runtime dependencies & integration
- `ExternalInterface` callbacks: `setRewardInfo`, `setMode`, `addPlayer`, `showDancer`, `setPlayerInQueue`, `onScrollList`, `setPVPMetaDataForBR`.
- `IggyTween` (bundled since the October 2026 build) with `fl.transitions.easing.None` drives the lootbox meter.
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
Trivial subclass of the texture-preview widget, no added logic. Placed as the `art` child of each reward slot (`reward0`–`reward3`) and as `dancePanel.art` / `art2`. `showDancer` sets the dance panel's `textureName`; nothing in the SWF sets the reward slots' — the engine writes it by path, `rewardsContainer.reward{0}.art`, with `{0}` the slot index in `iconFrameOrder` order (0 blood, 1 bronze, 2 silver, 3 gold), once `RequestRewards` has gone out.

### `RoyaleScoreCard_fla.rewardIconLarge_46` — Embed symbol85
Timeline symbol for the large reward icon (4 frames: bronze/silver/gold/blood), each `stop()`.

### `RoyaleScoreCard_fla.rewardIcon_54` — Embed symbol15
Timeline symbol for the small per-row reward icon (same 4-frame structure).

### Asset wrappers
`btnGreen`, `btnGreenIcon_small`, `btnGreenIcon_medium` — button skin assets, no logic. Standard scroll skins (12 classes).

## Notable logic
- **Static shared state**: `RoyaleScoreCard.currentMode`, `rewardQuantities`, `rewardPlacements`, `rewardToHighlight`, and `viewerFirstBlood` are all static, allowing `RoyalePlayerListing` and `RoyaleRewards` (which receive no constructor arguments) to read the current state directly without callbacks.
- **Dual list architecture**: The `StackList` (alive) keeps all 20 rows at fixed positions without scrolling; the `ScrollableTileView` (dead/over) enables scrolling as the board fills up. `hasAutoScrolled` ensures the view scrolls to the viewer's row exactly once on first render.
- **Reward placement algorithm**: Gold is always rank 0. The `silverCount` and `bronzeCount` parameters from `setRewardInfo` fill subsequent positions, so the reward tiers follow the match's configured prize structure.
- **PvP lootbox meter (October 2026 build)**: Bomber Royale now reports PvP lootbox progress the same way the PvP scorecard does, through `setPVPMetaDataForBR`, shown only once the match is over.
- **Dance panel dual-winner**: `showDancer` is called once per winner. The first call sets `art.textureName`; the second call sets `art2.textureName`, shifts `art` left by 150px instead of 75px, and concatenates names. Club name is blanked if winners are from different clubs.
