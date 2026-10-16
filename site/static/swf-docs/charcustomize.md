# charcustomize.swf
> The character customization window — the Barbershop (`$CharCustomize_Header`, "BARBERSHOP") — where players choose their character's race (class skin), head type, hairstyle, hair colour, and eye colour before finalizing their appearance. Appears during character creation or when re-customizing a character. Supports PC mouse interaction and full console controller navigation with directional mapping.

**Document/main class:** `CharCustomize` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 1 (the main class; all other non-framework files are asset wrappers)

## Main class: `CharCustomize`

A rich customization panel with 14 hair-style preview thumbnails paged in groups of 14 (7 per column), two `ArrowSelect` spinners (race, head type), a `hairStyleSelect` spinner, two colour picker buttons (hair/eye) that open floating `KiwiColorPicker` grids, and Accept/Cancel/Randomize action buttons. On console a `DirectionalMapping` graph connects all interactive elements for D-pad traversal; a glow filter (`GlowFilter`, colour 0xCCCC00, strength 100) is applied to the currently focused element. Frame scripts stop at frames 1 and 11.

### Public methods (all private; surface via ExternalInterface callbacks)

### Key fields
- `raceSelect : ArrowSelect` — race/class selector; change dispatches `OnChoiceChange(RACE, index, true)`. On screen it is the row captioned `$CharCustomize_Head` ("Head:", y 73, beside `raceSelect` at y 55.5) — the enum name and the caption disagree.
- `headTypeSelect : ArrowSelect` — head type; same dispatch pattern. On screen it is the row captioned `$CharCustomize_Skin` ("Skin:", y 131, beside `headTypeSelect` at y 112.5).
- `hairStyleSelect : ArrowSelect` — the "Hair:" row (`$CharCustomize_Hair`). It receives the hair-style names through `addAttributeChoice(HAIRSTYLE, name)`, already resolved, and its changes send `OnChoiceChange(HAIRSTYLE, index, true)` alongside the paged preview grid.
- `hairColorSelect / eyeColorSelect : KiwiColorPicker` — 3-row × 6-column colour grids; hidden until their button is clicked.
- `buttonHairColor / buttonEyeColor : LabelButton` — labelled `$CharCustomize_HairColor` / `$CharCustomize_EyeColor`; clicking shows the matching color picker.
- `container : MovieClip` — holds `preview01`–`preview14`, each an `ObjectPreview` symbol with `selected` and `highlighted` sub-clips. The engine addresses them by path (`container.preview{0}{1}` is a format string in the client, hence the leading zero) and writes `textureName` on each. `container` also holds `preview00` at (119, 50), 62 × 62: `configUI` strips its art and nothing in the SWF ever sets its texture — the engine renders the player's character into it.
- `currentPage : int` — zero-based page index into hair styles; `pageText` shows `"(page+1) / numPages"`.
- `pageLeft / pageRight : BaseButton` — page navigation.
- `acceptButton / cancelButton / randomizeButton : LabelButton` — labelled `$CharCustomize_Accept`, `$CharCustomize_Discard`, `$CharCustomize_Reroll`.
- `rotateLeftButton / rotateRightButton : BaseButton` — held-down rotation of the character preview; fire `OnRotateStart(true/false)` on `MOUSE_DOWN` and `OnRotateStop` on `MOUSE_UP`.
- `highlightedMC : MovieClip` — the currently console-focused element; has glow filter applied.
- `highlightFilter : GlowFilter` — gold glow (0xCCCC00, alpha 1, blur 2×2, strength 100, HIGH quality).
- `__enum : Object` — dynamic object populated by the game engine (via `configUI`'s `ExternalInterface.call("OnConfigured", PREVIEWS_PER_PAGE)`) with attribute-type ID constants: `RACE`, `HEADTYPE`, `HAIRSTYLE`, `HAIRCOLOR`, `EYECOLOR`.
- `PREVIEWS_PER_PAGE : int = 14` — constant; 7 previews per column in a 2-column layout.
- `NUM_SELECTIONS : int = 5` — number of selectable attribute categories (unused directly; for reference).

### Frame scripts / timeline
- `frame1` — `stop()`.
- `frame11` — `stop()`.
- Frame labels: frame 1 = `PC`, frame 11 = `Console`. The `Console` frame removes `randomizeButton` (and swaps in its own `closeButton`, `pageText` and captions) and adds `buttonLegend`, a `textField` captioned `$EpicPose_Rotate` ("Rotate"), the `raceSelectLB/RB`, `headSelectLB/RB`, `pageLB/RB` hint icons and the six `*Highlight` clips.

### ExternalInterface callbacks registered
| Callback | Action |
|---|---|
| `setHairStyles(count, selectedIndex)` | Recalculates pages; updates `pageText`; moves `selected` highlight to the correct preview slot. `selectedIndex` is the index of the current style, not a page — the view jumps to the page that holds it |
| `addAttributeChoice(attrId, label)` | Appends a choice label to the matching `ArrowSelect` (labels arrive already resolved) |
| `clearAttributeChoices(attrId, blank)` | Clears the spinner or colour picker for the attribute. For a spinner, `blank == 0` writes `$None` ("None") back into the field; the engine sends non-zero when it is about to refill the list |
| `selectChoice(attrId, index)` | Sets `ArrowSelect.selectedIndex`, which only invalidates: the resulting `CHANGE` (and so the `OnChoiceChange` echo back to the engine) is dispatched from the spinner's next `draw()`, not inside the callback. Randomize answers with a burst of these, one per attribute |
| `addColorChoice(attrId, color)` | Adds a colour swatch to hair or eye picker; colours beyond the 3 × 6 grid (18) are dropped |
| `selectColorChoice(attrId, color)` | Sets the picker's selected swatch and redraws the colour preview square on the button |
| `activateSelection()` | Confirms the highlighted element (preview click, colour confirm, accept/cancel) |
| `accept` / `cancel` / `randomize` | Map to the corresponding button actions. `cancel` first closes an open colour picker and only otherwise sends `OnRequestClose` |
| `rotateLeft` / `rotateRight` / `rotateRelease` | Map to rotation handlers |
| `moveSelection(dx, dy)` | Traverses the `DirectionalMapping` graph; handles page-flip at column edges |
| `changeLeftRight(delta)` | Cycles the focused `ArrowSelect` or pages previews |

### ExternalInterface calls dispatched
- `OnConfigured(PREVIEWS_PER_PAGE)` — sent immediately in `configUI` so the engine knows the grid size before sending hair data.
- `OnChoiceChange(attrId, index, commit:Boolean)` — attribute selection changed (true = confirm, false = preview-page change).
- `OnColorChange(attrId, colorValue)` — colour picker selection changed.
- `POST_SOUND_EVENT("Play_ui_window_click_item")` — played on every user action.
- `OnAccept`, `OnRequestClose`, `OnRandomize` — terminal actions.
- `OnRotateStart(isLeft:Boolean)`, `OnRotateStop` — model rotation. `OnRotateStop` is sent only on `MOUSE_UP` over the rotate button itself (or the `rotateRelease` callback); a release elsewhere sends no stop.

### Runtime dependencies & integration
- `IggyFunctions.inIggy` — controls whether callbacks are registered or dummy data is seeded.
- `IsConsole()`, `IsNX()` — NX (Nintendo Switch) enables additional highlight clip visibility (`.raceSelectHighlight`, etc.) instead of the glow filter approach. `highlightedMC` is only ever seeded inside `IsConsole()`, so on PC `moveSelection`, `activateSelection` and `changeLeftRight` do nothing; `randomizeButton` is wired only when not on console.
- `DirectionalMapping` graph connects: `raceSelectPanel → headSelectPanel → buttonHairColor → (preview grid) → acceptButton/cancelButton`. Previews form a 2-column grid with column-edge wrapping triggering page changes.
- Translate key: `$CharCustomize_Header`, `$CharCustomize_Accept`, `$CharCustomize_Discard`, `$CharCustomize_Reroll` ("Random"), `$CharCustomize_HairColor`, `$CharCustomize_EyeColor`. Timeline captions add `$CharCustomize_Head`, `$CharCustomize_Skin`, `$CharCustomize_Hair` (the English labels all end in a colon) and, on the console frame, `$EpicPose_Rotate`. `$CharCustomize_AcceptColor` ("Accept Color") is shipped but never used.

---

## Other game-specific classes (asset wrappers)

`hairStyleSelect`, `buttonEyeColor`, `buttonHairColor`, `rotateBtnLeft`, `rotateBtnRight`, `arrowBtnLeft`, `arrowBtnRight`, `btnArrowLeft`, `btnArrowRight`, `BtnGreen`, `btnGreenIcon_medium` — 11 symbol/skin asset classes, no logic.

Standard ColorPicker skins: `ColorPicker_downSkin`, `ColorPicker_upSkin`, `ColorPicker_backgroundSkin`, `ColorPicker_colorWell`, `ColorPicker_overSkin`, `ColorPicker_swatchSkin`, `ColorPicker_disabledSkin`, `ColorPicker_textFieldSkin`, `ColorPicker_swatchSelectedSkin` — 9 skin asset classes.

## Notable logic
- **`__enum` late-binding**: The attribute-type integer constants are not hardcoded in the AS3; instead the game engine populates `CharCustomize.__enum` (a plain `Object`) after `OnConfigured` is called. This means race, head type, hairstyle, hair-colour, and eye-colour IDs are runtime values, making the Flash decoupled from the server-side attribute schema.
- **Paged hair previews**: Hair styles are displayed 14 per page in a fixed 2×7 grid of `ObjectPreview` symbols (`preview01`–`preview14`). Navigating left at column 1 or right at column 7 (mod `PREVIEWS_PER_PAGE/2`) triggers `changePage`, which calls `OnChoiceChange` with the page-start index and `commit=false` so the engine loads the new page's thumbnails without finalizing a choice.
- **Dual highlight systems**: PC uses only the glow filter; NX also shows dedicated highlight `MovieClip` overlays (`raceSelectHighlight`, `headSelectHighlight`, `hairColorHighlight`, `eyeColorHighlight`, `acceptHighlight`, `cancelHighlight`) because the Switch platform may not render bitmap filters reliably.
- **Colour picker integration**: `KiwiColorPicker` is hidden by default. Opening it via `activateSelection` or button click transfers the glow filter to the picker, saves the previous element in `lastHighlight`, and restores it on cancel/confirm.
