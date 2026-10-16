# leaderboard.swf
> The full Leaderboard window shown in Trove when a player opens a leaderboard category. It displays ranked entries (scores/times) across multiple named leaderboards grouped in collapsible categories, supports an Everyone/Friends filter, a Favorite toggle, paginated entry browsing, and a sidebar contest-reward panel with a countdown timer.

**Document/main class:** `Leaderboard` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 10 (plus 12 `Leaderboard_fla` timeline symbols and ~15 asset-wrapper/skin classes)

---

## Main class: `Leaderboard`

`Leaderboard` is the root UIComponent for the entire leaderboard screen. On construction it calls `addFrameScript(0→frame1, 10→frame11)` and sets up the window header's translate key. `configUI()` is the primary initialisation point: it registers 31 `ExternalInterface.addCallback` entries (the game engine calls these) and wires click listeners for all pagination buttons, the filter radio buttons, and the favourite checkbox. In non-Iggy (preview) mode it populates stub data.

### Public / registered callbacks (via ExternalInterface.addCallback)
- `addCategory(id:int, name:String)` — adds a collapsible `BagContainerBasic` category row to the left-hand `categoryView` RowView.
- `addLeaderboard(catId:int, lbId:Number, name, desc, icon, displayType:int, defaultSelect:Boolean)` — adds a `SlotBasic` icon inside the given category bag (`tooltipName`/`tooltipDescription` = `name`/`desc`). `name` and `desc` arrive as translation keys (e.g. `$Leaderboard_Paragon_Bard` → "Weekly Highest Paragon Level with Bard"), resolved only where they are written into a text field. A favourited board is sent twice with the same `lbId`, once in its own category and once under Favorites. On PC, `defaultSelect` on the first board while none is current calls `setCategory(catId)` and `SetLeaderboard(lbId)`.
- `removeLeaderboard(catId:int, lbId:Number)` — removes the slot and its `LeaderboardInfo` metadata (unfavouriting removes the Favorites copy this way).
- `setLeaderboard(lbId:Number, hasFilter:Boolean)` — called by engine to switch the active leaderboard: shows the loading spinner, marks the board's slot in the current category as equipped, shows/hides the filter bar and its `pagesTxt` caption, and resets the filter to Everyone when the board has none. The entries are requested once `setLeaderboardInfo` follows.
- `setLeaderboardInfo(lbId:Number, count:int, isFav:Boolean, playerRank:int, ?:Number)` — receives row count, favourite state and the player's rank (0 = unranked); shows the rank in the footer's `playerScoreMC`, enables the Me button when ranked and `count > 10`, then calls `GetEntriesNearPlayer` (ranked) or `GetEntriesRange` (first page). The fifth argument is not read. The engine sends it again after every `SetLeaderboard`, and `count` can be lower than the number of places `GetEntriesRange` will actually serve.
- `addLeaderboardEntry(lbId:Number, name:String, rank:int, score:Number, isFriend:Boolean, isPlayer:Boolean)` — creates a `LeaderboardEntry` MC, formats rank/score, adds to `leaderboardView`. Rows carry no request id or range. Tied scores share a rank (four players tied for first are four rank-1 rows, and the next row is rank 5). After `GetEntriesNearPlayer` the first row to arrive sets `entryIndex`, because the engine chooses that window.
- `getEntriesRangeFailed / getEntriesNearPlayerFailed / setLeaderboardFailed` — clear the loading spinner.
- `setCategory(id:int)` — collapses every other category. The target only stays open if it was already expanded: the test is `bag.id == id && !bag.collapsed`, so a collapsed target is collapsed again. `switchCategory` (console) toggles the bag directly.
- `updateRewardDisplayOrder(reversed:Boolean)` — reverses reward tile order in `contestRewardsMC`.
- `setRewardInfo(slot:int, rewardId, icon, qty:int, progress:Number, threshold:Number, rankThreshold:int)` — populates one contest reward tile. `rankThreshold > 0` makes it a rank rung captioned `$Leaderboard_YourRank` ("Rank:") with `N+` (`1` alone for first place); otherwise a score rung captioned `$Leaderboard_YourScore` ("Score:") with `threshold+`. `progress == 1` puts the tile on its `complete` frame. In game `progress` arrives with the same value on every rung.
- `clearRewardInfos()` — clears and hides all reward tiles.
- `setTimeUntilReset(seconds:Number)` — delegates to `ContestRewards.setTimeUntilReset`, which calls `IggyFunctions.translate("$Leaderboard_ContestEnds_Time")`.
- `setMaxEarnedReward(slot:int)` — marks a tile as "inProgress" with the player icon.
- Console-navigation callbacks: `GET_CONFIGURED_DATA`, `HIGHLIGHT_CATEGORY`, `UNHIGHLIGHT_CATEGORY`, `SWITCH_CATEGORY`, `COLLAPSE_CATEGORY`, `HIGHLIGHT_SLOT`, `UNHIGHLIGHT_SLOT`, `ACTIVATE_SLOT`, `SWITCH_SECTIONS`, `MOVE_FILTERS_CURSOR`, `MOVE_REWARDS_CURSOR`, `PAGE_FIRST_SELECTED`, `PAGE_LEFT_SELECTED`, `PAGE_ME_SELECTED`, `PAGE_RIGHT_SELECTED`, `ON_FILTER_CHANGED`.

