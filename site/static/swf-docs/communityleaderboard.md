# communityleaderboard.swf
> The Community Leaderboard is a voting UI panel that lets players browse and vote for player-submitted items (e.g., contest entries) that are candidates for addition to the game. It shows up to five item tiles in a carousel, tracks per-player and global vote counts, and handles a currency-based vote-purchase flow.

**Document/main class:** `CommunityLeaderboard` (extends `UIComponent`)
**SWF-specific classes:** 10 (excluding shared framework)

---

## Main class: `CommunityLeaderboard`

`CommunityLeaderboard` is the root UI component that manages the leaderboard carousel. On initialisation (`configUI`) it registers all Iggy/ExternalInterface callbacks so the game engine can push data in, wires mouse listeners to the five `ItemView` tile instances and the left/right navigation arrows, and seeds the initial display state. It owns two parallel data collections: `itemObjects` (the full server-provided entry list) and `displayTiles` (the five visible `ItemView` instances that serve as a sliding window into that list).

### Public methods

- `SetLeaderboardData(name, desc, voting, nextStateChange, ?, locked, hasHistory, voteCostDesc) : void` — populates header and info-box text fields; stores state flags for voting/locked/history; triggers `SetInfoData`.
- `AddEntry(name, desc, canBuy, voting, costPrefab, voteCost, userVotes, totalVotes, voteCostDesc, voteId, texture, costTexture, tradeable) : void` — creates a new `ItemView`, calls `SetupItem`, appends it to `itemObjects`, and shows nav arrows when more than five entries exist.
- `ClearEntryList() : void` — empties `itemObjects` and hides nav arrows.
- `RefreshDisplay() : void` — re-maps the visible tiles to the current window position and re-draws selection state.
- `SelectItemsToDisplay() : void` — copies data from `itemObjects` into the five `displayTiles` using modular arithmetic around `currentSelection`.
- `ShiftDirection(right:Boolean) : void` — moves the carousel one step left or right. On PC it clamps `currentTile` to 0–4 and shifts the backing list when at an edge; on console it always re-centres on tile 2 and immediately shifts the backing list. After moving it calls `CanBuy` and `RequestWalletInfo` on the newly focused tile.
- `SetMode(param1:Boolean) : void` — when `true`, re-centres selection on tile 2.
- `UpdateEntryCost(voteId, cost, userVotes, totalVotes, canBuy) : void` — finds the entry by `voteId` in both `itemObjects` and `displayTiles` and refreshes vote counts and the purchase button.

### Key fields

- `itemObjects : Vector.<ItemView>` — master list of all leaderboard entries received from the server.
- `displayTiles : Vector.<ItemView>` — the five `ItemView` instances visible on screen (Tile1–Tile5).
- `currentSelection : int` — index into `itemObjects` of the entry currently mapped to the left edge of the window.
- `currentTile : int` — index (0–4) of the highlighted tile; `-1` when nothing is selected.
- `_voting : Boolean` — whether the leaderboard is currently in voting phase.
- `_locked : Boolean` — whether the voting window has closed.
- `_hasHistory : Boolean` — whether historical results are available.
- `_hasPendingVote : Boolean` — blocks `PurchaseRequest` calls while a vote is in-flight.
- `_nextStateChange : String` — human-readable countdown to the next phase transition (displayed in `InfoBox`).
- `_leaderboardName / _leaderboardDesc : String` — title and description text.
- `_voteCostDescription : String` — currency description for vote cost.
- `walletImage : ObjectPreview` — 40×40 icon for the current wallet currency, embedded inside `wallet.priceDisplay.preview`.
- `wallet : MovieClip` — wallet display area containing `priceDisplay` (a `walletPriceDisplay_16` timeline symbol).
- `displayProgress : MovieClip` — blocking overlay (a `BlockingStatus_17` symbol) shown while a server request is in-flight.
- `InfoBox : MovieClip` — panel showing leaderboard state, countdown, and description text.
- `LeaderboardTitleHeader : MovieClip` — animated title header.
- `btn_left / btn_right : MovieClip` — carousel navigation arrows, visible only when more than 5 entries exist.

### Frame scripts / timeline

No custom `addFrameScript` calls in the main class. Navigation buttons use `gotoAndStop("over")` / `gotoAndStop("up")` for hover states.

### Runtime dependencies & integration

**ExternalInterface callbacks registered (Iggy → Flash):**
| Callback name | Handler |
|---|---|
| `ShiftSelection` | `ShiftDirection` |
| `SetLeaderboardData` | `SetLeaderboardData` |
| `AddEntry` | `AddEntry` |
| `ClearEntryList` | `ClearEntryList` |
| `RefreshDisplay` | `RefreshDisplay` |
| `SetMode` | `SetMode` |
| `Purchase` | `onPurchase` |
| `UpdateEntryCost` | `UpdateEntryCost` |
| `HasPendingVote` | `HasPendingVote` |
| `SetDisplayProgress` | `SetDisplayProgress` |
| `UpdateEntryCanPurchase` | `UpdateEntryCanPurchase` |
| `SetWalletIcon` | `SetWalletIcon` |

