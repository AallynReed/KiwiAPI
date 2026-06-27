# lockbox.swf
> The Loot Box (lock box) opening window in Trove. Displays the current box item, unlock buttons (golden key and free/Flux), a karma progress meter with particle explosion effects, the resulting reward item, and an auto-open checkbox. Appears whenever the player interacts with an openable loot box.

**Document/main class:** `LockBox` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 6 (plus 4 LockBox_fla timeline symbols and several asset/rarity wrappers)

---

## Main class: `LockBox`

`LockBox` is the root component. The constructor wires up frame scripts for frames 0 and 10, forces `btn_unlock_key` visible, clears result display, creates and positions an `ExplodingProgressBar` over the karma meter area, hides `karmaMeter`, and initialises component inspector properties via five `__setProp_*` helpers. `configUI()` configures the shine timer (10 s interval), highlights the box-name text field, and registers all `ExternalInterface` callbacks; on non-Iggy preview it calls `setLockBoxKarma(69,70)` and wires a test unlock. `draw()` responds to `STATE` invalidation by enabling/disabling `btn_unlock_free` once any pending reward timer has finished.

### Public methods

- `set canOpen(value:Boolean) : void` — stores flag and invalidates `STATE`, which re-evaluates `btn_unlock_free.enabled` in `draw()`.
- `clearResults() : void` — hides `resultDisplay`, blanks result text and sets slot data to -1 with the "brown" frame.

### Key fields

- `btn_unlock_key : MovieClip` — golden key unlock button; click calls `OnUpgradeKey` (sound: `Play_ui_window_click_button`).
- `btn_unlock_free : MovieClip` — free (or Flux) unlock button; click calls `OnUpgradeFree`; enabled only when `canOpen && !waitOnTimer`.
- `winHeader : WindowHeaderSmall` — title bar; `allowFontResize = true`; title key `"$Lockbox_Header"`.
- `m_boxName : TextField` — box name label; highlighted via `HighlightUtil.highlightMovieClip`.
- `slot_currentItem : Slot` — displays the box being opened; quantity badge shows `itemsAvailable`; activates slot 1 on update.
- `resultDisplay : MovieClip` — contains `slot_resultItem` (Slot) and `txt_resultDetails` (TextField); shown after unlock resolves.
- `frame : MovieClip` — decorative frame; plays "shine" animation on a 10 s timer.
- `karmaMeter : MovieClip` — contains `karmaMask`; masked by itself; visible only when max karma > 0. Tooltip keys: `"$LockboxTooltip_KarmaDesc"` / `"$GemTooltip_Karma_Bar_Title"`.
- `progressBar : ExplodingProgressBar` — particle explosion overlay on the karma bar; positioned at `(72, 540)`.
- `m_autoOpenCheckBox : Checkbox` — label `"$Lootbox_AutoOpen"`; label cleared to `""` on console.
- `buttonLegend : MovieClip` — console button hint; navigated to `"KeyOpen"` frame via `gotoKeyOpenButtonLegend`.
- `shineTimer : Timer` — 10 000 ms, 1 repeat; triggers frame "shine" animation loop.
- `rewardTimer : Timer` — 400 ms (Uncommon) or 1000 ms (Rare) one-shot; delays re-enabling unlock buttons after a reward animation.
- `waitOnTimer : Boolean` — blocks `btn_unlock_free` re-enable until `rewardTimer` fires.
- `itemsAvailable : int` — quantity of boxes remaining; shown on `slot_currentItem.quantityBadge`.
- `_canOpen : Boolean` — backing field for `canOpen` setter.

### Frame scripts / timeline

- **Frame 0 (`frame1`)** — `stop()`. PC layout.
- **Frame 10 (`frame11`)** — `stop()`. Console layout (child clips expected to handle console display internally).

### Private methods of note

- `onGoldenKeyUnlock(e:MouseEvent)` — disables both buttons, updates quantity badge, fires `POST_SOUND_EVENT("Play_ui_window_click_button")` and `OnUpgradeKey`.
- `onFreeUnlock(e:MouseEvent)` — same flow, fires `OnUpgradeFree`.
- `showResultItem(icon, name, rarity, qty, showQty, rarityId, color, quality)` — configures `resultDisplay.slot_resultItem` (size 70, rarity, quantity, quality); translates rarity string to a `$LockBox_*` key; formats `txt_resultDetails` as HTML with colored item name; for rarity ID 6 (presumably Stellar/Rainbow) calls `rainbowify()`; sets result Y based on text height; fires per-rarity sound event; starts `rewardTimer` for Uncommon/Rare.
- `setLockBoxItem(icon, count)` — sets `slot_currentItem.iconImage`, shows/hides quantity badge, sets size to 55, calls `ExternalInterface.call("SLOT.ACTIVATE", 1)`.
- `setLockBoxKarma(current, max)` — shows/hides meter and repositions `resultDisplay.y` (504 if meter shown, 522 if not); at `current == max-1` calls `progressBar.initParticles("KarmaParticle", 3)`; at rollover (current==0, meter shrinking) calls `progressBar.start()`; sets `karmaMask.scaleX = current/max`.
- `recheckUnlockOptions()` — calls `RequestApplyUnlockOptions` to let the game re-evaluate which buttons should be enabled.
- `shine(e:TimerEvent)` — plays "shine" on `frame` MovieClip, resets and restarts timer.
- `gotoKeyOpenButtonLegend()` — navigates `buttonLegend` to "KeyOpen" stop frame.
- `rainbowify(field, start, end)` — applies a 6-color rainbow `TextFormat` cycle (red→orange→yellow→green→cyan→lavender) to a character range in a TextField. Used for Stellar/special rarity result text.