### Public methods
- `get lastPageIndex() : int` — computes the 1-based index of the first entry on the last page: `ITEMS_PER_PAGE * (ceil(numEntries/ITEMS_PER_PAGE) - 1)`.
- `get/set entryIndex : int` — current page start index; setter updates enabled state of First/Left/Right pagination buttons.
- `get resultsOnPage : int` — number of entries on the current page (capped at 10).
- `refreshLayout() : void` — calls `categoryView.refreshLayout()`.
- `onCategoryHeadingClick(bag:BagContainerBasic) : void` — handler when a category heading is clicked; refreshes layout.
- `onCategoryToggled(e:MouseEvent) : void` — if a different category was toggled, calls `setCategory`.

### Key fields
- `winHeader : WindowHeaderSmall` — title bar; translate key `$Leaderboard_Header`.
- `categoryView : RowView` — scrollable left column holding `BagContainerBasic` category groups.
- `leaderboardView : RowView` — right column listing `LeaderboardEntry` rows; vertical scroll disabled; item padding −1.
- `leaderboardHeaderMC : MovieClip` — hosts `LeaderboardHeader_42`; contains filter radio buttons, favorite checkbox, the `pagesTxt` filter caption, and leaderboard name text.
- `leaderboardFooterMC : MovieClip` (instances `Paging`) — pagination buttons (First/Left/Me/Right) and `playerScoreMC`, which shows the player's rank.
- `contestRewardsMC : MovieClip` (instances `ContestRewards`) — sidebar contest reward panel.
- `loadingMC : MovieClip` — spinner shown during async data loads.
- `leaderboardInfos : Array` — sparse array indexed by leaderboard ID holding `LeaderboardInfo` objects (name + displayType).
- `categoryBags : Dictionary` — maps category ID → `BagContainerBasic`.
- `currentLeaderboard : Number` — ID of currently active leaderboard; −1 = none.
- `currentCategory : int` — ID of expanded category.
- `numEntries : int` — total entry count from server.
- `ITEMS_PER_PAGE : int = 10` — page size.
- `SLOTS_PER_ROW : int = 3` — returned to engine via `LEADERBOARDS.CONFIGURED` in answer to `GET_CONFIGURED_DATA`; the engine walks the console cursor across the board grid by this row width.
- `currentFilterHighlight : MovieClip` — tracks which console cursor highlight is active in the filter bar / pager.

### Frame scripts / timeline
Stage 845×540, 24 fps, 20 frames; root labels `PC` (frame 1) and `Console` (frame 11).
- `frame1` — `stop()` (default PC layout).
- `frame11` — `stop()`; sends `leaderboardHeaderMC`, `leaderboardView`, `leaderboardFooterMC`, `loadingMC`, and `contestRewardsMC` to their `"Console"` label (console layout variant). `ButtonLegend_51` is placed on this frame only.

