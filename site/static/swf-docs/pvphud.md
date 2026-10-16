# pvphud.swf
> The heads-up display overlay shown during PvP matches in Trove. It displays a countdown timer, team scores, a personal score panel, an engine-written counter with its person and red-cross icons, a broadcast message area, and a scrolling message queue for kill-feed or event notifications.

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
- `updateTeamScore(teamIdx:int, color:uint, score:int, maxScore:int) : void` — looks up the child `PVPScore` named `"team" + teamIdx` inside `scoreContainer` (only `team1` and `team2` exist); calls `init(maxScore, color)` then `updateScore(score)`. `init` never reads `maxScore` and applies the colour only on its first call.
- `updatePersonalScore(current:Number, max:Number) : void` — if `max == 0` shows `int(current)`, otherwise shows `"int(current)/int(max)"` in `personalScoreContainer.score`.
- `setPersonalScoreType(type:int) : void` — stores `scoreType`; no visual change beyond caching.
- `setPersonalScoreIcon(icon:int) : void` — sends `personalScoreContainer.icon` to frame `icon + 2`. The icon clip (chid 17) is blank on frame 1 (the `reset()` state), a gold coin on frame 2 and a red ring on frame 3, so `0` = coin and `1` = ring.
- `updateTick(timeStr:String, msRemaining:uint, counting:Boolean) : void` — always makes `scoreContainer` visible and writes `timeStr` to `timeRemaining.textField`. Only when the third argument is true and the text has changed with time left: a 1.5→1 scale pulse under 10 s, red `0xCC0033` under 30 s, yellow `0xFFCC00` under 60 s; an unchanged string (or zero time) resets `timeRemaining.alpha` to 1 instead.

### Key fields

- `SCORE_INTEGER : int = 0` / `SCORE_TIME : int = 1` — score-mode constants.
- `timeRemaining : MovieClip` — timer display; child `textField` receives the formatted time string.
- `scoreContainer : MovieClip` — holds two `PVPScore` children named `"team1"` and `"team2"`.
- `broadcastContainer : MovieClip` — center-screen message area; child `broadcastText` (`TextField`) shows countdown/phase messages.
- `personalScoreContainer : MovieClip` — holds `icon` (MovieClip, multi-frame) and `score` (TextField); its own frames are labelled `coin` and `rings`, and it stays on `coin`.
- `teammateDeadIcon : MovieClip` — a red-cross bitmap (chid 27); shown/hidden by the engine, which sets it by name (reset hides it).
- `killCounterIcon : MovieClip` — a person-figure bitmap (chid 24) beside `counterText`; shown/hidden by the engine, which sets it by name (reset hides it).
- `counterText : TextField` — numeric count text written by the engine by name; the SWF only clears it in `reset()`.
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
- **Timer color escalation** — `updateTick` applies three discrete thresholds: white (> 60 s, set by `reset()`), `0xFFCC00` yellow (< 60 s), `0xCC0033` red (< 30 s), then a scale pulse on every change under 10 s (the red stays).
- **Score color packing** — team color arrives as RGBA in one uint and is shifted right 8 bits before being applied to `GlowFilter.color` and `textField.textColor`, stripping the low alpha byte.
- **Queue bypass** — `addImmediateMessage` clears the queue and writes directly to the textfield, useful for critical single-line notifications that should not be delayed by queued messages.