### Runtime dependencies & integration

- **Iggy callbacks registered:** `setLockBoxItem`, `showResultItem`, `setLockBoxKarma`, `onGoldenKeyUnlock`, `onFreeUnlock`, `gotoKeyOpenButtonLegend`.
- **ExternalInterface calls out:** `POST_SOUND_EVENT` (with event names `Play_ui_window_click_button`, `Play_ui_lockbox_common`, `Play_ui_lockbox_uncommon`, `Play_ui_lockbox_rare`), `OnUpgradeKey`, `OnUpgradeFree`, `RequestApplyUnlockOptions`, `SLOT.ACTIVATE`.
- **IggyTween:** not used directly in LockBox; `ExplodingProgressBar` uses the flint particle library instead.
- **translate keys:** `"$Lockbox_Header"` (winHeader title), `"$Lootbox_AutoOpen"` (checkbox), `"$Lockbox_OpenButton"` (free button label), `"$LockBox_Common"`, `"$LockBox_Uncommon"`, `"$LockBox_Rare"`, `"$LockBox_Got"`, `"$LockboxTooltip_KarmaDesc"`, `"$GemTooltip_Karma_Bar_Title"`.
- **org.flintparticles** library used by `ExplodingProgressBar` for karma-bar explosion effect.
- `IsConsole()` checked to clear auto-open label text.

---

## Other game-specific classes

- `ExplodingProgressBar` — standalone `MovieClip` that wraps the flint `Emitter2D`/`DisplayObjectRenderer` pipeline. Takes progress bar dimensions in the constructor; `initParticles(prefix, count)` instantiates particle MovieClips by name (`KarmaParticle0`–`KarmaParticle2`) via `getDefinitionByName`, tiles them across the bar width, and pre-builds a cascade of `GravityWell` + `Explosion` + `LinearDrag` + `DeathZone` actions. `start()` makes the renderer visible, starts the emitter, and fires the explosion timer (500ms / NUM_EXPLOSIONS intervals, 20 explosions total).

### LockBox_fla timeline symbols

- `LockBox_fla/frame_1` — Embed symbol120; decorative frame clip; stops at frame 78 (end of shine animation).
- `LockBox_fla/slotFrame_5` — Embed symbol86; two-frame slot border clip (states 1/2).
- `LockBox_fla/equipped_7` — Embed symbol90; two-frame equipped indicator clip.
- `LockBox_fla/qualityPips_13` — Embed symbol104; quality pip display; stops on frame 1.
- `LockBox_fla/buttonLegend_24` — Embed symbol137; console button legend; exposes `button_y` (`btn_console_north`), `button_console_east`, `button_a` (`btn_console_south`); two stops (frame 0 PC, frame 10 console layout).

### Particle assets (game-specific, top-level)

- `KarmaParticle0`, `KarmaParticle1`, `KarmaParticle2` — three particle sprite MovieClips used by `ExplodingProgressBar.initParticles`.
- `meter_karma`, `slot_large`, `btnKey`, `btnGreen` — embedded symbol wrappers for UI elements.
- `btn_console_east`, `btn_console_north`, `btn_console_south` — console button prompt symbols.

### Asset wrappers (rarity frames — not individually documented)

18 rarity frame symbols (`rarity_frame_common_png`, `rarity_frame_uncommon_png`, `rarity_frame_rare_png`, `rarity_frame_epic_png`, `rarity_frame_legendary_png`, `rarity_frame_relic_png`, `rarity_frame_shadow_png`, `rarity_frame_radiant1_png`, `rarity_frame_resplendent_png`, `rarity_frame_stellar`, plus `_over` variants) — pure bitmap/shape wrappers, not individually documented.

---

## Notable logic

- **Karma rollover detection:** `setLockBoxKarma` checks whether the meter is shrinking (`current == 0 && scaleX > 0 && scaleX < 1`) to fire the particle explosion; this handles the karma bar resetting at the start of a new karma tier.
- **Reward timer gating:** `waitOnTimer` prevents the unlock buttons from being prematurely re-enabled during a rarity animation (400 ms for Uncommon, 1 000 ms for Rare). `recheckUnlockOptions` is called both from `Common` rarity handling (immediate) and from the timer callback.
- **Two-step unlock button label:** `btn_unlock_free` label is `"$Lockbox_OpenButton"` (free unlock); `btn_unlock_key` has no label — it is icon-only (key image embedded in the MovieClip skin).
- **Rainbow rarity (ID 6):** Stellar or equivalent rarity applies alternating TextFormat color segments in a 6-step rainbow cycle to the item name portion of `txt_resultDetails`.
