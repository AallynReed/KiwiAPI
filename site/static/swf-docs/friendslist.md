# friendslist.swf
> The social Friends List panel, showing the player's friends and ignored users in two tabs. Appears when the player opens the social/friends menu in Trove, supporting both PC and console (Xbox One, NX/Switch, PS4/Orbis) platforms.

**Document/main class:** `FriendsList` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 11

## Main class: `FriendsList`

`FriendsList` is the root UIComponent for the friends panel. It owns two tab MovieClips (`friendsTab`, `ignoredTab`), a scrollable `FriendView` list, and an `inviteButton`. On construction it registers three frame scripts (frames 1, 11, 21 — all `stop()`), creates `FriendListItem` as the view's item template, and initialises a `WindowHeaderSmall` with title key `$FriendsList_WindowName`.

`configUI()` sets up the data layer: creates a `FriendsExtendedDataSource`, wires it to `FriendView`, sets vertical step to 20, translates the tab labels, and adds click listeners to both tabs. On console it also registers an `Event.RENDER` listener to populate button-legend text after the first frame. Non-Iggy (authoring preview) mode populates 100 dummy friends for layout testing.

In Iggy (runtime) mode a set of `ExternalInterface.addCallback` hooks are registered on the main class, plus extra console-only callbacks for the social context pop-out system.

### Public methods
- `activatePlayerInteractOption(index:int) : void` — calls `ExternalInterface.call("OnPlayerInteractOptionSelected", index, playerName)` using the topmost open `ContextPopOut`.

### Key fields
- `friendView : FriendView` — the virtualised scrollable list of friend rows.
- `openContextLists : Vector.<ContextPopOut>` — stack of open context pop-outs (social menu, sub-menus).
- `dataSource : FriendsExtendedDataSource` — backing data model.
- `selectedTab : int` — 0 = Friends, 1 = Ignored.
- `inviteButton : BaseButton`, `closeButton : MovieClip`, `friendsTab / ignoredTab : MovieClip`, `backBanner : MovieClip`, `bgFrame : MovieClip`, `tabWindow : MovieClip`, `buttonLegend : MovieClip`.

### Frame scripts / timeline
- Frame 1, 11, 21 — all `stop()`. Three visual states (PC friends, PC ignored, console variant implied by tab animation labels).

### Runtime dependencies & integration
**ExternalInterface callbacks registered (Iggy):**
- `onInvite` → `ExternalInterface.call("OnInvite")`
- `switchTab` — toggles between Friends/Ignored tabs, calls `OnTabClick`
- `openSocialMenu(showPvpInvite:Boolean)` — creates and positions a `ContextPopOut`
- `closeSocialMenu` — removes all open `ContextPopOut`s
- `previousContextList` — pops topmost context list, calls `OnCloseSocialMenu` when empty
- `moveContextListHighlight(dir:int)` — moves selection inside topmost `ContextListView`
- `activateContextListSelection` — triggers `activatePlayerInteractOption` on current selection
- `addItemToContextMenu(text:String)` — adds a `ContextListItem` to the topmost `ContextListView`

**Iggy calls fired:**
- `OnInvite`, `OnTabClick(tabIndex)`, `OnPlayerInteractOptionSelected(optionIndex, playerName)`, `OnCloseSocialMenu`

**translate keys:** `$FriendsList_WindowName`, `$FriendsList_FriendTab`, `$FriendsList_IgnoredTab`, `$Select_ButtonLegend`, `$ViewProfile_ButtonLegend_nx`, `$Remove_ButtonLegend`, `$Close_ButtonLegend`, `$LRToggle_ButtonLegend`

**Platform checks:** `IsConsole()`, `IsNX()` — tab animation labels differ (`selectedConsole`/`unselectedConsole`), button legend text changes, `PositioningButtons()` called on NX.

---

## Other game-specific classes

### `FriendViewDataSource` (extends `_kiwi.Core.UIComponent`)
Abstract sorted data store. Holds `itemData:Array` of `FriendListItemData` objects. `insertItemSorted()` maintains ordering: pending friend requests first, then online players, then offline — each group sorted case-insensitively by name (uses `natCaseCompare` on console). Fires `DataChangeEvent.DATA_CHANGE` / `PRE_DATA_CHANGE` on mutations.

### `FriendsExtendedDataSource` (extends `FriendViewDataSource`)
Adds Iggy callbacks: `clear`, `addFriend`, `addIgnored`, `removeFriend`, `updateFriend`. Constructs `FriendListItemData` and delegates to `insertItemSorted` / `removeAtIndex`. `updateFriend` re-inserts if name or online status changed (to maintain sort order).

### `FriendListItemData`
Plain value object: `accountId`, `name`, `isOnline`, `world`, `rank`, `canJoinWorld`, `isRequest`, `canAccept`, `canInvite`, `teamPvpEnabled`, `ignored`, `highlight`.

### `FriendView` (extends `_kiwi.Controls.ScrollableView`) — [Embed symbol165]
Virtualised list renderer. Maintains a pool of 16 reusable `ListItemBase` instances. Subscribes to `DataChangeEvent` from its `FriendViewDataSource`. Visible range is computed from the scroll rect; items outside are returned to pool. On console, exposes Iggy callbacks: `moveHighlight`, `moveHighlightBegin`, `moveHighlightEnd`, `unHighlightCurrent`, `joinFriend`, `inviteFriend`, `inviteToPVP`, `showProfile`, `onSelect`. Each fires the corresponding `ExternalInterface.call`: `OnAcceptRequest`, `OnWhisper`, `OnJoinWorld`, `OnInviteToJoinMe`, `OnInviteToJoinPVP`, `OnShowProfile`.

