# clubpicker.swf
> A modal dialog that appears when a player needs to select one of their clubs (guilds). Displays a scrollable list of club names with Select buttons and a Cancel option, used for club-context actions that require specifying which club to apply them to.

**Document/main class:** `ClubPicker` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 2 (plus button stubs and scroll skins)

---

## Main class: `ClubPicker`

Root dialog component. Constructor creates a `StackList` (5px item spacing) and adds it as a child of `listView` (a `ScrollableView`). Registers three `ExternalInterface` callbacks in Iggy mode: `setInstructions`, `addClub`, `highlightClub`. Wires `cancelButton` click to call `ExternalInterface.call("OnCancel")`. In test/preview mode populates 4 dummy clubs. Frame scripts stop at frames 1 (PC) and 11 (console layout).

### Public methods (all registered as ExternalInterface callbacks)

- `setInstructions(text:String) : void` — sets the `instructionsTextField` content directly.
- `addClub(clubId:String, clubName:String) : void` — creates a `ClubEntry(clubId, clubName)` and appends it to the `StackList`.
- `highlightClub(index:int) : void` — iterates all `ClubEntry` children of the stack list and calls `SetHighlighted(index == i)` on each, supporting controller cursor highlighting.

### Key fields

- `stackList : StackList` — internal vertical list that holds all `ClubEntry` instances; 5px item spacing.
- `listView : ScrollableView` — scrollable viewport containing the `StackList`.
- `instructionsTextField : TextField` — prompt text at the top of the dialog (e.g. "Choose a club").
- `cancelButton : LabelButton` — label `$Cancel`; click fires `ExternalInterface.call("OnCancel")`.

### Frame scripts / timeline

- Frame 1: `stop()` — PC layout.
- Frame 11: `stop()` — console layout variant.

### Runtime dependencies & integration

- `ExternalInterface.addCallback` registrations: `setInstructions`, `addClub`, `highlightClub`.
- Outbound `ExternalInterface.call`: `OnCancel()`, `OnSelect(clubId)` (fired from `ClubEntry`).
- translate keys: `$Cancel`, `$Select_ButtonLegend`.

---

## Other game-specific classes

- `ClubEntry` (extends `_kiwi.Core.UIComponent`, embeds `symbol14`) — one row in the club list. Constructor takes `(clubId:String, clubName:String)`. Fields: `nameTextField:TextField` (shows club name), `selectButton:LabelButton` (label `$Select_ButtonLegend`). `onSelect` click handler fires `ExternalInterface.call("OnSelect", clubId)`. `SetHighlighted(bool)` calls `HighlightUtil.highlightMovieClip` or `unhighlightMovieClip` on `selectButton` for controller cursor support; guards against redundant state changes via `m_isHighlighted`.

### Button and skin stubs (15 classes, no game logic)

- `btnGreen` (extends `LabelButton`, embeds `symbol24`) — 4-state green button (frame stops at 10, 20, 30, 40).
- `btnGreenIcon_small` (extends `LabelButton`, embeds `symbol11`) — 4-state small green icon button.
- Scroll skins (13): `focusRectSkin` (symbol28), `ScrollArrowDown_upSkin` (symbol53), `ScrollArrowDown_overSkin` (symbol41), `ScrollArrowDown_downSkin` (symbol37), `ScrollArrowDown_disabledSkin` (symbol56), `ScrollArrowUp_upSkin` (symbol47), `ScrollArrowUp_overSkin` (symbol44), `ScrollArrowUp_downSkin` (symbol34), `ScrollArrowUp_disabledSkin` (symbol57), `ScrollThumb_upSkin` (symbol50), `ScrollThumb_overSkin` (symbol43), `ScrollThumb_downSkin` (symbol40), `ScrollTrack_skin` (symbol31) — all are bare `MovieClip` embeds with no logic.

---

## Notable logic

- **Highlight guard:** `ClubEntry.SetHighlighted` checks `m_isHighlighted != param1` before calling `HighlightUtil`, preventing redundant filter operations when the game engine calls `highlightClub` repeatedly.
- **Club ID as string:** `clubId` is stored and passed back as a `String` (not `int`), allowing for opaque server-side IDs.
- **No pagination:** the `StackList` grows dynamically with each `addClub` call and relies entirely on the parent `ScrollableView` for scrolling when the list overflows the viewport.
