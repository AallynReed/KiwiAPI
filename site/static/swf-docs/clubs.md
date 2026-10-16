# clubs.swf
> The full Club management UI in Trove, opened from the social/club interface. It presents a tabbed window with five tabs: Club List (dashboard), Members, Fixtures, Adventures, and Permissions. Each tab is a dedicated pane class with its own server-driven data flow and console/PC control paths.

**Document/main class:** `ClubsUI` (extends `_kiwi.Core.UIComponent`) — header title embed `$Clubs_Header`
**SWF-specific classes:** 35 (game-logic); plus 16 `Clubs_fla/` timeline symbols; asset wrappers counted below.

---

## Main class: `ClubsUI`

Top-level coordinator. Constructor calls `addFrameScript(0, frame1, 10, frame11)`, wires the five tab header buttons to `onTabClicked`, and hides details (shows club list). It creates a `MemberRowExternalDataSource` and assigns it to `clubMembers.dataSource` for virtual-scroll row pooling.

`configUI` checks `IsConsole()` to switch `clubNavHeader` to its "Console" label frame, then registers the full surface of Iggy callbacks routing engine calls into the appropriate pane methods.

### Public methods
- `showDetails(clubId:String) : void` — Looks up the matching `ClubTile`, copies its `clubId`, `clubName`, and `clubIndex` into `clubMembers`, then calls `showTab(1)` and `expandAllSections()`.
- `hideDetails() : void` — Calls `showTab(0)`, returns to the club list.
- `setIsVaultWorld(isVault:Boolean) : void` — Propagates the flag to `clubAdventures` and `clubFixtures`; both panes hide themselves when true.
- `setToggleJoinChat(clubIndex:Number) : void` — Toggles `btnJoinChat.checked` on tile `club0` + (index + 1) (the index is 0-based), then calls `ExternalInterface.call("OnToggleJoinChat", clubId, checked)`.
- `setTogglePrimaryEditMOTD() : Boolean` — Delegates to `clubList.club01.onEditMotd(null)`.
- `cancelPrimaryEditMOTD() : void` — Delegates to `clubList.club01.cancelEditMOTD()`.
- `selectMember(category:int, index:Number) : void` — Delegates to `clubMembers.selectMember`.
- `setMotdText(clubIndex:Number, text:String) : void` — Looks up tile `club0` + index as a `ClubTilePrimary` and calls its `setMotdText`. The engine never calls it (the name is not in the client executable); the message of the day arrives as the `clubList.club01.motd` property instead.

### Key fields
- `clubList : ClubDashboardPane` — Five-slot club list view.
- `clubMembers : ClubMembersPane` — Virtual-scrolling member roster.
- `clubFixtures : ClubFixturesPane` — Slot-based fixture equip/unequip pane.
- `clubAdventures : ClubAdventuresPane` — Adventure NPC tile row.
- `clubPermissions : ClubPermissionsPane` — Rank-permission matrix.
- `clubNavHeader : MovieClip` — Tab strip with five named child buttons.
- `errorMessage : MovieClip` — Shown when primary slot is vacant or vault-world restrictions apply.
- `dataSource : MemberRowExternalDataSource` — Shared data bridge for `DynamicRowView` row pool. The engine writes into it by name: `dataSource.rowData` and `dataSource.updateSectionData` are property paths in the client.
- `tabFunctions : Array` — Parallel to `tabHeaders` (`tabBack`, `tabMembers`, `tabFixtures`, `tabAdventures`, `tabPermissions`); the outbound call made on tab click, with `clubMembers.clubId` as its argument.
- `HIGHLIGHT_COLOR : uint = 0x00FFFF` — Shared cyan highlight color used by all panes on console.

### Frame scripts / timeline
- `frame1()` (label `PC`) — `stop()`.
- `frame11()` (label `Console`) — `stop()`.

