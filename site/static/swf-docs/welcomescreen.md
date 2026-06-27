# welcomescreen.swf
> The Welcome Screen is the first major UI shown to the player on login, combining a rotating featured-content panel (store deals, events, new features), a daily bonus strip, a Message of the Day (MotD) band, and an event highlight panel. Clicking the daily bonus panel opens a full weekly-bonus detail popup.

**Document/main class:** `WelcomeScreen` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 5 (`WelcomeScreen`, `WelcomeFeaturedPanel`, `WelcomeDailyBonusPanel`, `WelcomeDailyBonusWindow`, `WelcomeMotD`) + 14 `WelcomeScreen_fla` timeline classes + ~8 asset wrappers

---

## Main class: `WelcomeScreen`

`WelcomeScreen` is the root component. On construction it registers frame scripts (frames 1 and 11, both `stop()`), hides `dailyBonusWindow` and `lightbox`, enables font-resize on the header, wires click listeners for the daily-bonus toggle (panel + close button + lightbox), adds a `MOUSE_LEAVE` listener to hide popups, registers five `ExternalInterface` callbacks (in Iggy), and configures the header title `$Welcome_WindowName`.

### Public methods

- `addFeaturedItem(idx, imagePath, title, subtitle, description, btnLabel, currencyCode, normalPrice, discountPrice) : void` — delegates to `featuredPanel.addFeaturedItem(…)`.
- `addEventItem(idx, imagePath, name, dateStr, description, …) : void` — populates the `eventPanel` MovieClip's `nameText`, `descriptionText`, `dateText` fields with layout adjustments; sets `eventPanel.art` ObjectPreview texture.
- `populateDailyBonus(todayIdx : int) : void` — stores `today`, calls `dailyBonusPanel.populateDailyBonus(today)` and `dailyBonusWindow.populateDailyBonus(today)`.
- `setMOTD(text : String) : void` — sets `MotD.text`; expands `lightbox.height` by `MotD.height` (645 console / 592 PC base + height).
- `highlightPanel(panelIdx : int) : void` — shows/hides `featuredPanel.selected` and `dailyBonusPanel.selected`; dispatches synthetic ROLL_OVER/ROLL_OUT events to drive popup visibility on each panel.
- `onDailyBonusClicked(e : MouseEvent = null) : void` — toggles `dailyBonusWindow.visible` and `lightbox.visible`; when opening, repopulates the window and pauses `featuredPanel.autoScroll`; when closing, resumes auto-scroll.
- `hidePopups(e : Event) : void` — delegates `onShowHidePopup` to both `featuredPanel` and `dailyBonusPanel`.

### Key fields

- `featuredPanel : WelcomeFeaturedPanel` — rotating featured-content area.
- `dailyBonusPanel : WelcomeDailyBonusPanel` — compact 7-day bonus strip with hover popup.
- `dailyBonusWindow : WelcomeDailyBonusWindow` — full weekly-bonus detail window (initially hidden).
- `lightbox : MovieClip` — semi-transparent overlay shown when `dailyBonusWindow` is open.
- `eventPanel : MovieClip` (instance of `EventPanel_2`) — single-event highlight area with name, date, description text fields and an `ObjectPreview` art slot.
- `header : WindowHeaderSmall` — window header; title `$Welcome_WindowName`; `allowFontResize = true`.
- `MotD : WelcomeMotD` — message-of-the-day band at the bottom.
- `today : int` — index of today's day (0 = Monday … 6 = Sunday).
- `static DAYS_S : Array` — `["MON","TUE","WED","THU","FRI","SAT","SUN"]` — shared with `WelcomeDailyBonusPanel`.

### Frame scripts / timeline

- **Frame 1** (`frame1`): `stop()`.
- **Frame 11** (`frame11`): `stop()`.

### Runtime dependencies & integration

