# starbar.swf
> The Star Bar is a persistent HUD element that displays up to three stacked progress bars tracking the player's daily/weekly earning progress (Stars, Tome XP, and Auto-Use Tome XP) plus up to two Reliquary "mega" reward slots. It supports two display modes — Normal (Trove dimension) and Discovery/Geode — and notifies the engine when bars complete so rewards can be animated and sounds played.

**Document/main class:** `StarBar` (extends `UIComponent`)
**SWF-specific classes:** 12

## Main class: `StarBar`

`StarBar` extends `_kiwi.Core.UIComponent` and is the root document class. The constructor initialises six progress-bar references into two parallel arrays (`normalProgressBars` for Trove and `discoveryProgressBars` for Geode mode), snapshots each bar's initial Y position for later layout reflow, and sets localisation-key tooltip strings on the three normal bars. It then registers Iggy callbacks (when running inside the game engine) or populates mock test data (when running outside Iggy).

`addFrameScript(0, frame1)` stops the timeline on frame 1 (idle). Frame 11 (`frame11`) stops the timeline and switches both `starProgressBar` and `tomeProgressBar` to their "Console" sub-label, indicating a console-specific appearance variant.

Data flow is entirely inbound from the engine via `ExternalInterface` callbacks, with one outbound call (`NOTIFY_RESIZED`) when the total bar height changes.

### Public methods
- `setAutoUseIcon(textureName:String) : void` — loads a texture into the auto-use icon `ObjectPreview` instances and sizes their containers; applied to both normal and geode auto-use bars.
- `setNewProgressPercent(barIndex:int, percent:Number) : void` — sets the "new total" progress fraction on bar `barIndex` in both normal and geode arrays, which may trigger a fill animation/reward if percent reaches 1.
- `setFilledPercent(barIndex:int, percent:Number) : void` — sets the "already filled" (baseline) fraction for bar `barIndex` in both arrays.
- `setRewardAmount(barIndex:int, amount:int) : void` — updates the numeric reward label on bar `barIndex` in both arrays.
- `setRewardImage(barIndex:int, imageName:String) : void` — sets the reward icon texture on bar `barIndex` in both arrays.
- `setDisplayMode(mode:int) : *` — sets `m_mode` (0 = Normal, 1 = Discovery/Geode). Visibility of individual bars is governed by subsequent `setVisible` calls; `UpdateHeight` must be called to reflow.
- `UpdateHeight() : *` — repositions visible bars by stacking them using their saved Y positions array; repositions mega bars below; recalculates total component height (`48 + visibleCount * 40 + (megaVisible ? 88 : 0)`) and calls `ExternalInterface.call("NOTIFY_RESIZED", 374, this.height)` when height changes.
- `setMegaVisible(index:int, visible:Boolean) : void` — shows or hides a `ReliquaryItem` mega slot and triggers `UpdateHeight`.
- `setVisible(barIndex:int, visible:Boolean) : void` — shows or hides bar `barIndex` in the active mode's array (only the active mode's bar is shown), then triggers `UpdateHeight`.
- `isWaitingOnAnimation(barIndex:int) : Boolean` — returns whether bar `barIndex` in the active mode is blocked on a completion animation, so the engine can defer further progress updates.

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
- `mega0`, `mega1` : `ReliquaryItem` — mega reward display slots.

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

**ExternalInterface outbound calls (Flash → engine):**
- `"NOTIFY_RESIZED"` with fixed width `374` and computed height — called from `UpdateHeight` whenever the bar stack height changes.
- `"UIComponent.OnShowTooltip"` / `"UIComponent.OnHideTooltip"` — called by `ProgressBar` on hover/rollout.
- `"POST_SOUND_EVENT", "Play_ui_starbar_reward"` — called by `ProgressBar` when a bar reaches 100% and its reward animation begins.

**Tooltip localisation keys:** `$StarBar_Tooltip_Title`, `$StarBar_Tooltip_Description`, `$TomeBar_Tooltip_Title`, `$TomeBar_Tooltip_Description`, `$AutoUseBar_Tooltip_Title`, `$AutoUseBar_Tooltip_Description`.

## Other game-specific classes

- `ProgressBar` — base `UIComponent` for all six progress bars. Manages `filledPercent` (baseline fill), `newProgressPercent` (target fill including new progress), `rewardAmount` (numeric label via `amountTextField`), `rewardImage` (icon via `ArtClip iconMC`), and `waitingOnAnimation` flag. On `draw()` scales `fillingMaskMC` and `newProgMaskMC` widths proportionally and moves a `highlight` sprite to the leading edge. When `newProgressPercent` reaches 1 and no animation is pending, plays the embedded `rewardAnimation` MovieClip and fires `POST_SOUND_EVENT`. Shows/hides a `completeMC` overlay at 100%.
- `StarProgressBar` — extends `ProgressBar`; embeds symbol67; two frame stops (PC / Console).
- `TomeProgressBar` — extends `ProgressBar`; embeds symbol65; two frame stops.
- `AutoUseTomeProgressBar` — extends `ProgressBar`; embeds symbol63; two frame stops.
- `StarProgressBarGeode` — extends `ProgressBar`; embeds symbol51; two frame stops (Geode variant of star bar).
- `TomeProgressBarGeode` — extends `ProgressBar`; embeds symbol28; two frame stops.
- `AutoUseTomeProgressBarGeode` — extends `ProgressBar`; embeds symbol24; two frame stops.
- `ReliquaryItem` — extends `MovieClip`; embeds symbol81. Represents a mega Reliquary reward slot with an `ArtClip image`, an `xpBar` fill strip scaled via `scaleX`, a `completeMC` overlay shown at fill ≥ 1. Exposes `fill` (Number 0–1) and `iconImage` (String texture name) setters.
- `EventHighlight` — extends `UIComponent`; embeds symbol6. Displays an event description label with an auto-sizing text field and a background scaled to fit. On console `htmlText` is used; on PC `text`. Calls `fitBackgroundToText()` to resize the background MovieClip to match text height.
- `StarBar_fla/reward_animation_13` — extends `MovieClip`; embeds symbol47. The 47-frame reward burst animation. Stops at frame 47 via `halt()`. Used as the `rewardAnimation` child inside `ProgressBar` subclasses.

## Notable logic
- **Dual-mode symmetry**: every data setter (`setNewProgressPercent`, `setFilledPercent`, `setRewardAmount`, `setRewardImage`) unconditionally updates both the normal and geode bar at the given index. Visibility is then controlled separately by `setVisible` + `m_mode`, so the engine can switch modes without resending all data.
- **Dynamic height reflow**: `UpdateHeight` is the only mechanism that resizes the component. It packs visible bars contiguously using pre-captured Y positions, then places both mega slots at the same Y regardless of individual visibility, and broadcasts the new pixel height to the engine via `NOTIFY_RESIZED` (fixed width 374). The formula `48 + n * 40 + (megaVisible ? 88 : 0)` encodes the bar row height (40 px) and mega section height (88 px).
- **Animation gating**: `isWaitingOnAnimation` lets the engine pause further `setNewProgressPercent` calls while the reward animation is playing, preventing visual glitches from overlapping fills. `ProgressBar` clears `waitingOnAnimation` and resets `filledPercent` to 0 in the last-frame script of `rewardAnimation`.
- **Sound**: reward sound `"Play_ui_starbar_reward"` is triggered from within `ProgressBar.set newProgressPercent` via `POST_SOUND_EVENT`, not from `StarBar` itself.
