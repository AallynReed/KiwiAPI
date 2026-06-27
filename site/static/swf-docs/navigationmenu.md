# navigationmenu.swf
> The persistent navigation tray and expandable full-screen menu that lets players open any major game window (Store, Character, Inventory, Classes, Clubs, Friends, etc.). The tray is always visible in-game; the grid menu expands on demand and supports both mouse/keyboard and console D-pad navigation.

**Document/main class:** `NavigationMenu` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 30 (excluding framework)

## Main class: `NavigationMenu`

`NavigationMenu` owns the persistent bottom tray (a handful of `BaseButton` instances) and a hidden `navMenu` MovieClip that contains the full button grid. On construction it sets `configUI_waitForTargetFrame = true`, hides `navMenu`, and registers frame scripts at frames 1, 11, and 22 (all `stop()`).

`configUI()` iterates the `menuItems` array (21 named sections) and for each wires up the corresponding `<name>Button` `BaseButton` inside `navMenu`, clearing its text and storing the name as `.data`. It also sets all `<name>Highlight` MovieClip alphas to 0. The six tray buttons are stored in `trayButtons[]` and wired to `onTrayOption`. On console, `DirectionalMapping` children are added to each button to encode the 2-D navigation graph (varies between NX and non-NX). After setup, `ExternalInterface.call("OnConfigured")` signals the game engine. `setupTranslation()` is called to set localised text; buttons whose text wraps to two lines get a `+17` y-offset applied.

Highlight animation uses a looping `IggyTween` pair (fade-out then fade-in, 1 second each, alpha between 0.4 and 1.0) on the `<name>Highlight` MovieClip. Text colour changes to `0xFFFE02` (yellow) when highlighted and `0xFFFFFF` when not.

### Public methods
- `showMenu(visible:Boolean) : void` — shows/hides `navMenu`. On hide, unhighlights the current selection.
- `setMenuHotkey(name:String, label:String) : void` — sets hotkey label text on a named menu button; on console uses `htmlText`, on PC uses `text`. On Orbis (PS4), shifts label y by `ORBIS_OPTIONS_BUTTON_OFFSET` (15px).
- `setTrayHotkey(index:int, label:String) : void` — sets hotkey label on tray button by index; also syncs `storeDealsIcon` when updating the store button.
- `setButtonEnabled(name:String, enabled:Boolean) : void` — enables/disables a named menu button and its text field; disabled text is coloured grey (0x666666).
- `showTrayButton(index:int, visible:Boolean) : void` — shows/hides a tray button; adjusts x-position of all subsequent tray buttons by ±65px to close/open gaps.
- `playDailyLoginAnim(show:Boolean) : void` — shows/hides daily login button and starts/stops its looping animation timer.
- `loopDailyLoginAnim(event:TimerEvent=null) : void` — plays `dailyRewardsIcon` from frame 1 if stopped; restarts timer.
- `playClaimsAnim(show:Boolean) : void` — starts/resets the claims animation timer.
- `loopClaimsAnim(event:TimerEvent=null) : void` — plays `claimsIcon` from frame 1; restarts timer.
- `playStoreDealAnim(show:Boolean) : void` — shows/hides `storeDealsIcon`.

### Key fields
- `navMenu : MovieClip` — the expandable grid panel containing all named `<name>Button`, `<name>Highlight`, `<name>TextField` children.
- `navMenuButton : BaseButton` — tray button 0 (opens the grid menu).
- `navStoreButton : BaseButton` — tray button 1 (quick store access).
- `atlasIcon : AtlasMC` — tray button 2 (Atlas world map icon).
- `bomberRoyaleIcon : BaseButton` — tray button 3.
- `claimsIcon : BaseButton` — tray button 4, animated.
- `dailyRewardsIcon : BaseButton` — tray button 5, animated.
- `storeDealsIcon : MovieClip` — store deal badge, shares hotkey text with `navStoreButton`.
- `currentSelection : MovieClip` — the button currently highlighted in the grid (console nav).
- `trayButtons : Array` — ordered list of the six tray `BaseButton`s.
- `menuItems : Array` — 21-element String array: `["store","marketplace","character","inventory","classChanger","achievement","leaderboard","collections","activities","clubs","friendList","likedWorlds","cornerstone","map","welcome","claims","dailyLogin","howtoplay","settings","bomberRoyale","atlas"]`.
- `claimAnimTimer : Timer` — 5 s repeating timer for claims icon.
- `dailyLoginAnimTimer : Timer` — 5 s repeating timer for daily login icon.
- `fadeInTween / fadeOutTween : IggyTween` — looping alpha tweens on the active highlight.
- `TWEEN_ALPHA_MAX / MIN / TIME` — 1.0, 0.4, 1 s.