### Runtime dependencies & integration
**Iggy callbacks (non-console):**
`setIsVaultWorld`, `showDetails`, `hideDetails`, `selectMember`, `setMotdText` (never called by the engine), `showTab`,
`clubList.selectClub`, `clubList.club01.setXPbar`, `clubList.club01.clearLog`, `clubList.club01.addLogEntry`, `clubList.club01.clearBuffs`, `clubList.club01.addBuff`, `clubList.club01.ToggleShowXPInfo`,
`clubMembers.setPrimaryChangeCooldown`, `clubMembers.setRankData`, `clubMembers.resetRankData`, `clubMembers.clearMembers`, `clubMembers.setupDetails`, `clubMembers.updateMemberOnlineStatus`, `clubMembers.resetMemberRank`, `clubMembers.setIsEditingMOTD`,
`clubPermissions.setRankPermission`, `clubPermissions.toggleEditPermissions`, `clubPermissions.moveSelection`, `clubPermissions.toggleSelectedPermission`,
`clubFixtures.clear`, `clubFixtures.setRentDueAmount`, `clubFixtures.setRentDueTime`, `clubFixtures.addFixtureTypeHeading`, `clubFixtures.addListFixtureData`, `clubFixtures.categoryComplete`, `clubFixtures.refreshLayout`, `clubFixtures.addEquippedFixture`, `clubFixtures.addEmptyFixture`, `clubFixtures.addLockedFixture`, `clubFixtures.clearBuffs`, `clubFixtures.addBuff`, `clubFixtures.showBuffPanel`,
`clubAdventures.showTile`, `clubAdventures.setDeparture`, `clubAdventures.setSchedule`, `clubAdventures.setCurrentClock`, `clubAdventures.refreshLayout`.

**Console-only additional callbacks:**
`setToggleJoinChat`, `setTogglePrimaryEditMOTD`, `cancelPrimaryEditMOTD`,
`clubAdventures.highlightButton`,
`clubMembers.joinMember`, `clubMembers.selectNext`, `clubMembers.selectPrevious`, `clubMembers.highlightHeader`, `clubMembers.highlightClubAction`, `clubMembers.onSetPrimaryPressed`, `clubMembers.onHeaderPressed`, `clubMembers.toggleEditMode`, `clubMembers.setOpenSelectedRosterItem`, `clubMembers.moveSelectedDropDown`, `clubMembers.cancelEditMOTD`,
`clubFixtures.moveSelection`, `clubFixtures.activateCurrentSelection`, `clubFixtures.upgradeCurrentSelection`, `clubFixtures.unequipCurrentSelection`.

**Outbound calls:** `OnReturnToClubList`, `OnShowClubDetails`, `OnShowClubFixtures`, `OnShowClubAdventures`, `OnShowClubPermissions` (tab clicks, each with `clubMembers.clubId`), `OnToggleJoinChat(clubId, on)`, `TOOLTIP.HIDE`, `TOOLTIP.SHOW`.

**Property paths the engine writes** (no callback; each path is a literal in the client): `clubList.club01`…`club05` (`clubId`, `clubName`, `rank`, `clubLevel`, `clubPowerRank`, `clubMemberCount`, `willJoinChat`, `slotVacant`; on `club01` also `motd`, `canEditMOTD`, `clubitsBalance`, `hasOtherClubs`); `clubMembers` (`canPromote`, `canDemote`, `canKick`); `clubMembers.btnSetPrimary`, `.btnInvite`, `.editRoster`, `.editMotd`, `.flagPrimary`, `.textMOTD`; `clubFixtures.payRentButton`, `.autoPayButton` (`checked`), `.buffs`, `.rentDueAmt`, `.rentDueTime`, `.clubitsContainer.textClubitsBalance`; `clubAdventures.clubitsContainer.textClubitsBalance`; `clubAdventures.adventuresContainer.adventureTile{0}` with `.nameText`, `.descriptionText`, `.portrait` (`iconImage`), `.banishCost.amount`, `.wakeCost.amount`, `.banish`, `.wake`; `clubPermissions.editPermissions`; and `dataSource.rowData` / `dataSource.updateSectionData` for the roster. Iggy resolves each segment of a path as a property of the object before it, not as a child name: the timeline placements make every container on the way (`adventuresContainer`, `adventureTile0`–`4`, `clubitsContainer`) a property of its parent, and a display object that only carries the instance name is never reached.

