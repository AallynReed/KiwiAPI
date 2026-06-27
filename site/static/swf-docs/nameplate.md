# nameplate.swf
> Floating nameplate displayed above players and NPCs in the Trove world, showing the entity's name, club affiliation, insignia (shield + wings), and optionally a tether/group icon with a leader badge.

**Document/main class:** `Nameplate` (extends `flash.display.MovieClip`)
**SWF-specific classes:** 2

## Main class: `Nameplate`

`Nameplate` extends `MovieClip` directly (not `UIComponent`). The constructor sets up the insignia clip's scale and position offset, creates two `IggyTween` instances for fade-in and fade-out of the insignia, and registers four `ExternalInterface` callbacks. It also initializes an `AnimatorFactory3D` with a baked `Matrix3D` to apply a fixed 3-D perspective transform to the `insignia` clip (translation x≈486, scaled ~0.99884). A `ADDED_TO_STAGE` listener fires once to set the root perspective projection (90.19° FOV, center 275×200).

Rendering is not invalidation-based — each setter directly mutates text fields or triggers a `stage.invalidate()` + `RENDER` listener to fire a one-shot `RequestRedraw` call back to the game.

### Public methods
- `setText(name:String, color:uint, shieldFrame:int, wingsFrame:int) : void` — sets `textField.text` and color, jumps `insignia.shield` and `insignia.wings` to the given frames, then queues a `RequestRedraw` on the next `RENDER` event.
- `setClubText(text:String, color:uint) : *` — sets `clubField.text` and color directly.
- `showIcon(visible:Boolean) : void` — fades the `insignia` in or out using `IggyTween`. If the opposing tween is running it is stopped first. Calls `TweenComplete` via `ExternalInterface` after the tween finishes (via `onFinalTweenRender`).
- `setTetherIcon(textureName:String, isLeader:Boolean) : void` — loads a texture into the `groupIcon` `ObjectPreview` resized to 30×30; the loaded callback repositions `tetherArtContainer` to the left of the name text and calls `RequestRedraw`. Also sets `leaderIcon.visible`.
- `__setPerspectiveProjection_(e:Event) : void` — `ADDED_TO_STAGE` handler; sets root perspective projection FOV and center point.

### Key fields
- `textField : TextField` — entity name display.
- `clubField : TextField` — club/guild name display.
- `insignia : MovieClip` — composite clip with `shield` and `wings` sub-clips; supports per-frame variants for different insignia styles. Alpha-tweened by `showIcon`.
- `tetherArtContainer : MovieClip` — contains `groupIcon` (`ObjectPreview`) and `leaderIcon`; repositioned relative to the name text width.
- `fadeInTween : IggyTween` — tweens `insignia.alpha` 0→1 over `TWEEN_TIME` (0.2 s).
- `fadeOutTween : IggyTween` — tweens `insignia.alpha` 1→0 over `TWEEN_TIME` (0.2 s).
- `TWEEN_TIME : Number = 0.2` — duration in seconds for insignia fade.
- `__animFactory_insigniaaf1 : AnimatorFactory3D` — baked 3-D animation factory; initialized once and applied to `insignia`.

### Frame scripts / timeline
None defined as `addFrameScript` calls. Timeline animation is handled via `AnimatorFactory3D` baked data rather than frame scripts.

### Runtime dependencies & integration
- `IggyFunctions.inIggy` — gates `ExternalInterface` callbacks; in preview mode sets `insignia.shield` and `insignia.wings` to frame 8.
- `ExternalInterface.addCallback("setText", setText)` — sets name text, color, shield frame, wings frame.
- `ExternalInterface.addCallback("setClubText", setClubText)` — sets club name and color.
- `ExternalInterface.addCallback("showIcon", showIcon)` — triggers insignia fade in/out.
- `ExternalInterface.addCallback("setTetherIcon", setTetherIcon)` — loads group/tether icon texture.
- `ExternalInterface.call("RequestRedraw")` — fired after text changes and after tween completes, via `RENDER` event listeners.
- `ExternalInterface.call("TweenComplete")` — fired once the insignia fade tween finishes, allowing the game to update layout.
- `IggyTween` — Iggy runtime tween class (not documented); used for `alpha` property on `insignia`.
- `_kiwi.Core.ObjectPreview` — loads and displays a named texture; used for `groupIcon`.
- `fl.motion.AnimatorFactory3D` / `MotionBase` — used to apply a single-keyframe 3-D matrix to `insignia` at startup.
- `Event.ADDED_TO_STAGE` — sets perspective projection on root.
- `Event.RENDER` — one-shot listener used to coalesce `RequestRedraw` and `TweenComplete` calls after layout changes.

---

## Other game-specific classes

- `image` (extends `_kiwi.Core.ObjectPreview`) — Embed `symbol1`; bare wrapper used as the concrete `ObjectPreview` symbol in the library. No additional logic.

---

## Notable logic
- The `setTetherIcon` loaded callback repositions `tetherArtContainer` using live `stage.width / 2 - textField.textWidth / 2`, so the group icon always hugs the left edge of the player's name regardless of name length.
- `showIcon` is guard-heavy: it checks both the current `alpha` and whether the opposing tween `isPlaying` before starting a tween, preventing redundant or conflicting animations.
- The 3-D matrix baked into `AnimatorFactory3D` effectively places the insignia at a specific x-offset (~486 px) with minimal perspective distortion, matching a fixed layout position rather than runtime data.
- `leaderIcon` defaults to hidden in the constructor and is only revealed when `setTetherIcon` is called with `isLeader = true`.