### Frame scripts / timeline
- Frame 1, 11, 22 — all `stop()`. The three states correspond to different console/platform UI layouts (PC, console non-NX, NX implied).

### Runtime dependencies & integration
**ExternalInterface callbacks registered (Iggy):**
- `showMenu`, `setMenuHotkey`, `setTrayHotkey`, `playClaimsAnim`, `loopClaimsAnim`, `playDailyLoginAnim`, `playStoreDealAnim`, `moveSelection`, `highlightButton`, `activateButton`, `showTrayButton`, `setButtonEnabled`

**ExternalInterface calls fired:**
- `OnConfigured` — fired at end of `configUI()`.
- `OnMenuOptionSelected(name:String)` — fired when a grid menu button is clicked or `activateButton()` is called.
- `OnTrayOptionSelected(index:int)` — fired when a tray button is clicked.

**Console D-pad navigation:**
`moveSelection(dx, dy)` reads the `DirectionalMapping` child of `currentSelection` to find the adjacent button, then calls `unhighlightButton` + `highlightButton`. `DirectionalMapping` graph wiring differs between NX (no achievement button, leaderboard takes its position) and other consoles.

**NX-specific:** `achievementButton` and its text field are hidden; `leaderboardButton` is moved to take its position.

**IggyTween:** Used for a continuous alpha pulse (0.4 ↔ 1.0, 1 s per leg) on the `<name>Highlight` overlay of the currently highlighted menu button.

---

## Other game-specific classes

### `navMenu` (extends `_kiwi.Core.UIComponent`) — [Embed symbol321]
The expandable menu grid symbol. Dynamically accessed via dot-notation from `NavigationMenu` (`navMenu.storeButton`, `navMenu.storeHighlight`, `navMenu.storeTextField`, etc.). Three timeline frames (1, 11, 21 — all stop). Contains frame-based `__setProp` handler to configure `bomberRoyaleButton` at different frame ranges. All button instances are `BaseButton` with tooltip properties.

### `NavigationMenu_fla/` timeline symbols (4 classes)
- `ButtonLegend_30` — PC button legend symbol (frame 30).
- `ButtonLegendConsole_31` — console button legend symbol (frame 31).
- `texttween_26` — text tween animation clip (frame 26).
- `navStoreButtonDeal_39` — store-deal tray button variant (frame 39); has `textField:TextField` and `storeDealMC:MovieClip`; two frames (1 and 11), both stop.

### Individual tray/button wrapper classes (20 classes)
`AtlasBtn`, `AtlasMC`, `bomberRoyaleBtn`, `bomberRoyaleMC`, `claimsBtn`, `claimsMC`, `dailyLoginBtn`, `dailyRewardsMC`, `storeNavBtn`, `menuNavBtn`, `buttonLabels`, `characterBtn`, `adventureBtn`, `achievementBtn`, `classBtn`, `clubsBtn`, `collectionBtn`, `cornerstoneBtn`, `creditsBtn`, `friendsBtn`, `howtoplayBtn`, `inventoryBtn`, `leaderboardBtn`, `likedworldsBtn`, `mapBtn`, `marketplaceButn`, `patronBtn`, `settingsBtn`, `storeBtn`, `tinyQuestBtn`, `welcomeBtn` — individual button/MC symbol wrappers for each nav entry; these are the named child instances inside `navMenu`; no independent logic beyond their symbol frames.

---

## Notable logic
- **21-entry menu array:** The `menuItems` array drives all button discovery; adding a new menu section only requires adding its name string and placing matching `<name>Button` / `<name>Highlight` / `<name>TextField` instances in the `navMenu` symbol.
- **Tray layout shift:** `showTrayButton` shifts the x-position of all tray buttons to the right of the toggled one by ±65 px, keeping them packed without gaps.
- **Store deals badge:** `storeDealsIcon` is a separate MovieClip that replaces `navStoreButton` visually when a deal is active; `setTrayHotkey` syncs both with the same label.
- **NX achievement suppression:** The achievement button and its text field are hidden and the leaderboard button is moved into its grid position so layout remains consistent.
- **Highlight pulse loop:** Two `IggyTween` instances chain via `motionFinishCallback` so the highlight fades out then back in indefinitely while a button is selected.
