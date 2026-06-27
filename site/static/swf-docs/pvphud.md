# pvphud.swf
> The heads-up display overlay shown during PvP matches in Trove. It displays a countdown timer, team scores, a personal score panel, kill-counter icons, a broadcast message area, and a scrolling message queue for kill-feed or event notifications.

**Document/main class:** `PVPHUD` (extends `_kiwi.Core.UIComponent`)
**SWF-specific classes:** 5 (excluding framework; 2 are `PVPHUD_fla` timeline symbols)

---

## Main class: `PVPHUD`

`PVPHUD` extends `UIComponent` and acts as the top-level controller for the PvP HUD. Its constructor calls `reset()` immediately. All Iggy callbacks are registered inside `configUI()` (framework lifecycle, called on first render) rather than the constructor, ensuring the stage is available before callbacks are wired.

The class manages two score display modes — `SCORE_INTEGER` (0) and `SCORE_TIME` (1) — via `scoreType` and `scoreIcon` state fields, though the mode is stored but not yet visually distinguished beyond icon frame selection.

### Public methods

- `reset() : void` — clears all text fields, resets `timeRemaining` color to white (0xFFFFFF), hides `scoreContainer`, `teammateDeadIcon`, `killCounterIcon`; resets `scoreType` and `scoreIcon` to −1.
- `setMessage(text:String) : void` — sets `broadcastContainer.broadcastText.htmlText` with full alpha.
- `addImmediateMessage(text:String) : void` — clears the message queue and directly sets `messageQueue.textfield.htmlText`; bypasses queue ordering.
- `updateCountdown(seconds:int) : void` — displays the countdown integer in `broadcastText`, then launches three `IggyTween`s on `broadcastContainer` (scaleX, scaleY grow from `2+(10−n)/2`→1; alpha fade 1→0 over 1 s); also calls `messageQueue.reset()`. Scale start size varies with remaining time, making later countdown ticks appear larger.
- `prepareForBattle() : void` — clears the timer text and calls `setMessage(IggyFunctions.translate("$PVP_Prepare"))`.
- `beginMatch() : void` — calls `setMessage(IggyFunctions.translate("$PVP_Begin"))`, fades `broadcastContainer` alpha to 0 over 2 s via `IggyTween`, resets message queue.
- `updateTeamScore(teamIdx:int, color:uint, score:int, maxScore:int) : void` — looks up the child `PVPScore` named `"team" + teamIdx` inside `scoreContainer`; calls `init(maxScore, color)` then `updateScore(score)`.
- `updatePersonalScore(current:Number, max:Number) : void` — if `max == 0` shows `int(current)`, otherwise shows `"int(current)/int(max)"` in `personalScoreContainer.score`.
- `setPersonalScoreType(type:int) : void` — stores `scoreType`; no visual change beyond caching.
- `setPersonalScoreIcon(icon:int) : void` — sends `personalScoreContainer.icon` to frame `icon + 2` (1-indexed offset).
- `updateTick(timeStr:String, msRemaining:uint, showScore:Boolean) : void` — makes `scoreContainer` visible when `showScore` is true; updates `timeRemaining.textField.text`; applies pulsing scale tweens when `msRemaining < 10 000` (under 10 s) or color changes for < 30 s (0xCC_CC_33 yellowish) and < 60 s (0xFF_A5_00 orange-ish).

### Key fields

- `SCORE_INTEGER : int = 0` / `SCORE_TIME : int = 1` — score-mode constants.
- `timeRemaining : MovieClip` — timer display; child `textField` receives the formatted time string.
- `scoreContainer : MovieClip` — holds two `PVPScore` children named `"team0"` and `"team1"`.
- `broadcastContainer : MovieClip` — center-screen message area; child `broadcastText` (`TextField`) shows countdown/phase messages.
- `personalScoreContainer : MovieClip` — holds `icon` (MovieClip, multi-frame) and `score` (TextField).
- `teammateDeadIcon : MovieClip` — icon indicating a dead teammate; shown/hidden by game calls (reset hides it).
- `killCounterIcon : MovieClip` — icon for kill counter; shown/hidden by game (reset hides it).
- `counterText : TextField` — numeric text for the kill counter.
- `messageQueue : MessageQueue` — framework scrolling message queue for kill-feed entries.
- `dropShadowFilter : DropShadowFilter` — shared drop-shadow (offset 2, 45°, low quality) applied to `timeRemaining`.

### Runtime dependencies & integration

**ExternalInterface callbacks registered (Iggy → Flash):**
| Callback | Method |
|---|---|
| `reset` | `reset` |
| `setMessage` | `setMessage` |
| `updateCountdown` | `updateCountdown` |
| `prepareForBattle` | `prepareForBattle` |
| `beginMatch` | `beginMatch` |
| `updateTick` | `updateTick` |
| `updateTeamScore` | `updateTeamScore` |
| `updatePersonalScore` | `updatePersonalScore` |
| `setPersonalScoreType` | `setPersonalScoreType` |
| `setPersonalScoreIcon` | `setPersonalScoreIcon` |
| `addMessage` | `messageQueue.addMessage` (delegated directly) |
| `addImmediateMessage` | `addImmediateMessage` |

**IggyFunctions usage:**
- `IggyFunctions.translate("$PVP_Prepare")` — localisation key for pre-battle message.
- `IggyFunctions.translate("$PVP_Begin")` — localisation key for match-start message.
- `IggyFunctions.inIggy` — gates all callback registration.

**IggyTween usage:**
- Countdown scale pulse on `broadcastContainer` (scaleX, scaleY, alpha).
- Per-tick pulse on `timeRemaining` (scaleX, scaleY) when under 10 s.
- Match-start alpha fade on `broadcastContainer`.

---

## Other game-specific classes

- `PVPScore` — `[Embed symbol="symbol7"]` MovieClip; displays a single team's score with animated scale-bounce and glow-filter flash on score change (Strong easeIn out, Elastic easeOut back). Holds `scalar` (MovieClip with `textField`) and applies `GlowFilter`/`DropShadowFilter` per frame via `ENTER_FRAME`. Color is set from the `color` argument shifted right 8 bits (`>> 8`).
- `messageQueue` — `[Embed symbol="symbol3"]` thin wrapper extending `_kiwi.Controls.MessageQueue`; no added logic.

**PVPHUD_fla timeline symbols:**
- `PVPHUD_fla.minigameIcon_7` — `[Embed symbol="symbol17"]` multi-frame icon MovieClip; frame 1 stops.
- `PVPHUD_fla.miniGameScores_6` — `[Embed symbol="symbol20"]` personal-score container; holds `icon` (MovieClip) and `score` (TextField); frame 1 stops.

---

## Notable logic

- **Countdown scale sizing** — `updateCountdown` derives the tween start scale as `2 + (10 − n) / 2`, so the number "10" appears at scale 2 and the number "1" appears at scale 6.5, creating an urgency ramp.
- **Timer color escalation** — `updateTick` applies three discrete color thresholds: white (> 60 s), 0xFF_A5_00 orange (< 60 s), 0xCC_CC_33 yellow (< 30 s), then scale pulse (< 10 s).
- **Score color packing** — team color is passed as a packed uint shifted right 8 bits before being applied to `GlowFilter.color` and `textField.textColor`, stripping the alpha byte.
- **Queue bypass** — `addImmediateMessage` clears the queue and writes directly to the textfield, useful for critical single-line notifications that should not be delayed by queued messages.