**ExternalInterface calls made (Flash → Iggy/game):**
- `CanBuy(voteId)` — queries whether the player can currently vote for the focused entry.
- `RequestWalletInfo(voteId)` — requests currency balance update for the focused entry.
- `PurchaseRequest(voteId)` — submits a vote purchase; only fires if `_hasPendingVote` is false.
- `ReapplyVotes()` — connected to a (private) `ReApplyVotes` handler; re-applies pending votes.
- `SetFocused(bool)` — tells the engine whether focus is on the info panel or the tile selection.

**Translate keys used:**
- `$CL_StateChange` — label for the "time remaining" info box line.
- `$CommunityLeaderboard_Voting` / `$CommunityLeaderboard_Displaying`
- `$CommunityLeaderboard_VotingEnded` / `$CommunityLeaderboard_DisplayingEnded`
- `$CommunityLeaderboard_PlayerVotes` / `$CommunityLeaderboard_GlobalVotes` (in `ItemView`)

---

## Other game-specific classes

### `ItemView` (extends `UIComponent`) — [Embed symbol "symbol58"]
A single leaderboard entry tile. Holds all per-entry data (`_itemName`, `_voteId`, `_itemvoteCost`, `_numberOfUserVotes`, `_numberOfTotalVotes`, `_canBuy`, `_voting`, `_tradeable`, etc.). Renders a 128×128 item preview (`ObjectPreview`) and a 40×40 cost-currency icon, per-player and global vote count text fields, a purchase button (`LabelButton`), a market-tradeable badge, and a hover-activated info popup (`infoPopup`). Key methods: `SetupItem` (full data load), `Transfer` (copy data from another `ItemView`), `UpdateVoteText` (refresh counts and purchase button), `SelectTile` (show/hide selection highlight), `SetCanBuy` (enable/disable purchase button).

### `btnGreenIcon_small` (extends `LabelButton`) — [Embed symbol "symbol32"]
Green icon-label button skin used for the purchase button on each tile. Four-state timeline (frames 10/20/30/40 = up/over/down/disabled), each stopped by a frame script.

### `btnPageExtreme` (extends `LabelButton`) — [Embed symbol "symbol11"]
Alternate label-button skin (used for page-navigation extremes). Same four-state timeline pattern as `btnGreenIcon_small`.

### `CommunityLeaderboard_fla` timeline symbols (5 classes)
| Class | Symbol | Notes |
|---|---|---|
| `timer_pulse_anim_3` | symbol73 | Single-frame stop; pulse animation for countdown timer. |
| `InfoPopup_11` | symbol53 | Two-state MC (frames 1/11 stopped); exposes `textField:TextField` for item description on hover. |
| `walletPriceDisplay_16` | symbol94 | Wallet currency row; exposes `preview:MovieClip` (for `ObjectPreview` child) and `price:TextField`. |
| `BlockingStatus_17` | symbol108 | Two-state overlay (frame 11 = Console) with a `statusMessage` sub-MC; used as the `displayProgress` spinner. |
| `btnInfo2_10` | symbol49 | Info icon button MC, four-state timeline; used as `ItemView.infoButton`. |
| `btn_skip_13` | symbol89 | Four-frame MC (frames 1–4 each stopped); skip navigation button. |

---

## Notable logic

- **Carousel window arithmetic:** `SelectItemsToDisplay` maps `displayTiles[i]` to `itemObjects[(currentSelection + i) % itemObjects.length]`, creating a circular ring. Overflow wraps to 0; underflow wraps to `itemObjects.length - 1`.
- **PC vs. console navigation difference:** On PC, `ShiftDirection` clamps `currentTile` within the visible 5-tile strip and only scrolls the item list when hitting an edge. On console (`IsConsole()`), `currentTile` is always forced to 2 (centre) and the list scrolls immediately on every press.
- **Purchase guard:** `onPurchase` checks `_hasPendingVote` before emitting `PurchaseRequest`. The game sets `HasPendingVote(true)` immediately and clears it after the round-trip completes, preventing double-votes.
- **Wallet display:** `SetWalletIcon(textureName, priceText)` populates the `walletImage` ObjectPreview and the price label; the wallet area is a child MC containing a `walletPriceDisplay_16` symbol.
- **State label logic:** `GetStateText()` produces a localised string from the 2×2 matrix of `_voting` × `_locked` flags.