**Durations are milliseconds.** The club log entry age, primary-change cooldown, rent due time, hero departure, sleep schedule and sleep/wake clock all go through `TimeUtil.localizeTime`, which takes milliseconds.

**Events listened:** `MouseEvent.CLICK` on each tab header button; `ClubTilePrimary.EVENT_VACANT` from `clubList.club01`.

**translate keys:** `$Clubs_Header` (window header), `$Club_NeedPrimary`, `$Club_CreateOrJoin` (error message).

---

## Other game-specific classes

### `ClubDashboardPane` (extends `UIComponent`) — [Embed symbol364]
Five-slot club list. Holds `club01:ClubTilePrimary` and `club02–05:ClubTile` (typed as `ClubTileSecondary` at runtime). Sets channel numbers 1–5 and disables `club02.moveUp`. Exposes `getClub(clubId)` (linear search) and `selectClub(index)` (console selection highlight + button-legend text; the index is 0-based and matched against `club0` + (index + 1)).

Frame 11 (`Console`): calls `gotoAndPlay("Console")` on all five tiles.

### `ClubTile` (extends `UIComponent`)
Base class for all club tiles. Displays: `textClubName`, `textMemberType`, `textClubLevel`, `textClubPowerRank`, `textClubMemberCount`, `btnJoinChat:Checkbox`, `btn_forward:BaseButton`, `btn_world:LabelButton`, `moveUp:MovieClip`. The engine fills a tile by writing its setters through the `clubList.club0N` property paths (`clubId`, `clubName`, `rank`, `clubLevel`, `clubPowerRank`, `clubMemberCount`, `willJoinChat`, `slotVacant`); `slotVacant = true` hides the tile.

Fires: `OnShowClubDetails(clubId)`, `OnJoinWorld(clubId)`, `OnToggleJoinChat(clubId, checked)`, `OnIncreaseClubDisplayPriority(clubId)`. A private `onMarkPrimary` (→ `OnSetPrimary(clubId)`) exists but is never attached to a listener; Set Primary is sent from `ClubMembersPane`.

### `ClubTilePrimary` (extends `ClubTile`) — [Embed symbol356]
The primary (first-slot) club tile. Adds: XP bar, buff scroll (`ScrollableTileView buffs`), MOTD text field with inline edit (toggle via `editMotd` button or `onEditMotd()`), club log, rent-due icon, XP info tooltip button. Its extra setters are also engine-written properties: `motd`, `canEditMOTD` (shows the `editMotd` pencil), `clubitsBalance`, `hasOtherClubs`.

**Key methods:**
- `setXPbar(total, dailyCap, dailyEarned, nextLevel)` — `xpBarCurrent.width` = total / nextLevel and `xpBarDaily.width` = (total − dailyEarned + dailyCap) / nextLevel, i.e. where the bar will stand once today's cap is reached; the tooltip fills `$Club_ExperienceTooltipDesciption` ("Total experience: {0}\nDaily experience: {1}/{2}\nExperience for next level: {3}") with total, dailyEarned, dailyCap, nextLevel.
- `addLogEntry(age, text)` — `age` is how long ago the entry happened, in ms; appends `TimeUtil.localizeTime(age) + " : " + text + "\n"` to `textClubLog`.
- `addBuff(iconUrl, active)` — instantiates `BuffIconMedium`, adds to `buffs` ScrollableTileView. No description is sent here; only `clubFixtures.addBuff` carries one.
- `onEditMotd(e)` — toggles `textFieldBg` and switches `textMOTD.type`; Enter or Escape also closes it. On close calls `ExternalInterface.call("OnSetMOTD", id, text, 0)` (source 0 = primary tile), where `id` is the club id, or `"-1"` when the text is unchanged or empty; `cancelEditMOTD` always sends `"-1"`.
- `slotVacant` setter — overrides parent to dispatch `DataEvent(EVENT_VACANT, {slotVacant, hasOtherClubs})`.
- `ToggleShowXPInfo()` — shows/hides XP tooltip via `TOOLTIP.SHOW` / `TOOLTIP.HIDE`.

