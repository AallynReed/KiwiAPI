# navigationmenu.swf
> The persistent navigation tray and pop-up grid menu that lets players open any major game window (Store, Character, Inventory, Classes, Clubs, Friends, etc.). The tray is always visible in-game; the grid menu expands on demand and supports both mouse/keyboard and console D-pad navigation.

**Document/main class:** `NavigationMenu` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 33 (+ 4 `NavigationMenu_fla` timeline symbols, excluding framework)

## Main class: `NavigationMenu`

`NavigationMenu` owns the persistent bottom tray (a handful of `BaseButton` instances) and a hidden `navMenu` MovieClip that contains the full button grid. On construction it sets `configUI_waitForTargetFrame = true`, hides `navMenu`, and registers frame scripts at frames 1, 11, and 22 (all `stop()`).

`configUI()` iterates the `menuItems` array (22 named sections) and for each wires up the corresponding `<name>Button` `BaseButton` inside `navMenu`, clearing its text and storing the name as `.data`. It also sets all `<name>Highlight` MovieClip alphas to 0. The six tray buttons are stored in `trayButtons[]` and wired to `onTrayOption`. On console, `DirectionalMapping` children are added to each button to encode the 2-D navigation graph (varies between NX and non-NX). After setup, `ExternalInterface.call("OnConfigured")` signals the game engine. `setupTranslation()` is called to set localised text; when an entry's `<name>TextField` label wraps to two lines, that button's hotkey `textField` is moved down 17 px.

Highlight animation uses a looping `IggyTween` pair (fade-out then fade-in, 1 second each, alpha between 0.4 and 1.0) on the `<name>Highlight` MovieClip. Text colour changes to `0xFFFE02` (yellow) when highlighted and `0xFFFFFF` when not.

### Public methods
- `showMenu(visible:Boolean) : void` — shows/hides `navMenu`. On hide, unhighlights the current selection.
- `setMenuHotkey(name:String, label:String) : void` — writes the hotkey text into `<name>Button`'s own `textField`, which `configUI()` cleared (`name` is the `menuItems` id); on console uses `htmlText`, on PC uses `text`. Display only: the key bindings themselves are handled by the engine, not by this SWF.
- `setTrayHotkey(index:int, label:String) : void` — sets the hotkey label on `trayButtons[index]`; also syncs `storeDealsIcon` when updating the store button. On Orbis (PS4) every call also moves the label up by `ORBIS_OPTIONS_BUTTON_OFFSET` (15 px).
- `setButtonEnabled(name:String, enabled:Boolean) : void` — sets `<name>Button.enabled`, hides/shows that button's hotkey `textField`, and colours the `<name>TextField` label grey (0x666666) or white.
- `showTrayButton(index:int, visible:Boolean) : void` — shows/hides `trayButtons[index]`; only when its visibility actually changes, adjusts x-position of all subsequent tray buttons by ±65px to close/open gaps.
- `playDailyLoginAnim(show:Boolean) : void` — shows/hides the menu's Daily Login entry (`dailyLoginButton` and `dailyLoginTextfield` together, so the entry disappears rather than greying out) and starts/resets `dailyLoginAnimTimer`. It does not touch the `dailyRewardsIcon` tray button.
- `loopDailyLoginAnim(event:TimerEvent=null) : void` — plays `dailyRewardsIcon` from frame 1 if stopped; restarts timer.
- `playClaimsAnim(show:Boolean) : void` — starts/resets the claims animation timer.
- `loopClaimsAnim(event:TimerEvent=null) : void` — plays `claimsIcon` if it is resting on frame 1 and starts `claimAnimTimer` if it is not running. It is also an engine callback, so a direct call starts the loop too; only `playClaimsAnim(false)` stops it.
- `playStoreDealAnim(show:Boolean) : void` — shows/hides `storeDealsIcon`.

