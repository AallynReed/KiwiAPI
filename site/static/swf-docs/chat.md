# chat.swf
> The in-game chat window for Trove. It displays scrollable message logs in named tabs, a text input bar with channel selector, a console radial menu for quick chat options, and a social context pop-out system (recent players, nearby, clubs, friends, emotes, player-interact menus).

**Document/main class:** `Chat` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 11 (excluding skin/asset wrappers and framework files)

---

## Main class: `Chat`

`Chat` is the root document class. The constructor adds two frame scripts (frames 1 and 11), creates a default "All Messages" tab, selects it, and hides the radial. `configUI()` disables tab-focus on the scrollbar, links it to `logView`, and registers all Iggy `ExternalInterface` callbacks. Outside Iggy a few test messages are injected and the input is activated immediately.

The window has two height states: **expanded** (370px, y=0, shown when input is active) and **collapsed** (218px, y=180, shown when input is inactive). `onSetActive(bool)` drives the transition.

### Public methods

- `goToSector(sector:int) : void` — Navigates the console radial to the given sector frame and applies "highlightedTextFormat" to the matching sector label.
- `setSectorText(sector:int, text:String) : void` — Sets the text of a radial sector label directly.
- `activateSocialMenuOption(option:int) : void` — Dispatches a social menu selection: opens Quick Chat Settings, executes `/epicpose`, or pushes a new `ContextPopOut` for Players/Clubs/Friends/Emotes.
- `activatePlayersListOption() : void` — Pushes a PLAYER_INTERACT `ContextPopOut` with the selected player name as header.
- `activateClubsListOption() : void` — Pushes a CLUB_MEMBERS `ContextPopOut` with the selected club name as header.
- `activateEmotesListOption() : void` — Reads the emote command string from `emotes[]` and calls `ExternalInterface.call("OnExecute", "/" + emote)`.
- `activatePlayerInteractOption(option:int) : void` — Calls `ExternalInterface.call("OnPlayerInteractOptionSelected", option, playerName)`.
- `disableSocialMenuEmotesOption() : void` — Disables the Emotes item (index 4) in the first open context list.

### Key fields

- `tabLogs : Array` — Array of `ChatLog` instances, one per tab. Index matches tab id passed by the engine.
- `logView : ChatLogView` — The scrollable message display area.
- `chatInput : ChatInput` — The text input component with channel selector.
- `scrollbar : UIScrollBar` — Scrollbar linked to `logView`.
- `radial : MovieClip` — Console radial menu (visible only when input is active on console).
- `buttonLegendRadial : MovieClip` — Button legend shown alongside the radial.
- `openContextLists : Array` — Stack of currently open `ContextPopOut` instances.
- `emotes : Array` — String list of emote command names (e.g. `"sit"`, `"dance"`), populated by `setUpEmotesList`.
- `mounted : Boolean` — Whether the player is on a mount; controls which emotes are available.

### Frame scripts / timeline

- **frame 1** (`frame1`) — `stop()`.
- **frame 11** (`frame11`) — `stop()`. (Console variant frame.)

### Runtime dependencies & integration

**ExternalInterface callbacks registered:**
`onSetActive`, `addTab`, `removeTab`, `renameTab`, `addMessage`, `onAdjustScrollPos`, `setDisplayTimeStamp`, `goToSector`, `setSectorText`, `openSocialMenu`, `closeSocialMenu`, `previousContextList`, `moveContextListHighlight`, `activateContextListSelection`, `addItemToContextMenu`, `setUpEmotesList`, `disableSocialMenuEmotesOption`

**ExternalInterface calls (Flash → engine):**
`OnCloseSocialMenu`, `OnOpenQuickChatSettings`, `OnExecute`, `OnPlayerInteractOptionSelected`, `OnChatFadeComplete`, `OnExecute` (emotes), `CheckEmotesCount`, `OnSetUpRecentChatList`, `OnSetUpNearbyPlayersList`, `OnSetUpFriendsList`, `OnSetUpClubsList`, `OnSetUpClubMembersList`, `OnSetUpEmotesList`, `OnRequestContextMenu`, `OnAutocomplete`, `OnCycleWhisperTarget`, `OnTabAutocomplete`, `OnCycleInputHistory`, `expandChannelSelect`, `collapseChannelSelect`, `highlightChannelSelection`, `unhighlightChannelSelection`, `moveChannelHighlight`, `addSelectableChannel`, `clearSelectableChannels`, `selectChannel`

**Translate keys (in `ContextPopOut.configurePopout`):** `$ContextPopout_Option_RecentPlayers`, `$ContextPopout_Option_Nearby`, `$MainMenu_Clubs`, `$MainMenu_FriendsList`, `$ContextPopout_Option_Emotes`, `$ContextPopout_Option_QuickChatSettings`, `$EpicPose_Header`, `$ContextPopout_Header_SocialMenu`, `$ContextPopout_Header_RecentPlayers`, `$ContextPopout_Header_Nearby`, `$Clubs_Header`, `$FriendsList_WindowName`, `$ContextPopout_Header_Emotes`, `$ChatMenu_ViewProfile`, `$ChatMenu_Whisper`, `$FriendRequest_JoinMe`, `$ChatMenu_ClubInvite`, `$ChatMenu_Ignore`, `$ChatMenu_ReportSpam`, `$ChatMenu_AddFriend`, `$Chat_To_Prefix`

**Translate keys (emotes):** `$PlayerEmotes_Sit/Pose/Wave/Sleep/Dance/Laugh/Shrug/Cry/Bow`, `$MountEmotes_Prance`, `$MountEmotes_Stomp`, `$MountEmotes_Spit`