Translate keys: `$Club_MOTDInstructions`, `$Club_ExperienceTooltipTitle`, `$Club_PayRent`, `$Club_PayRentDesc`, `$Club_ExperienceTooltipDesciption`.

### `ClubTileSecondary` (extends `ClubTile`) — [Embed symbol260]
Secondary tile (slots 2–5). No additional logic beyond component-inspector property wiring and two frame stops.

### `ClubMembersPane` (extends `_kiwi.Controls.DynamicRowView`) — [Embed symbol544]
Virtual-scrolling member roster. Sections are built by `setupDetails(numOnline)` (one "Online" section). Rows are `MemberRow` instances pooled via `MemberRowExternalDataSource`.

**Key methods:**
- `setupDetails(numOnline:int)` — clears and re-adds one section.
- `clearMembers()` — clears `clubMembers` vector; invalidates data.
- `setRankData(id, label, selectable, editable)` / `resetRankData()` — maintain the static `rankData:Array` used by all visible `MemberRow` instances. One rank per call: a `selectable` rank is put at the front of the dropdown list (`{label, data:id}`), and members holding a rank sent with `editable` false get no dropdown.
- `setEditMode(editing:Boolean)` — toggles `_showRankDropdown` static flag; calls `editVisibleRows` to show/hide the rank ComboBox on each visible row; sends `OnEditModeSet(editing)`.
- `selectMember(category, index)` — scrolls to and highlights the specified row; tracks `m_selectedRosterItem`.
- `onHeaderPressed(col:int)` / `onSortMemberList(e)` — calls `ExternalInterface.call("OnSortMemberList", col + 1, ascending)` and updates sort-arrow frame labels. Criteria 1–5 are the `sortCriteria1`–`5` columns, `$Club_Name`, `$Club_Rank`, `$Club_PowerRank`, `$Club_Donations`, `$Club_LastLogin`; `ascending` is true on a new column and flips on each repeat press.
- `setIsEditingMOTD(editing:Boolean)` — enables `textMOTD` as an `INPUT` field; on close (or Enter/Escape) calls `ExternalInterface.call("OnSetMOTD", id, text, 1)` (source 1 = members pane), `id` being `"-1"` when the text is unchanged or empty.
- `setPrimaryChangeCooldown(ms:Number)` — sets tooltip text on `btnSetPrimary` using `TimeUtil.localizeTime` and `$Club_NoPrimaryChange_Description`. It only reads `btnSetPrimary.enabled`; the engine sets that itself through the `clubMembers.btnSetPrimary` property path.
- `updateMemberOnlineStatus(memberId, online, lastLogin)` — rewrites `textLastLogin` on the matching visible row; the `online` flag is not used.
- `resetMemberRank(memberId)` — puts the matching visible row's rank dropdown back to the member's current rank.
- `canPromote` / `canDemote` / `canKick` setters (engine-written properties) only show or hide the console button-legend entries.
- `onSetPrimaryClicked` / `onSetPrimaryPressed` — `OnSetPrimary(clubId, clubIndex − 1)`, the 0-based slot of the open club.

Promote, demote and kick have no calls of their own: each is `OnMemberRankChanged` with the target rank id (see `MemberRow`), and the engine raises its own confirmation before applying it.

`draw()` fills both slots of `$Club_NumOnline` ("Online: {0}/{1}") with the same `setupDetails` number.

**Events fired:** `OnLeave(clubId)`, `OnInvite()`, `OnSetPrimary`, `OnEditModeSet`, `OnSortMemberList`, `OnSetMOTD`, `TOOLTIP.SHOW`, `TOOLTIP.HIDE`.

