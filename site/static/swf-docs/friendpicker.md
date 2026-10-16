# friendpicker.swf
> A modal dialog that displays the player's friends list with search filtering, allowing the player to select a friend or manage social actions. Its header is hard-wired to `$FriendPicker_Header` ("HEART-A-PHONE"): it is the picker for a Heart-a-Phone's recipient.

**Document/main class:** `FriendPicker` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 8 (excluding framework/asset wrappers)

## Main class: `FriendPicker`

Hosts a `FriendView` scrollable list backed by a `FriendsFilteredDataSource`, a `WindowHeader` titled `$FriendPicker_Header`, a `TextField` search input, and a search-highlight overlay `MovieClip`. On construction it wires `PickerListItem` as the list row template, populates dummy data when not running in Iggy, and on console platforms registers an `ENTER_FRAME` listener to wait for the target frame before attaching the keyboard listener. On PC, keyboard focus is immediately set to the search input via `configUI`.

### Public methods
- `setSearchFocus(focus:Boolean) : void` — Toggled by `ExternalInterface` callback (console); shows/hides the search highlight clip, changes the search field text colour, and manages stage focus.

### Key fields
- `dataSource : FriendsFilteredDataSource` — the filtered data model feeding `listView`.
- `listView : FriendView` — the virtualized scrollable friend list.
- `searchInput : TextField` — the PC search box; its `KEY_UP` event drives `dataSource.filterString`.
- `searchHighlight : MovieClip` — visible highlight ring around the search field on console.
- `closeButton : MovieClip` — close affordance (click handling delegated to game layer).
- `__id0_ : WindowHeader` — titled `$FriendPicker_Header`; set disabled (decorative).

### Frame scripts / timeline
- `frame1` / `frame11` / `frame21` — each calls `stop()`, one per frame label: `PC` (frame 1), `Console` (frame 11: removes `closeButton`, swaps in a console `searchInput` and adds `buttonLegendClose`) and `ConsoleLoc` (frame 21: another `searchInput` variant and `buttonLegendLoc`).

### Runtime dependencies & integration
- `IggyFunctions.inIggy` — branch for live vs. preview mode.
- `ExternalInterface.addCallback("setSearchFocus", ...)` — registered only on console.
- `IggyFunctions.translate("$FriendPicker_Header")` — window title.
- `IsConsole()` — controls whether keyboard or controller nav is used.

---

## Other game-specific classes

### `FriendsFilteredDataSource` (extends `FriendViewDataSource`)
Adds real-time name filtering on top of the sorted base data source. Maintains a parallel `excludedData` array of items that don't match the current filter. When `filterString` changes, items are moved between `itemData` and `excludedData` in-place, dispatching `DataChangeEvent`s so the view refreshes. `addFriend(accountId, name, isOnline, action)` and `clear` are exposed via `ExternalInterface`. The `action` field is the label shown on that row's select button, chosen per friend by the engine; it arrives as a `$` translation key, which Iggy resolves only once it reaches the button's TextField.

### `FriendViewDataSource` (extends `_kiwi.Core.UIComponent`)
Base data source for the friend list. Holds `itemData:Array` of friend objects and fires `DataChangeEvent` on add/remove/clear. Inserts items sorted: friend-requests first (by name), then online friends (by name), then offline friends (by name). Uses `natCaseCompare` on console for locale-aware sorting; `localeCompare` on PC.

### `FriendView` (extends `_kiwi.Controls.ScrollableView`) — Embed symbol84
Virtualized list renderer with a 16-item pool. Uses `FriendViewDataSource` for data. Supports both `FriendListItem` rows (full social panel) and `PickerListItem` rows (select-only). Exposes `ExternalInterface` callbacks (registered on every platform, not only console): `moveHighlight(delta)`, `moveHighlightBegin`, `moveHighlightEnd`, `unHighlightCurrent`, `joinFriend`, `inviteFriend`, `showProfile`, `onSelect`. Its `OnAcceptRequest`, `OnWhisper`, `OnJoinWorld`, `OnInviteToJoinMe` and `OnShowProfile` calls only go out through a `FriendListItem`; this screen's rows are `PickerListItem`s, so `joinFriend`, `inviteFriend` and `showProfile` do nothing here and those five calls never fire. Scroll-wrapping and highlight-on-refresh are configurable. Frame scripts: `frame1`, `frame11`.