- `ExternalInterface.addCallback("addFeaturedItem", featuredPanel.addFeaturedItem)`.
- `ExternalInterface.addCallback("addEventItem", addEventItem)`.
- `ExternalInterface.addCallback("populateDailyBonus", populateDailyBonus)`.
- `ExternalInterface.addCallback("setMOTD", setMOTD)`.
- `ExternalInterface.addCallback("changeFeatureShown", featuredPanel.changeFeatureShown)`.
- `ExternalInterface.addCallback("highlightPanel", highlightPanel)`.
- `ExternalInterface.addCallback("highlightDay", dailyBonusWindow.highlightDay)`.
- `ExternalInterface.addCallback("onActionClicked", featuredPanel.onActionClicked)`.
- `ExternalInterface.addCallback("onDailyBonusClicked", onDailyBonusClicked)`.
- Translate key: `$Welcome_WindowName` (header).

---

## Other game-specific classes

### `WelcomeFeaturedPanel` (extends `UIComponent`, embeds `assets.swf#symbol182`)
Rotating featured-content panel displaying one item at a time from `featuredItems : Vector.<Object>`. Supports 7 panel types: `panelNewPlayer`, `panelPack`, `panelRadiant`, `panelChaos`, `panelDragon`, `panelGenericA`, `panelGenericB` (constants `FEATURE_NORMAL…FEATURE_BOMBER`).

- `addFeaturedItem(…)` — pushes an object to `featuredItems`; if it's the first item, calls `showFeaturedItem()` immediately.
- `showFeaturedItem()` — fades in the correct panel type, sets `nameText`, `descriptionText`, `ObjectPreview art.textureName`; populates `featurePopup` with the action button label, description CTA text, and background/price display; calls `ExternalInterface.call("OnResetYaw", isDragon)`.
- `changeFeatureShown(delta)` — fades out current panel via `IggyTween`, then calls `showFeaturedItem` on finish; wraps `featureShown` index with modulo.
- `onActionClicked(e)` — when popup is visible, calls `ExternalInterface.call("OnShowDeals")`, `"OnShowChaosChests"`, or `"RequestBomberRoyale"` depending on feature type.
- `onShowHidePopup(e)` — `IggyTween` fades `featurePopup.alpha` on ROLL_OVER/ROLL_OUT; pauses/resumes `autoScroll` Timer.
- `autoScroll : Timer` — fires every 1 second; advances to next feature after `AUTO_SCROLL_SECONDS (6)` ticks.
- Price display: uses `KiwiTextUtil.getCurrencySymbol` and `KiwiTextUtil.formatPrice` for discount/normal price sticker.
- `arrowLeft / arrowRight : BaseButton` — left/right nav arrows.
- `featurePopup : MovieClip` — hover popup with `featureBtn (LabelButton)`, `featuredText`, `featuredTextCTA`, `background`.
- `valueSticker : MovieClip` — shows discount vs normal price.

### `WelcomeDailyBonusPanel` (extends `UIComponent`, embeds `assets.swf#symbol185`)
Compact strip showing today's bonus icon with a hover "learn more" popup.

- Seven `bonus_MON … bonus_SUN` MovieClip children; only today's is visible.
- `tomorrowBonus` child contains matching `bonus_*` children for tomorrow's preview; `dayLabel` set to `$Welcome_DailyBonus_Tomorrow`.
- `populateDailyBonus(todayIdx)` — shows correct day icon, hides others, also shows tomorrow's in `tomorrowBonus`; calls `KiwiTextUtil.resizeFont(dailyName, 20)` on each.
- `onShowHidePopup(e)` — `IggyTween` fades `learnMorePopup.alpha` on ROLL_OVER/ROLL_OUT.
- `selected : MovieClip` — selection highlight; driven from `WelcomeScreen.highlightPanel`.

### `WelcomeDailyBonusWindow` (extends `UIComponent`, embeds `assets.swf#symbol112`)
Full weekly-bonus detail popup (toggled by clicking the daily bonus panel).

