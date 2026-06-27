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
- `setToggleJoinChat(clubIndex:Number) : void` — Toggles `btnJoinChat.checked` on the corresponding tile, then calls `ExternalInterface.call("OnToggleJoinChat", clubId, checked)`.
- `setTogglePrimaryEditMOTD() : Boolean` — Delegates to `clubList.club01.onEditMotd(null)`.
- `cancelPrimaryEditMOTD() : void` — Delegates to `clubList.club01.cancelEditMOTD()`.
- `selectMember(category:int, index:Number) : void` — Delegates to `clubMembers.selectMember`.
- `setMotdText(clubIndex:Number, text:String) : void` — Finds the primary tile by index and calls `setMotdText`.

### Key fields
- `clubList : ClubDashboardPane` — Five-slot club list view.
- `clubMembers : ClubMembersPane` — Virtual-scrolling member roster.
- `clubFixtures : ClubFixturesPane` — Slot-based fixture equip/unequip pane.
- `clubAdventures : ClubAdventuresPane` — Adventure NPC tile row.
- `clubPermissions : ClubPermissionsPane` — Rank-permission matrix.
- `clubNavHeader : MovieClip` — Tab strip with five named child buttons.
- `errorMessage : MovieClip` — Shown when primary slot is vacant or vault-world restrictions apply.
- `dataSource : MemberRowExternalDataSource` — Shared data bridge for `DynamicRowView` row pool.
- `tabFunctions : Array` — Parallel to `tabHeaders`; JS function names called on tab click.
- `HIGHLIGHT_COLOR : uint = 0x00FFFF` — Shared cyan highlight color used by all panes on console.

### Frame scripts / timeline
- `frame1()` — `stop()`.
- `frame11()` — `stop()`.

### Runtime dependencies & integration
**Iggy callbacks (non-console):**
`setIsVaultWorld`, `showDetails`, `hideDetails`, `selectMember`, `setMotdText`, `showTab`,
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

**Outbound calls:** `OnReturnToClubList`, `OnShowClubDetails`, `OnShowClubFixtures`, `OnShowClubAdventures`, `OnShowClubPermissions` (tab clicks), `OnToggleJoinChat`, `TOOLTIP.HIDE`, `TOOLTIP.SHOW`.

**Events listened:** `MouseEvent.CLICK` on each tab header button; `ClubTilePrimary.EVENT_VACANT` from `clubList.club01`.

**translate keys:** `$Clubs_Header` (window header), `$Club_NeedPrimary`, `$Club_CreateOrJoin` (error message).

---

## Other game-specific classes

### `ClubDashboardPane` (extends `UIComponent`) — [Embed symbol364]
Five-slot club list. Holds `club01:ClubTilePrimary` and `club02–05:ClubTile` (typed as `ClubTileSecondary` at runtime). Sets channel numbers 1–5 and disables `club02.moveUp`. Exposes `getClub(clubId)` (linear search) and `selectClub(index)` (console selection highlight + button-legend text).

Frame 11 (`Console`): calls `gotoAndPlay("Console")` on all five tiles.

### `ClubTile` (extends `UIComponent`)
Base class for all club tiles. Displays: `textClubName`, `textMemberType`, `textClubLevel`, `textClubPowerRank`, `textClubMemberCount`, `btnJoinChat:Checkbox`, `btn_forward:BaseButton`, `btn_world:LabelButton`, `moveUp:MovieClip`.

Fires: `ExternalInterface.call("OnShowClubDetails", clubId)`, `"OnJoinWorld"`, `"OnSetPrimary"`, `"OnToggleJoinChat"`, `"OnIncreaseClubDisplayPriority"`.

### `ClubTilePrimary` (extends `ClubTile`) — [Embed symbol356]
The primary (first-slot) club tile. Adds: XP bar, buff scroll (`ScrollableTileView buffs`), MOTD text field with inline edit (toggle via `editMotd` button or `onEditMotd()`), club log, rent-due icon, XP info tooltip button.

