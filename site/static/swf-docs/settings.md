# settings.swf
> The in-game Settings window, opened from the escape menu. It presents a left-side category list and a dynamically swapped content pane for Language, Audio, Video, Controls, Hotkeys, Social, Payments, and Misc on PC; and AudioVideo, Controls, Social, Legal, and Gameplay on console. Each pane notifies the game engine when any option changes.

**Document/main class:** `Settings` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 45 (excluding pure skin/asset wrappers)

## Main class: `Settings`

`Settings` manages the category list and content-pane lifecycle. On construction it stores the placeholder position, removes the placeholder from the display list, and conditionally registers four `ExternalInterface` callbacks. Accept/Cancel buttons and the `categoriesList` `Listbox` are wired immediately. A `perspectiveProjection` is configured (FOV 74.54, centre 275×200) on `ADDED_TO_STAGE`. In `configUI()`, the appropriate category list is populated depending on `IsConsole()` and `IsRunningSystemConsole()`; the Chinese locale (`LOCALE_ZH`) hides the Payments category.

### Public methods
- `hideProfanityFilter() : void` — forwards to the active `SocialPane` if it is currently displayed, shifting checkbox positions upward to fill the gap.
- `__setPerspectiveProjection_(e : Event) : void` — sets 3-D perspective on the root.

### Key fields
- `__id0_ : WindowHeaderSmall` — the panel header; title key `$EscapeMenu_Settings`.
- `categoriesList : Listbox` — left-side navigation list; items are added with translated labels and int data values.
- `contentPlaceholder : MovieClip` — removed at runtime; its `x`/`y` become `contentOrigin`.
- `contentOrigin : Point` — position where all content panes are placed.
- `currentContentPane : MovieClip` — the currently visible pane; previous pane is removed before the new one is added.
- `displayedPanelCategory : int` — tracks which category is shown to avoid redundant swaps.
- `acceptButton, cancelButton : MovieClip` — each wraps a `btn` child that fires `OnAccept` / `OnRequestClose`.
- `btnLegend : MovieClip` — console button-legend strip; some children hidden per platform (NX hides `btnX`, non-NX console hides `btnY`, non-system-console hides `btnY2`).
- `__setPropDict : Dictionary` — frame-based prop-setting deduplication dictionary.
- `__lastFrameProp : int` — last frame at which `__setProp_handler` ran.

### Frame scripts / timeline
- **frame 1** (`frame1`): `stop()`
- **frame 11** (`frame11`): `stop()` — triggers deferred `cancelButton`/`acceptButton` component-inspector setup (props applied only when current frame is in range 11–20).

### Runtime dependencies & integration

**ExternalInterface callbacks registered (game → Flash):**
| Callback | Handler | Description |
|---|---|---|
| `switchCategory(id)` | `switchCategory(int)` | Destroys the current pane and instantiates the appropriate pane class at `contentOrigin`. |
| `onAccept(e)` | `onAccept` | Calls `OnAccept` back. |
| `onCancel(e)` | `onCancel` | Calls `OnRequestClose` back. |
| `setSelectedCategoryIndex(id)` | `setSelectedCategoryIndex` | Selects a list item by data value without switching content. |
| `hideProfanityFilter()` | `hideProfanityFilter` | Forwards to `SocialPane`. |

**ExternalInterface calls dispatched (Flash → game):**
- `OnAccept` — accept button or callback.
- `OnRequestClose` — cancel button or callback.
- `RequestSwitchCategory(id)` — when the player clicks a category in the list; the game responds with `switchCategory`.

**translate keys:** `$EscapeMenu_Settings`, `$Settings_Category_Language`, `$Settings_Category_Audio`, `$Settings_Category_Controls`, `$Settings_Category_Hotkeys`, `$Settings_Category_Video`, `$Settings_Category_Social`, `$Settings_Category_Payment`, `$Settings_Category_Misc`, `$Settings_Category_Audio_Video`, `$Settings_Category_Legal`, `$Settings_Category_Gameplay`, `$Accept`, `$Cancel`.

## Other game-specific classes

### Content pane classes (PC)

- **`LanguagePane`** (embed `symbol239`, extends UIComponent) — single `KiwiComboBox` (`lanaguageSelect`). Callback `addLanguage(culture, label)` populates the combo. Property `selectedCulture : String` gets/sets by matching `.data`. Fires `OnCategoryOptionChanged(categoryId)` on change.

- **`AudioPane`** (embed `symbol317`, extends UIComponent) — four `fl.controls.Slider` widgets (master, music, SFX, ambiance; range 0–10, live dragging) plus a `backgroundAudioCB` Checkbox (`$Settings_BackgroundAudio`). All fire `OnCategoryOptionChanged` on change.