- `static DAYS_L : Array` — `["Monday","Tuesday","Wednesday","Thursday","Friday","Saturday","Sunday"]`.
- `static BONUSES : Array` — `["shadow","gathering","gem","adventure","dragon","xp","loot"]`.
- Day-label TextFields on each day MovieClip are set to `$Monday` … `$Sunday` in constructor.
- `populateDailyBonus(dayIdx)` — fills four `dailyBonusDetails*` and `dailyBonusDetailsPatron*` TextFields using translate keys of the form `$Welcome_daily_bonus_details_<bonus>_1..4` and `…_patron`; repositions bullet points dynamically based on `textHeight`; hides bullet 4 if text is empty; shows the matching `bonus_*` icon.
- `highlightDay(dayIdx)` — marks the clicked day tab "over" / "up" then repopulates.
- `closeButton : WindowCloseButton` — wired by parent to `onDailyBonusClicked`.
- `dailyheader : WindowHeaderSmall` — title `$Welcome_daily_bonus_header`.

### `WelcomeMotD` (extends `UIComponent`, embeds `assets.swf#symbol206`)
Message of the Day band positioned below the main panels.

- `set text(str)` — if empty, hides `messageIcon` / `background` and resets `y = 412`; otherwise sets `textfield.text`, sizes `background`, and repositions icon/text based on `textfield.textHeight` vs `MIN_HEIGHT (54)`.
- `get height()` — returns `this.y - 412` (distance the MotD has pushed down from baseline).

### WelcomeScreen_fla timeline symbols (14 classes)

- `EventPanel_2` (embeds `assets.swf#symbol216`) — event panel MovieClip with `nameText`, `dateText`, `descriptionText` TextFields and `art : PreviewContainer`.
- `DailyBonus_Loot_22`, `DailyBonus_XP_23`, `DailyBonus_Dragon_24`, `DailyBonus_Adventure_25`, `DailyBonus_Gem_26`, `DailyBonus_Gathering_27`, `DailyBonus_Shadow_28` — per-bonus-type icon/label clips for the compact panel; each has a `dailyName` TextField.
- `DailyBonusPopup_29` — the "learn more" hover popup.
- `SelectedStroke_15` — selection highlight graphic.
- `FeaturedPanelNewFeatureA_34`, `FeaturedPanelNewFeatureB_33` — generic featured panel layouts A and B.
- `FeaturedPanelChaos_36` — Chaos Chest panel layout.
- `FeaturedPanelDragonCoin_35` — Dragon Coin panel layout.
- `FeaturedPanelPackRegular_38`, `FeaturedPanelPackStarter_39` — store pack panel layouts.
- `FeaturedPanelRadiantDayspring_37` — Radiant Dayspring panel.
- `FeaturedPopup_45` — the hovering popup that appears over the featured panel.

### Asset wrappers (no logic)

~8 classes: `PreviewContainer`, `btnGreen`, `ButtonArrowLeft`, `ButtonArrowRight`, `DailyBonus_Tomorrow_Loot/XP/Dragon/Adventure/Gem/Gathering/Shadow` (tomorrow-bonus icon clips) — all trivial MovieClip/UIComponent subclasses with no game-logic code.

---

## Notable logic

- **Auto-scroll timer**: `WelcomeFeaturedPanel.autoScroll` is a `Timer(1000)` that counts to `AUTO_SCROLL_SECONDS (6)` before calling `changeFeatureShown(1)`; it pauses on ROLL_OVER and when the daily-bonus window is open.
- **IggyTween panel cross-fades**: Feature panel transitions use `IggyTween` on the panel's `alpha` (0.2 s ease), chained: fade-out → `showFeaturedItem` callback → fade-in new panel.
- **Dynamic MotD layout**: `setMOTD` adjusts `lightbox.height` by the MotD component's `height` property, which is computed from the component's `y` offset rather than its clip height.
- **Patron vs non-patron bonus details**: `populateDailyBonus` in `WelcomeDailyBonusWindow` fills both a standard and a patron variant of each detail text field (e.g. `dailyBonusDetails1` and `dailyBonusDetailsPatron1`) from separate translate keys, allowing the window to simultaneously show both versions side-by-side.
- **Dynamic bullet positioning**: Detail text fields and bullet points are repositioned vertically based on the measured `textHeight` of the preceding text field, allowing variable-length localized strings without clipping.
