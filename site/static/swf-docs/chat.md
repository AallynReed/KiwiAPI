# chat.swf
> The in-game chat window for Trove. It displays a scrollable message log (the tab machinery exists, but only tab 0 is ever used) and a text input bar; on console it adds a channel selector, a radial menu for quick chat options, and a social context pop-out system (recent players, nearby, clubs, friends, emotes, player-interact menus).

**Document/main class:** `Chat` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 11 (excluding skin/asset wrappers and framework files)

---

## Main class: `Chat`

`Chat` is the root document class. The constructor adds two frame scripts (frames 1 and 11), creates a default "All Messages" tab, selects it, and hides the radial. `configUI()` disables tab-focus on the scrollbar, links it to `logView`, and registers all Iggy `ExternalInterface` callbacks. Outside Iggy a few test messages are injected and the input is activated immediately.

The window has two height states: **expanded** (370px, y=0, shown when input is active) and **collapsed** (218px, y=180, shown when input is inactive). `onSetActive(bool)` drives the transition. The stage is 440 × 400, and the engine anchors it to the bottom-left of the screen.

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

- `tabLogs : Array` — Array of `ChatLog` instances, one per tab. Only index 0 (the "All Messages" log) is ever used: the engine sends every message to tab 0, `addTab`/`removeTab`/`renameTab` are registered but never called (none of the three names is in the client executable), and `selectTab` is private and never registered.
- `logView : ChatLogView` — The scrollable message display area.
- `chatInput : ChatInput` — The text input component with channel selector.
- `scrollbar : UIScrollBar` — Scrollbar linked to `logView`.
- `radial : MovieClip` — Console radial menu (visible only when input is active on console).
- `buttonLegendRadial : MovieClip` — Button legend shown alongside the radial.
- `openContextLists : Array` — Stack of currently open `ContextPopOut` instances.
- `emotes : Array` — String list of emote command names (e.g. `"sit"`, `"dance"`), populated by `setUpEmotesList`.
- `mounted : Boolean` — Whether the player is on a mount; controls which emotes are available.

### Frame scripts / timeline

- **frame 1** (`frame1`, label `PC`) — `stop()`.
- **frame 11** (`frame11`, label `Console`) — `stop()`.

### Runtime dependencies & integration

**ExternalInterface callbacks registered:**
`Chat`: `onSetActive`, `addTab`, `removeTab`, `renameTab`, `addMessage`, `onAdjustScrollPos`, `setDisplayTimeStamp`, `goToSector`, `setSectorText`, `openSocialMenu`, `closeSocialMenu`, `previousContextList`, `moveContextListHighlight`, `activateContextListSelection`, `addItemToContextMenu`, `setUpEmotesList`, `disableSocialMenuEmotesOption`
`ChatInput`: `setDefaultChannel`, `setInput`, `getInput`
`ChannelList` (console channel selector): `expandChannelSelect`, `collapseChannelSelect`, `highlightChannelSelection`, `unhighlightChannelSelection`, `moveChannelHighlight`, `addSelectableChannel`, `clearSelectableChannels`, `selectChannel`

**ExternalInterface calls (Flash → engine):**
`OnExecute` (Enter, `/epicpose`, emotes), `OnAutocomplete`, `OnCycleWhisperTarget`, `OnTabAutocomplete`, `OnCycleInputHistory`, `OnRequestContextMenu`, `OnChatFadeComplete`, `OnCloseSocialMenu`, `OnOpenQuickChatSettings`, `OnPlayerInteractOptionSelected`, `CheckEmotesCount`, `OnSetUpRecentChatList`, `OnSetUpNearbyPlayersList`, `OnSetUpFriendsList`, `OnSetUpClubsList`, `OnSetUpClubMembersList`, `OnSetUpEmotesList`