- **`ControlsPane`** (embed `symbol279`, extends UIComponent) — mouse invert X/Y checkboxes, mouse sensitivity slider (0–10, snap 0.02), controller-support checkbox, swap-thumbsticks checkbox, camera stick invert X/Y, thumbstick sensitivity slider, `preferredPlatformUI` KiwiComboBox. Callbacks `addPlatformUI(id, label)` and `hidePlatformUI()`. Enabling/disabling controller support dynamically adds/removes the Controller entry from the combo box. Fires `OnCategoryOptionChanged`.

- **`VideoPane`** (embed `symbol208`, extends UIComponent) — the most complex PC pane. Quality preset buttons (Low/Medium/High/Ultra/Custom as toggle `LabelButton`s), window-mode combo (`Windowed`/`Fullscreen`/`Fullscreen Windowed`), render-device combo, display-modes combo (populated via `addDisplayMode(id,w,h,hz)` → `"WxH @HzHz"`). Advanced section (hidden unless Custom preset): draw distance, supersample, shader complexity, FOV (60–100), LOD distance (15–100), VFX LOD (0.05–1), vsync, post-FXAA, bloom, DOF, SSAO, lens distortion. Gamma and brightness sliders (0–2, displayed via `Lerp` into range 0.2–5). `TimedConfirmPopup` embedded (hidden by default; used for resolution-change confirmation). Callbacks: `addRenderDevice`, `clearDropDowns`, `addDisplayMode`, `forceUpdateSliderText`, `notifyQualityPresetLevel`. Fires `OnCategoryOptionChanged`, `OnVideoPresetClicked(presetId)`, `OnRenderDeviceChanged`.

- **`SocialPane`** (embed `symbol211`, extends UIComponent) — checkboxes: profanity filter, appear offline, auto-join global chat, auto-join trade chat, show player nameplates, show own nameplate, display club name. Tether section (invitation behavior combo + icon checkboxes for self/others) is created but immediately hidden via `disableTethering()`. `hideProfanityFilter()` shifts visible checkboxes up. Fires `OnCategoryOptionChanged`.

- **`MiscPane`** (embed `symbol237`, extends UIComponent) — combat text, intro movies, player location, multithreaded, cornerstone damageable, suppress broadcast chat spam, skip auction claim confirmation checkboxes; UI scale slider (1–2, snap 0.05); rarity combo (populated via `addRarity`); EULA button (`OnView3rdPartyLicenses`) and credits button (`OnCreditsButtonClicked`). Contains a `TimedConfirmPopup` (hidden). Fires `OnCategoryOptionChanged`.

- **`PaymentsPane`** (embed `symbol229`, extends UIComponent) — payment method combo (`methodsCB`), add/remove button (`OnAddRemovePaymentMethods`). Callbacks `addPaymentMethod(id, label, select)`, `clearPaymentMethods()`, `notifyLoggedIntoSteam(bool)` (hides art if on Steam). Fires `OnCategoryOptionChanged`.

- **`KeyBindings`** (embed `symbol262`, extends UIComponent) — `GroupedItemList` with `UIScrollBar`; each entry is a `KeyBindingItem`. Callbacks `addScope(id, name)`, `addKeyBinding(scopeId, actionId, name, keyStr, consoleStr)`, `hideCapturePrompt()`, `updateKeyBinding(actionId, keyStr, consoleStr)`. Clicking a binding button shows `captureWaitMC` and fires `OnTryUpdateHotkey(scopeId, actionId, inputType)` (0=keyboard, 1=console). "Restore Defaults" button fires `OnRestoreDefaults`.

### Content pane classes (console / system-console)

- **`ConsoleAudioVideoPane`** (embed `symbol312`, extends UIComponent) — music, SFX, ambiance sliders; brightness and gamma sliders (with formatted text display using `Lerp`); UI scale slider. Platform-specific: adds display-area item for Durango. Callbacks `forceUpdateSliderText`, `highlightSelection(idx)` (moves `optionHighlight.y`), `changeSettingValue(idx, delta)` (adjusts slider values by fixed increments), `restoreDefaults`. Fires `OnContentPaneLoaded(count)` on first `ENTER_FRAME`.

- **`ConsoleControlsPane`** (embed `symbol299`, extends UIComponent) — invert camera stick X/Y, thumbstick sensitivity slider, auto-aim checkbox. Two frame stops (frames 1, 11). Callbacks `highlightSelection`, `changeSettingValue`, `restoreDefaults`. Fires `OnContentPaneLoaded`.

