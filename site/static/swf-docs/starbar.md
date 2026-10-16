# starbar.swf
> The Star Bar is a persistent HUD element that displays up to three stacked progress bars (the once-a-day Star Bar, the Tome Bar and the Auto-Use Bar) plus up to two Reliquary "mega" items. It supports two display modes — Normal (in the world) and Discovery/Geode (in the Geode caves) — reports its height so the engine can stack the quest tracker below it, and plays its own reward animation and sound when a star bar fills.

**Document/main class:** `StarBar` (extends `UIComponent`)
**SWF-specific classes:** 12

## Main class: `StarBar`

`StarBar` extends `_kiwi.Core.UIComponent` and is the root document class. The constructor initialises six progress-bar references into two parallel arrays (`normalProgressBars` for Trove and `discoveryProgressBars` for Geode mode), snapshots each bar's initial Y position for later layout reflow, and sets localisation-key tooltip strings (`UITooltipTitle` / `UITooltipDescription`) on the three normal bars. It then registers Iggy callbacks (when running inside the game engine) or populates mock test data (when running outside Iggy).

`addFrameScript(0, frame1)` stops the timeline on frame 1 (label `PC`, idle). Frame 11 carries the root's `Console` label, which `UIComponent`'s constructor jumps to when `IsConsole()` (Xbox One / PS4 platform); `frame11` stops there and switches both `starProgressBar` and `tomeProgressBar` to their own `Console` labels.

Data arrives from the engine through the nine `ExternalInterface` callbacks and through direct property writes on the two reliquary items (`mega0`, `mega1`). `StarBar` itself makes one outbound call (`NOTIFY_RESIZED`) when the total bar height changes; the tooltip and reward-sound calls come from the bars.

### Public methods
- `setAutoUseIcon(textureName:String) : void` — sets the texture on the two auto-use `ObjectPreview` icons and adds each to its bar's `barIcon.autoUseDisplay`, sized 40×40 (normal) / 30×30 (geode).
- `setNewProgressPercent(barIndex:int, percent:Number) : void` — sets the total progress fraction (0–1, clamped: the settled progress plus what has just been earned, not the increment) on bar `barIndex` in both normal and geode arrays. Reaching 1 with no animation pending also sets the filled fraction to 1; a star bar then plays its reward animation and sound, while the tome and auto-use bars show `completeMC`.
- `setFilledPercent(barIndex:int, percent:Number) : void` — sets the settled (baseline) fraction, 0–1 clamped, for bar `barIndex` in both arrays.
- `setRewardAmount(barIndex:int, amount:int) : void` — updates the numeric reward label on bar `barIndex` in both arrays.
- `setRewardImage(barIndex:int, imageName:String) : void` — sets the reward icon texture on bar `barIndex` in both arrays.
- `setDisplayMode(mode:int) : *` — sets `m_mode` (0 = Normal, in the world; 1 = Discovery/Geode, in the Geode caves). Visibility of individual bars is governed by subsequent `setVisible` calls; `UpdateHeight` must be called to reflow.
- `UpdateHeight() : *` — repositions visible bars by stacking them using their saved Y positions array; repositions mega bars below; recalculates total component height (`48 + visibleCount * 40 + (megaVisible ? 88 : 0)`) and calls `ExternalInterface.call("NOTIFY_RESIZED", 374, this.height)` when height changes. `UIComponent.height` is a stored size, so the reported height is this formula, not the clip's drawn bounds.
- `setMegaVisible(index:int, visible:Boolean) : void` — shows or hides a `ReliquaryItem` mega slot and, only when that changes its visibility, triggers `UpdateHeight`.
- `setVisible(barIndex:int, visible:Boolean) : void` — shows or hides bar `barIndex` in the active mode's array (only the active mode's bar is shown), then triggers `UpdateHeight` on every call.
- `isWaitingOnAnimation(barIndex:int) : Boolean` — returns whether bar `barIndex` in the active mode is blocked on a completion animation, so the engine can defer further progress updates. Only the star bars have a `rewardAnimation`, so this is never true for indices 1 and 2; it returns false for any mode other than 0 or 1.