### `FriendListItem` (extends `ListItemBase`)
Full-featured friend row. Fields: `playerNameTextField`, `worldTextField`, `rankTextField`, `joinButton`, `inviteToJoinMeButton`, `whisperButton`, `acceptButton`, `removeButton`, `optionsButton`, `mc_status`, `mc_row_bg`. Tracks `playerName`, `online`, `isRequest`, `canAccept`, `world`, `rank`, `canJoinWorld`, `canInvite`, `ignored`, `highlight` as invalidating properties. On draw: shows/hides buttons based on state, displays `$FriendRequest_WaitOnAccept` or `$FriendRequest_WaitOnOther` for pending requests, plays `mc_status` to "online"/"offline" label. Console uses frame labels `OverConsole`/`UpConsole` and hides most buttons, showing only `optionsButton` (labelled `$ContextPopout_Header_SocialMenu`) when highlighted. Button clicks call: `OnAcceptRequest(id)`, `OnRemove(id, isRequest)`, `OnWhisper(name)`, `OnJoinWorld(id)`, `OnInviteToJoinMe(id)`.

### `PickerListItem` (extends `ListItemBase`) — Embed symbol22
Simplified friend row used in picker mode. Shows `playerNameTextField`, `selectButton` (authored label `$FriendPicker_Send`, overwritten per row by the `action` string), `mc_status`, and `itemHighlight`. On console, `selectButton` and `itemHighlight` are only visible when `highlight` is true. Click on `selectButton` fires `ExternalInterface.call("OnSelect", identity)` with the account id string; the engine does not check that id against the friends list.

### `FriendListItemData`
Plain data value object: `accountId`, `name`, `isOnline`, `world`, `rank`, `canJoinWorld`, `isRequest`, `canAccept`, `canInvite`, `ignored`, `highlight`.

### `ListItemBase` (extends `_kiwi.Core.UIComponent`)
Abstract base for list rows. Holds `dataIndex:int`, `identity:String` (set from `accountId`), and `alternateColor:Boolean`. `clone()` throws `IllegalOperationError` — must be overridden.

### `FriendPicker_fla.Status_28` — Embed symbol3
Timeline symbol for the online/offline status indicator dot. Two frames (10 and 20) each `stop()`, corresponding to "online" and "offline" states.

### Asset wrappers (8 classes)
`ScrollArrowDown_disabledSkin`, `ScrollArrowDown_downSkin`, `ScrollArrowDown_overSkin`, `ScrollArrowDown_upSkin`, `ScrollArrowUp_disabledSkin`, `ScrollArrowUp_downSkin`, `ScrollArrowUp_overSkin`, `ScrollArrowUp_upSkin`, `ScrollThumb_downSkin`, `ScrollThumb_overSkin`, `ScrollThumb_upSkin`, `ScrollTrack_skin`, `focusRectSkin`, `BtnGreen`, `btnGreen_small` — pure skin/shape symbols, no logic.

## Notable logic
- **Dual-mode list**: The same `FriendView` can render either `FriendListItem` (full social panel) or `PickerListItem` (just a select button). The mode is set by `FriendPicker` assigning `new PickerListItem()` as `listView.itemTemplate` — making this a picker rather than a friend-management panel.
- **Sorted insertion**: Friends are always kept in order: pending requests → online → offline, each sub-group alphabetical, so there is never a "sort" button.
- **Filter/exclude pattern**: Rather than rebuilding the list from scratch on each keystroke, `FriendsFilteredDataSource` moves items between `itemData` and `excludedData`, keeping the sort order intact and firing minimal `DataChangeEvent`s.
- **Console focus callback**: `setSearchFocus` is registered via `ExternalInterface` so the game engine can programmatically move the virtual cursor into the search field on controller platforms.