### Runtime dependencies & integration
- **IggyFunctions.inIggy** — gates all `ExternalInterface` wiring.
- **translate keys**: `$Leaderboard_Header` (window title), `$Leaderboard_ContestEnds_Time` (in `ContestRewards`), `$Leaderboard_FilterEveryone`, `$Leaderboard_FilterFriends`, `$Leaderboard_YourScore`, `$Leaderboard_YourRank`; static on the timeline: `$Leaderboard_FilterSelect` ("Filter:"), `$Leaderboard_Rank_Header`, `$Leaderboard_Name_Header`, `$Leaderboard_Score_Header`, `$Leaderboard_Loading`, `$Leaderboard_Rewards_Header`, `$Leaderboard_ContestEnds_Header`.
- **ExternalInterface.call** (outgoing): `SetLeaderboard(lbId)`, `GetEntriesRange(lbId, firstRank, lastRank)` (1-based ranks), `GetEntriesNearPlayer(lbId, 10)`, `SetFilter(0|1)`, `SetFavorite(lbId, bool)`, `LEADERBOARDS.CONFIGURED(3)`. `SetLeaderboard` is answered by `setLeaderboard` and then `setLeaderboardInfo`. Through the shared controls: a mouse click on a board icon goes out as `SLOT.ACTIVATE(lbId)` (from `SlotBasic`), not `SetLeaderboard`; board icons also send `SLOT.POINTER_ENTER`/`LEAVE` and `TOOLTIP.SHOW`/`HIDE`, and reward tiles (`ArtClip`, `data` = reward id, no name or description of their own) send only `SLOT.POINTER_ENTER`/`LEAVE`, which makes the engine show its own item tooltip.
- **IsConsole() / IsNX()** — runtime platform checks controlling category auto-collapse, glow-filter visibility, and console-cursor initialisation.
- **NumberFormat** (`_kiwi.Util.NumberFormat`) — formats scores (integer or 2-decimal float based on `LeaderboardInfo.displayType`).
- **DirectionalMapping** — placed as children of each console highlight MovieClip to form a navigable D-pad grid (left/right in filter row links back to pager).
- **GlowFilter** — applied to `BagContainerBasic.heading` when console highlights a category (colour `0xCCCC00`, inner, high quality, strength 100).

---

## Other game-specific classes

- `LeaderboardInfo` — plain data class: holds `name:String`, `displayType:int` (0 = integer, 1 = float), `refCount:int` for reference tracking.
- `LeaderboardEntry` — Embed symbol32; `MovieClip` with `friendIcon`, `txt_userName`, `txt_time`, `txt_rank`; two frame stops at frames 2 and 3, labelled `inactive` and `active`. `friendIcon` is made visible for friends and for the player's own row, but only friends are sent to its `friend` frame; the player's row stays on the empty `none` frame.
- `LeaderboardEntryRowView` — Embed symbol184; extends `_kiwi.Controls.RowView`; frame 1 stops, frame 11 calls `viewportMovieClip.gotoAndPlay("Console")`.
- `ContestRewards` — Embed symbol270; manages 5 `rewardTile` MCs inside a `contest` MovieClip; `setReward` fills icon/quantity/progress bar/`youIconMC`; `setTimeUntilReset` calls `TimeUtil.formatCountdown` and `IggyFunctions.translate("$Leaderboard_ContestEnds_Time")`; `setDisplayOrder` reverses tile array for alternate display order.
- `FilterContainer` — Embed symbol178; extends `_kiwi.Controls.RadioButtonContainer`; two `RadioButton` children (`everyoneRadioBtn` / `friendsRadioBtn`) with translate keys `$Leaderboard_FilterEveryone` / `$Leaderboard_FilterFriends`; frame 11 sends both buttons to `"Console"`.
- `Paging` — Embed symbol231; footer navigation MovieClip with `btnPageFirst`/`Left`/`Right` (`btnArrow*` symbols), `btnPageMe` (`btnGreen_small`), `playerScoreMC`, and per-button highlight MCs.
- `Equipped` — Embed symbol105; trivial `MovieClip` asset (equipped-indicator icon).
- `SlotBackground / SlotBackgroundLocked / SlotFrameNormal / SlotFrameMedium / SlotFrameHigh / SlotFrameLarge / slot_large` — rarity-frame and slot-background asset wrappers (7 classes).