**IggyTween:** `ChatLogView` uses `IggyTween` for a 15-second idle fade-out (alpha 1→0), reversed on new messages or input activation. `ContextListView` uses `IggyTween` for pulsing highlight alpha on the selected menu item.

**Tab focus:** `disableTabFocus` recursively walks the scrollbar's display tree to prevent keyboard-tab stealing.

---

## Other game-specific classes

- `ChatLog` (extends `EventDispatcher`) — Named message buffer capped at 125 messages (`MaxMessages`). Dispatches `DataChangeEvent` (ADD/REMOVE/REMOVE_ALL) to listeners. `insert` pushes a `ChatMessage` and enforces the cap by shifting from the front.
- `ChatMessage` — Plain data object: `author`, `channel`, `content`, `timeStamp` (12-hour H:MM computed at construction), `authorColor`, `messageColor`, `showAuthor`, `wasSent`, `lockboxBroadcast`, `displayChatTimeStamp`.
- `ChatLogView` (extends `KiwiComponent`) — [Embed symbol90] Virtualised message list. Maintains `_renderers` (laid-out `ChatItemRenderer`s) and `_pendingRenderers` (not yet positioned). `draw()` positions pending renderers then recalculates scroll. 15-second `Timer` → `IggyTween` fades the log out; resets on new message or input open. Calls `ExternalInterface.call("OnChatFadeComplete")` when fade ends.
- `ChatItemRenderer` (extends `KiwiComponent`) — [Embed symbol63] Renders one `ChatMessage` as HTML text in `msgTextField`. `formatMessage` builds `[channel][timestamp][author]: content` with `<font color>` tags. Right-click (or regular click outside Iggy) on the text field calls `ExternalInterface.call("OnRequestContextMenu", x, y, author, content, lockboxBroadcast)`. Alternating rows get a visible `BGPixel` background via `addBG()`.
- `ChatInput` (extends `UIComponent`) — [Embed symbol85] Text input bar. Keyboard handler: Enter → `OnExecute`, Space → `OnAutocomplete`, Tab → `OnCycleWhisperTarget`/`OnTabAutocomplete`, Up/Down → `OnCycleInputHistory`. Exposes `setInput`/`getInput`/`setDefaultChannel` via Iggy. On console shows `ChannelList`; on PC shows `defaultChannelTextField` label.
- `ChannelList` (extends `UIComponent`) — [Embed symbol73] Console channel selector dropdown. Hides the first `NumHiddenChannels` (3) channels from the selectable list. Expand/collapse/highlight/move/select all driven by Iggy callbacks. `selectChannel` sends `/channelname` via `OnAutocomplete` on next frame.
- `ChannelSelectOption` — Timeline symbol for a single selectable channel row in the dropdown (no logic file, referenced in `ChannelList`).
- `ContextPopOut` (extends `UIComponent`) — [Embed symbol53] A pop-out panel showing a header and a `ContextListView`. Menu type constants: `SOCIAL_MENU=0`, `RECENT_PLAYERS=1`, `NEARBY=2`, `CLUBS=3`, `FRIENDS=4`, `EMOTES=5`, `QUICK_CHAT_SETTINGS=6`, `EPIC_POSE=7`, `PLAYER_INTERACT=8`, `CLUB_MEMBERS=9`. `configurePopout` builds the item list based on type; PLAYER_INTERACT menu differs for console platforms (Durango/Orbis) by adding `$ChatMenu_ViewProfile`. Auto-widens background/legend/header when item text overflows default width.
- `ContextListView` (extends `_kiwi.Controls.ScrollableView`) — [Embed symbol43] Scrollable list of `ContextListItem` MovieClips. Manages single-selection highlight with pulsing `IggyTween` (alpha oscillates 0.4↔1.0 over 0.9s). `disableOption` greys out an item. `setMenuActive` shows/hides the A-button confirm hint.
- `ContextListItem` — MovieClip item row used by `ContextListView` and `ContextPopOut`; referenced as `new ContextListItem()`.

**Asset-wrapper skin classes (12 total):** `ScrollArrowDown_*Skin` (4), `ScrollArrowUp_*Skin` (4), `ScrollThumb_*Skin` (3), `ScrollTrack_skin`, `TextInput_upSkin`, `TextInput_disabledSkin`, `focusRectSkin`, `btn_console_south`, `btn_console_north`.

**Embedded PNG assets (2):** `btn_XBOne_A/png`, `btn_XBOne_Y/png`.

**Chat_fla timeline symbols (2):** `ChannelSelectOptionBackground_12`, `ChatRadial_14`.

---

## Notable logic

- **Context-list stack:** `openContextLists` is a push-down stack. Each social sub-menu push calls `setMenuActive(false)` on the previous top. `previousContextList` pops, re-activates the new top, or calls `OnCloseSocialMenu` and restores the radial if the stack empties.
- **Fade-out lifecycle:** `ChatLogView._timeout` (15 s, single-shot `Timer`) fires `_fadeOut.start()`. Any new message or `inputToggled(true)` calls `resetFadeOut(true)` which resets the timer; `resetFadeOut(false)` stops the fade without restarting the timer (used when input opens).
- **Emote list per mount:** `setUpEmotesList("")` gives foot-emotes (9 options); `"pegasus"` → prance; `"pegasus_shadow"` → stomp; `"llama_basic"` → spit; anything else → empty list.
- **Message cap:** `ChatLog.enforceMessageLimit` shifts from the front and fires a REMOVE event per dropped message, allowing `ChatLogView.onMessageRemoved` to clean up renderers.
- **Odd-row striping:** `currentMessageIndex % 2 != 0` triggers `ChatItemRenderer.addBG()`, which stretches `BGPixel` to the full renderer width/height for alternating row shading.