**What the engine sends:**
- `addMessage(tab, channel, author, content, messageColor, authorColor, wasSent, showAuthor = true, lockboxBroadcast = false)` — the two colours arrive in the opposite order to `ChatMessage`'s constructor arguments. `tab` is always 0. `channel` is a display label the engine has already translated (the renderer prints it without `translate`); it is empty on incoming whispers, NPC speech and system or command replies. An incoming whisper carries the sender as `author`, `wasSent` false and `messageColor` `0xDC70F0`; `wasSent` is true only on the player's own outgoing whisper (a channel line the player sends comes back with `wasSent` false). Replies to commands such as `/who`, `/chatlist` and `/mastery` arrive with no channel and no author, in `0xFFFF00` with author colour `0xC4C400`.
- `authorColor` is never absent: a speaker with a chosen name colour gets one of ten fixed colours, anyone else gets the message colour dimmed per channel (× 0.77, truncated; × 0.65 on a message the player sent).
- `content` typed by players arrives HTML-escaped (`<3` arrives as `&lt;3`), while game-composed lines such as loot broadcasts carry their own `<font color>` markup; both are written straight into `htmlText`.
- When the screen loads, the engine replays the session's chat into it through `addMessage`.
- `setInput(text)` is called from the engine's input thread, not on the frame tick. The engine answers `OnTabAutocomplete`/`OnCycleWhisperTarget` through it a frame or two later, with the line it believes is being typed.
- `setDefaultChannel(name, colour)` is sent at login and after every channel switch. The colour comes from the channel list the server sends (the client has no channel colour table of its own); on PC it colours the `name:` prefix.
- `setDisplayTimeStamp(on)` carries the game's chat timestamp option.
- The engine opens chat (`onSetActive(true)`) only from its own Enter and `/` key handlers and from the friends list's whisper action; nothing this SWF calls can open it.

**What the outbound calls do:**
- `OnExecute(text)` runs the line, and the engine closes chat after every line, including an empty one. A line sent while chat is closed is dropped without any reply.
- `OnAutocomplete(text)` acts only on a line starting with `/`, and only on `/r`, `/reply`, `/s`, `/say` or a channel the engine knows by its command (`/2`, `/c1`): `/r` and `/reply` are expanded to a `/whisper` line, and a channel command becomes the speaking channel. Nothing is sent and chat stays open (it also works while chat is closed); each channel switch is answered with `setDefaultChannel`.
- `OnRequestContextMenu(x, y, author, content, lockboxBroadcast)` opens a menu the engine draws itself; its rows never pass through this SWF (`addItemToContextMenu` only fills the gamepad `ContextPopOut`). `x, y` is the bottom-left of the message text in stage coordinates.
- `OnPlayerInteractOptionSelected(index, name)` sends the index into the `PLAYER_INTERACT` list `ContextPopOut` built (Windows: 0 whisper, 1 add friend, 2 join me, 3 ignore, 4 report spam) and the pop-out's header, the player's name.

**Console-only paths:** the engine feeds the channel list (`clearSelectableChannels`, one `addSelectableChannel` per channel, then `expandChannelSelect`) only from its controller command handler, so on a keyboard client the list stays empty; the radial is only shown when `IsConsole()`; and `CheckEmotesCount`, the six `OnSetUp*List` calls, `OnCloseSocialMenu`, `OnOpenQuickChatSettings` and `OnPlayerInteractOptionSelected` are only reachable from the gamepad social menu.

**Translate keys (in `ContextPopOut.configurePopout`):** `$ContextPopout_Option_RecentPlayers`, `$ContextPopout_Option_Nearby`, `$MainMenu_Clubs`, `$MainMenu_FriendsList`, `$ContextPopout_Option_Emotes`, `$ContextPopout_Option_QuickChatSettings`, `$EpicPose_Header`, `$ContextPopout_Header_SocialMenu`, `$ContextPopout_Header_RecentPlayers`, `$ContextPopout_Header_Nearby`, `$Clubs_Header`, `$FriendsList_WindowName`, `$ContextPopout_Header_Emotes`, `$ChatMenu_ViewProfile`, `$ChatMenu_Whisper`, `$FriendRequest_JoinMe`, `$ChatMenu_ClubInvite`, `$ChatMenu_Ignore`, `$ChatMenu_ReportSpam`, `$ChatMenu_AddFriend`, `$Chat_To_Prefix`

**Translate keys (emotes):** `$PlayerEmotes_Sit/Pose/Wave/Sleep/Dance/Laugh/Shrug/Cry/Bow`, `$MountEmotes_Prance`, `$MountEmotes_Stomp`, `$MountEmotes_Spit`

**IggyTween:** `ChatLogView` uses `IggyTween` for a 15-second idle fade-out (alpha 1→0), reversed on new messages or input activation. `ContextListView` uses `IggyTween` for pulsing highlight alpha on the selected menu item.

**Tab focus:** `disableTabFocus` recursively walks the scrollbar's display tree to prevent keyboard-tab stealing.

---

## Other game-specific classes