**Static fields:** `rankData:Array`, `_showRankDropdown:Boolean`, `_isEditingMOTD:Boolean`.

**Frame scripts:** frames 1, 11, 21 (labels `PC`, `Console`, `ConsoleLoc`; `UIComponent` picks `ConsoleLoc` on console when the locale is neither English nor `zh-cn`).

**translate keys:** `$Invite`, `$Clubs_EditRoster`, `$Clubs_StopEditRoster`, `$Club_Leave`, `$Clubs_SetPrimary`, `$Club_NumOnline`, `$Club_NoPrimaryChange_Title`, `$Club_NoPrimaryChange_Description`, `$Club_AlreadyPrimary_Title`, `$Club_AlreadyPrimary_Description`.

### `ClubFixturesPane` (extends `_kiwi.Controls.ScrollableTileView`) — [Embed symbol471]
Fixture equip/inventory pane. Declares 12 `Slot` fields (`slot_base0/1`, `slot_combat0–3`, `slot_utility0–2`, `slot_ultimate0/1`, `slot_event0`); `slot_event0` is left out of `fixtureSlots` and hidden, so 11 plots are live. Fixture types are indexed `base`, `combat`, `utility`, `ultimate`, `event` (0–4), and a plot is addressed as `slot_<type><index>`. The scrollable list is populated with `BagContainerBasic` category sections, each holding `FixtureRow` items. `iconClubit` (sprite 462 → shape 385 → 22 × 27 bitmap 329) is the clubit coin beside the rent amount.

