# nameplate.swf
> Floating nameplate displayed above players and NPCs in the Trove world, showing the entity's name, club affiliation, insignia (shield + wings), and optionally a tether/group icon with a leader badge.

**Document/main class:** `Nameplate` (extends `flash.display.MovieClip`)
**SWF-specific classes:** 2

## Main class: `Nameplate`

`Nameplate` extends `MovieClip` directly (not `UIComponent`). The constructor sets up the insignia clip's scale and position offset, creates two `IggyTween` instances for fade-in and fade-out of the insignia, and registers four `ExternalInterface` callbacks. It also initializes an `AnimatorFactory3D` with a baked `Matrix3D` to apply a fixed 3-D perspective transform to the `insignia` clip (translation x≈486, scaled ~0.99884). A `ADDED_TO_STAGE` listener fires once to set the root perspective projection (90.19° FOV, center 275×200).

Rendering is not invalidation-based — each setter directly mutates text fields or triggers a `stage.invalidate()` + `RENDER` listener to fire a one-shot `RequestRedraw` call back to the game.

### Public methods
- `setText(name:String, color:uint, shieldFrame:int, wingsFrame:int) : void` — `name` is the composed string: the engine has already joined the player's title onto the name, and sends nothing that separates the two. `color` arrives with `0xFF` in the top (alpha) byte. Sets `textField.text` and color, jumps `insignia.shield` and `insignia.wings` to the given frames, then queues a `RequestRedraw` on the next `RENDER` event.
- `setClubText(text:String, color:uint) : *` — sets `clubField.text` and color directly. The engine sends the club name already wrapped in angle brackets (`<SORUDEN>`, from a `<{0}>` format string in the client), so the brackets are the game's formatting, not part of the name.
- `showIcon(visible:Boolean) : void` — fades the `insignia` in or out using `IggyTween`; the engine sends `showIcon(false)` for a head that should not carry a crest. The crest is the only thing this fades. If the opposing tween is running it is stopped first. Calls `TweenComplete` via `ExternalInterface` after the tween finishes (via `onFinalTweenRender`).
- `setTetherIcon(textureName:String, isLeader:Boolean) : void` — loads a texture into the `groupIcon` `ObjectPreview` resized to 30×30; the loaded callback repositions `tetherArtContainer` to the left of the name text and calls `RequestRedraw`. Also sets `leaderIcon.visible`.
- `__setPerspectiveProjection_(e:Event) : void` — `ADDED_TO_STAGE` handler; sets root perspective projection FOV and center point.

### Key fields
- `textField : TextField` — entity name display.
- `clubField : TextField` — club/guild name display.
- `insignia : MovieClip` — composite clip (character 101) with `shield` (20 frames) and `wings` (26 frames) sub-clips, stepped by `setText`'s frame arguments. Alpha-tweened by `showIcon`.
- `tetherArtContainer : MovieClip` — contains `groupIcon` (`ObjectPreview`) and `leaderIcon`; repositioned relative to the name text width. `leaderIcon` (character 2) is an empty one-frame sprite, so setting it visible draws nothing: the stock plate marks the group leader without showing anything for it.
- `fadeInTween : IggyTween` — tweens `insignia.alpha` 0→1 over `TWEEN_TIME` (0.2 s).
- `fadeOutTween : IggyTween` — tweens `insignia.alpha` 1→0 over `TWEEN_TIME` (0.2 s).
- `TWEEN_TIME : Number = 0.2` — duration in seconds for insignia fade.
- `__animFactory_insigniaaf1 : AnimatorFactory3D` — baked 3-D animation factory; initialized once and applied to `insignia`.

### Frame scripts / timeline
None defined as `addFrameScript` calls. Timeline animation is handled via `AnimatorFactory3D` baked data rather than frame scripts. The 1060 × 174 stage has one frame whose root holds exactly four named children: `tetherArtContainer`, `textField`, `clubField`, `insignia`. Both text fields carry a `DropShadowFilter` in their placement (distance 2, angle 45°, black, alpha 1, no blur, strength 1). Rendered stock, the name's ink sits on rows 95–123 and the club's on 131–154, 19 rows above the bottom edge.

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
- The 3-D matrix baked into `AnimatorFactory3D` (`overrideTargetTransform`) replaces the constructor's 0.85 scale and +6.6 offset, so what runs is scale 0.99884 at x 486.05: in game the crest sits centred above the name, not at the left edge where a static render of the file shows it. It is a fixed layout position rather than runtime data.
- `leaderIcon` defaults to hidden in the constructor and is only revealed when `setTetherIcon` is called with `isLeader = true`.