### Key fields
- `navMenu : MovieClip` — the expandable grid panel containing all named `<name>Button`, `<name>Highlight`, `<name>TextField` children. The label names are not consistent in case: on the `PC` frame they are `<name>TextField` except `dailyLoginTextfield`, `atlasTextfield` and `tinyQuestTextfield`; on the console frames they are `<name>Textfield` except `activitiesTextField` and `bomberRoyaleTextField`. The highlight colour, `setButtonEnabled` greying and the two-line offset look labels up as `<name>TextField`, so they only reach the labels spelled that way; `playDailyLoginAnim` and the NX block use the `Textfield` spelling.
- `navMenuButton : BaseButton` — tray button 0 (opens the grid menu).
- `navStoreButton : BaseButton` — tray button 1 (quick store access).
- `atlasIcon : AtlasMC` — tray button 2 (Atlas world map icon).
- `bomberRoyaleIcon : BaseButton` — tray button 3. Since the October 2026 build its art (`bomberRoyaleMC`) and the menu's `bomberRoyaleBtn` show crossed swords instead of a bomb.
- `claimsIcon : BaseButton` — tray button 4, animated.
- `dailyRewardsIcon : BaseButton` — tray button 5, animated.
- `storeDealsIcon : MovieClip` — store deal badge, shares hotkey text with `navStoreButton`.
- `currentSelection : MovieClip` — the button currently highlighted in the grid (console nav).
- `trayButtons : Array` — ordered list of the six tray `BaseButton`s.
- `menuItems : Array` — 22-element String array: `["store","marketplace","character","inventory","classChanger","achievement","leaderboard","collections","activities","clubs","friendList","likedWorlds","cornerstone","map","welcome","claims","dailyLogin","howtoplay","settings","bomberRoyale","atlas","tinyQuest"]`.
- `claimAnimTimer : Timer` — 5 s repeating timer for claims icon.
- `dailyLoginAnimTimer : Timer` — 5 s repeating timer for daily login icon. `configUI()` adds its `TIMER` listener (`loopDailyLoginAnim`) to `dailyRewardsIcon` instead of to the timer, and `loopDailyLoginAnim` is not an engine callback, so the timer's ticks never reach it.
- `fadeInTween / fadeOutTween : IggyTween` — looping alpha tweens on the active highlight.
- `TWEEN_ALPHA_MAX / MIN / TIME` — 1.0, 0.4, 1 s.

### Frame scripts / timeline
- Frames 1, 11 and 22 are labelled `PC`, `Console` and `ConsoleLoc`; each frame script is just `stop()`. `UIComponent` leaves PC on frame 1 and sends consoles (`IsConsole()`) to `Console`, or to `ConsoleLoc` when the locale is neither English nor Chinese. NX differences are handled in code (`IsNX()`), not by a frame.
- `atlasIcon`, `bomberRoyaleIcon` and `storeDealsIcon` are placed on the `PC` frame only; `Console` removes them and repositions the store, claims and daily-rewards tray buttons.

### Runtime dependencies & integration
**ExternalInterface callbacks registered (Iggy):**
- `showMenu`, `setMenuHotkey`, `setTrayHotkey`, `playClaimsAnim`, `loopClaimsAnim`, `playDailyLoginAnim`, `playStoreDealAnim`, `moveSelection`, `highlightButton`, `activateButton`, `showTrayButton`, `setButtonEnabled`

**ExternalInterface calls fired:**
- `OnConfigured` — fired from `configUI()` once the buttons, callbacks and D-pad graph are wired, before `setupTranslation()` runs.
- `OnMenuOptionSelected(name:String)` — fired when a grid menu button is clicked or `activateButton()` is called. Sends the entry's `menuItems` id (the button's `.data`, or for `activateButton()` the selected instance name minus `Button`), and the engine opens that window.
- `OnTrayOptionSelected(index:int)` — fired when a tray button is clicked. Sends its `trayButtons` index (0 menu, 1 store, 2 atlas, 3 bomber royale, 4 claims, 5 daily rewards), the same index `setTrayHotkey` and `showTrayButton` take.

**Console D-pad navigation:**
`moveSelection(dx, dy)` reads the `DirectionalMapping` child of `currentSelection` to find the adjacent button (`dx` 1/-1 = east/west, checked first; `dy` 1/-1 = south/north), skips an invisible neighbour by continuing in the same direction from it, then calls `unhighlightButton` + `highlightButton`. `DirectionalMapping(north, east, south, west)` graph wiring differs between NX (no achievement button, leaderboard takes its position) and other consoles.

On both branches the south chain runs `claimsButton` → `tinyQuestButton` → `dailyLoginButton`. `tinyQuestButton` maps north→`claimsButton`, east→`howtoplayButton`, south→`dailyLoginButton`, west→`leaderboardButton`; `dailyLoginButton` maps north→`tinyQuestButton`, east→`howtoplayButton`, south→`null`, west→`leaderboardButton`. On non-NX consoles `leaderboardButton`'s east edge points at `tinyQuestButton`. Before the October 2026 build the two were the other way round (`claimsButton` → `dailyLoginButton` → `tinyQuestButton`).