**Leaderboard_fla timeline symbols (12):**
- `LeaderboardHeader_42` (symbol291) — header bar; holds `leaderboardName` TextField, `filtersMC` (`FilterContainer`), `favoriteCheckbox` (`Checkbox`), highlight MCs, `pagesTxt` (the static `$Leaderboard_FilterSelect` "Filter:" caption, shown and hidden with the filter); frame 11 console stop.
- `LeaderboardLocalScore_23` (symbol230) — small MC in the footer whose `score` TextField shows the player's rank under the static `$Leaderboard_YourRank` ("Rank:") caption.
- `contest_5` (symbol266) — holds 5 `rewardTile` MCs, `rewardsText`, `contestEndsText`, `contestTimeText`.
- `rewardTile_6` (symbol260) — one reward rung; labelled frames `incomplete`, `inProgress` (set by `setMaxEarnedReward`), `complete`; `iconMC` is an `ArtClip`.
- `ButtonLegend_51` (symbol307) — console button-legend overlay with a `buttonLegendClose` sub-MC.
- `YouIcon_11` (symbol30) — icon with three stopped labels: `none` (empty), `player`, `friend`.
- `CollapsedIcon_60` (symbol114) — 2-state icon (expanded/collapsed) for category bag headers.
- `contestBackground_13`, `equipped_69`, `qualityPips_67`, `slotFrame_66`, `slotFrameLarge_76` — decorative/container symbols with only frame-stop scripts.

**Asset wrappers (skins + PNG embeds, ~18 classes):** `ScrollArrowDown_*Skin`, `ScrollArrowUp_*Skin`, `ScrollThumb_*Skin`, `ScrollTrack_skin`, `ScrollBar_thumbIcon`, `focusRectSkin`, `btnArrowFirst/Left/Right/Last`, `btnGreen_small`, `filterBtnHighlight`, `navigationButtonHighlight`, `favoriteBtnHighlight`, `contestHighlight`, `sectionCategorySelectHighlight`, `sectionFiltersSelectHighlight`, `sectionRewardsSelectHighlight`, `rarity_frame_*_png` (8 variants), `dummy`.

---

## Notable logic

- **Pagination flow**: `setLeaderboardInfo` decides whether to call `GetEntriesNearPlayer` (player has a rank) or `GetEntriesRange` (first page). Pager buttons update `entryIndex` and call `GetEntriesRange(lbId, entryIndex, entryIndex + resultsOnPage)`. The engine also answers ranges far wider than a page (500 places in one request).
- **Score formatting**: `formatScore` dispatches on `LeaderboardInfo.displayType` — integer (no decimals) vs floating-point (2 decimal places with trailing zeros), using `_kiwi.Util.NumberFormat`. What the score means depends on the board: boards 1 and 20 carry mastery points.
- **Category/slot selection model**: categories are `BagContainerBasic` instances in a `RowView`; each leaderboard is a `SlotBasic` inside a bag. A mouse click on a slot sends `SLOT.ACTIVATE(lbId)`; `SetLeaderboard` is sent for the default board and after a filter change. The `setSelected/clearSelected` pair sets `slot.equipped` flags.
- **Filter switch**: both filter handlers send `SetFilter` followed by `SetLeaderboard(currentLeaderboard)`, so a filter change arrives back as a fresh `setLeaderboard` + `setLeaderboardInfo` for the same board.
- **Console navigation**: a `DirectionalMapping` child object on each highlight MC forms an explicit D-pad adjacency graph; `moveFiltersCursor` walks it. `switchSections` toggles between 3 named sections (category=0, filters=1, rewards=2).
- **Contest rewards XP bar**: `setReward` sets `progressMC.meterMC.height = backgroundHeight * progress` and offsets it from the bottom.
- **RefCount on LeaderboardInfo**: adding the same leaderboard ID twice increments `refCount` (which happens for every favourited board, listed in its own category and under Favorites); removing it decrements and splices only when count reaches 0.