### `ListItemBase` (extends `_kiwi.Core.UIComponent`)
Abstract base for list rows. Holds `dataIndex:int`, `identity:String` (account ID), and `alternateColor:Boolean`. `clone()` must be overridden.

### `FriendListItem` (extends `ListItemBase`) — [Embed symbol56]
Concrete friend row. Three timeline frames: frame 1 = PC layout, frames 2–3 = console unhighlighted/highlighted. Buttons: `joinButton`, `inviteToJoinMeButton`, `whisperButton`, `acceptButton`, `removeButton`, `pvpInviteButton`, `optionsButton` (console social menu button). On console, action buttons are hidden and `optionsButton` visibility tracks the highlight state. `draw()` invalidates on STATE/DATA: updates text colours, button visibility based on `isRequest`, `ignored`, `canJoinWorld`, `canInvite`, `teamPvpEnabled`, `online`. Fires: `OnAcceptRequest`, `OnRemove`, `OnWhisper`, `OnJoinWorld`, `OnInviteToJoinMe`, `OnInviteToJoinPVP`. translate keys: `$FriendRequest_WaitOnAccept`, `$FriendRequest_WaitOnOther`, `$WhisperUser`, `$FriendRequest_Join`, `$FriendRequest_JoinMe`, `$FriendRequest_Accept`, `$ContextPopout_Header_SocialMenu`.

### `ContextPopOut` (extends `_kiwi.Core.UIComponent`) — [Embed symbol102]
Pop-out panel that appears beside a friend row on console. Takes a `FriendListItem` reference and a `showPvpInvite` flag. Menu types: `SOCIAL_MENU` (0), `WHISPER` (1), `JOIN` (2), `INVITE` (3), `PVPTEAM` (4), `IGNORE` (5), `PROFILE` (6). For `SOCIAL_MENU` builds a list of `ContextListItem` entries with translated labels; for other types fires `ExternalInterface.call` to let the game populate the list (`OnSetUpRecentChatList`, `OnSetUpNearbyPlayersList`, `OnSetUpFriendsList`, `OnSetUpClubsList`, `OnSetUpClubMembersList`). Has `headerText:String` shown in the header TextField. Owns a `ContextListView` and a `btnLegend` MovieClip.

### `ContextListView` (extends `_kiwi.Controls.ScrollableView`, implements `IEventDispatcher`) — [Embed symbol94]
Vertical scrollable list of `ContextListItem` MovieClips. Tracks `curSelection:int`. `addItem()` auto-selects the first enabled item. `moveHighlight(dir)` cycles through enabled items, changing text colour to yellow (0xFFFF00) for selected / white for others. `setMenuActive()` shows/hides the `btnA` icon and the parent `ContextPopOut`'s legend.

### `ContextListItem`
Simple item MovieClip with `itemText:TextField` and `btnA` icon (A-button indicator shown on console). Referenced but not separately read — used as a generic container.

### `PickerListItem`
Referenced in `FriendView` as an alternate item type for picker mode. Not detailed further as only cast/checked there.

### Asset wrappers (13 classes)
`ScrollArrowDown_disabledSkin`, `ScrollArrowDown_downSkin`, `ScrollArrowDown_overSkin`, `ScrollArrowDown_upSkin`, `ScrollArrowUp_disabledSkin`, `ScrollArrowUp_downSkin`, `ScrollArrowUp_overSkin`, `ScrollArrowUp_upSkin`, `ScrollThumb_downSkin`, `ScrollThumb_overSkin`, `ScrollThumb_upSkin`, `ScrollTrack_skin`, `ScrollBar_thumbIcon`, `focusRectSkin`, `btn_XBOne_A`, `btn_console_south`, `btnBomberRoyale`, `btnKick`, `btnGreen_blank`, `BtnGreen`, `btnGreen_small` — scrollbar/button skin and icon symbols; no logic.

### `FriendsList_fla/` timeline symbols (4 classes)
- `bannerTop_2` — top banner MovieClip (frame 2 symbol).
- `bannerBottom_5` — bottom banner MovieClip (frame 5 symbol).
- `ButtonLegend_31` — PC button legend (frame 31 symbol).
- `rowBackground_57` — alternating row background (frame 57 symbol).
- `Status_60` — online/offline status indicator animation (frame 60 symbol).

---

## Notable logic
- **Sorted insertion:** Friends are kept sorted online-first, then by name, using different comparison functions per platform (`natCaseCompare` on console vs. `localeCompare` on PC).
- **Context pop-out stack:** Multiple nested pop-outs can be pushed (e.g. Social Menu → sub-list); `previousContextList` pops one level, firing `OnCloseSocialMenu` only when fully empty.
- **Virtual list pool:** `FriendView` pre-allocates 16 `FriendListItem` clones; only visible items are parented to the display list — items scrolled out of view are returned to the pool.
- **Platform branching:** Extensive `IsConsole()` / `IsNX()` checks throughout; console hides action buttons (whisper/join/invite) on each row and replaces them with an `optionsButton` that triggers the context pop-out system.