- **`ConsoleSocialPane`** (embed `symbol287`, extends UIComponent) — global chat, trade chat, profanity filter, appear offline, display club name checkboxes; four editable quick-chat TextFields (`customText0`–`3`); optional Switch Profile link (Durango only). Inline text editing: clicking a quick-chat field enters edit mode, Enter submits, cancel restores `previousText`. Callbacks `highlightSelection`, `changeSettingValue`, `restoreDefaults`, `setCustomText(idx, text)`, `onEditTextField`, `onSetTextCancel`. Fires `OnSetEditTextMode(bool)`, `OnCallSwitchProfile`, `OnContentPaneLoaded`.

- **`ConsoleLegalPane`** (embed `symbol296`, extends UIComponent) — two selectable text items: view EULA and unlink account. `changeSettingValue(0, 0)` fires `OnViewEULA`. Fires `OnContentPaneLoaded`.

- **`ConsoleMiscPane`** (embed `symbol289`, extends UIComponent) — combat text, player location, vibration (NX uses key `$Settings_Vibration_NX`), cornerstone damageable checkboxes; rarity `ArrowSelect`. Callbacks `highlightSelection`, `changeSettingValue`, `restoreDefaults`, `addRarity`. Fires `OnContentPaneLoaded`.

- **`PCControllerAudioPane`, `PCControllerControlsPane`, `PCControllerVideoPane`, `PCControllerLanguagePane`, `PCControllerMiscPane`, `PCControllerSocialPane`, `PCControllerPaymentsPane`** — controller-overlay variants of the PC panes, instantiated when `IsConsole() && !IsRunningSystemConsole()`. Not individually detailed here (files present, same pattern as PC panes).

### Helper / popup classes

- **`TimedConfirmPopup`** (embed `symbol81`, extends UIComponent) — countdown popup with "Keep" and "Revert" buttons. `secondsLeft` setter formats `$Settings_RevertCountdownFormat` replacing `{time}`. "Revert" fires `OnRevertSettings`; "Keep" fires `OnKeepSettings`.

- **`ConfirmPopup`** (embed `symbol319`, dynamic MovieClip) — simpler popup with a single "OK" button (`$OK`).

- **`InputCapturePrompt`** (embed `symbol261`, dynamic MovieClip) — pure visual overlay shown while waiting for a key press; no code.

- **`KeyBindingItem`** (embed `symbol274`, dynamic MovieClip) — a row in the bindings list; has `bindingNameTextField` (action name), `bindingButton` (keyboard key, `btnGreenIcon_small`), `bindingButtonConsole` (console button, `btnGreenIcon_small`).

### Button and skin asset wrappers

- `BtnGreen`, `btnGreen_small`, `btnGreenIcon_small` — `LabelButton` subclasses, embedded symbol assets.
- `InputCapturePrompt` — pure MovieClip (see above).
- Scroll/slider/listbox skins (24 classes): `ScrollArrowUp/Down_*Skin`, `ScrollThumb_*Skin`, `ScrollTrack_skin`, `ScrollBar_thumbIcon`, `SliderThumb_*Skin`, `SliderTrack_*Skin`, `SliderTick_skin` — all pure bitmap/shape embed symbols, no logic.
- Cell renderer skins (7 classes): `CellRenderer_*Skin` — list-item visual states.
- ComboBox skins (4 classes): `ComboBox_*Skin`.
- `TextInput_upSkin`, `TextInput_disabledSkin`, `List_skin`, `focusRectSkin` — UI element skin symbols.

### Settings_fla timeline symbols (3 classes)

- `Settings_fla/acceptButton_9` — embedded accept-button clip; component-inspector props set label to `$Accept` on frames 11–20.
- `Settings_fla/cancelButton_11` — embedded cancel-button clip; label `$Cancel` on frames 11–20.
- `Settings_fla/header_background_115` — header background graphic clip.

## Notable logic
- **Pane hot-swap:** `switchCategory` removes the current pane with `removeChild` and immediately `addChild`s a freshly constructed replacement. No pooling; pane state is discarded on every category switch.
- **Console vs. PC branching:** The main class checks both `IsConsole()` and `IsRunningSystemConsole()` independently. `IsConsole() && !IsRunningSystemConsole()` = a gamepad player on PC (uses PCController* panes). `IsRunningSystemConsole()` = native console (uses Console* panes with d-pad navigation driven by index-based `highlightSelection`/`changeSettingValue` callbacks).
- **Frame-based prop application:** `__setProp_handler` fires on every `FRAME_CONSTRUCTED` event and guards `cancelButton`/`acceptButton` component inspector calls with a frame-range check (frames 11–20), preventing double-application.
- **Locale guard:** The Payments category is hidden if `_locale == LOCALE_ZH` (Chinese).
- **VideoPane quality preset flow:** Clicking a preset button fires `OnVideoPresetClicked(presetId)` to the game (which applies actual render settings), then calls `notifyQualityPresetLevel(level)` back. Level -1 means Custom: the advanced sub-widgets become visible and the "Custom" indicator button hides.