**Key methods:**
- `setXPbar(current, daily, dailyStart, max)` — scales `xpBarCurrent.width` and `xpBarDaily.width`; builds tooltip via `$Club_ExperienceTooltipDesciption`.
- `addLogEntry(time, text)` — appends `TimeUtil.localizeTime(time) + " : " + text + "\n"` to `textClubLog`.
- `addBuff(iconUrl, active)` — instantiates `BuffIconMedium`, adds to `buffs` ScrollableTileView.
- `onEditMotd(e)` — toggles `textFieldBg` and switches `textMOTD.type`; on close calls `ExternalInterface.call("OnSetMOTD", clubId, text, 0)` (channel 0 = primary tile).
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
- `setRankData(id, label, canPromote, canEdit)` / `resetRankData()` — maintain the static `rankData:Array` used by all visible `MemberRow` instances.
- `setEditMode(editing:Boolean)` — toggles `_showRankDropdown` static flag; calls `editVisibleRows` to show/hide the rank ComboBox on each visible row.
- `selectMember(category, index)` — scrolls to and highlights the specified row; tracks `m_selectedRosterItem`.
- `onHeaderPressed(col:int)` / `onSortMemberList(e)` — calls `ExternalInterface.call("OnSortMemberList", col, ascending)` and updates sort-arrow frame labels.
- `setIsEditingMOTD(editing:Boolean)` — enables `textMOTD` as an `INPUT` field; on close calls `ExternalInterface.call("OnSetMOTD", clubId, text, 1)` (channel 1 = members pane).
- `setPrimaryChangeCooldown(sec:Number)` — sets tooltip text on `btnSetPrimary` using `TimeUtil.localizeTime` and `$Club_NoPrimaryChange_Description`.

**Events fired:** `OnLeave`, `OnInvite`, `OnSetPrimary`, `OnEditModeSet`, `OnSortMemberList`, `OnSetMOTD`, `TOOLTIP.SHOW`, `TOOLTIP.HIDE`.

**Static fields:** `rankData:Array`, `_showRankDropdown:Boolean`, `_isEditingMOTD:Boolean`.

**Frame scripts:** frames 1, 11, 21 (three visual states: PC, Console, Console-edit).

**translate keys:** `$Invite`, `$Clubs_EditRoster`, `$Clubs_StopEditRoster`, `$Club_Leave`, `$Clubs_SetPrimary`, `$Club_NumOnline`, `$Club_NoPrimaryChange_Title`, `$Club_NoPrimaryChange_Description`, `$Club_AlreadyPrimary_Title`, `$Club_AlreadyPrimary_Description`.

### `ClubFixturesPane` (extends `_kiwi.Controls.ScrollableTileView`) — [Embed symbol471]
Fixture equip/inventory pane. Hosts 11 named `Slot` fields (`slot_base0/1`, `slot_combat0–3`, `slot_utility0–2`, `slot_ultimate0/1`; `slot_event0` hidden). The scrollable list is populated with `BagContainerBasic` category sections, each holding `FixtureRow` items.

**Key methods:**
- `addFixtureTypeHeading(name)` — creates a `BagContainerBasic` category row.
- `addListFixtureData(id, name, desc, icon, type, tier, maxTier)` — defers locked items (id == -1) until `categoryComplete()`.
- `addEquippedFixture / addEmptyFixture / addLockedFixture` — populate the fixed slot grid at the top.
- `moveSelection(dx, dy)` — console D-pad navigation between slots and list rows using `DirectionalMapping` children.
- `activateCurrentSelection / upgradeCurrentSelection / unequipCurrentSelection` — console actions.
- `setRentDueAmount(n) / setRentDueTime(sec)` — update rent UI; time uses `TimeUtil.localizeTime(sec, 2)` and `$ClubsUI_RentDueTime`.
- Drag-drop: `SlotDragDropHelper.registerDropCallback(onDrop)` — equips a dragged item into the first empty slot of the matching type via `ExternalInterface.call("OnEquipFixture", itemId, slotIndex)`.

**Events fired:** `OnPayRent`, `OnAutoPayRent`, `OnEquipFixture`, `OnUnequipFixture`, `OnUpgradeFixture`, `TOOLTIP.SHOW`, `TOOLTIP.HIDE`.

**translate keys:** `$ClubsUI_PayRent`, `$ClubsUI_RentDueTime`, `$ClubUI_Fixture_Tier`, `$Clubs_Upgrade`.

### `ClubAdventuresPane` (extends `UIComponent`) — [Embed symbol403]
Adventure NPC tile row. Manages up to 5 `adventureTile*` MovieClips inside `adventuresContainer` and matching empty-slot clips inside `adventuresEmptyContainer`.

**Key methods:**
- `showTile(index, visible)` — shows/hides adventure tile vs. empty-slot placeholder.
- `setDeparture(index, seconds)` — sets `leavesIn` text via `TimeUtil.localizeTime(sec, 2, true)` and `$ClubUI_AdventureNpc_LeavesIn`.
- `setSchedule(index, sleepDur, wakeDur)` — sets `circadianRhythm` text; zero `sleepDur` means "Never Sleeps" (`$ClubUI_AdventureNpc_NeverSleeps`).
- `setCurrentClock(index, awake, timeUntilChange, canWake)` — updates sleep/wake countdown and enables/disables the `wake` button.
- `refreshLayout(numActive)` — repositions `adventuresContainer` X offset and calls `background.gotoAndStop("tier" + (n-2))` to resize background.
- `onAdventureClicked(e)` — when a `BaseButton` child is clicked, calls `ExternalInterface.call("adventureNPC." + buttonName, tileIndex)` (e.g. `adventureNPC.banish`, `adventureNPC.wake`).
- `visible` setter — hides if `isVaultWorld`.

