# claims.swf

> The Claims panel in Trove, showing pending claimable rewards (items, currency, etc.) earned from leaderboards, PvP, mastery, badges, or other sources. Players can claim each reward individually or all at once.

**Document/main class:** `Claims` (extends `UIComponent`)
**SWF-specific classes:** 2 (`Claims`, `ClaimItemRow`)

---

## Main class: `Claims`

Manages a scrollable list of unclaimed rewards. On construction it adds frame scripts (frames 0, 10, 20), initializes `claimDictionary`, and sets the window header title to `$Claims`. `configUI()` configures `itemView` (a `ScrollableTileView`) to use a vertical scrollbar with 2 px item spacing, disables the `claimAll` button initially, sets its label to `$ClaimAll`, and registers all `ExternalInterface` callbacks. In non-Iggy mode a test claim is added automatically.

Frame scripts stop at frames 1, 11, and 21. Frame 21 sets `buttonLegend` to `"ConsoleLoc"`.

### Public methods

- `addClaim(id, icon, quantity, name, description, source, target, expires, timeUntilExpiration) : void` — creates a `ClaimItemRow`, adds it to `itemView` (deferred via `ENTER_FRAME` on console until the row is on its target frame), calls `claimItem.init(...)`, stores it in `claimDictionary[id]`, enables `claimAll`, and clears `noClaimsText`.
- `setClaimed(id, hours, minutes) : void` — looks up the row by id in `claimDictionary`, calls `row.setClaimed(hours, minutes)`, increments `numClaimed`, and disables `claimAll` when all items are claimed.
- `onClaimAllClicked(e:MouseEvent) : void` — calls `ExternalInterface.call("OnClaimAll")`.
- `selectClaimItem(id) : void` — triggers `onClaimClicked` or `onFindClicked` on the matching row depending on whether its claim background is visible.
- `sourceClaimItem(id) : void` — triggers `onSourceClicked` on the matching row.

### Key fields

- `__id1_ : WindowHeaderSmall` — window header; title `$Claims`.
- `itemView : ScrollableTileView` — scrollable container for `ClaimItemRow` instances; vertical step 10 px (non-console), spacing `(0,2,0,0)`.
- `claimAll : LabelButton` — "Claim All" button; enabled only when at least one unclaimed item remains.
- `noClaimsText : TextField` — shown when the claim list is empty; cleared as soon as any claim is added.
- `goldFrame / frameBackground : MovieClip` — decorative frame elements whose height is dynamically adjusted by `updateButtonLegendBackground`.
- `buttonLegend / buttonLegendBG : MovieClip` — console button legend and its background; resized based on visible legend entries.
- `claimDictionary : Dictionary` — maps claim `id:String` → `ClaimItemRow`.
- `numClaimed : int` — count of items that have been claimed; used to disable `claimAll`.
- `maxNumVisible : Number = 5` / `scrollRowBuffer : Number = 10` — layout constants.

### Frame scripts / timeline

| Frame | Action |
|-------|--------|
| 0 | `stop()` |
| 10 | `stop()` |
| 20 | `stop()`, `buttonLegend.gotoAndStop("ConsoleLoc")` |

### Runtime dependencies & integration

**ExternalInterface callbacks registered (inIggy):**
`addClaim`, `setClaimed`, `selectClaimItem`, `sourceClaimItem`, `highlightClaim`, `unhighlightClaim`

**ExternalInterface calls (outbound):**
`OnClaimAll` (claim-all button)

**Highlight/unhighlight:** `highlightClaim(id, direction)` scrolls the item into view (1.5× height buffer), applies `outlineDisplayObject` to `bg` and `bg_claimed`, sets both to frame `"on"`, and updates the button legend select string. `unhighlightClaim(id)` removes outlines and resets to `"off"`. Direction `1` = scrolling down, `-1` = scrolling up.

**Button legend resize:** `updateButtonLegendBackground` measures the actual text height of all four legend entries (`buttonLegendSelect`, `buttonLegendClaimAll`, `buttonLegendSource`, `buttonLegendClose`), finds the max visible bottom, and adjusts `buttonLegendBG.height`, `goldFrame.height`, and `frameBackground.height` by the delta.

**translate() keys (via `IggyFunctions.translate`):** `$Claims`, `$ClaimAll`, `$Claim_ButtonLegend`, `$Find_ButtonLegend`

**Events listened:** `MouseEvent.CLICK` on `claimAll`, `Event.FRAME_CONSTRUCTED` (prop-set handler), `Event.ENTER_FRAME` (console deferred addItem)

---

## Other game-specific classes

- `ClaimItemRow` — `UIComponent` (embedded symbol 123). One row in the claims list. Displays item slot (`Slot`), quantity, name (`itemNameText`), description (`itemDescText`), expiration countdown (`expirationText`), source-type buttons (`sourceButtons` MC with frames for leaderboards/battle/mastery/collections), a `claimButton` (`LabelButton`, label `$Claim`), and a `findButton` (`BaseButton`, initially hidden). `init()` populates all fields; `setClaimed(hours, minutes)` hides the claim button, shows the find button if a `targetId` exists, and updates `expirationText` with the cooldown via `TimeUtil.formatTime`. `setExpirationTime()` calls `TimeUtil.formatCountdown` and uses `$Claims_Expires` / `$Claims_Expired` translate keys with `{0}`/`{1}` placeholders. Calls `ExternalInterface.call("OnClaim", claimId)`, `OnShowSource(sourceId)`, `OnShowTarget(claimId)`. Two frame-stop states at frames 0 and 10 (console variant). Visibility is deferred by one ENTER_FRAME via `onRendered` to avoid a blank first frame.

### Claims_fla/ timeline symbols (6 total)

- `ButtonLegend_4` — console button legend container.
- `bg_claimed_33` / `itemRowBG_34` — claim row background states.
- `slotFrame_36` — slot frame decoration.
- `equipped_38` — equipped indicator clip.
- `qualityPips_44` — quality pip indicator.
- `sourceButtons_46` — source-type button container (4 frames: leaderboards, battle, mastery, collections).

**Asset wrappers (not detailed):** 20 skin/png classes — `rarity_frame_*_png` / `*_over_png` (10 rarity tiers × 2), `ScrollArrow*_*Skin` (8), `ScrollThumb_*Skin` (3), `ScrollBar_thumbIcon`, `ScrollTrack_skin`, `focusRectSkin`. Plus top-level button MCs (`leaderboardButton`, `btnFind`, `pvpBtn`, `badgesBtn`, `masteryBtn`, `btnGreen_wide`, `btnGreenIcon_small`) and `dummy.as`.

---

## Notable logic

- **Source routing:** `ClaimItemRow.setUpMenu()` iterates `sources = ["leaderboards","battle","mastery","collections"]` and does `sourceId.indexOf(sources[i]) > -1` to pick a frame for `sourceButtons`. Only one source type is shown per row; if none match, the source button area and label are hidden.
- **Console deferred addItem:** On console, `addClaim` defers adding the row to `itemView` via an `ENTER_FRAME` loop that checks `claim.allOnTargetFrame()` before calling `itemView.addItem(claim)`. This ensures the row's MC is fully initialized before it enters the scroll view.
- **Expiration countdown:** Uses `_kiwi.Util.TimeUtil.formatCountdown(seconds)` which returns a `FormatCountdownResult` with `value` and `units` fields, inserted into the `$Claims_Expires` string template as `{0}` and `{1}`.