**Key methods:**
- `clear()` — empties the list, both buff lists and every plot. The engine sends `clubFixtures.clear` and then the whole board again each time a club is opened.
- `addFixtureTypeHeading(name)` — creates a `BagContainerBasic` category row.
- `addListFixtureData(id, name, desc, icon, type, tier, maxTier)` — defers locked items (id == -1) until `categoryComplete()`. `tier` and `maxTier` are 0-based (shown +1 in `$ClubUI_Fixture_Tier`, "Tier {0} / {1}"); the upgrade button is hidden when they are equal. Right-clicking a row's art equips it into the first plot of its type whose icon is empty (`OnEquipFixture(id, plotIndex)`).
- `addEquippedFixture(index, type, name, desc, icon) / addEmptyFixture(index, type, name, desc) / addLockedFixture(index, type, name, desc)` — populate the fixed slot grid at the top; `name`/`desc` become the plot's tooltip. For empty and locked plots the engine sends the name already formatted from `$ClubUI_Fixture_Empty_Name` ("Empty {0} slot") or `$ClubUI_Fixture_Locked_Name` ("Locked {0} slot"). An equipped plot stores its type in `Slot.data`; right-clicking it sends `OnUnequipFixture(type, index)`, the fixture type rather than an id.
- `moveSelection(dx, dy)` — console D-pad navigation between slots and list rows using `DirectionalMapping` children.
- `activateCurrentSelection / upgradeCurrentSelection / unequipCurrentSelection` — console actions.
- `setRentDueAmount(n) / setRentDueTime(ms)` — update rent UI (and re-centre `iconClubit`); time uses `TimeUtil.localizeTime(ms, 2)` and `$ClubsUI_RentDueTime`.
- `addBuff(description, icon, active)` — note the argument order differs from the dashboard's `addBuff(icon, active)`; adds a `BuffIconDesc` to `buffs` and a `BuffIconSmall` to `buffIcons`.
- Drag-drop: `SlotDragDropHelper.registerDropCallback(onDrop)` — hit-tests the drop point against the 11 live plots and sends `ExternalInterface.call("OnEquipFixture", itemId, plotIndex)` for the plot under it (`plotIndex` is the digit at the end of the plot's name).

**Events fired:** `OnPayRent()`, `OnAutoPayRent(checked)`, `OnEquipFixture(fixtureId, plotIndex)`, `OnUnequipFixture(type, plotIndex)`, `OnUpgradeFixture(fixtureId)`, `TOOLTIP.SHOW`, `TOOLTIP.HIDE`.

**translate keys:** `$ClubsUI_PayRent`, `$ClubsUI_RentDueTime`, `$ClubUI_Fixture_Tier`, `$Clubs_Upgrade`.

### `ClubAdventuresPane` (extends `UIComponent`) — [Embed symbol403]
Adventure NPC tile row. Manages up to 5 `adventureTile*` MovieClips inside `adventuresContainer` and matching empty-slot clips inside `adventuresEmptyContainer`. The hero's name, description, portrait and the two prices are not sent through callbacks: the engine writes them through the `clubAdventures.adventuresContainer.adventureTile{0}` property paths (see Runtime dependencies), so the tile and child names are part of the contract.

**Key methods:**
- `showTile(index, visible)` — shows/hides adventure tile vs. empty-slot placeholder.
- `setDeparture(index, ms)` — sets `leavesIn` text via `TimeUtil.localizeTime(ms, 2, true)` and `$ClubUI_AdventureNpc_LeavesIn`.
- `setSchedule(index, sleepDur, wakeDur)` — both in ms; sets `circadianRhythm` text from `$ClubUI_AdventureNpc_CircadianRhythm`; zero `sleepDur` means "Never Sleeps" (`$ClubUI_AdventureNpc_NeverSleeps`) and also disables `wake`. The English text of that key ("Asleep: {0}\nAwake: {1}") carries `\n` as two literal characters, which the field shows as written.
- `setCurrentClock(index, awake, ms, canWake)` — `wake` is enabled only when `canWake && !awake`. At 2000 ms or more the countdown reads `$ClubUI_AdventureNpc_SleepsIn` / `$ClubUI_AdventureNpc_WakesIn`; below that it shows `$ClubUI_AdventureNpc_FallingAsleep` / `$ClubUI_AdventureNpc_WakingUp` instead.
- `refreshLayout(numActive)` — clamps the count to at least 3 (`MIN_ADVENTURES_SHOWN`), repositions `adventuresContainer` X offset, calls `background.gotoAndStop("tier" + (n-2))` to resize background, and hides the tiles past the count.
- `onAdventureClicked(e)` — when a `BaseButton` child is clicked, calls `ExternalInterface.call("adventureNPC." + buttonName, tileIndex)` (e.g. `adventureNPC.banish`, `adventureNPC.wake`).
- `visible` setter — hides if `isVaultWorld`.

translate keys: `$ClubUI_AdventureNpc_Remove`, `$ClubUI_AdventureNpc_Wake`, `$ClubUI_AdventureNpc_LeavesIn`, `$ClubUI_AdventureNpc_CircadianRhythm`, `$ClubUI_AdventureNpc_NeverSleeps`, `$ClubUI_AdventureNpc_SleepsIn`, `$ClubUI_AdventureNpc_WakesIn`, `$ClubUI_AdventureNpc_FallingAsleep`, `$ClubUI_AdventureNpc_WakingUp`.

### `ClubPermissionsPane` (extends `_kiwi.Controls.SpliceableTileView`) — [Embed symbol571]
Permission matrix: ranks 3–8 across the columns, permission rows down. Each row is a `PermissionRow` with six `PermissionsCheckbox` children (one per rank). The column headers `header3`–`header8` are authored text, not engine data: `$ClubRank_Member` (#4DE578), `$ClubRank_Captain` (#2AB8E9), `$ClubRank_Manager` (#B974E9), `$ClubRank_Officer` (#F55CBD), `$ClubRank_Leader` (#FF8502), `$ClubRank_Master` (#F5E425), each colour baked into its field. The engine shows or enables the Edit button through the `clubPermissions.editPermissions` property path.

**Key methods:**
- `setRankPermission(rankId, permId, permName, checked, unlockMaster)` — finds or creates a `PermissionRow` by `permName` ID, sets the checkbox state.
- `toggleEditPermissions()` — enables/disables all `PermissionRow.locked` states; shows "Restore Defaults" button on PC.
- `moveSelection(dx, dy)` — console D-pad navigates the 2D permission grid; scrolls to keep selected row visible.
- `toggleSelectedPermission()` — fires `ExternalInterface.call("OnPermissionChanged", permId, rankId, newValue)`.
- `onPermissionClicked(e)` — mouse path equivalent; disables the clicked checkbox until confirmed.
- `onDefaultsClicked` — calls `ExternalInterface.call("OnRestoreDefaultPermissions")`.

**translate keys:** `$Clubs_EditPermissions`, `$Clubs_RestoreDefaults`.

### `MemberRow` (extends `_kiwi.Controls.DynamicRowViewRow`) — [Embed symbol153]
Virtual-scroll row widget. Displays `textName`, `textRank`, `textLastLogin`, `textPowerRank`, `textDonations`. When `showEditDropdown = true` the `editRankComboBox` (KiwiComboBox) is shown. On rank change calls `ExternalInterface.call("OnMemberRankChanged", clubId, memberId, rankId)`, `rankId` being the `data` of the chosen `rankData` entry (the id `setRankData` sent). `btn_join` would be visible only when `online && !isSelf`, but nothing ever sets `online` (`MemberRowData` has no such field) and `btn_join` has no listener; `onJoin` → `OnJoinMember(clubId, memberId)` is only reached from the console `clubMembers.joinMember` callback.

`setData(obj)` — Reads `MemberRowData` fields, repopulates `editRankComboBox` from `ClubMembersPane.rankData`, syncs all text fields, alternates row background frame (`rowIndex % 2 * 10 + 1`).

### `MemberRowData`
Plain value object: `clubId`, `memberId`, `name`, `rank`, `powerRank:int`, `donations:int`, `lastLogin`, `isSelf:Boolean`. `clear()` resets all fields to defaults. `lastLogin` arrives as a ready-worded, translated phrase, not a timestamp, and there is no online field.

### `MemberRowExternalDataSource` (extends `DynamicRowViewExternalDataSource`)
Bridges the row pool to game data. `getData(section, row)` calls super, which sends `GetRowData(section, row)`; the engine answers synchronously by writing the member's fields onto `dataSource.rowData` before the call returns. It then calls `rowView.setRowData(section, row, rowData)` and resets `rowData`. Only rows the view is about to show are requested.

### `PermissionRow` (extends `UIComponent`) — [Embed symbol109]
One row in the permissions grid. Holds a `label:TextField` and `rank3`–`rank8:PermissionsCheckbox`. A `rank2` field is declared too, but no instance of that name is placed on the symbol, so it stays null. The `locked` setter toggles `enabled` on all rank checkboxes (except `rank8` when `unlockMaster` is false).

### `PermissionsCheckbox` (extends `_kiwi.Controls.Checkbox`) — [Embed symbol107]
Checkbox with a rank-color indicator (`color:MovieClip`) and a selection highlight (`checkSelected:MovieClip`). `checked`/`enabled` setters update `color.alpha` via `colorAlpha` (1 = checked+enabled, 0.6 = checked+disabled, 0 = unchecked). `selected` shows/hides `checkSelected`; mouse enter/leave drive it for hover.

### `FixtureRow` (extends `_kiwi.Controls.SlotBasic`) — [Embed symbol189]
List row for a fixture in the scrollable inventory. Fields: `fixtureName`, `fixtureDetail`, `fixtureTier`, `upgradeContainer` (with labeled button), `fixtureBorder`, `background`. `selected` setter drives `fixtureBG` and `background` frame labels. Only fires drag start if target is `ObjectPreview`.

### `BuffIcon` (extends `SlotBasic`)
Base class for the three buff icon variants. Slot image size 44px; holds `descriptionTF:TextField` and optional `bg:MovieClip` whose height is resized in `draw()` to match content.

- `BuffIconDesc` — Full description variant used in the hover-expanded buff panel.
- `BuffIconMedium` — Medium variant used in `ClubTilePrimary.buffs`.
- `BuffIconSmall` — Small variant (slotImageSize 32px) used in `ClubFixturesPane.buffIcons`.

### `MembersList` (extends `MovieClip`) — [Embed symbol1] — Unused thin wrapper; asset container for the DynamicRowView template.
### `Equipped` (extends `MovieClip`) — [Embed symbol80] — Asset wrapper for equipped-fixture visual.

---

## Timeline symbols (`Clubs_fla` package) — 16 classes
`membersHeader_28` (5 sort-criteria child clips), `xpBar_95` (xpBarCurrent/xpBarDaily), `HeaderArrow_31`, `CheckBoxColor_75`, `SelectedStrokeClub_103`, `SelectedStrokeClubPrimary_91`, `adventuresBG_79`, `club_name_slot_BG_102`, `buttonLegendPermissions_26`, `clubNavHeader_48`, `btn_mark_primary_46`, `rowBackground_120`, `rowUpgradeContainer_118`, `fixtureInventorySlot_116`, `fixtureSlotBorder_117`, `fixturesRowBackground_114`. All are plain `MovieClip` subclasses with `stop()` frame scripts; no custom game logic.

## Asset-wrapper classes (skin / shape / bitmap symbols) — ~30 classes
`CellRenderer_*Skin` (7), `ComboBox_*Skin` (4), `ScrollArrow*_*Skin` (8), `ScrollThumb_*Skin` (3), `ScrollTrack_skin`, `ScrollBar_thumbIcon`, `List_skin`, `focusRectSkin`, `window_inner`, `SlotFrameHigh/Medium/Normal`, `SlotBackgroundLocked`, `btn_XBOne_A/B/X/png`, `btn_console_east/south/west`, plus minor button asset symbols (`btnArrowLeft`, `btnArrowRight`, `btnChat`, `btnDemote`, `btnDetail`, `BtnGreen`, `btn_pencil`, `editBtn`, `joinButton`, `moveUpBtn`, `iconRentDue`, `fixtureSlot*` subtypes, `artClip`, `SecondaryHeaderLeft`).

---

## Notable logic

- **Virtual scroll:** `ClubMembersPane` uses the `_kiwi.Controls.DynamicRowView` row-pool pattern. `MemberRowExternalDataSource.getData()` is called by the framework; the engine fills the shared `rowData` during the synchronous `GetRowData` call, and the data source then calls `rowView.setRowData(section, row, rowData)` — but `rowData` is always a single shared `MemberRowData` that gets cleared after each call, so the actual data must be written into the row's display fields by `MemberRow.setData()` which is triggered synchronously.
- **Vault-world mode:** When `setIsVaultWorld(true)`, both `ClubFixturesPane` and `ClubAdventuresPane` force `visible = false` regardless of the tab, and `errorMessage` is repositioned and shown.
- **MOTD dual-channel:** The `OnSetMOTD` call passes source `0` when submitted from the primary tile's inline edit and `1` when submitted from the Members pane's edit button. Both send the call even when nothing changed, with `"-1"` in place of the club id when the text is unchanged or empty (and always on cancel).
- **Fixture drag-drop:** `SlotDragDropHelper` intercepts drag events globally; `ClubFixturesPane.onDrop` uses `hitTestPoint` across all 11 live plots and calls `OnEquipFixture(itemId, plotIndex)` for the plot under the drop point.
- **Locked fixtures ordering:** `addListFixtureData` defers items with `id == -1` (locked) into `lockedFixtures[]` and only adds them when `categoryComplete()` is called, ensuring unlocked items appear first within each category.
- **Permission matrix navigation:** The grid tracks `_lastSelectedX` (rank 3–8) and `_lastSelectedY` (row index) independently and uses hardcoded scroll constants (`3.31` pixels per row) to keep the selected item in view.