**NX-specific:** `achievementButton` and its text field are hidden; `leaderboardButton` is moved to take its position.

**IggyTween:** Used for a continuous alpha pulse (0.4 ↔ 1.0, 1 s per leg) on the `<name>Highlight` overlay of the currently highlighted menu button.

---

## Other game-specific classes

### `navMenu` (extends `_kiwi.Core.UIComponent`) — [Embed symbol324]
The expandable menu grid symbol. Dynamically accessed via dot-notation from `NavigationMenu` (`navMenu.storeButton`, `navMenu.storeHighlight`, `navMenu.storeTextField`, etc.). Three timeline frames (1, 11, 21 — all stop), labelled `PC`, `Console` and `ConsoleLoc` and chosen the same way as the root's. `PC` places 19 of the 22 `<name>Button`s: `achievementButton`, `howtoplayButton` and `settingsButton` exist only on the console frames, as do all 22 `<name>Highlight` clips and the button legend. Contains frame-based `__setProp` handler to configure `bomberRoyaleButton` at different frame ranges. All button instances are `BaseButton` with tooltip properties.

Entry labels and the unnamed section headers are translation keys left in the text fields' initial text. The headers are `$Character`, `$Progression`, `$NavHeader_Activities`, `$Shopping`, `$Social` and `$World` (already upper-case in the language file), plus `$System` on the console frames. The `activities` entry reads `$NavMenuActivities` (English "Adventures") and the `bomberRoyale` entry reads `$MainMenu_PVP` (English "PVP") on every frame.

### `NavigationMenu_fla/` timeline symbols (4 classes)
- `ButtonLegend_31` — console button legend (south glyph + `$Select_ButtonLegend`, east glyph + `$Close_ButtonLegend`), placed in `navMenu` at its `Console` frame; Embed symbol320.
- `ButtonLegendConsole_32` — the same legend (same glyphs and keys) that replaces `ButtonLegend_31` at `navMenu`'s `ConsoleLoc` frame; Embed symbol323.
- `texttween_27` — the `<name>Highlight` overlay; every highlight instance is this symbol. It has `Static` and `Pulse` frame labels but stays stopped on frame 1, since the pulse comes from `IggyTween`; Embed symbol285.
- `navStoreButtonDeal_40` — store-deal tray button variant; Embed symbol354; has `textField:TextField` and `storeDealMC:MovieClip`; two frames (1 and 11), both stop.

### Individual tray/button wrapper classes (31 classes)
`AtlasBtn`, `AtlasMC`, `bomberRoyaleBtn`, `bomberRoyaleMC`, `claimsBtn`, `claimsMC`, `dailyLoginBtn`, `dailyRewardsMC`, `storeNavBtn`, `menuNavBtn`, `buttonLabels`, `characterBtn`, `adventureBtn`, `achievementBtn`, `classBtn`, `clubsBtn`, `collectionBtn`, `cornerstoneBtn`, `creditsBtn`, `friendsBtn`, `howtoplayBtn`, `inventoryBtn`, `leaderboardBtn`, `likedworldsBtn`, `mapBtn`, `marketplaceButn`, `patronBtn`, `settingsBtn`, `storeBtn`, `tinyQuestBtn`, `welcomeBtn` — individual button/MC symbol wrappers for each nav entry; these are the named child instances inside `navMenu`; no independent logic beyond their symbol frames.

---

## Notable logic
- **22-entry menu array:** The `menuItems` array drives all button discovery; adding a new menu section only requires adding its name string and placing matching `<name>Button` / `<name>Highlight` / `<name>TextField` instances in the `navMenu` symbol. `tinyQuest` is the most recent addition — its `tinyQuestBtn` symbol had already shipped in the library before the entry was wired up.
- **Tray layout shift:** `showTrayButton` shifts the x-position of all tray buttons to the right of the toggled one by ±65 px, keeping them packed without gaps.
- **Store deals badge:** `storeDealsIcon` is a separate MovieClip that replaces `navStoreButton` visually when a deal is active; `setTrayHotkey` syncs both with the same label.
- **NX achievement suppression:** The achievement button and its text field are hidden and the leaderboard button is moved into its grid position so layout remains consistent.
- **Highlight pulse loop:** Two `IggyTween` instances chain via `motionFinishCallback` so the highlight fades out then back in indefinitely while a button is selected.