### Key fields
- `normalProgressBars : Array` — `[starProgressBar, tomeProgressBar, autoUseTomeProgressBar]`; used when `m_mode == 0`.
- `discoveryProgressBars : Array` — `[geodeStarProgressBar, geodeTomeProgressBar, geodeAutoUseTomeProgressBar]`; used when `m_mode == 1`.
- `megaProgressBars : Array` — `[mega0, mega1]`; `ReliquaryItem` instances shown below the stacked bars.
- `normalProgressBarPositionsY : Array` — snapshot of each normal bar's initial Y, used for layout reflow.
- `discoveryProgressBarPositionsY : Array` — snapshot of each geode bar's initial Y.
- `megaProgressBarPositionY : int` — baseline Y for both mega slots.
- `m_mode : int` — 0 = Normal, 1 = Discovery/Geode; gates which bar array is visible.
- `autoUseIcon : ObjectPreview` — 30×30 icon for the normal auto-use bar.
- `geodeAutoUseIcon : ObjectPreview` — 20×20 icon for the geode auto-use bar.
- `starProgressBar`, `tomeProgressBar`, `autoUseTomeProgressBar` : `UIComponent` — the three normal-mode bar instances on the timeline; tooltip keys `$StarBar_Tooltip_*`, `$TomeBar_Tooltip_*`, `$AutoUseBar_Tooltip_*` are set in the constructor.
- `geodeStarProgressBar`, `geodeTomeProgressBar`, `geodeAutoUseTomeProgressBar` : `UIComponent` — geode-mode bar instances.
- `mega0`, `mega1` : `ReliquaryItem` — mega reward display slots; the engine writes their `fill` and `iconImage` directly by instance name.

### Frame scripts / timeline
- `frame1()` — `stop()`. Default idle state.
- `frame11()` — `stop()` then calls `gotoAndPlay("Console")` on both `starProgressBar` and `tomeProgressBar` to activate their console visual variants.

### Runtime dependencies & integration
**Iggy callbacks registered (engine → Flash):**
- `setNewProgressPercent(barIndex, percent)`
- `setFilledPercent(barIndex, percent)`
- `setRewardAmount(barIndex, amount)`
- `setRewardImage(barIndex, imageName)`
- `isWaitingOnAnimation(barIndex)` → Boolean
- `setVisible(barIndex, visible)`
- `setDisplayMode(mode)`
- `setMegaVisible(index, visible)`
- `setAutoUseIcon(textureName)`

`barIndex` is 0 = star, 1 = tome, 2 = auto-use tome (the order of `normalProgressBars` / `discoveryProgressBars`); the bars are addressed by nothing else. `setMegaVisible`'s `index` selects `mega0` / `mega1`.

**Engine property writes (no callback):** the engine sets `fill` (0–1) and `iconImage` (texture) directly on the root's `mega0` and `mega1` `ReliquaryItem` instances, and keeps rewriting `fill` as reliquary progress changes. `setMegaVisible` only shows and hides them.

**ExternalInterface outbound calls (Flash → engine):**
- `"NOTIFY_RESIZED"` with fixed width `374` and computed height — called from `UpdateHeight` whenever the bar stack height changes. The engine treats the reported height as the strip's footprint: `questtracker.swf` is stacked directly below it, and the SWF is not scaled to the reported size.
- `"UIComponent.OnShowTooltip"(stageX, stageY, title, description)` / `"UIComponent.OnHideTooltip"` — the keyed tooltip comes from `UIComponent`'s `ROLL_OVER` / `ROLL_OUT` handlers, which the `UITooltipTitle` setter attaches, so only the three normal bars send `$…_Tooltip_*` keys. `ProgressBar` also sends the pair from its own `MOUSE_OVER` / `ROLL_OUT` / `MOUSE_LEAVE` listeners, passing `tooltipName` / `tooltipDescription`, which nothing in this SWF sets.
- `"POST_SOUND_EVENT", "Play_ui_starbar_reward"` — called by `ProgressBar` when a bar reaches 100% and its reward animation begins; only the two star bars have one.

**Tooltip localisation keys:** `$StarBar_Tooltip_Title`, `$StarBar_Tooltip_Description`, `$TomeBar_Tooltip_Title`, `$TomeBar_Tooltip_Description`, `$AutoUseBar_Tooltip_Title`, `$AutoUseBar_Tooltip_Description`. English titles are "Star Bar", "Tome Bar" and "Auto-Use Bar"; the descriptions say the Star Bar can be filled once per day for Cubits and Flux, with stars earned in dungeons, and that the Tome and Auto-Use bars charge in Dungeons, Geode Caves and Delves.