translate keys: `$ClubUI_AdventureNpc_Remove`, `$ClubUI_AdventureNpc_Wake`, `$ClubUI_AdventureNpc_LeavesIn`, `$ClubUI_AdventureNpc_CircadianRhythm`, `$ClubUI_AdventureNpc_NeverSleeps`, `$ClubUI_AdventureNpc_SleepsIn`, `$ClubUI_AdventureNpc_WakesIn`, `$ClubUI_AdventureNpc_FallingAsleep`, `$ClubUI_AdventureNpc_WakingUp`.

### `ClubPermissionsPane` (extends `_kiwi.Controls.SpliceableTileView`) — [Embed symbol571]
Permission matrix: ranks 3–8 across the columns, permission rows down. Each row is a `PermissionRow` with six `PermissionsCheckbox` children (one per rank).

**Key methods:**
- `setRankPermission(rankId, permId, permName, checked, unlockMaster)` — finds or creates a `PermissionRow` by `permName` ID, sets the checkbox state.
- `toggleEditPermissions()` — enables/disables all `PermissionRow.locked` states; shows "Restore Defaults" button on PC.
- `moveSelection(dx, dy)` — console D-pad navigates the 2D permission grid; scrolls to keep selected row visible.
- `toggleSelectedPermission()` — fires `ExternalInterface.call("OnPermissionChanged", permId, rankId, newValue)`.
- `onPermissionClicked(e)` — mouse path equivalent; disables the clicked checkbox until confirmed.
- `onDefaultsClicked` — calls `ExternalInterface.call("OnRestoreDefaultPermissions")`.

**translate keys:** `$Clubs_EditPermissions`, `$Clubs_RestoreDefaults`.

### `MemberRow` (extends `_kiwi.Controls.DynamicRowViewRow`) — [Embed symbol153]
Virtual-scroll row widget. Displays `textName`, `textRank`, `textLastLogin`, `textPowerRank`, `textDonations`. When `showEditDropdown = true` the `editRankComboBox` (KiwiComboBox) is shown. On rank change calls `ExternalInterface.call("OnMemberRankChanged", clubId, memberId, rankData)`. `btn_join` is visible only when `online && !isSelf`; click fires `OnJoinMember`.

`setData(obj)` — Reads `MemberRowData` fields, repopulates `editRankComboBox` from `ClubMembersPane.rankData`, syncs all text fields, alternates row background frame (`rowIndex % 2 * 10 + 1`).

### `MemberRowData`
Plain value object: `clubId`, `memberId`, `name`, `rank`, `powerRank:int`, `donations:int`, `lastLogin`, `isSelf:Boolean`. `clear()` resets all fields to defaults.

### `MemberRowExternalDataSource` (extends `DynamicRowViewExternalDataSource`)
Bridges the row pool to game data. `getData(section, row)` calls super then `rowView.setRowData(section, row, rowData)` and resets `rowData`.

### `PermissionRow` (extends `UIComponent`) — [Embed symbol109]
One row in the permissions grid. Holds a `label:TextField` and `rank3`–`rank8:PermissionsCheckbox`. The `locked` setter toggles `enabled` on all rank checkboxes (except `rank8` when `unlockMaster` is false).

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

- **Virtual scroll:** `ClubMembersPane` uses the `_kiwi.Controls.DynamicRowView` row-pool pattern. `MemberRowExternalDataSource.getData()` is called by the framework; it then calls `rowView.setRowData(section, row, rowData)` — but `rowData` is always a single shared `MemberRowData` that gets cleared after each call, so the actual data must be written into the row's display fields by `MemberRow.setData()` which is triggered synchronously.
- **Vault-world mode:** When `setIsVaultWorld(true)`, both `ClubFixturesPane` and `ClubAdventuresPane` force `visible = false` regardless of the tab, and `errorMessage` is repositioned and shown.
- **MOTD dual-channel:** The `OnSetMOTD` call passes channel `0` when submitted from the primary tile's inline edit and channel `1` when submitted from the Members pane's edit button, allowing the game to distinguish the source.
- **Fixture drag-drop:** `SlotDragDropHelper` intercepts drag events globally; `ClubFixturesPane.onDrop` uses `hitTestPoint` across all 11 fixture slots and calls `OnEquipFixture` on the first hit slot.
- **Locked fixtures ordering:** `addListFixtureData` defers items with `id == -1` (locked) into `lockedFixtures[]` and only adds them when `categoryComplete()` is called, ensuring unlocked items appear first within each category.
- **Permission matrix navigation:** The grid tracks `_lastSelectedX` (rank 3–8) and `_lastSelectedY` (row index) independently and uses hardcoded scroll constants (`3.31` pixels per row) to keep the selected item in view.