- `ChatLog` (extends `EventDispatcher`) — Named message buffer capped at 125 messages (`MaxMessages`). Dispatches `DataChangeEvent` (ADD/REMOVE/REMOVE_ALL) to listeners. `insert` pushes a `ChatMessage` and enforces the cap by shifting from the front.
- `ChatMessage` — Plain data object: `author`, `channel`, `content`, `timeStamp` (local clock at construction as `H:MM`; hours above 12 have 12 subtracted, so there is no AM/PM and the midnight hour reads `0`), `authorColor`, `messageColor`, `showAuthor`, `wasSent`, `lockboxBroadcast`, `displayChatTimeStamp`.
- `ChatLogView` (extends `KiwiComponent`) — [Embed symbol90] Message list with one `ChatItemRenderer` per held message (no virtualisation; the 125-message cap bounds it). Maintains `_renderers` (laid-out `ChatItemRenderer`s) and `_pendingRenderers` (not yet positioned). `draw()` positions pending renderers then recalculates scroll. 15-second `Timer` → `IggyTween` fades the log out; resets on new message or input open. Calls `ExternalInterface.call("OnChatFadeComplete")` when fade ends.
- `ChatItemRenderer` (extends `KiwiComponent`) — [Embed symbol63] Renders one `ChatMessage` as HTML text in `msgTextField`. `formatMessage` builds `[channel][timestamp][author]: content` with `<font color>` tags: the channel and timestamp only when `channel` is non-empty, the author only when `showAuthor` is set, prefixed with `$Chat_To_Prefix` ("To:") on a `wasSent` line and written without brackets on a `lockboxBroadcast`. Right-click (or regular click outside Iggy) on the text field calls `ExternalInterface.call("OnRequestContextMenu", x, y, author, content, lockboxBroadcast)`, but only for a line with an author that is not `wasSent`, and only while the log is fully opaque. Alternating rows get a visible `BGPixel` background via `addBG()`.
- `ChatInput` (extends `UIComponent`) — [Embed symbol85] Text input bar. Keyboard handler (ignored while an IME composition is active): Enter → `OnExecute(text)`, Space → `OnAutocomplete(text)`, Tab → `OnCycleWhisperTarget(text)` then `OnTabAutocomplete(text)`, re-taking focus and parking the caret at the end in between if focus has left the box (Tab is also Flash's focus-traversal key), Up/Down → `OnCycleInputHistory(-1/1)`. Exposes `setInput`/`getInput`/`setDefaultChannel` via Iggy; `setInput` ignores text over 256 characters, and `onSetActive` empties the box on every call. On console shows `ChannelList`; on PC shows `defaultChannelTextField` label.
- `ChannelList` (extends `UIComponent`) — [Embed symbol73] Console channel selector dropdown. Hides the first `NumHiddenChannels` (3) channels from the selectable list. Expand/collapse/highlight/move/select all driven by Iggy callbacks. `selectChannel` sends `"/" + name.split(".")[0].toLowerCase()` via `OnAutocomplete` on the next frame; channel names carry their command before the dot (`2. World` → `/2`, a club's `c1. <name>` → `/c1`), so this switches channel without sending a line or closing chat.
- `ChannelSelectOption` — [Embed symbol71] MovieClip row for one selectable channel in the dropdown: `background` (frames `selected`/`unselected`), `channelText`, `btnA`, `btnY`; frames 1 and 11 `stop()`.
- `ContextPopOut` (extends `UIComponent`) — [Embed symbol53] A pop-out panel showing a header and a `ContextListView`. Menu type constants: `SOCIAL_MENU=0`, `RECENT_PLAYERS=1`, `NEARBY=2`, `CLUBS=3`, `FRIENDS=4`, `EMOTES=5`, `QUICK_CHAT_SETTINGS=6`, `EPIC_POSE=7`, `PLAYER_INTERACT=8`, `CLUB_MEMBERS=9`. `configurePopout` builds the item list based on type. The PLAYER_INTERACT list differs by platform: on Windows it is `$ChatMenu_Whisper`, `$ChatMenu_AddFriend`, `$FriendRequest_JoinMe`, `$ChatMenu_Ignore`, `$ChatMenu_ReportSpam`; on Durango/Orbis (when not running on Windows) it is `$ChatMenu_ViewProfile`, `$ChatMenu_Whisper`, `$FriendRequest_JoinMe`, `$ChatMenu_ClubInvite`, `$ChatMenu_Ignore`, `$ChatMenu_ReportSpam`. The selected row's index into this list is what `OnPlayerInteractOptionSelected` sends. Auto-widens background/legend/header when item text overflows default width.
- `ContextListView` (extends `_kiwi.Controls.ScrollableView`) — [Embed symbol43] Scrollable list of `ContextListItem` MovieClips. Manages single-selection highlight with pulsing `IggyTween` (alpha oscillates 0.4↔1.0 over 0.9s). `disableOption` greys out an item. `setMenuActive` shows/hides the A-button confirm hint.
- `ContextListItem` — [Embed symbol58] MovieClip item row (`itemText`, `highlight`, `btnA`) used by `ContextListView` and `ContextPopOut`; referenced as `new ContextListItem()`.

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