## Other game-specific classes

- `ProgressBar` — base `UIComponent` for all six progress bars. Manages `filledPercent` (baseline fill), `newProgressPercent` (target fill including new progress), `rewardAmount` (numeric label via `amountTextField`), `rewardImage` (icon via `ArtClip iconMC`), and `waitingOnAnimation` flag; sets `mouseChildren = false`. The two percent setters clamp to 0–1 and invalidate only on a change. On `draw()` scales `fillingMaskMC` and `newProgMaskMC` widths proportionally and moves a `highlight` sprite (geode bars only) to the leading edge. When `newProgressPercent` reaches 1 and no animation is pending, plays the embedded `rewardAnimation` MovieClip and fires `POST_SOUND_EVENT` — only the two star bars have one. Shows/hides a `completeMC` overlay at 100% (tome and auto-use bars only; the star bars have none).
- `StarProgressBar` — extends `ProgressBar`; embeds symbol67; two frame stops (PC / Console).
- `TomeProgressBar` — extends `ProgressBar`; embeds symbol65; two frame stops.
- `AutoUseTomeProgressBar` — extends `ProgressBar`; embeds symbol63; two frame stops.
- `StarProgressBarGeode` — extends `ProgressBar`; embeds symbol51; two frame stops (Geode variant of star bar).
- `TomeProgressBarGeode` — extends `ProgressBar`; embeds symbol28; two frame stops.
- `AutoUseTomeProgressBarGeode` — extends `ProgressBar`; embeds symbol24; two frame stops.
- `ReliquaryItem` — extends `MovieClip`; embeds symbol81. Represents a mega Reliquary reward slot with an `ArtClip image`, an `xpBar` fill strip scaled via `scaleX`, a `completeMC` overlay shown at fill ≥ 1. Exposes `fill` (Number 0–1) and `iconImage` (String texture name) setters, which the engine writes directly; each applies only on a change, and `iconImage` sizes the image to 63×63.
- `EventHighlight` — extends `UIComponent`; embeds symbol6. Displays an event description label with an auto-sizing text field and a background scaled to fit. On console `htmlText` is used; on PC `text`. Calls `fitBackgroundToText()` to resize the background MovieClip to match text height.
- `StarBar_fla/reward_animation_13` — extends `MovieClip`; embeds symbol47. The 47-frame reward burst animation. Stops at frame 47 via `halt()`. Placed as the `rewardAnimation` child in `StarProgressBar` and `StarProgressBarGeode` only.

## Notable logic
- **Dual-mode symmetry**: every data setter (`setNewProgressPercent`, `setFilledPercent`, `setRewardAmount`, `setRewardImage`) unconditionally updates both the normal and geode bar at the given index. Visibility is then controlled separately by `setVisible` + `m_mode`, so the engine can switch modes without resending all data. The geode bars have no `amountTextField` or `iconMC`, so the reward count and image appear only on the normal bars.
- **Dynamic height reflow**: `UpdateHeight` is the only mechanism that resizes the component, and it runs only from `setVisible` (every call) and `setMegaVisible` (on an actual change); the progress, reward and mode setters never reflow. It packs visible bars contiguously using pre-captured Y positions, then places both mega slots at the same Y regardless of individual visibility, and broadcasts the new pixel height to the engine via `NOTIFY_RESIZED` (fixed width 374). The formula `48 + n * 40 + (megaVisible ? 88 : 0)` encodes the bar row height (40 px) and mega section height (88 px).
- **Animation gating**: `isWaitingOnAnimation` lets the engine pause further `setNewProgressPercent` calls while the reward animation is playing, preventing visual glitches from overlapping fills. `ProgressBar` clears `waitingOnAnimation` and resets `filledPercent` to 0 in the last-frame script of `rewardAnimation`. Only the star bars carry a `rewardAnimation`; at 100% the tome and auto-use bars just show `completeMC` and never set `waitingOnAnimation`.
- **Sound**: reward sound `"Play_ui_starbar_reward"` is triggered from within `ProgressBar.set newProgressPercent` via `POST_SOUND_EVENT`, not from `StarBar` itself, and only for the star bars.
