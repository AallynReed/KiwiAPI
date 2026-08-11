# Trove UI SWF Documentation
A reference for Trove's **108 Flash UI files**. Each page covers that SWF's game-specific ActionScript classes; the shared `_kiwi`/`fl`/`mx` widget framework is omitted from the per-screen pages and documented once in the **[Shared Library Reference](./SHARED_LIBRARIES.md)**.

## How Trove UI works (quick primer)
- Each SWF has a **document class** (e.g. `CharSheet`) instantiated when the UI loads.
- The game ↔ UI bridge is **Iggy** (Trove's Scaleform-like Flash runtime). AS3 calls `IggyFunctions.translate(key)` for localization and registers/invokes native callbacks; the engine pushes data in and reads UI events out.
- Widgets come from the embedded `_kiwi` component library (buttons, lists, scroll panes, windows).
- Timeline symbol classes appear under a `<Name>_fla.*` package.

## In-world HUD & overlays
| SWF | Size | Main class | Summary |
|---|---|---|---|
| [armorhud.swf](./armorhud.md) | 66 KB | `ArmorHudUI` | A compact HUD element that displays the player's current armor value as a hexagonal fill gauge with a numeric readout. Appears in adventu... |
| [healthmeter.swf](./healthmeter.md) | 20 KB | `HealthMeter` | Displays a boss (Titan) health bar during encounters in Trove. The UI shows the Titan's name and a horizontally-scaled mask representing ... |
| [playerhealth.swf](./playerhealth.md) | 13 KB | `—` | A minimal HUD element that displays the player's health (and potentially energy/resource) as a percentage bar with a numeric label. It co... |
| [playerhud.swf](./playerhud.md) | 267 KB | `PlayerHUD` | The Player HUD is the persistent in-game overlay showing the player's class insignia, power rank, XP/prestige bar, active buff icons with... |
| [hotbar.swf](./hotbar.md) | 319 KB | `Hotbar` | The persistent bottom-of-screen HUD shown during gameplay in Trove. It renders the player's quick-slot items, health and energy globes, f... |
| [reticle.swf](./reticle.md) | 9 KB | `—` | The player's reticle (crosshair) and nearby status-bar HUD shown in Trove during gameplay. It renders health and energy bars flanking the... |
| [compass.swf](./compass.md) | 253 KB | `Compass` | The in-game compass HUD bar displayed at the top of the screen while exploring. It renders icons for nearby dungeons, lairs, quests, play... |
| [waypointdisplay.swf](./waypointdisplay.md) | 25 KB | `Root` | Renders in-world waypoint markers on the Trove HUD — directional arrows and icons that track named points of interest (flags, bases, reso... |
| [nameplate.swf](./nameplate.md) | 277 KB | `Nameplate` | Floating nameplate displayed above players and NPCs in the Trove world, showing the entity's name, club affiliation, insignia (shield + w... |
| [speechbubble.swf](./speechbubble.md) | 8 KB | `SpeechBubble` | Displays a floating speech-bubble overlay anchored to the bottom-center of the screen, used in Trove for NPC or contextual dialogue. The ... |
| [damageui.swf](./damageui.md) | 32 KB | `DamageUIBase` | Full-screen vignette overlay that visualizes the player's health state in Trove. Appears whenever the player is at low or critical health... |
| [dpsui.swf](./dpsui.md) | 86 KB | `DPSUI` | A minimal combat overlay that displays two live text fields — current DPS and cumulative damage dealt — updated by the game engine via Ex... |
| [starbar.swf](./starbar.md) | 126 KB | `StarBar` | The Star Bar is a persistent HUD element that displays up to three stacked progress bars tracking the player's daily/weekly earning progr... |
| [zonebanner.swf](./zonebanner.md) | 74 KB | `ZoneBanner` | Displays a zone-entry banner overlay in Trove when the player enters a new area, showing the zone name (uppercased) and level range. The ... |
| [rampagealert.swf](./rampagealert.md) | 77 KB | `RampageAlert` | Displays a full-screen animated alert when a Rampage challenge begins in Trove. The SWF plays a timeline animation, shows localized warni... |
| [rampagealert02.swf](./rampagealert02.md) | 79 KB | `RampageAlert` | A second variant of the Rampage challenge alert animation for Trove. Functionally identical to `rampagealert.swf` — it displays the same ... |
| [broadcast.swf](./broadcast.md) | 18 KB | `Broadcast` | A minimal HUD overlay that displays two lines of broadcast text in Trove: one system-driven message and one player-driven message. It app... |
| [notifications.swf](./notifications.md) | 26 KB | `NotificationManager` | Displays transient text notifications in the Trove HUD. Notifications appear stacked vertically, fade in on arrival, and fade out after a... |
| [shortcuttray.swf](./shortcuttray.md) | 127 KB | `ShortcutTray` | The shortcut tray is the persistent HUD toolbar that gives players quick access to core UI panels (Store, Character, Collections, Invento... |
| [radial.swf](./radial.md) | 68 KB | `Radial` | Three-sector radial menu used to select mount actions in Trove. Appears when the player opens the mount radial wheel, presenting three la... |
| [draghost.swf](./draghost.md) | 6 KB | `DragHost` | A lightweight drag-image host used in Trove's inventory or item UI. When the player begins dragging an item, the engine pushes a texture ... |
| [dimmer.swf](./dimmer.md) | 18 KB | `Dimmer` | A full-screen semi-transparent overlay used to darken the background behind modal dialogs and popups. Has no interactive elements; it sim... |
| [contextmenu.swf](./contextmenu.md) | 19 KB | `KiwiContextMenu` | A lightweight right-click context menu popup that displays a dynamic list of labelled options. It appears wherever the game engine trigge... |
| [tooltip.swf](./tooltip.md) | 96 KB | `Tooltip` | The Trove item tooltip panel that floats near the cursor or a selected item, displaying the item name (with optional rainbow/shadow effec... |
| [worldtooltip.swf](./worldtooltip.md) | 73 KB | `Tooltip` | Rich item/world-object tooltip panel displayed when the player hovers over or inspects an item in Trove. Supports a primary info panel wi... |
| [message.swf](./message.md) | 285 KB | `Message` | A small in-game message/tutorial overlay in Trove used to display a contextual text message, an optional item or art image, an objective ... |
| [calltoaction.swf](./calltoaction.md) | 37 KB | `CallToAction` | A tutorial/onboarding overlay that draws a semi-transparent mask over the screen with a cutout ellipse highlighting a specific UI area, a... |

## Character, gems & progression
| SWF | Size | Main class | Summary |
|---|---|---|---|
| [inventory.swf](./inventory.md) | 588 KB | `Inventory` | The player's Inventory window, showing item slots organized across Adventure, Build, Currency, Geode, and Discovery tabs, plus Personal C... |
| [charsheet.swf](./charsheet.md) | 910 KB | `CharSheet` | The Character Sheet window in Trove, opened from the main HUD. It shows the player's equipped gear slots, class abilities, stats, name/ti... |
| [charcustomize.swf](./charcustomize.md) | 105 KB | `CharCustomize` | The character customization window where players choose their character's race (class skin), head type, hairstyle, hair colour, and eye c... |
| [subclassselect.swf](./subclassselect.md) | 100 KB | `SubClassSelect` | Panel for selecting a subclass (secondary class passive) in Trove. Displays a scrollable tiled grid of available subclasses, each shown a... |
| [progression.swf](./progression.md) | 618 KB | `Progression` | An interactive skill-tree / progression map UI that displays a zoomable, pannable node graph showing unlocked, locked, and upgradeable ab... |
| [progressioninfo.swf](./progressioninfo.md) | 154 KB | `ProgressionInfo` | The progression/crafting info panel shown when the player selects an upgrade node, a craftable item, or a progression choice. Displays a ... |
| [titlesselector.swf](./titlesselector.md) | 177 KB | `TitlesSelector` | The Titles selector window where players browse, filter, favourite, and equip name titles (prefixes and suffixes) for their character. It... |
| [collections.swf](./collections.md) | 260 KB | `Collections` | The Collections window, used to browse and equip cosmetic collections (mounts, sails, styles, etc.) organized into named tabs and collaps... |
| [collectionsnew.swf](./collectionsnew.md) | 542 KB | `CollectionsNew` | The main Collections window in Trove, showing all collectable cosmetic items (mounts, styles, etc.) organized into meta-categories and su... |
| [collectionconsumeresult.swf](./collectionconsumeresult.md) | 121 KB | `CollectionConsumeResult` | A small confirmation dialog shown after consuming a collection item, displaying the resulting item's name, a title string, an item icon s... |
| [effectviewer.swf](./effectviewer.md) | 176 KB | `EffectViewer` | A scrollable panel that lists all active status effects on the player, supporting search by name and a "removeable only" filter. It appea... |
| [epicpose.swf](./epicpose.md) | 121 KB | `EpicPose` | The Epic Pose UI panel lets players take a screenshot of their character in a posed state. It provides camera-angle selection via an arro... |
| [gems.swf](./gems.md) | 237 KB | `Gems` | The Gem Forge UI panel, accessible from the character sheet. It displays 12 gem equipment slots (organised into blue/yellow/red/opal sets... |
| [gemforge.swf](./gemforge.md) | 216 KB | `GemForge` | The Gem Forge crafting window, opened at a Gem Forge station. Lets players drag gems into 12 inventory slots and one central upgrade slot... |
| [moduleloadout.swf](./moduleloadout.md) | 389 KB | `ModuleLoadout` | The Module Loadout panel in Trove where a player equips and swaps active modules, passive modules, and reliquaries on their character, an... |

## Crafting, economy & loot
| SWF | Size | Main class | Summary |
|---|---|---|---|
| [crafting.swf](./crafting.md) | 559 KB | `Crafting` | The Crafting window is Trove's item-crafting UI, shown when the player interacts with a crafting bench. It presents a categorised recipe ... |
| [forge.swf](./forge.md) | 1.1 MB | `Forge2` | The Forge crafting window, opened at Forge stations in Trove. Allows players to upgrade equipment by dragging items into a central slot, ... |
| [companionforge.swf](./companionforge.md) | 345 KB | `CompanionForge` | The Companion Forge is the UI panel for upgrading companion items in Trove. It displays the currently-forged item (name, rarity, star rat... |
| [geodeincubator.swf](./geodeincubator.md) | 221 KB | `GeodeIncubator` | The Geode Incubator / Reliquary window lets players place eggs or reliquaries into one of three slots, apply Karma-based buff influences,... |
| [marketplace.swf](./marketplace.md) | 480 KB | `Marketplace` | The Trove player-driven auction marketplace, allowing players to search and purchase listings from other players (Buy tab) and create or ... |
| [kiwistore.swf](./kiwistore.md) | 712 KB | `StoreBase` | The Trove in-game item store (the "Kiwi Store"), presenting purchasable products in a tabbed, scrollable tile grid with support for Patro... |
| [npcstore.swf](./npcstore.md) | 280 KB | `NPCStore` | The NPC vendor shop window shown when a player interacts with an in-game merchant. Displays a paginated grid of purchasable products with... |
| [trade.swf](./trade.md) | 292 KB | `Trade` | The player-to-player trade (and deconstructor) window in Trove. Shows a pending-player list while waiting for a trade partner, then switc... |
| [lockbox.swf](./lockbox.md) | 291 KB | `LockBox` | The Loot Box (lock box) opening window in Trove. Displays the current box item, unlock buttons (golden key and free/Flux), a karma progre... |
| [rewardcrate.swf](./rewardcrate.md) | 353 KB | `RewardCrate` |  |
| [dailyrewards.swf](./dailyrewards.md) | 201 KB | `DailyRewards` | The Daily Login Rewards window, showing up to 7 day-tiles representing the current weekly reward cycle. Each tile displays an item icon, ... |
| [lootcollector.swf](./lootcollector.md) | 162 KB | `LootCollector` | The Loot Collector panel appears after fighting enemies or opening containers — it lists items waiting to be deconstructed (collected) or... |
| [recipeconsumeprompt.swf](./recipeconsumeprompt.md) | 59 KB | `RecipeConsumePrompt` | A modal confirmation dialog shown when the player attempts to consume (learn) a recipe item. It lists one or more recipes with their icon... |
| [dropprompt.swf](./dropprompt.md) | 94 KB | `DropPrompt` | A modal quantity-picker dialog that appears when a player chooses to drop (discard) an item from their inventory. It displays a prompt me... |
| [cornerstone.swf](./cornerstone.md) | 96 KB | `CornerstoneWindow` | The Cornerstone window lets players manage their four Cornerstone plot slots — the personal building plots that follow a character in Tro... |

## Social
| SWF | Size | Main class | Summary |
|---|---|---|---|
| [friendslist.swf](./friendslist.md) | 224 KB | `FriendsList` | The social Friends List panel, showing the player's friends and ignored users in two tabs. Appears when the player opens the social/frien... |
| [friendpicker.swf](./friendpicker.md) | 92 KB | `FriendPicker` | A modal dialog that displays the player's friends list with search filtering, allowing the player to select a friend or manage social act... |
| [clubs.swf](./clubs.md) | 726 KB | `ClubsUI` | The full Club management UI in Trove, opened from the social/club interface. It presents a tabbed window with five tabs: Club List (dashb... |
| [clubpicker.swf](./clubpicker.md) | 163 KB | `ClubPicker` | A modal dialog that appears when a player needs to select one of their clubs (guilds). Displays a scrollable list of club names with Sele... |
| [chat.swf](./chat.md) | 311 KB | `Chat` | The in-game chat window for Trove. It displays scrollable message logs in named tabs, a text input bar with channel selector, a console r... |
| [leaderboard.swf](./leaderboard.md) | 396 KB | `Leaderboard` | The full Leaderboard window shown in Trove when a player opens a leaderboard category. It displays ranked entries (scores/times) across m... |
| [communityleaderboard.swf](./communityleaderboard.md) | 245 KB | `CommunityLeaderboard` | The Community Leaderboard is a voting UI panel that lets players browse and vote for player-submitted items (e.g., contest entries) that ... |
| [likedworlds.swf](./likedworlds.md) | 94 KB | `LikedWorlds` | The Liked Worlds panel in Trove that displays the player's saved/liked club worlds in a scrollable list, allowing them to teleport to a w... |

## World & navigation
| SWF | Size | Main class | Summary |
|---|---|---|---|
| [map.swf](./map.md) | 274 KB | `Map` | The in-game world map overlay, displaying a texture of the current zone with a dynamic legend that shows which point-of-interest categori... |
| [atlas.swf](./atlas.md) | 1.8 MB | `Atlas` | The interactive world map ("Atlas") that allows players to navigate between Trove's biomes and portals. It is a zoomable, draggable map o... |
| [atlasinfobox.swf](./atlasinfobox.md) | 159 KB | `AtlasInfoBox` | Tooltip-style info box displayed on the Atlas (world map) when a portal or location is selected. Shows the world name, a description, and... |
| [delveselectorui.swf](./delveselectorui.md) | 106 KB | `DelveSelector` | The "Dial-a-Depth" Delve selector popup, shown when a player chooses to enter a Delve dungeon. It lets the player pick a depth level (wit... |
| [shadowtower.swf](./shadowtower.md) | 239 KB | `ShadowTower` | The Shadow Tower window, opened when interacting with a Shadow Tower portal. Lets players select a floor (boss), choose a difficulty (Nor... |
| [terraformoverview.swf](./terraformoverview.md) | 66 KB | `TerraformOverview` | Shows a countdown overlay during a Terraformer activation sequence in Trove, displaying a live ticking timer and cancellation instruction... |
| [claims.swf](./claims.md) | 358 KB | `Claims` | The Claims panel in Trove, showing pending claimable rewards (items, currency, etc.) earned from leaderboards, PvP, mastery, badges, or o... |

## Quests & activities
| SWF | Size | Main class | Summary |
|---|---|---|---|
| [questtracker.swf](./questtracker.md) | 142 KB | `QuestTracker` | The Quest Tracker is Trove's heads-up objective overlay, shown persistently during gameplay to display active quests, the Golden Thread p... |
| [tinyquestui.swf](./tinyquestui.md) | 249 KB | `TinyQuestUI` | The Tiny Quest UI window displays the player's active quests organised into collapsible sections (Active, Completed, Claim, Cancel), each... |
| [tinyquestofferui.swf](./tinyquestofferui.md) | 414 KB | `TinyQuestOfferUI` | The Tiny Quest offer/accept dialog shown when a player opens a Tiny Quest for review. It displays the quest title, difficulty star rating... |
| [activitytrackerui.swf](./activitytrackerui.md) | 531 KB | `ActivityTrackerUI` | The Activity Tracker is a multi-tab quest/objective panel that surfaces the player's active Trove pursuits across several categories: Eve... |
| [guideui.swf](./guideui.md) | 472 KB | `GuideUI` | The Guide UI is Trove's in-game compendium and activity browser, displayed whenever the player opens the Guide (collection/achievement/st... |
| [minigamescorecard.swf](./minigamescorecard.md) | 409 KB | `MiniGameScorecard` | End-of-match scorecard window displayed after a Trove minigame concludes. Shows a ranked leaderboard of players with their scores, a set ... |

## PvP & Battle Royale
| SWF | Size | Main class | Summary |
|---|---|---|---|
| [pvphud.swf](./pvphud.md) | 29 KB | `PVPHUD` | The heads-up display overlay shown during PvP matches in Trove. It displays a countdown timer, team scores, a personal score panel, kill-... |
| [pvpscorecard.swf](./pvpscorecard.md) | 106 KB | `PVPScoreCard` | End-of-match scorecard displayed after a PVP battle in Trove. Shows both teams' player scores (kills, deaths, captures, returns), a victo... |
| [pvpinstructionsui.swf](./pvpinstructionsui.md) | 182 KB | `PVPInstructionsUI` | A simple modal dialog shown before a PvP match begins, presenting the rules or instructions for the game mode. It has a single "OK" butto... |
| [pvpplayercounterui.swf](./pvpplayercounterui.md) | 1 KB | `—` | A PvP HUD element that displays a player count or score counter during PvP matches in Trove. The SWF's scripts/ directory is entirely emp... |
| [royalescorecard.swf](./royalescorecard.md) | 106 KB | `RoyaleScoreCard` | The end-of-match (and mid-match) scoreboard for Trove's Bomber Royale mode. Displays a ranked list of all players with stats (kills, dama... |
| [tutorialroyaleintro.swf](./tutorialroyaleintro.md) | 286 KB | `TutorialRoyaleIntro` | A mode-selection interstitial shown to players entering a Bomber Royale tutorial for the first time, presenting two choices: play the Adv... |

## Menus, system & flow
| SWF | Size | Main class | Summary |
|---|---|---|---|
| [mainmenu.swf](./mainmenu.md) | 31 KB | `MainMenu` | The persistent in-game main menu bar displayed during gameplay. It provides a collapsible dropdown of navigation options, a hub shortcut ... |
| [navigationmenu.swf](./navigationmenu.md) | 261 KB | `NavigationMenu` | The persistent navigation tray and expandable full-screen menu that lets players open any major game window (Store, Character, Inventory,... |
| [settings.swf](./settings.md) | 393 KB | `Settings` | The in-game Settings window, opened from the escape menu. It presents a left-side category list and a dynamically swapped content pane fo... |
| [escapewindow.swf](./escapewindow.md) | 121 KB | `EscapeWindow` | The Escape Window is the in-game pause/escape menu that appears when a player presses Escape in Trove. It presents navigation buttons for... |
| [loginui.swf](./loginui.md) | 83 KB | `LoginBase` | The Trove login screen UI, presenting username/password fields and a login button. Supports both mouse/keyboard input and Iggy console co... |
| [pressstart.swf](./pressstart.md) | 72 KB | `PressStart` | The Trove console title / main-menu screen displayed after boot, showing the account name and a navigable list of options (Play, Credits,... |
| [accountlinking.swf](./accountlinking.md) | 349 KB | `AccountLinking` | The console account-linking flow shown when a player on a platform (Xbox / PS4 / NX) needs to link or create a Trion/Gamigo account befor... |
| [eulaui.swf](./eulaui.md) | 533 KB | `EULAUI` | Modal dialog shown at first launch (or on demand) requiring the player to read and accept the Trove End User Licence Agreement, and a sec... |
| [welcomescreen.swf](./welcomescreen.md) | 1.1 MB | `WelcomeScreen` | The Welcome Screen is the first major UI shown to the player on login, combining a rotating featured-content panel (store deals, events, ... |
| [credits.swf](./credits.md) | 2.0 MB | `Credits` | A scrolling credits roll dialog shown in Trove when the player views the game credits. It displays HTML-formatted credits text that auto-... |
| [info.swf](./info.md) | 30 KB | `Info` | The Trove in-game chat / info-message log that displays timed text messages (channel messages, system notifications) in a scrolling stack... |
| [debugui.swf](./debugui.md) | 36 KB | `DebugBase` | Minimal developer/debug overlay UI that displays a live key-value list of engine debug entries. Entries can be added, updated, or removed... |
| [modloader.swf](./modloader.md) | 118 KB | `ModLoader` | A window UI for browsing and managing installed game mods. Shows a scrollable list of mods on the left and a detail panel with title, aut... |
| [messagedialog.swf](./messagedialog.md) | 92 KB | `MessageDialog` | General-purpose modal confirmation dialog used throughout Trove for purchase prompts, unlock confirmations, and other binary or multi-cho... |
| [inputpromptui.swf](./inputpromptui.md) | 79 KB | `InputPromptUI` | A modal text-input dialog used whenever the game needs the player to type a short string — for example, naming a character or entering te... |
| [controllernotification.swf](./controllernotification.md) | 13 KB | `ControllerNotificationManager` | A small toast-style notification popup that slides in from the bottom of the screen to display short controller-related messages (e.g. it... |
| [releaseui.swf](./releaseui.md) | 67 KB | `ReleaseUI` | Displays a release/loading screen overlay in Trove that shows a primary message and a secondary instruction line. Supports both plain-tex... |
| [tutorial.swf](./tutorial.md) | 239 KB | `Tutorial` | A paginated tutorial / help slideshow panel shown to players when they first start Trove or enter a new game mode. It presents a series o... |
| [displayarea.swf](./displayarea.md) | 148 KB | `DisplayArea` | A console TV-safe-zone calibration overlay shown to players on console platforms. It presents a resizable safe-zone rectangle with Accept... |
| [background.swf](./background.md) | 350 KB | `Background` | Fullscreen animated background displayed during Trove's login/launcher screen and loading states. Handles aspect-ratio-correct scaling of... |

## Platform-shared component libraries
| SWF | Size | Main class | Summary |
|---|---|---|---|
| [uips4shared.swf](./uips4shared.md) | 58 KB | `—` | PlayStation 4 (Western regions) shared component library. This is **not a screen** — it is a collection of button-icon and cursor assets ... |
| [uips4jshared.swf](./uips4jshared.md) | 58 KB | `—` | PlayStation 4 **Japan** shared component library. This is **not a screen** — it is a collection of button-icon and cursor assets loaded b... |
| [uiswitchshared.swf](./uiswitchshared.md) | 44 KB | `—` | Nintendo Switch shared component library. This is **not a screen** — it is a collection of button-icon and cursor assets loaded by the Sw... |
| [uixbshared.swf](./uixbshared.md) | 54 KB | `—` | Xbox shared component library. This is **not a screen** — it is a collection of button-icon and cursor assets loaded by the Xbox build's ... |
