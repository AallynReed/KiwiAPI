# Trove UI — Shared Library Reference

This is the **single, global reference** for the shared framework code embedded in Trove's Flash UI. The exact same library classes are compiled into **all 108 `ui/*.swf` files**; the per-SWF docs in this folder deliberately **omit** them (documenting each ~214-class framework 108 times would be useless noise) and point here instead.

Everything below was reverse-engineered from the decompiled ActionScript 3 (JPEXS ffdec). Where a class appears in several SWFs, the canonical (largest) copy was read.

## Architecture at a glance

Trove's UI is authored in Flash/ActionScript 3 and runs inside **Iggy**, Trion's embedded Flash runtime baked into the C++ game client (a Scaleform-style player). The layers, from the engine outward:

```
┌──────────────────────────────────────────────────────────────┐
│  Game client (C++)                                             │
│      ⇅  ExternalInterface  +  IggyFunctions / IggyTween        │  ← engine ↔ Flash bridge
├──────────────────────────────────────────────────────────────┤
│  _kiwi.*   — Trove's custom component framework (ACTIVE)       │
│     Core (UIComponent base, FocusManager, Constraints)         │
│     Controls (buttons, inputs, lists, slots, tile/row views)   │
│     Util / Constants / Interfaces                              │
├──────────────────────────────────────────────────────────────┤
│  com.kiwi.*  — older kiwi namespace (LEGACY, a few SWFs)       │
├──────────────────────────────────────────────────────────────┤
│  fl.*  /  mx.*  — Adobe STOCK Flash CS + Flex framework        │  ← not Trove-authored
└──────────────────────────────────────────────────────────────┘
```

Key facts that explain almost every UI file:

- **Every screen's document/main class extends `_kiwi.Core.UIComponent`** (the kiwi base class), which provides the invalidate→validate draw cycle, sizing, stage-resize handling, and the `ExternalInterface` plumbing. A handful of low-level HUD clips extend raw `flash.display.MovieClip` instead.
- **The engine drives the UI** by registering AS3 callbacks via `ExternalInterface` (engine→Flash) and reading events the UI calls back out (Flash→engine). `IggyFunctions.translate(key)` resolves localization keys like `$Objective_FaeForest`.
- **Widgets come from `_kiwi.Controls`** — you rarely see raw text fields or buttons; screens compose `LabelButton`, `TextInput`, `DynamicRowView`, `Slot`, etc.
- **`fl.*` and `mx.*` are Adobe stock**, pulled in by the Flash IDE / Flex Component Kit. They are documented here for completeness but were not written by Trove; `mx.*` shows up almost entirely through `chat.swf`.

## Namespace map

| Namespace | Classes | Origin | Role |
|---|---:|---|---|
| `IggyFunctions`, `IggyTween` (top-level) | 2 | Trove | Engine↔Flash bridge: localization, ExternalInterface helpers, tween engine |
| `_kiwi.*` | 74 | Trove | **Active** custom UI component framework (Core + Controls + Util) |
| `com.kiwi.*` | 16 | Trove | Legacy/older kiwi namespace, still referenced by a few SWFs |
| `fl.*` (+ `flash.utils`) | 64 | Adobe (stock) | Flash CS component set (Button, List, ComboBox, ScrollBar, tween/motion) |
| `mx.*` | 58 | Adobe (stock) | Flex framework remnants (interfaces, ResourceManager, Flex Component Kit) |
| **Total** | **214** | | |

## Contents

1. [Iggy runtime bridge](#iggy-runtime-bridge-top-level-classes) — `IggyFunctions`, `IggyTween`
2. [`_kiwi` base layer](#_kiwi-base-layer--core-managers-constraints-constants-interfaces) — `UIComponent`, `FocusManager`, constraints, constants, interfaces
3. [`_kiwi.Util`](#_kiwiutil--formatting--helper-utilities) — formatting & helpers
4. [`_kiwi.Controls` — buttons, inputs & chrome](#_kiwicontrols--buttons-inputs--chrome)
5. [`_kiwi.Controls` — lists, slots, tiles & data views](#_kiwicontrols--lists-slots-tiles--data-views)
6. [`com.kiwi.*`](#comkiwi--legacy-kiwi-namespace) — legacy namespace
7. [`fl.*`](#fl--adobe-flash-component-framework-stock) — Adobe Flash components (stock)
8. [`mx.*`](#mx--adobe-flex-framework-stock-partial) — Adobe Flex framework (stock)

---

## Iggy runtime bridge (top-level classes)

Iggy is Trove's embedded Scaleform-derived Flash runtime running inside the C++ game client. The two top-level classes `IggyFunctions` and `IggyTween` form the entire bridge layer between AS3 SWF logic and the host engine. They live at the root package (no namespace) and are compiled into every one of the 108 SWFs as shared stubs — the real implementations are resolved by the Iggy runtime at load time; in standalone Flash Player the stubs either fall through to plain Flash equivalents or do nothing.

---

### `IggyFunctions`

**Source:** `accountlinking/scripts/IggyFunctions.as`

A pure-static utility class. Every method is `public static`; the class itself is never instantiated (the constructor exists only because AS3 requires one). The class acts as the single choke-point through which SWFs call back into the host engine.

#### Public static properties

| Name | Type | Default | Purpose |
|---|---|---|---|
| `inIggy` | `*` (untyped) | `false` | Runtime flag set to `true` by the Iggy host when the SWF is running inside the game client. SWF code reads this to branch between in-game and standalone behaviour. |
| `HITTEST_NO_MOUSE` | `int` (const) | `1` | Bit flag for `setHittestProperties`: disable mouse interaction on an object. |
| `HITTEST_NO_GET_OBJECTS_UNDER_POINT` | `int` (const) | `2` | Bit flag: exclude from Flash's own `getObjectsUnderPoint` hit-test. |
| `HITTEST_NO_IGGY_GET_OBJECTS_UNDER_POINT` | `int` (const) | `4` | Bit flag: exclude from Iggy's extended hit-test (`iggyGetObjectsUnderPoint`). |

#### Public static methods

```
translate(key:String) : *
```
Looks up a localisation key in the Iggy string table and returns the translated string. In the standalone stub the method returns `key` unchanged (identity function). Every human-visible string in the UI is passed through `translate` — including numeric format tokens such as `$DigitGroupDelimiter`, time-unit labels such as `$TimeUnit_Hours`, and compound format templates such as `$Time_Localized2`. SWF code never hard-codes locale-sensitive text directly.

```
setTextureForBitmap(bmp:Bitmap, name:Object, w:int = -1, h:int = -1) : *
```
Asks the Iggy host to swap the bitmap data of `bmp` for the named engine texture `name` (must be a `String` or `null`; throws `TypeError` otherwise). Optional `w`/`h` override the texture dimensions. The stub body is empty — the real binding is injected by the host.

```
iggyGetObjectsUnderPoint(container:DisplayObjectContainer, pt:Point) : Array
```
Extended hit-test that mirrors Flash's `getObjectsUnderPoint`. The stub delegates directly to the standard Flash method; in the game client Iggy may replace this with depth-sorted, Z-aware logic.

```
setHittestProperties(obj:InteractiveObject, flags:int) : *
```
Applies one or more `HITTEST_*` bit flags to `obj` to control how Iggy's input system treats it. Stub body is empty; the host intercepts the call.

```
getHittestProperties(obj:InteractiveObject) : *
```
Returns the current hit-test flags previously set on `obj`. The stub always returns `0`.

```
setObjectAntialiasingEnable(obj:DisplayObject, enabled:Boolean) : *
```
Toggles Iggy's proprietary antialiasing on a display object. Stub body is empty.

```
setDepth(obj:DisplayObject, depth:Number) : *
```
Stores a Z-depth value for `obj` as a dynamic property `_iggy_depth` on the object. Iggy uses depth values to resolve draw order independently of the Flash display list. The stub performs the property write so that host-level reads of `_iggy_depth` work correctly even when the Iggy injection is not active.

```
getDepth(obj:DisplayObject) : *
```
Retrieves the Z-depth previously set with `setDepth`. Returns `0` if no depth has been assigned.

#### How SWFs use `IggyFunctions`

All human-readable and localised text goes through `translate`:

```as3
// KiwiTextUtil.addDigitDelimiters — real call pattern
var sep:String = IggyFunctions.translate("$DigitGroupDelimiter");

// TimeUtil.localizeTime — real call pattern
var template:String = IggyFunctions.translate("$Time_Localized2");
var unitLabel:String = IggyFunctions.translate("$TimeUnit_Hours");
```

Depth control is used by layered UI components that need to place elements above or below Flash's normal stacking order without re-parenting:

```as3
IggyFunctions.setDepth(tooltipClip, 100);
```

Hit-test flags are applied to overlay clips that should pass mouse events through to elements underneath:

```as3
IggyFunctions.setHittestProperties(glassPane, IggyFunctions.HITTEST_NO_MOUSE);
```

---

### `IggyTween`

**Source:** `atlas/scripts/IggyTween.as`

A frame-driven tween engine. It extends `EventDispatcher` and drives property animation by listening to `Event.ENTER_FRAME` on an internally owned `Sprite`. The class is a drop-in replacement for Flash's own `Tween` (same mental model: object, property, easing function, begin, finish, duration) but adds Iggy-specific extras such as collision management, weak-reference listeners, and a structured finish callback.

#### Constructor

```
IggyTween(
    obj         : Object,    // target object whose property is animated
    prop        : String,    // name of the property to animate (e.g. "alpha", "x")
    func        : Function,  // easing function (t, b, c, d) -> value; pass null for linear
    begin       : Number,    // start value; pass NaN to read obj[prop] at construction time
    finish      : Number,    // end value
    duration    : Number,    // length of animation in frames (or seconds if useSeconds=true)
    useSeconds  : Boolean = false,  // if true, duration is in seconds; uses getTimer()
    useWeakRef  : Boolean = false,  // if true, ENTER_FRAME listener uses a weak reference
    manageCollisions : Boolean = false  // if true, stops any prior tween on same obj+prop
)
```

Construction immediately calls `start()`, so the tween begins on the very next frame after creation. If `manageCollisions` is `true` and another `IggyTween` is already animating the same `obj`/`prop` pair, it is stopped, cancelled, and `motionOverride()` is called on it before the new tween proceeds.

The default easing function (used when `func` is `null`) is linear:
```
f(t, b, c, d) = b + c * (t / d)
```
where `t` = current time, `b` = begin, `c` = total change (`finish - begin`), `d` = duration.

#### Public properties

| Name | Type | Purpose |
|---|---|---|
| `begin` | `Number` | Start value of the animation. |
| `finish` | `Number` | End value of the animation. |
| `duration` | `Number` | Total duration in frames or seconds. |
| `func` | `Function` | Easing function `(t, b, c, d) -> Number`. |
| `isPlaying` | `Boolean` | Read-only in practice; `true` while the ENTER_FRAME listener is active. |
| `looping` | `Boolean` | If `true`, the tween rewinds to `time=0` when it reaches `duration` instead of stopping. |
| `position` | `Number` | Current interpolated value (last value written to `obj[prop]`). |
| `useSeconds` | `Boolean` | Whether timing is in seconds (`getTimer()`-based) or frames. |
| `motionFinishCallback` | `Function` | Called with no arguments (or with `callbackParameters` if set) when the tween completes. |
| `callbackParameters` | `Array` | If non-null, passed as the single argument to `motionFinishCallback`. |

#### Read-only accessors

| Accessor | Type | Returns |
|---|---|---|
| `obj` | `Object` | The target object being animated. |
| `prop` | `String` | The property name being animated. |
| `time` | `Number` (get/set) | Current position in the timeline. Setting this directly advances or rewinds the tween. |

#### Public methods

```
start() : *
```
Rewinds the tween to `time=0` and begins ENTER_FRAME playback. Called automatically by the constructor. If the tween is already playing, `start()` restarts it from the beginning.

```
stop() : *
```
Removes the ENTER_FRAME listener and sets `isPlaying = false`. Does not reset the position.

```
resume() : *
```
Resumes playback from wherever `time` currently is. No-ops if already playing or if the target object has been cancelled.

```
rewind(t:Number = 0) : *
```
Snaps the internal time pointer to `t` without starting playback. If `useSeconds` is `true`, also recalculates `_beginTime` so elapsed-time calculations remain correct.

```
continueTo(newFinish:Number, newDuration:Number = NaN) : *
```
Restarts the tween from the current `position` to a new `finish` value. If `newDuration` is omitted, the current `duration` is reused. This creates seamless chained animations without a visible jump.

```
yoyo() : *
```
Reverses the tween: calls `continueTo(begin, time)`, animating back to the original start value in the same amount of time already elapsed. Useful for ping-pong loops.

```
fforward() : *
```
Fast-forwards to the end of the tween (jumps `time` to `duration`), immediately applying the final value without firing callbacks.

```
nextFrame() : *
```
Advances the tween by one step. Called internally from the ENTER_FRAME handler. In frame mode increments `_time` by 1; in second mode computes elapsed seconds from `getTimer() - _beginTime`.

```
prevFrame() : *
```
Decrements `_time` by 1 (frame mode only). Provided for manual scrubbing; not used by the playback loop.

#### Lifecycle / override hooks (public, empty in base class)

These methods are called at key moments so that subclasses can react without overriding the core logic:

| Method | Called when |
|---|---|
| `motionStart()` | Tween begins playing (from `start()`). |
| `motionStop()` | Tween stops (from `stop()`). |
| `motionResume()` | Tween resumes (from `resume()`). |
| `motionLoop()` | Tween loops back to `time=0` (when `looping=true`). |
| `motionOverride()` | This tween is preempted by a new tween on the same `obj`/`prop` (collision management). |

#### Collision management

When `manageCollisions = true`, `IggyTween` maintains a static `Dictionary` keyed by `[obj][prop]`. Creating a new tween that collides with an existing one automatically stops and nullifies the old tween before the new one starts. This prevents two tweens from fighting over the same property.

#### Usage example (from real call patterns in the codebase)

```as3
// Fade in a clip over 0.3 seconds using a custom easing function
var tween:IggyTween = new IggyTween(
    myClip,          // target
    "alpha",         // property
    fl_EaseOut,      // easing function reference
    0,               // begin
    1,               // finish
    0.3,             // duration (seconds)
    true,            // useSeconds
    false,           // useWeakRef
    true             // manageCollisions — cancels any prior alpha tween on myClip
);
tween.motionFinishCallback = onFadeInComplete;

// Chain: once done, slide to a new position
tween.motionFinishCallback = function():void {
    new IggyTween(myClip, "x", null, myClip.x, 400, 15); // 15-frame linear slide
};
```

## _kiwi base layer — Core, Managers, Constraints, Constants, Interfaces

Every Trove UI screen is built on the `_kiwi` package. It provides the component lifecycle (invalidation/validation/draw), sizing and positioning contracts, constraint-based layout, gamepad/keyboard focus management, ExternalInterface plumbing to the game engine, and a small set of shared constants and interface contracts. The classes in this layer are identical across all 108 SWFs; they are shared by embedding the same compiled bytecode in each file.

### Class summary

| Class | Size | Role |
|---|---|---|
| `_kiwi/Core/UIComponent.as` | ~50 KB | Base class for every component and document root; lifecycle, sizing, focus, ExternalInterface |
| `_kiwi/Managers/FocusManager.as` | ~30 KB | Keyboard/gamepad focus routing; tab-order and group navigation |
| `_kiwi/Constraints/Constraints.as` | ~13 KB | Anchor-based layout engine; repositions/resizes child elements on parent resize |
| `_kiwi/Constraints/ConstrainedElement.as` | ~0.9 KB | Value object holding one element's anchor offsets and edge flags |
| `_kiwi/Core/ObjectPreview.as` | ~6.5 KB | Renders a game texture or item icon into the UI via Iggy/ExternalInterface |
| `_kiwi/Core/SlotDragDropHelper.as` | ~2.6 KB | Static utility that initiates drag-and-drop for inventory slot items |
| `_kiwi/Constants/ButtonStates.as` | ~1.7 KB | String constants for mouse/keyboard/console button states |
| `_kiwi/Constants/Colors.as` | ~0.8 KB | uint colour constants used throughout the UI |
| `_kiwi/Constants/ConstraintMode.as` | ~0.3 KB | Two-value enum controlling how constraints scale their children |
| `_kiwi/Interfaces/IFocusManager.as` | ~1.3 KB | Interface contract for FocusManager |
| `_kiwi/Interfaces/IFocusManagerComponent.as` | ~0.5 KB | Interface marking a component as focus-capable |
| `_kiwi/Interfaces/IFocusManagerGroup.as` | ~0.3 KB | Interface for radio-button-style focus groups |

---

### UIComponent

**Package:** `_kiwi.Core` | **Extends:** `flash.display.MovieClip`

`UIComponent` is the root base class that nearly every document class and control in every Trove SWF extends. It layers a component lifecycle (invalidate/validate/draw), managed sizing and positioning, frame-label-driven platform branching, ExternalInterface callbacks, focus management scaffolding, tooltip handling, and text-composition support on top of Flash's `MovieClip`.

#### Platform and locale detection

The constructor is the only place platform and locale are resolved. It calls `ExternalInterface.call("GetPlatform")` and `ExternalInterface.call("GetLocale")` (falling back to `Capabilities.os` and `"en"` respectively). Child components inherit from the root UIComponent rather than making their own ExternalInterface calls.

```actionscript
public static const PLATFORM_WINDOWS:String = "Windows".toLowerCase();  // "windows"
public static const PLATFORM_ORBIS:String   = "PS4".toLowerCase();       // "ps4"
public static const PLATFORM_DURANGO:String = "Xbone".toLowerCase();     // "xbone"
public static const LOCALE_EN:String = "en";
public static const LOCALE_ZH:String = "zh-cn";
public static const LOCALE_DE:String = "de";
public static const LOCALE_FR:String = "fr";
```

`IsConsole() : Boolean` returns `true` when `_platform` is Orbis or Durango.

#### Frame branching (target-frame system)

During construction the component inspects its timeline labels and picks a `_targetFrame` to jump to:

- Console: prefers `"ConsoleLoc"` (non-English non-Chinese), then `"Console"`, then `"Durango"` or `"Orbis"` specifically.
- PC: jumps to `"PCLoc"` for non-English, non-Chinese locales.

If a target frame is selected the component calls `gotoAndStop(_targetFrame)`, hides itself (`visible = false`), then listens on `ENTER_FRAME` to restore visibility once the clip has actually reached the target label (`setVisibleOnFrameEnter`). This prevents a one-frame flash of the wrong artwork. Overrides of `gotoAndStop` / `gotoAndPlay` throw an `Error` if called with a frame other than `_targetFrame`, preventing accidental frame conflicts.

Public helpers for consumers:

| Method | Signature | Description |
|---|---|---|
| `targetFrame` | `get targetFrame():String` | The resolved target label, or `""` |
| `onTargetFrame` | `():Boolean` | True when `currentLabel == _targetFrame` (or no target) |
| `hasAnyTargetFrame` | `():Boolean` | True if this or any ancestor has a target frame |
| `allOnTargetFrame` | `():Boolean` | True when this and all ancestors are on their targets |
| `listenForFrame` | `(clip:MovieClip, frame:String, func:Function):*` | Calls `func` when `clip` reaches `frame` label |
| `listenForAllOnTarget` | `(uiComponent:UIComponent, func:Function):*` | Calls `func` when `uiComponent.allOnTargetFrame()` becomes true |

#### Invalidation / validation lifecycle

The invalidation system defers drawing to the next Flash render event, batching multiple property changes into a single draw pass.

```actionscript
public function invalidate(... rest):void
```
Marks the component dirty. Accepts zero or more `InvalidationType` string constants (`InvalidationType.ALL`, `InvalidationType.SIZE`, `InvalidationType.STATE`, `InvalidationType.DATA`). Registers a `RENDER` listener on stage so that `validateNow` fires at the end of the current frame. If the stage is not yet available, waits for `ADDED_TO_STAGE` instead.

```actionscript
public function validateNow(param1:Event = null):void
```
Runs the two-phase update:
1. If not yet initialized, calls `configUI()` (runs once, first time the component is validated).
2. Calls `draw()`, then `validate()`.

```actionscript
protected function configUI():void     // override: one-time setup after first validation
protected function draw():void         // override: apply invalidated state to display
protected function validate():void     // clears _invalid and _invalidTypes
```

`draw()` in the base implementation:
- On `STATE` invalid: jumps to `_newFrame` via `gotoAndStop`, then calls `updateAfterStateChange()`.
- On `SIZE` invalid: re-draws the focus rect if focused; calls `constraints.update(width, height)` if a `Constraints` instance is attached.
- Recursively calls `draw()` on all `UIComponent` children (breadth-first traversal with a work-stack to avoid deep recursion).

```actionscript
protected function setState(param1:String):void
```
Sets `_state` and schedules a frame jump to the matching timeline label (only if that label exists in `_labels`).

```actionscript
public function isInvalid(param1:String, ...rest):Boolean
```
Returns true if any of the supplied types (or `ALL`) are in the pending invalid list.

```actionscript
protected function callLater(param1:Function):void
```
Schedules a function to run after the next `RENDER` event. Functions scheduled in the same frame are all called before `inCallLaterPhase` is cleared.

```actionscript
public static var inCallLaterPhase:Boolean = false;
```
Set to `true` while `callLaterDispatcher` is iterating. Used by components to avoid re-entrant `callLater` scheduling.

#### Sizing and positioning

`UIComponent` maintains its own `_width`/`_height`/`_x`/`_y` floats, separate from Flash's internal values. On PC, `move()` rounds coordinates to integers (`Math.round`); on console they are passed through as-is. This prevents sub-pixel jitter on PC without impacting console.

| Method / Property | Signature | Notes |
|---|---|---|
| `width` / `height` | `get/set` override | Stored in `_width`/`_height`; setting calls `setSize` which invalidates `SIZE` |
| `x` / `y` | `get/set` override | Returns `_x`/`_y`; setting calls `move()` |
| `scaleX` / `scaleY` | `get/set` override | Computed from `_width/_originalWidth`; setting re-measures via `super.width` |
| `setSize(w,h)` | `public` | Sets `_width`/`_height`, invalidates `SIZE`, dispatches `ComponentEvent.RESIZE` |
| `setActualSize(w,h)` | `public` | Sets both `super.width/height` and `_width`/`_height` directly without invalidation |
| `move(x,y)` | `public` | Sets `_x`/`_y`, rounds on PC, dispatches `ComponentEvent.MOVE` |

`_originalWidth`/`_originalHeight` are captured at construction from `super.width/scaleX` and are used as the denominator for `scaleX`/`scaleY` overrides.

#### Stage-resize handling

```actionscript
protected function onStageResized(param1:Number, param2:Number, param3:Number):void
```
Empty in the base; Iggy registers it as an ExternalInterface callback (`"UIComponent.onStageResized"`) in the constructor. Document classes that need to reflow on window resize override this method. The three parameters are the new stage width, height, and scale factor.

#### ExternalInterface plumbing

The constructor registers two ExternalInterface callbacks when running inside Iggy:

- `"UIComponent.onStageResized"` → `this.onStageResized`
- `"UIComponent.onClipboardPaste"` → `this.onClipboardPaste`

When the component is the root SWF (parent == stage), three more callbacks are registered after `ADDED_TO_STAGE`:

- `"UIComponent.textCompositionReplace"` — replaces the in-progress IME composition range in the focused TextField
- `"UIComponent.textCompositionEnded"` — clears the composition range
- `"UIComponent.OnTextfieldFocusIn"` / `"UIComponent.OnTextfieldFocusOut"` — called outbound when a TextField gains/loses focus, passing the field's screen-space bounds so the game can reposition the on-screen keyboard

Tooltip callbacks are outbound only:

```actionscript
ExternalInterface.call("UIComponent.OnShowTooltip", stageX, stageY, title, description)
ExternalInterface.call("UIComponent.OnHideTooltip")
```

These are triggered by `ROLL_OVER` / `ROLL_OUT` when `UITooltipTitle` is set. An optional `fixedTooltipOffsetX`/`fixedTooltipOffsetY` (both `public Number`) anchors the tooltip to a fixed offset from the component's local origin rather than the cursor position.

#### Focus

`UIComponent` is focusable if it implements `IFocusManagerComponent` (in which case `tabEnabled` is set to `true` in the constructor). Focus event handlers are wired only when `tabEnabled`.

| Method / Property | Notes |
|---|---|
| `focusEnabled:Boolean` | get/set; controls whether FocusManager considers this object |
| `mouseFocusEnabled:Boolean` | get/set; controls whether mouse clicks focus this object |
| `focusManager:IFocusManager` | get/set; walks the display list to find the nearest registered FocusManager |
| `setFocus():void` | Sets `stage.focus = this` |
| `getFocus():InteractiveObject` | Returns `stage.focus` |
| `drawFocus(param1:Boolean):void` | Override to render/clear the focus indicator; base removes `_uiFocusRect` child |

Focus managers are stored in a static `Dictionary<DisplayObject,IFocusManager>` keyed by the topmost display container. A companion `Dictionary<IFocusManager,Dictionary>` tracks which UIComponents are using each manager; when the last user is removed from stage the manager's `deactivate()` is called and its entry is deleted.

```actionscript
protected function createFocusManager():void
```
Called on `ADDED_TO_STAGE`. Finds the topmost accessible display container (stage or self if cross-domain) and creates a `FocusManager` if none exists yet for it.

#### State management

```actionscript
protected var _state:String = "";
protected var _labels:Array;        // timeline label names from GenerateLabelArray
protected var _newFrame:String = "";
```

`setState(state)` checks whether `state` is a known label before scheduling a frame jump, preventing exceptions on components that lack the label.

```actionscript
protected function updateAfterStateChange():void   // empty hook; called after gotoAndStop in draw()
```

#### Utility helpers

```actionscript
public function setDataHelper(value:*, stored:*):*
```
Returns `value` unchanged if it differs from `stored` and marks the component `DATA` invalid; otherwise returns `stored` as-is. Used by subclass setters to avoid unnecessary invalidations:
```actionscript
_myData = setDataHelper(newValue, _myData);
```

```actionscript
public function getTextFieldTextHeight(tf:TextField):Number
public function vcenterTextfieldToClip(tf:TextField, clip:MovieClip):*
```
Compute the true rendered text height (summing line metrics) and vertically centre a TextField within a MovieClip's bounds.

```actionscript
protected function outlineDisplayObject(obj:DisplayObject, inner:Boolean=false):void
protected function removeDisplayObjectOutline(obj:DisplayObject):void
```
Add/remove a yellow `GlowFilter` (`Colors.HIGHLIGHT_YELLOW`, blur 2, strength 100, `BitmapFilterQuality.HIGH`) as a focus/hover highlight on any `DisplayObject`. Does not add a second glow if one is already present.

#### Key fields summary

| Field | Type | Notes |
|---|---|---|
| `initialized` | `Boolean` | True after first `configUI()` call |
| `constraints` | `Constraints` | Set by subclasses; `draw()` calls `constraints.update(w,h)` on SIZE invalid |
| `focusTarget` | `IFocusManagerComponent` | Allows a container to delegate focus to a specific child |
| `_width`, `_height` | `Number` | Managed size (not Flash's internal) |
| `_x`, `_y` | `Number` | Managed position |
| `_originalWidth`, `_originalHeight` | `Number` | Captured at init; used for scale computation |
| `_platform` | `String` | Lowercased platform string from `GetPlatform` |
| `_locale` | `String` | Lowercased locale from `GetLocale` |
| `_targetFrame` | `String` | Timeline label to jump to; `""` if none |
| `_state` / `_newFrame` | `String` | Current/pending timeline state label |
| `_labels` | `Array` | All timeline label names on this clip |
| `_invalidTypes` | `Array` | Pending invalid type strings |
| `_invalid` | `Boolean` | True when validation is pending |
| `_callLaterMethods` | `Dictionary` | Functions pending `callLater` dispatch |
| `fixedTooltipOffsetX/Y` | `Number` | Tooltip anchor offset from component origin |

---

### FocusManager

**Package:** `_kiwi.Managers` | **Implements:** `IFocusManager`

`FocusManager` provides keyboard and gamepad focus navigation within a `DisplayObjectContainer` subtree (the _form_). One instance is created per top-level container by `UIComponent.createFocusManager()` and stored in the static `UIComponent.focusManagers` Dictionary.

#### Activation and registration

```actionscript
public function FocusManager(form:DisplayObjectContainer)
public function activate():void
public function deactivate():void
```

`activate()` walks the `form` subtree to build the initial focusable-object set (`addFocusables`), then subscribes to:

- `Event.ADDED` / `Event.REMOVED` on `form` — to keep `focusableObjects` current
- `FocusEvent.MOUSE_FOCUS_CHANGE` / `FocusEvent.KEY_FOCUS_CHANGE` on `stage`
- `Event.ACTIVATE` / `Event.DEACTIVATE` on `stage` (restores `lastFocus` on re-activation)
- `MouseEvent.MOUSE_DOWN` and `KeyboardEvent.KEY_DOWN` on `form`

`deactivate()` tears down all listeners and clears internal state.

#### Focusable object tracking

An object is tracked in `focusableObjects:Dictionary` if it:
- Is an `IFocusManagerComponent` with `focusEnabled == true` and `tabEnabled == true`, AND is visible in the tab tree (`isTabVisible`)
- OR is a plain `InteractiveObject` with `tabEnabled == true` that is its own `findFocusManagerComponent` result

The `_calculateCandidates` flag is set to `true` whenever the set changes. The sorted `focusableCandidates:Array` is rebuilt lazily by `sortFocusableObjects()` on the next TAB navigation.

#### Sort order

By default, candidates are sorted by display-list depth path (`sortByDepth`): the path from each object to `form` is encoded as a hex string of child indices, producing a document-order sort. If any focusable has a `tabIndex > 0`, the entire list is re-sorted by `tabIndex` ascending, with depth as a tiebreaker.

#### Tab navigation

```actionscript
public function getNextFocusManagerComponent(reverse:Boolean = false):InteractiveObject
```
Returns the next (or previous, if `reverse`) valid focus candidate, wrapping around. Skips objects that are not visible/enabled (`isEnabledAndVisible`) or that belong to the same `IFocusManagerGroup` as the current focus. For `IFocusManagerGroup` members, automatically finds the `selected` member of the group rather than the first candidate in sort order.

`keyFocusChangeHandler` intercepts `KEY_FOCUS_CHANGE` (TAB / keyCode 0) and calls `setFocusToNextObject`, preventing the default browser/Flash tab behaviour.

#### Mouse focus

`mouseFocusChangeHandler` calls `preventDefault()` on all mouse focus changes unless the target is a `TextField`, effectively preventing mouse clicks from focusing non-TextField elements through the default path. Instead, `mouseDownHandler` manually calls `setFocus()` on the topmost `IFocusManagerComponent` ancestor (`getTopLevelFocusTarget`) that has both `focusEnabled` and `mouseFocusEnabled` true.

After a mouse down, `showFocusIndicator` is set to `false` (hides the focus ring); after a TAB key event it is set back to `true`.

#### Default button

```actionscript
public var defaultButton:BaseButton     // get/set
public var defaultButtonEnabled:Boolean
public function sendDefaultButtonEvent():void
```
When `defaultButtonEnabled` is true, pressing `Keyboard.ENTER` while any non-button component is focused dispatches a `MouseEvent.CLICK` on `defButton`. `defButton` tracks the currently focused button (overrides `defaultButton` while a button has focus); it reverts to `defaultButton` when focus leaves all buttons.

#### Key public API

| Member | Notes |
|---|---|
| `getFocus():InteractiveObject` | Returns `findFocusManagerComponent(stage.focus)` |
| `setFocus(obj:InteractiveObject):void` | Calls `obj.setFocus()` if `IFocusManagerComponent`; otherwise sets `stage.focus` |
| `findFocusManagerComponent(io):InteractiveObject` | Walks up the display list to find the nearest `IFocusManagerComponent` ancestor |
| `getNextFocusManagerComponent(reverse):InteractiveObject` | Next/previous tab stop |
| `showFocusIndicator:Boolean` | Whether the focus ring is currently visible |
| `form:DisplayObjectContainer` | The managed subtree root |
| `defaultButton:BaseButton` | The button activated by ENTER |
| `sendDefaultButtonEvent():void` | Dispatches a CLICK on `defButton` |

---

### Constraints system

The `Constraints` class implements an anchor-based layout engine analogous to CSS absolute positioning with edge pinning. A `UIComponent` optionally exposes a `public var constraints:Constraints` field; `UIComponent.draw()` calls `constraints.update(width, height)` whenever the component is `SIZE`-invalid, making the system automatic for any component that uses it.

#### ConstrainedElement

```actionscript
public class ConstrainedElement
```
A plain value object capturing a child element's anchor state at registration time:

| Field | Type | Description |
|---|---|---|
| `clip` | `DisplayObject` | The managed child |
| `edges` | `uint` | Bitmask of active anchors |
| `left` | `Number` | Distance from scope left edge at registration |
| `top` | `Number` | Distance from scope top edge at registration |
| `right` | `Number` | Distance from scope right edge at registration |
| `bottom` | `Number` | Distance from scope bottom edge at registration |
| `scaleX` / `scaleY` | `Number` | Child's scale at registration (used in COUNTER_SCALE mode) |

#### Constraints

```actionscript
public class Constraints extends EventDispatcher
public function Constraints(scope:Sprite, scaleMode:String = "counter_scale")
```

**Edge bitmask constants:**

| Constant | Value | Meaning |
|---|---|---|
| `LEFT` | `1 << 0` | Pin to left edge |
| `RIGHT` | `1 << 1` | Pin to right edge |
| `TOP` | `1 << 2` | Pin to top edge |
| `BOTTOM` | `1 << 3` | Pin to bottom edge |
| `ALL` | `LEFT|RIGHT|TOP|BOTTOM` | Pin all four edges |
| `CENTER_H` | `1 << 4` | Stretch to fill width (left+right margins maintained) |
| `CENTER_V` | `1 << 5` | Centre vertically in scope |
| `CENTER_V_SINGLE_LINE_TEXT` | `1 << 6` | Centre vertically only when text fits in one line |

**Scale modes** (from `ConstraintMode`):

| Mode | Behaviour |
|---|---|
| `COUNTER_SCALE` (`"counter_scale"`) | Children are repositioned/rescaled to counteract the scope's own scale, keeping them at their original apparent size. Scale factors propagate through parent Constraints via `parentXAdjust`/`parentYAdjust`. |
| `REFLOW` (`"reflow"`) | Children are repositioned in local coordinates without counteracting scale. The scope itself is scaled to `parentXAdjust`/`parentYAdjust` during `update`. |

**Public API:**

```actionscript
function addElement(name:String, obj:DisplayObject, edges:uint):void
```
Registers `obj` under `name` with the given edge bitmask. Captures current position and distances-to-edges as the anchor baselines. If the scope's parent is the Stage, uses `stageWidth/stageHeight` instead of `scope.width/height`.

```actionscript
function removeElement(name:String):void
function removeAllElements():void
function getElement(name:String):ConstrainedElement
function updateElement(name:String, obj:DisplayObject):void  // swap the DisplayObject for an existing entry
```

```actionscript
function update(newWidth:Number, newHeight:Number):void
```
Iterates all registered elements and repositions/resizes each according to its edge bitmask and the current scale mode. After completing, dispatches `ComponentEvent.RESIZE` if any listeners are attached. The parent-constraints chain is maintained automatically: when `scope` is added to the stage, `addToParentConstraints()` walks up the display list to find the nearest ancestor with a `constraints` property and subscribes to its `RESIZE` event so that `parentXAdjust`/`parentYAdjust` stay current.

**Typical usage in a screen class:**

```actionscript
// In configUI():
constraints = new Constraints(this);
constraints.addElement("background", bg, Constraints.ALL);
constraints.addElement("title", titleClip, Constraints.LEFT | Constraints.TOP);
constraints.addElement("closeBtn", closeButton, Constraints.RIGHT | Constraints.TOP);
```

`UIComponent.draw()` will call `constraints.update(width, height)` automatically on every SIZE invalidation.

---

### ObjectPreview

**Package:** `_kiwi.Core` | **Extends:** `UIComponent`

`ObjectPreview` renders a single game texture (item icon, equipment image, etc.) into the Flash UI as a `Bitmap`. The texture is fetched from the game engine via `IggyFunctions` (a thin ExternalInterface bridge); the class registers a static `"objectPreviewReady"` callback so the engine can notify any waiting instances when a texture becomes available.

#### Construction

```actionscript
public function ObjectPreview(imageWidth:Number = -1, imageHeight:Number = -1)
```
Pass `-1` for either dimension to use the texture's natural size. The bitmap starts as a 1×1 transparent placeholder (`new BitmapData(1,1)`).

#### Setting a texture

```actionscript
public function set textureName(param1:String):void
public function get textureName():String
```
Setting `textureName` to a non-empty string:
1. Adds `this` to a static `listeners:Array`.
2. Calls `ExternalInterface.call("UIComponent.CheckTextureExists", textureName)`.
3. When the engine has the texture ready it calls back into `objectPreviewReady(textureName)`, which finds all matching listeners and calls `replaceTexture()`.

Setting `textureName` to `null` or `""` clears the bitmap via `IggyFunctions.setTextureForBitmap(image, null)` and removes the listener.

```actionscript
public function resize(newWidth:Number, newHeight:Number):void
```
Updates the target dimensions and re-requests the texture if already loaded.

#### Lifecycle

`configUI()` adds the internal `Bitmap` as a child. On `REMOVED_FROM_STAGE` the listener is cleaned up to prevent stale callbacks.

| Member | Type | Notes |
|---|---|---|
| `textureName` | `String` | get/set; triggers async texture load |
| `imageWidth` / `imageHeight` | `int` (get) | Current bitmap pixel dimensions |
| `loadedCallback` | `Function` | Optional; called with `this` when the texture finishes loading |
| `selected` / `highlighted` | `MovieClip` | Timeline clips for selection/highlight overlays |
| `getBitmapBounds():Rectangle` | method | Returns bitmap bounds in its own coordinate space |

#### Static callback

```actionscript
public static function objectPreviewReady(textureName:String):void
```
Called by the engine. Iterates `listeners` in reverse, finds all `ObjectPreview` instances whose `textureName` matches, and calls `replaceTexture()` on each. This is registered as an ExternalInterface callback in the constructor (only once per instance in Iggy mode).

---

### SlotDragDropHelper

**Package:** `_kiwi.Core`

`SlotDragDropHelper` is a fully static utility class that manages drag initiation for inventory slot items. There is no instance state; all fields are class-level.

#### How it works

```actionscript
public static function startDrag(
    source:DisplayObject,
    startX:Number, startY:Number,
    slotId:Number,
    textureName:String
):*
```

Call `startDrag` on `MOUSE_DOWN` inside a slot component. The helper:
1. Subscribes to `MOUSE_MOVE`, `MOUSE_UP`, and `ROLL_OUT` (all capture-phase) on `source`.
2. On `MOUSE_MOVE`, computes the distance moved from `(startX, startY)`. If it exceeds the `DragStartDelta` threshold (15 pixels), calls `internalStartDrag`.
3. On `ROLL_OUT` (the mouse leaving the slot before 15px), immediately calls `internalStartDrag` without waiting for the distance check.
4. `internalStartDrag` fires `ExternalInterface.call("SLOT.DRAG_START", slotId, stageX, stageY, textureName)` to hand off drag handling to the game engine, then calls `stopWatchingMouse`.
5. `MOUSE_UP` without a drag cancels the watch via `stopWatchingMouse`.

```actionscript
public static function registerDropCallback(callback:Function):*
```
Registers the given AS3 function as `ExternalInterface.addCallback("DRAGHOST.DROP", callback)`. Slot containers call this once during `configUI()` so the engine can call back when a drag is dropped.

| Static member | Type | Notes |
|---|---|---|
| `DragStartDelta` | `Number` (const) | 15 px — minimum drag distance before drag starts |
| `startDrag(...)` | static method | Begin watching for drag initiation |
| `registerDropCallback(fn)` | static method | Register engine drop callback |

---

### Constants and interfaces

#### ButtonStates

`_kiwi/Constants/ButtonStates.as` — string constants used as timeline label names and state identifiers in `BaseButton` and all control subclasses.

**PC / mouse states:**

| Constant | Value |
|---|---|
| `UP` | `"up"` |
| `OVER` | `"over"` |
| `DOWN` | `"down"` |
| `RELEASE` | `"release"` |
| `OUT` | `"out"` |
| `DISABLED` | `"disabled"` |
| `SELECTING` | `"selecting"` |
| `TOGGLE` | `"toggle"` |
| `KB_SELECTING` | `"kb_selecting"` |
| `KB_RELEASE` | `"kb_release"` |
| `KB_DOWN` | `"kb_down"` |

**Console variants** (suffix `Console`, used when `IsConsole()` is true):

`UP_CONSOLE` → `"upConsole"`, `OVER_CONSOLE` → `"overConsole"`, `DOWN_CONSOLE` → `"downConsole"`, `RELEASE_CONSOLE` → `"releaseConsole"`, `OUT_CONSOLE` → `"outConsole"`, `DISABLED_CONSOLE` → `"disabledConsole"`. The `SELECTING`, `TOGGLE`, `KB_*` console variants share the same string values as their PC counterparts.

#### Colors

`_kiwi/Constants/Colors.as` — `uint` colour constants.

| Constant | Hex value |
|---|---|
| `BLACK` | `0x000000` |
| `WHITE` | `0xFFFFFF` |
| `GRAY` | `0x696969` |
| `RED` | `0xFF0000` |
| `GREEN` | `0x00FF00` |
| `BLUE` | `0x0000FF` |
| `YELLOW` | `0xFFFF00` |
| `MAGENTA` | `0xFF00FF` |
| `CYAN` | `0x00FFFF` |
| `GOLD` | `0xFFD702` |
| `LIGHT_YELLOW` | `0xEBE15E` |
| `HIGHLIGHT_YELLOW` | `0xCD0D00` (sic — this is actually a dark red-orange; the name is misleading) |

`HIGHLIGHT_YELLOW` (`0xCD0D00`) is the colour used by `UIComponent.outlineDisplayObject` for focus/hover glows.

#### ConstraintMode

`_kiwi/Constants/ConstraintMode.as` — two string constants that select the `Constraints` layout algorithm.

| Constant | Value | Meaning |
|---|---|---|
| `COUNTER_SCALE` | `"counter_scale"` | Children are counter-scaled to appear at their natural size despite scope scaling |
| `REFLOW` | `"reflow"` | Children reflow in local space; scope scale is set to `parentXAdjust`/`parentYAdjust` |

#### IFocusManager

```actionscript
public interface IFocusManager
```
The full contract for a focus manager:

- `defaultButton:BaseButton` — get/set the button activated by ENTER
- `defaultButtonEnabled:Boolean` — get/set whether ENTER fires the default button
- `nextTabIndex:int` — next available auto-assign tab index
- `showFocusIndicator:Boolean` — get/set visibility of focus rings
- `getFocus():InteractiveObject` — current focused object
- `setFocus(obj:InteractiveObject):void` — programmatically set focus
- `showFocus():void` / `hideFocus():void` — show/hide focus indicator globally
- `activate():void` / `deactivate():void` — lifecycle
- `findFocusManagerComponent(io:InteractiveObject):InteractiveObject` — walk up to nearest focusable ancestor
- `getNextFocusManagerComponent(reverse:Boolean=false):InteractiveObject` — next tab stop
- `form:DisplayObjectContainer` — get/set the managed root container

#### IFocusManagerComponent

```actionscript
public interface IFocusManagerComponent
```
Marks a component as focus-capable. Implemented by controls that participate in tab navigation.

- `focusEnabled:Boolean` — get/set; when false the FocusManager ignores this component entirely
- `mouseFocusEnabled:Boolean` — get; when false mouse clicks do not focus this component
- `tabEnabled:Boolean` — get; the native Flash tab-enabled flag
- `tabIndex:int` — get; sort priority (0 = depth order)
- `setFocus():void` — called by FocusManager to assign focus
- `drawFocus(focused:Boolean):void` — called by FocusManager to render/clear the focus indicator

`UIComponent` provides concrete implementations of all these members. Components that want to be focusable simply `implements IFocusManagerComponent` and the base class handles the rest.

#### IFocusManagerGroup

```actionscript
public interface IFocusManagerGroup
```
Marks a component as belonging to a named radio-button-style group. The `FocusManager` uses this to ensure that only the `selected` member of a group receives focus when tabbing into it.

- `groupName:String` — get/set; all components with the same name form one group
- `selected:Boolean` — get/set; the currently selected member of the group

## _kiwi.Util — formatting & helper utilities

Six classes that handle all number formatting, time/countdown display, text field manipulation, glow-highlight effects, and object pooling across the UI. They are pure utility classes — all meaningful methods are `public static` except in `NumberFormat` (instance-based formatter) and `ObjectPool` (instance-based pool).

### Summary table

| Class | Kind | Core responsibility |
|---|---|---|
| `KiwiTextUtil` | Static utility | Text field helpers: digit delimiters, ellipsis truncation, rainbow/colourize, font sizing, currency symbols, price formatting |
| `NumberFormat` | Instance formatter | Configurable number-to-string formatting with grouping separators, decimal separators, and fractional digit control |
| `TimeUtil` | Static utility | Converts millisecond durations to localised human-readable strings and countdown values |
| `FormatCountdownResult` | Value object | Holds a `(value, units)` pair returned by `TimeUtil` |
| `HighlightUtil` | Static utility | Applies/removes a `GlowFilter` on any `DisplayObject` |
| `ObjectPool` | Instance pool | Fixed-size reuse pool for any AS3 class |

---

### `KiwiTextUtil`

**Source:** `activitytrackerui/scripts/_kiwi/Util/KiwiTextUtil.as`

Handles all string and `TextField` manipulation that recurs across the UI. Relies on `IggyFunctions.translate` for locale-sensitive tokens.

#### Private constants

- `colors : Array` — six rainbow colours (red, orange, yellow, green, cyan, purple) used by `rainbowify`.
- `RAINBOW_FONT`, `FONT_OPEN`, `FONT_CLOSE` — sentinel strings used to detect `<font color=rainbow>` tags inside HTML text before the text is assigned to a `TextField`.

#### Public static methods

```
addDigitDelimiters(n:int) : String
```
Converts an integer to a string and inserts the locale digit-group delimiter (fetched via `IggyFunctions.translate("$DigitGroupDelimiter")`) every three digits from the right. Example: `1234567` → `"1,234,567"` (comma or locale equivalent). Used everywhere a raw count is displayed.

```
removeDigitDelimiters(s:String) : int
```
Inverse of `addDigitDelimiters`. Strips all digit-group delimiter characters from `s` and parses the result as `int`. Used when reading back user-entered numbers from text fields.

```
ellipsify(tf:TextField) : void
```
Truncates `tf.text` by removing four characters at a time from the right and appending `"..."` until the text fits within `tf.width` (tested as `textWidth + 4 <= width`). Cleans up the common artefact of `" ..."` (space before ellipsis) by replacing it with `"..."`. Applied to labels that must not overflow their bounding box.

```
colorize(tf:TextField, html:String) : void
```
Processes the special `<font color=rainbow>...</font>` pseudo-tag inside an HTML string and writes the result to `tf`. Any segment wrapped in `<font color=rainbow>` is extracted, the surrounding HTML is applied normally, and then `rainbowify` is called on the character range that corresponded to that segment. All other standard HTML font tags are passed through unchanged.

```
rainbowify(tf:TextField, begin:int = 0, end:int = -1) : void
```
Applies the six-colour rainbow palette to individual characters in `tf` from index `begin` to `end` (defaults to `tf.text.length`). Each character gets its own `TextFormat` with one of the six colours cycling in order. Used for special cosmetic item names and event labels.

```
setFontSize(tf:TextField, size:int) : void
```
Sets the font size of the entire `TextField` via `TextFormat` in a null-safe way.

```
resizeFont(tf:TextField, minSize:int = 14, maxLines:int = 1) : int
```
Shrinks the font size of `tf` one point at a time until the text fits within the field's width/height and does not exceed `maxLines` lines, stopping at `minSize`. Returns the final font size. Used for labels that must stay on one line regardless of content length.

```
expandTextFieldHeightToTextHeight(tf:TextField) : *
```
If `tf.textHeight` exceeds `tf.height`, resizes the field to fit. Called before vertical alignment to avoid clipping.

```
alignTextFieldVertically(tf:TextField, centerY:Number) : *
```
Calls `expandTextFieldHeightToTextHeight` then sets `tf.y = centerY - textHeight * 0.5`. Vertically centres a dynamically sized text field around a given Y coordinate.

```
getCurrencySymbol(isoCode:String) : String
```
Maps ISO 4217 currency codes to display symbols. Supported codes and their symbols:

| Code | Symbol |
|---|---|
| `USD` | `$` |
| `EUR` | `€` |
| `GBP` | `£` |
| `BRL` | `R$` |
| `CNY` / `JPY` | `¥` |
| `KRW` | `₩` |
| *(anything else)* | `""` |

```
formatPrice(cents:Number) : String
```
Divides `cents` by 100 and formats the result with exactly two decimal places (`toFixed(2)`). Used for real-money item prices in the store UI.

---

### `NumberFormat`

**Source:** `leaderboard/scripts/_kiwi/Util/NumberFormat.as`

An instance-based formatter that mirrors AS3's `flash.globalization.NumberFormatter` but without the locale dependency, so it works identically across all platforms at runtime. Configuration is done through properties before calling a format method.

#### Properties (all get/set)

| Property | Type | Default | Meaning |
|---|---|---|---|
| `fractionalDigits` | `int` | `-1` | Number of decimal places. `-1` means no rounding: uses `Number.toString()`. |
| `trailingZeros` | `Boolean` | `false` | When `fractionalDigits > 0`, whether to keep trailing zeros in the decimal portion. |
| `groupingSeparator` | `String` | `","` | Character inserted between digit groups. |
| `decimalSeparator` | `String` | `"."` | Character used as the decimal point. |

#### Public methods

```
formatNumber(n:Number) : String
```
The main formatting method. Algorithm:
1. If `fractionalDigits >= 0`, calls `n.toFixed(fractionalDigits)`; otherwise calls `n.toString()`.
2. Splits at `decimalSeparator` to isolate integer and fractional parts.
3. If `trailingZeros` is `false` (and `fractionalDigits > 0`), trims trailing `"0"` characters from the fractional part.
4. Calls `addGroupingSeparators` on the integer part.
5. Reassembles with `decimalSeparator` between the two parts (omits the separator if there is no fractional part).

```
formatInt(n:int) : String
```
Convenience wrapper: casts `n` to `Number` and delegates to `formatNumber`.

```
// private
addGroupingSeparators(intStr:String) : String
```
Inserts `groupingSeparator` every three digits from the right (standard thousands grouping). Works iteratively by slicing off three-character chunks from the right of the string.

#### Typical usage in the leaderboard

```as3
var fmt:NumberFormat = new NumberFormat();
fmt.fractionalDigits = 0;
scoreLabel.text = fmt.formatNumber(playerScore);  // e.g. "1,234,567"
```

---

### `TimeUtil`

**Source:** `activitytrackerui/scripts/_kiwi/Util/TimeUtil.as`

Converts millisecond durations into localised, human-readable time strings. Depends on `IggyFunctions.translate` for all unit labels and format templates, so the output language matches the game client's locale automatically.

#### Private static data

`timeData : Array` — ordered array of time-unit descriptors from largest to smallest:

| Label key | Milliseconds |
|---|---|
| `$TimeUnit_Years` | 31 536 000 000 |
| `$TimeUnit_Months` | 2 628 000 000 |
| `$TimeUnit_Days` | 86 400 000 |
| `$TimeUnit_Hours` | 3 600 000 |
| `$TimeUnit_Minutes` | 60 000 |
| `$TimeUnit_Seconds` | 1 000 |

#### Public static methods

```
localizeTime(ms:Number, units:int = 2, short:Boolean = false) : String
```
The primary display method. Produces a fully localised string such as `"3 hours 22 minutes"` or `"3h 22m"` (short form).

- `ms`: duration in milliseconds (sign is ignored via `Math.abs`).
- `units`: how many time units to include. `1` = most significant unit only; `2` = two units (the default).
- `short`: `true` for abbreviated unit labels (`_short` suffix on translation keys).

Internally calls `getTimeUnits` to compute a `Vector.<FormatCountdownResult>`, then fetches a localisation template key of the form `$Time_Localized{units}` (or `$Time_Localized{units}_short`) and substitutes `{0}` / `{1}` / ... with the computed values and translated unit names. This means the grammar, word order, and spacing of the result is fully controlled by the game's string table.

```
formatCountdown(ms:Number) : FormatCountdownResult
```
Returns a single `FormatCountdownResult` containing the most significant unit's numeric value and translated label. Used for simple countdown timers where only one unit needs to be shown (e.g. `"5 minutes"`). Internally calls `getTimeUnits(ms, 1, false)[0]`.

```
formatTime(hours:int, minutes:int, use12Hour:Boolean = true) : String
```
Formats a clock time as `"H:MM"`. If `use12Hour` is `true` and `hours > 12`, subtracts 12 to convert to 12-hour format. Always zero-pads minutes to two digits. Returns a plain string like `"3:05"`.

#### Private helper

```
getTimeUnits(ms:Number, maxUnits:int, short:Boolean) : Vector.<FormatCountdownResult>
```
Core decomposition logic. Iterates `timeData` from largest to smallest unit. For each unit whose value fits into the remaining `ms`, computes the count (using `Math.round` when `maxUnits == 1` for a single-unit result, or `Math.floor` otherwise to avoid rounding up to the next unit), looks up the translated label (appending `_single` for singular `1` in long form, `_short` for abbreviated form), and pushes a `FormatCountdownResult`. The remaining `ms` is reduced via modulo before moving to the next unit. If no unit matched (i.e. the duration is less than one second), returns a single `FormatCountdownResult("", "")`.

---

### `FormatCountdownResult`

**Source:** `activitytrackerui/scripts/_kiwi/Util/FormatCountdownResult.as`

Minimal value object. Holds one component of a decomposed time duration.

#### Properties

| Name | Type | Meaning |
|---|---|---|
| `value` | `String` | Numeric count as a string (e.g. `"3"`). |
| `units` | `String` | Translated unit label (e.g. `"hours"` or `"h"`). |

#### Constructor

```
FormatCountdownResult(value:String, units:String)
```

Created exclusively by `TimeUtil.getTimeUnits`. Callers read `.value` and `.units` to build display strings.

---

### `HighlightUtil`

**Source:** `charsheet/scripts/_kiwi/Util/HighlightUtil.as`

Applies and removes a `GlowFilter` highlight effect on any `DisplayObject`. Used in the character sheet and equipment slots to draw attention to selectable items.

#### Public static methods

```
setHighlightMovieClip(obj:DisplayObject, on:Boolean, color:uint = 0xCCCC00) : void
```
Convenience toggle. If `on` is `true`, delegates to `highlightMovieClip`; otherwise delegates to `unhighlightMovieClip`. Default colour is a golden yellow (`0xCCCC00`).

```
highlightMovieClip(obj:DisplayObject, color:uint = 0xCCCC00, inner:Boolean = true) : void
```
Constructs a `GlowFilter` with the following fixed settings and applies it to `obj.filters`:

| Filter property | Value |
|---|---|
| `blurX` / `blurY` | `2` |
| `color` | `color` parameter (default `0xCCCC00`) |
| `inner` | `inner` parameter (default `true` — glow is inside the shape) |
| `quality` | `BitmapFilterQuality.HIGH` |
| `strength` | `100` |

```
unhighlightMovieClip(obj:DisplayObject) : void
```
Clears all filters on `obj` by setting `obj.filters = []`.

---

### `ObjectPool`

**Source:** `accountlinking/scripts/_kiwi/Util/ObjectPool.as`

A fixed-size, index-cycling object pool. Avoids repeated allocation of frequently created/destroyed UI objects.

#### Constructor

```
ObjectPool(template:Object, count:int)
```
Reads `template.constructor` to obtain the class, then pre-allocates exactly `count` instances of that class. Both the `objects` array (the instances) and the `inUse` boolean array are populated at construction time. The pool size is fixed — it never grows.

#### Public methods

```
getObject() : Object
```
Scans the pool from the current `next` cursor, cycling via modulo, to find the first instance where `inUse` is `false`. Marks it `true` and returns it. Returns `null` if every slot is in use. The cursor advances on each call so successive calls distribute evenly across the pool rather than always checking slot 0 first.

```
returnObject(obj:Object) : void
```
Finds `obj` by identity (`===`) in the `objects` array and sets the corresponding `inUse` entry to `false`, making the slot available for the next `getObject` call.

#### Notes

- The pool does not reset or clear the returned object before handing it out; callers are responsible for re-initialising instance state.
- There is no `reset()` or `releaseAll()` method; individual objects must be returned one at a time.
- If all slots are exhausted `getObject` returns `null` silently; call sites should guard against `null`.

## _kiwi.Controls — Buttons, inputs & chrome

The `_kiwi/Controls/` package is the entire interactive widget layer of Trove's Flash UI. It covers every visible piece that a user touches or that the engine updates: buttons in two tiers (graphic-only and labelled), a single-line text field, check/radio toggles, a combo-box, a colour-swatch picker, a countdown clock, an art/icon cell, a health/resource bar, static labels, window chrome headers and their close button, section headings, tooltip surfacing, a message-banner queue, and several small plumbing types. Almost every class extends the kiwi `UIComponent` (itself a thin wrapper over the FL component framework), with a handful of simpler clips extending `MovieClip` or `Sprite` directly. The entire package is shared identically across all 108 SWFs.

### Summary table

| Class | Extends | Purpose |
|---|---|---|
| `BaseButton` | `UIComponent` (`IFocusManagerComponent`) | State-machine button base: manages up/over/down/disabled/toggle/checked states; dispatches `ComponentEvent.BUTTON_DOWN` and `MouseEvent.CLICK`; calls `ExternalInterface("POST_SOUND_EVENT")` on press |
| `LabelButton` | `BaseButton` (`IFocusManagerComponent`) | The primary labelled button; adds a `textField`, optional `highlight` clip, optional font-resize, 3D-transform animation data keyed to eight timeline frames |
| `TextInput` | `UIComponent` (`IFocusManagerComponent`) | Single-line editable field with normal/focused/disabled states; wraps a `TextField`; dispatches `Event.CHANGE`, `TextEvent.TEXT_INPUT`, `ComponentEvent.ENTER` |
| `Checkbox` | `UIComponent` | Two-state check widget; timeline frames `checked`/`unchecked` (plus disabled variants and console variants); dispatches `Event.CHANGE` on click |
| `RadioButton` | `UIComponent` | Single radio option; timeline frames `checked`/`unchecked`/`checked disabled`/`unchecked disabled`; dispatches `Event.CHANGE` bubbling upward |
| `RadioButtonContainer` | `UIComponent` | Groups `RadioButton` children; enforces mutual exclusivity; exposes `setChecked(rb:RadioButton)` and `numRadioButtons` |
| `ArrowSelect` | `UIComponent` | Left/right arrow control for cycling a fixed `choices:Array`; exposes `selectedIndex`; dispatches `"ArrowSelectUserChange"` with `direction` property |
| `ArrowButton` | `BaseButton` | Concrete left-arrow button asset (symbol 146), four-frame timeline |
| `ArrowButtonDown` | `BaseButton` | Concrete down-arrow button asset (symbol 137), four-frame timeline |
| `KiwiComboBox` | `fl.controls.ComboBox` | Drop-down list styled in Open Sans 12 pt white; adds `setFontSize(size:int)`, `setScrollSize(n:Number)`, and viewport-scroll compensation when hosted inside a `DynamicRowView` |
| `KiwiColorPicker` | `UIComponent` | Grid colour-swatch palette; programmatically populated via `addColor(color:uint)`; tracks selected swatch with a highlight overlay; supports console D-pad navigation via `moveHorizontal`/`moveVertical`; dispatches `Event.CHANGE` |
| `KiwiColorPicker2` | `MovieClip` | Thin SWF-symbol stub (`dynamic`); no logic — used as a raw asset placeholder for a second colour-picker variant |
| `Range` | `Object` | Plain value object: `start:int`, `end:int`, `contains(n:int):Boolean` |
| `DirectionalMapping` | `MovieClip` | Console focus-navigation helper; stores north/east/south/west `MovieClip` neighbours; `getAdjacent(dx,dy)` returns the neighbour for a given d-pad direction; placed as child named `"directionalMapping"` |
| `Label` | `UIComponent` | Non-interactive text label; supports `text`, `htmlText`, `wordWrap`, `autoSize` (left/right/center), `condenseWhite`, `selectable` |
| `ArtClip` | `UIComponent` | Icon cell wrapping an `ObjectPreview` bitmap; exposes `iconImage:String` (texture name), `data:Object`, `ghosted:Boolean` (greyscale matrix filter); fires `SLOT.POINTER_ENTER`/`SLOT.POINTER_LEAVE`/`TOOLTIP.SHOW`/`TOOLTIP.HIDE` via `ExternalInterface` on hover |
| `ResourceBar` | `UIComponent` | Horizontal fill bar (200 px wide at 100 %); `percent:Number` drives bar width and a percentage `TextField`; `color:uint` tints the bar clip via `fl.motion.Color` |
| `ArcMask` | `Sprite` | Procedural arc/pie-slice mask drawn in ActionScript; `percent:Number` controls sweep; inner/outer radii each accept a `[min,max]` range interpolated by percent; used for circular cooldown overlays |
| `BitmapAnimator` | `MovieClip` | Transient shake + brightness-flash effect; constructed with `(animTarget, filterTarget)`, then `playShakeAnimation(durationMs, shakeAmt, brightness)` adds itself to the parent display list and self-removes when done |
| `CountdownTimer` | `UIComponent` | Self-ticking HH:MM:SS display; `remainingSeconds` setter restarts the internal `Timer`; optional `expiredText`, `showHours`, `showLocalizedTime` (delegates to `TimeUtil`); accepts tick and complete callbacks via `setCallbacks(tick, complete)` |
| `WindowHeader` | `UIComponent` | Full-width window title bar; exposes `title:String` → `winTitleTextField`; constraint-tracked to resize with the component |
| `WindowHeaderSmall` | `UIComponent` | Compact title bar; same `title` API; adds `allowFontResize` (falls back to 14 pt); uses `KiwiTextUtil.alignTextFieldVertically` for vertical centring |
| `WindowHeaderLoc` | `MovieClip` | Raw symbol stub for a localised full window header (dynamic clip, no logic beyond `winTitleTextField` child) |
| `WindowHeaderSmallLoc` | `WindowHeaderSmall` | SWF-symbol override of `WindowHeaderSmall`; inherits all logic, just swaps the embedded asset |
| `WindowCloseButton` | `BaseButton` | "X" close button; on click calls `ExternalInterface("POST_SOUND_EVENT","Play_ui_window_close")` then `ExternalInterface("OnRequestClose")` |
| `SecondaryHeader` | `UIComponent` | Section-level sub-header with optional font-resize and optional `vcenterSingleLineText` vertical centring mode; two-frame timeline (normal / alternate style) |
| `ItemGroupHeading` | `MovieClip` | Thin group-label clip; exposes `nameTextField:TextField`; no behaviour logic |
| `InlineTooltip` | `UIComponent` | Inline item-detail panel; holds a large `Slot`, name/description/powerRank/rarity text fields, and seven `statRow` sub-clips; populated via `ExternalInterface` callbacks `ADD_STAT` / `COMPARISON.ADD_STAT`; `clear()` resets all fields |
| `MessageQueue` | `MovieClip` | Serial banner system; `addMessage(text:String)` queues HTML strings shown one at a time with 0.15 s fade-in → 2 s hold → 0.5 s fade-out via `IggyTween` |
| `ResizeBroadcastMovieClip` | `UIComponent` (`IEventDispatcher`) | `UIComponent` subclass that fires `Event.RESIZE` whenever its `width` or `height` setters are called; event routing is delegated to an internal `EventDispatcher` |
| `DataEvent` | `flash.events.Event` | Custom event carrying an arbitrary `data:Object` payload; used throughout the Controls package for typed event dispatch |

---

### Key widgets

#### BaseButton

**Extends:** `UIComponent`, implements `IFocusManagerComponent`

The root of every interactive button in the toolkit. State is managed through a static `stateMap` keyed on logical state names (`"up"`, `"over"`, `"down"`, `"release"`, `"out"`, `"disabled"`, `"selecting"`, `"toggle"`), with each key mapping to an ordered list of timeline frame-label candidates. On console, frame labels get a `_console` suffix first; selected buttons prepend a `selected_` prefix. The constructor builds a transparent hit-area `Sprite` from the union of non-text child bounds and assigns it to `hitArea`, ensuring accurate click regions regardless of the visual clip's shape.

**Important public members:**

```as3
get/set enabled   : Boolean          // disables mouse, jumps to "disabled" frame
get/set toggle    : Boolean          // turns button into a sticky toggle; adds CLICK listener
get/set checked   : Boolean          // current toggle state; drives "selecting" vs "out" frame
get/set selected  : Boolean          // secondary selection flag (does not drive frame state)
get/set ghosted   : Boolean          // applies a greyscale ColorMatrixFilter
get/set data      : Object           // arbitrary payload
get/set clickSoundEvent : String     // Wwise event posted on mouse-down (default "Play_ui_button_select")
set mouseStateLocked : Boolean       // freezes visual state; queues pending state for release
```

**Events dispatched:**

| Event | When |
|---|---|
| `ComponentEvent.BUTTON_DOWN` (bubbles) | Mouse or Space key pressed |
| `MouseEvent.CLICK` (bubbles) | Space or Enter key released |
| `Event.CHANGE` (bubbles) | Toggle mode: state flipped |

**Usage:** Place a `BaseButton` subclass on stage. Set `toggle = true` and listen for `Event.CHANGE` for sticky toggles; otherwise listen for `MouseEvent.CLICK`. Set `ghosted = true` to visually dim without disabling hit-testing.

---

#### LabelButton

**Extends:** `BaseButton`, implements `IFocusManagerComponent`

The standard text-labelled button. Embeds asset symbol 153. Adds a `textField:TextField` rendered via `htmlText` (so HTML tags work) and an optional `highlight:MovieClip` child. Eight 10-frame animation segments (frames 10, 20, … 80) each stop immediately — these are the state-change animation stubs. 3D-matrix `AnimatorFactory3D` objects are inlined in the constructor for each segment but run at identity, meaning the structure is ready for per-SWF animation overrides.

**Important public members:**

```as3
get/set label             : String   // sets textField.htmlText; dispatches ComponentEvent.LABEL_CHANGE
var m_ResizeText          : Boolean  // if true, calls KiwiTextUtil.resizeFont to shrink to fit
var adjustHeightMultiplier: Number   // vertical alignment tuning (default 0.5)
var adjustHeightAdditive  : Number   // vertical alignment tuning (default –2)
var adjustXPosition       : Number   // horizontal nudge of textField after alignment
```

**Usage:**

```as3
var btn:LabelButton = new LabelButton();
btn.label = "Confirm";
btn.setSize(120, 30);
btn.addEventListener(MouseEvent.CLICK, onConfirm);
addChild(btn);
```

---

#### TextInput

**Extends:** `UIComponent`, implements `IFocusManagerComponent`

Single-line editable text field. Embeds symbol 60. Three timeline states (`normal`, `focused`, `disabled`) are string constants on the class. The inner `textField` is set to `TextFieldType.INPUT` only when both `enabled` and `editable` are true; otherwise it is `DYNAMIC`. On focus-in the entire content is selected. `constraints` pins the `textField` to all four edges so the component resizes correctly.

**Important public members:**

```as3
get/set text              : String
get/set htmlText          : String
get/set editable          : Boolean
get/set maxChars          : int
get/set restrict          : String       // character whitelist/blacklist
get/set displayAsPassword : Boolean
get/set defaultTextFormat : TextFormat
get/set horizontalScrollPosition : int
get    maxHorizontalScrollPosition : int
get    length / textWidth / textHeight : Number
setSelection(begin:int, end:int) : void
appendText(s:String)             : void
setFocus()                       : void
```

**Events dispatched:**

| Event | When |
|---|---|
| `Event.CHANGE` (bubbles) | Text content changed by user |
| `TextEvent.TEXT_INPUT` (bubbles) | Character about to be inserted |
| `ComponentEvent.ENTER` (bubbles) | Enter key pressed |

---

#### Checkbox

**Extends:** `UIComponent`

Two-state toggle with an associated text label. Embeds symbol 170. Timeline frames: `checked`, `unchecked`, `checked disabled`, `unchecked disabled` — plus optional `checkedConsole` variants. Clicking the component calls `toggleSelected`, which flips `checked` and dispatches `Event.CHANGE`. The `textField` child is constrained to all edges.

**Important public members:**

```as3
get/set checked : Boolean   // drives gotoAndStop to the correct frame label
get/set label   : String    // sets textField.text; dispatches ComponentEvent.LABEL_CHANGE
get/set enabled : Boolean   // inherited; drives disabled frame variants
```

**Events dispatched:** `Event.CHANGE` (bubbles) on user click.

**Usage:**

```as3
var cb:Checkbox = new Checkbox();
cb.label = "Show offline friends";
cb.checked = true;
cb.addEventListener(Event.CHANGE, onChanged);
```

---

#### KiwiComboBox

**Extends:** `fl.controls.ComboBox`

A styled wrapper around the standard FL `ComboBox`. Applies Open Sans 12 pt white to the main text field, dropdown list renderer, and component style on construction. The `open()` override compensates for scroll position when the control is hosted inside a `DynamicRowView` by walking up the display hierarchy and subtracting each `DynamicRowView`'s viewport scroll.

**Important public members (additions over `ComboBox`):**

```as3
var isDynamicRowViewChild : Boolean            // enables scroll compensation in open()
setFontSize(size:int)     : void               // updates all three text style targets
setScrollSize(n:Number)   : void               // sets vertical page and line scroll; normalises mouse-wheel delta
```

Standard `ComboBox` population applies: call `addItem({label:"...", data:...})` and listen for `Event.CHANGE`.

---

#### KiwiColorPicker

**Extends:** `UIComponent`

A grid of colour swatches built at runtime. Programmatic swatches are 51 × 28 px with 6/5 px gutters. An `alphaMask:MovieClip` overlay can dismiss the picker on click (sets `visible = false`). The selected swatch is indicated by positioning `colorSelected:MovieClip` over it; a hover highlight is also supported (NX/console only uses a separate `colorHighlighted` clip; PC uses a `GlowFilter`). Console navigation uses `moveHorizontal`/`moveVertical` to move the highlight index, and `setSelectedColor()` to commit.

**Important public members:**

```as3
set rows/columns   : uint             // grid dimensions — must be set before addColor()
addColor(c:uint)   : void             // adds next swatch; max is rows × columns
clear()            : void             // removes all swatches, resets state
get/set selected   : uint             // colour value of selected swatch
moveHorizontal(delta:int) : void      // console d-pad horizontal
moveVertical(delta:int)   : Boolean   // console d-pad vertical; returns false at boundary
highlightSelection()      : void      // applies GlowFilter to current index (PC) or shows overlay (NX)
unhighlightSelection()    : void
setSelectedColor()        : void      // commits highlighted index as selected; dispatches Event.CHANGE
```

**Events dispatched:** `Event.CHANGE` (non-bubbling) when a swatch is clicked or `setSelectedColor()` is called.

---

#### CountdownTimer

**Extends:** `UIComponent`

A self-managing countdown display. Setting `remainingSeconds` restarts an internal 1 000 ms `Timer` that ticks from the current wall clock (`flash.utils.getTimer`) to avoid drift. Output is formatted as `HH:MM:SS` (hours optional) or delegated to `TimeUtil.localizeTime` when `showLocalizedTime` is true. When the remaining time reaches zero and `expiredText` is set, that string is displayed instead; the timer then stops.

**Important public members:**

```as3
set remainingSeconds    : Number    // sets and starts/restarts the countdown
set showHours           : Boolean   // default true; hides HH: segment when false
set expiredText         : String    // shown when time reaches 0; empty shows "00:00:00"
set showLocalizedTime   : Boolean   // routes formatting through TimeUtil
setCallbacks(tick:Function, complete:Function) : void
    // tick(secondsLeft:Number) — called every second
    // complete()              — called when timer reaches 0
```

Timeline has two frames (0 and 10, both stopped) that can carry visual state changes keyed externally.

---

#### ArtClip

**Extends:** `UIComponent`

A container that hosts an `ObjectPreview` bitmap cell and wires it to Trove's C++ tooltip system via `ExternalInterface`. On roll-over it calls `SLOT.POINTER_ENTER` and optionally `TOOLTIP.SHOW`; on roll-out it calls `SLOT.POINTER_LEAVE` and `TOOLTIP.HIDE`. The `image:ObjectPreview` child is sized by `imageWidth`/`imageHeight` (default 51 × 51 px). Setting `iconImage` to an empty string hides the image child.

**Important public members:**

```as3
get/set iconImage    : String     // texture name passed to ObjectPreview; empty string → hidden
get/set data         : Object     // passed to SLOT.POINTER_ENTER/LEAVE
get/set ghosted      : Boolean    // greyscale ColorMatrixFilter identical to BaseButton
var displayName      : String     // shown in TOOLTIP.SHOW
var description      : String     // shown in TOOLTIP.SHOW
set imgWidth/imgHeight : int      // resize image and invalidate
setImageSize(w:int, h:int) : void // set both dimensions at once
handleRollOver(e:MouseEvent = null) : void   // public; can be called programmatically
handleRollOut(e:Event = null)       : void
showTooltipAt(x:Number, y:Number)  : void   // show tooltip at explicit stage coords
```

---

#### ResourceBar

**Extends:** `UIComponent`

A horizontal progress bar 200 px wide at full scale. The `bar:MovieClip` child is scaled to `200 * percent` pixels wide. A percentage integer (0–100) is written to both `textField` and `textFieldShadow` text fields. Bar colour is applied via `fl.motion.Color.setTint(color, 0.6)` on the `bar` clip's `colorTransform`.

**Important public members:**

```as3
get/set percent : Number   // 0.0–1.0; drives bar width and label text
get/set color   : uint     // 24-bit RGB; tints the bar at 60 % tint strength
```

---

#### Label

**Extends:** `UIComponent`

The standard non-interactive text display. Exposes both plain `text` and `htmlText` setters, `wordWrap`, `autoSize` (using `TextFieldAutoSize` constants), `condenseWhite`, and `selectable`. When `autoSize` is not `"none"`, the component recalculates its own `_width` by measuring `textField.textWidth` plus the constrained margins, then repositions itself for `RIGHT` or `CENTER` alignment.

**Important public members:**

```as3
get/set text          : String
get/set htmlText      : String
get/set wordWrap      : Boolean
get/set autoSize      : String    // TextFieldAutoSize constant
get/set condenseWhite : Boolean
get/set selectable    : Boolean
```

---

### Additional class notes

**ArrowButton / ArrowButtonDown** — concrete `BaseButton` subclasses (symbols 146 and 137 respectively) used inside `ArrowSelect`; add no public API beyond the inherited button interface.

**ArrowSelect** — dispatches the custom `"ArrowSelectUserChange"` event (constant `USER_CHANGE`) with a dynamic `direction` property (`-1` or `+1`). `selectedIndex` wraps: setting it below 0 wraps to the last item and vice-versa. `clear(resetText:Boolean = true)` empties the choices array.

**RadioButtonContainer** — auto-discovers `RadioButton` children on `configUI`; the first child is checked by default. `setChecked(rb)` imperatively sets the selection and unchecks all others. Does not dispatch any event itself — listeners should be attached to individual `RadioButton` children.

**KiwiColorPicker2** — a `dynamic` `MovieClip` stub with no methods; exists as a SWF-symbol placeholder (symbol 62) for a second picker asset variant.

**Range** — a plain value object; `contains(n)` returns `true` if `start <= n <= end`.

**DirectionalMapping** — placed by code (not by the timeline) as a named child `"directionalMapping"` on focusable clips; the focus manager retrieves it via `getChildByName` and calls `getAdjacent(dx, dy)` to resolve the next focus target.

**ArcMask** — `draw()` is called immediately when `percent` changes; the arc is drawn from angle 0 sweeping through `percent * arcSize` radians in 12 segments. Negative `rotation` reverses sweep direction. Used for circular HUD cooldowns.

**BitmapAnimator** — not placed on stage by the toolkit; constructed directly: `new BitmapAnimator(animTarget, filterTarget)` then `playShakeAnimation(durationMs, shakePixels, brightnessAmt)`. Self-removes from the display list when animation completes. If a prior `BitmapAnimator` exists on the same parent it is replaced.

**WindowHeader** — the tall window title bar; `title:String` setter writes to `winTitleTextField`. Console mode listens on `ENTER_FRAME` to rebuild constraints whenever the frame changes.

**WindowHeaderSmall** — compact title bar; adds `allowFontResize` (shrinks to 14 pt when text overflows) and calls `KiwiTextUtil.alignTextFieldVertically(tf, 22)` for pixel-perfect vertical centring.

**WindowHeaderLoc / WindowHeaderSmallLoc** — localisation-aware symbol swaps; `WindowHeaderLoc` is a raw `dynamic MovieClip` stub while `WindowHeaderSmallLoc` extends `WindowHeaderSmall` and inherits all of its logic.

**WindowCloseButton** — fires `"OnRequestClose"` and the close-window Wwise sound through `ExternalInterface` on every click; no need to wire a separate listener for the close action.

**SecondaryHeader** — section-level sub-heading between the window title and content; supports the same `title`, `allowFontResize`, and constraint mechanics as `WindowHeaderSmall`, plus `vcenterSingleLineText` mode that selects `CENTER_V` vs `CENTER_V_SINGLE_LINE_TEXT` depending on line count.

**ItemGroupHeading** — bare `MovieClip` with a `nameTextField`; all text assignment is done by the parent container.

**InlineTooltip** — populated entirely through `ExternalInterface` callbacks registered as `"ADD_STAT"` and `"COMPARISON.ADD_STAT"`. Up to 7 `statRow` sub-clips are revealed in sequence. `clear()` must be called before reuse to reset all visible fields.

**MessageQueue** — `addMessage(text)` is the only public input; messages serialise through fade in/hold/fade out automatically. `clearQueue()` stops all tweens and resets state without hiding the clip; `reset()` hides it.

**ResizeBroadcastMovieClip** — `internal` (package-private); used by layout containers to get resize events when a parent's dimensions change programmatically rather than through the component framework.

**DataEvent** — minimal typed event wrapper; construct with `new DataEvent(type, dataObject, bubbles, cancelable)`. The `data` property is public and untyped.

## _kiwi.Controls — Lists, slots, tiles & data views

This group contains the data-driven container and list widgets of the kiwi toolkit. They range from the atomic inventory slot (which knows about rarity, drag-drop, and tooltips) through tile grids and simple stacking lists up to the fully virtualised `DynamicRowView` system, which pages row data lazily from the game engine via JavaScript callbacks. All classes live under the `_kiwi/Controls/` package and extend either `UIComponent` (the kiwi base) or one of the scroll-container base classes defined here.

---

### Summary table

| Class | Extends | Purpose |
|---|---|---|
| `Slot` | `UIComponent` | Full inventory slot: icon, rarity frame, quantity badge, quality stars, hotkey label, cooldown overlay, drag-drop, tooltip bridging |
| `SlotBasic` | `UIComponent` | Lightweight slot (uses `SlotBasic` art symbol): supports locked/darkened/equipped states and a configurable frame tier; no rarity system |
| `BagContainer` | `ResizeBroadcastMovieClip` | Collapsible bag section built from `InventoryRow` rows of full `Slot`s; manages slot lifecycle, capacity counter, and equip/selection state |
| `BagContainerBasic` | `UIComponent` | Same collapsible-bag pattern using `SlotBasic` slots; slots are laid out into a configurable grid directly rather than via `InventoryRow` helpers |
| `DynamicRowView` | `ScrollableView` | Virtualised, section-aware scrolling list; pools row widgets via `ObjectPool` and requests data lazily from an `ExternalDataSource` |
| `DynamicRowViewSection` | `UIComponent` | One collapsible section header inside a `DynamicRowView`; owns positioned row children and calculates its own collapsed/expanded height |
| `DynamicRowViewRow` | `UIComponent` | Abstract base for a single data row; subclassed per screen; exposes `setData()`, `reset()`, and lifecycle hooks `wasAddedToView`/`willRemoveFromView` |
| `DynamicRowViewExternalDataSource` | `Object` | Bridge between `DynamicRowView` and the game engine; translates `ExternalInterface` callbacks into `addSection`/`setRowData` calls on the view |
| `DynamicRowViewUpdateSectionData` | `Object` | Plain value-object DTO: `numRows`, `sectionName`, `sectionInfoText`; used by `ExternalDataSource.updateCurrentView()` |
| `ScrollableTileView` | `ScrollableView` | Wrapping tile grid with configurable x/y spacing and margins; auto-centres a single-row result; optional carousel mode (no wrap) |
| `ScrollableView` | `UIComponent` | Base scroll container: manages `viewportMovieClip` with a `scrollRect`, drives `vScrollbar`/`hScrollbar`, exposes `scrollV`/`scrollH`/`maxScrollV` |
| `SpliceableTileView` | `ScrollableTileView` | `ScrollableTileView` extended with string-keyed identity (`addItemWithId`, `removeItemById`) and index remapping after mid-list removal |
| `StackList` | `UIComponent` | Simple vertical stack: `addChild` appends below the previous item; supports `ItemSpacing`, `removeChild` shifts items up, `RefreshLayout` reflows all |
| `Listbox` | `UIComponent` | Option list backed by a `StackList`; single-selection with a floating `ListboxItem_Selected` overlay; dispatches `Event.SELECT` on change |
| `ListboxItem_Unselected` | `MovieClip` | Skin MC for a non-selected Listbox row; exposes `textfield` |
| `ListboxItem_Selected` | `MovieClip` | Skin MC that floats over the selected row inside `Listbox`; exposes `textfield` |
| `ListboxItem_Disabled` | `MovieClip` | Skin MC for a greyed-out, non-interactive Listbox row; exposes `textfield` |
| `GroupedItemList` | `UIComponent` | Scrollable list of named groups; each group gets an `ItemGroupHeading` label and a `StackList` of items; attaches an external `UIScrollBar` |
| `ClipListContainer` | `ResizeBroadcastMovieClip` | Vertical list of `MovieClip` children; auto-stacks with a configurable `rowMargin`; resizes self and calls an optional callback on every layout change |
| `OptionsContainer` | `UIComponent` | Hover-highlighted vertical menu of two-label option rows (primary text + secondary text); calls `ExternalInterface.call("OnOptionSelected", index)` or an `onItemSelected` function callback |
| `AlignmentCellsRow` | `UIComponent` | Single-row layout helper with three fixed cells: `ALIGN_LEFT=0`, `ALIGN_CENTER=1`, `ALIGN_RIGHT=2`; positions any `DisplayObject` into the chosen cell |
| `StatRow` | `UIComponent` | One name/value pair row for character-sheet stat display; supports `bonus` suffix text, a `bonusLevel` clip, optional red cap colour, and tooltip on hover |
| `RowView` | `ScrollableView` | Simple non-virtualised scrolling list of arbitrary `MovieClip` items stacked vertically with configurable padding |
| `TabHeader` | `UIComponent` | A single navigation tab button; `label` + optional `iconImage`; `set selected(bool)` drives a "selected"/"unselected" timeline frame; supports font down-sizing for long labels |
| `KiwiScrollBar` | `UIScrollBar` | Thin subclass of the FL `UIScrollBar`; adds `fixedThumbHeight` setter and a `variableThumbHeight` mode that sizes the thumb proportionally to visible content |

---

### Key classes

#### `Slot` — `extends UIComponent`

The primary inventory-slot widget used across hotbar, inventory, trade, and equipment screens. Embedded from `symbol37` in `assets.swf`.

**Rarity constants** (public `const`, all `uint`): `Rarity_Unset=-1`, `Rarity_Common=0` … up through `Rarity_Mystic5=31`. Each tier maps to a named `BitmapData` asset (`rarity_frame_<tier>[_large]_png`) loaded via `getDefinitionByName` at draw time; there are separate normal and `_over` (selected) variants.

**Key properties:**

| Property | Type | Notes |
|---|---|---|
| `data` | `Object` | Opaque payload; passed to all `ExternalInterface` calls and to `SlotDragDropHelper.startDrag` |
| `iconImage` | `String` | Texture name fed to the embedded `ObjectPreview`; setting `""` automatically sets `styleHidden=true` |
| `rarity` | `uint` | Triggers `STYLES` invalidation; drives rarity-frame bitmap selection |
| `quality` | `int` | Controls `qualityStars` MC frame (1-indexed) |
| `quantity` / `showQuantity` / `showQuantityWithX` | `Number`/`Boolean` | Badge text on `quantityBadge`; badge resizes its background to fit |
| `selected` | `Boolean` | Shrinks slot to 85 % scale on PC, enlarges to 110 % on console; drives `rarityAnchorMC` animation |
| `equipped` | `Boolean` | Shows `equippedBorder`; suppresses roll-over highlight |
| `ghosted` | `Boolean` | Applies `GhostedFilter` (desaturating `ColorMatrixFilter`) to `rarityAnchorMC`, `art`, `slotFrame` |
| `styleHidden` | `Boolean` | Applies `GhostedFilter + OutlineFilter` to `art` only (empty-slot look) |
| `hotkey` | `String` | Shows in `hotkeyTextField`; on console renders as HTML |
| `percent` | `Number` | `[0,1]` fill for `bar.fill` MC; `<0` hides the bar |
| `dragEnabled` | `Boolean` | Gates `SlotDragDropHelper.startDrag` call on mouse-down |
| `clickFeedback` | `Boolean` | Adds/removes `MOUSE_DOWN`/`CLICK` listeners dynamically |

**Important methods:**

```actionscript
public function copyFrom(src:Slot):void        // bulk-copy all state to this slot
public function clear():void                   // reset data, name, rarity, quantity, quality, image, tooltips
public function activate():void                // fires SLOT.ACTIVATE(data) via ExternalInterface
public function startCooldown(total:Number, elapsed:Number, scale:Number=1):void   // tweens radial sweep on cooldown MC
public function stopCooldown():void
public function setSlotSize(size:Number):void  // resizes the ObjectPreview image container
public function setRarityScale(s:Number):void  // uniform scale on rarityAnchorMC
public function showTooltip():void             // calls SLOT.POINTER_ENTER + TOOLTIP.SHOW
public function showTooltipAt(x:Number=NaN, y:Number=NaN):void
public function hideTooltip():void
```

**Mouse/tooltip bridge:** On roll-over the slot calls `ExternalInterface.call("SLOT.POINTER_ENTER", data, stageX, stageY)` and optionally `TOOLTIP.SHOW`. On roll-out it calls `TOOLTIP.HIDE` and `SLOT.POINTER_LEAVE`. Click calls `SLOT.ACTIVATE(data)` and plays a UI sound via `ExternalInterface.call("POST_SOUND_EVENT", ...)`.

---

#### `SlotBasic` — `extends UIComponent`

A simpler slot skin (symbol `symbol94`) that omits the full rarity system. Uses `SlotBasic` art and `ObjectPreview` for icon display. Supports `locked` (swaps background to `SlotBackgroundLocked`), `darkened`, `equipped` (injects an `Equipped` MC), and a numeric `frameType` (0/1 = Normal, 2 = Medium, 3 = High) that instantiates the appropriate `SlotFrameXxx` MC. Drag-drop and tooltip bridging work identically to `Slot`. Used inside `BagContainerBasic`.

Key methods: `copyFrom(src:SlotBasic)`, `clear()`, `activate()`, `showTooltipAt(x,y)`, `slotImageSize` setter (resizes `ObjectPreview`).

---

#### `BagContainer` — `extends ResizeBroadcastMovieClip`

A collapsible inventory bag section that holds full `Slot` instances. Internally it keeps a `ClipListContainer` of `InventoryRow` MCs, each row containing a fixed number of `Slot` children named `slot_0` … `slot_N`.

```actionscript
public function BagContainer(expandCallback:Function, id:uint)
```

**Key methods:**

```actionscript
public function addObject(slotId:int, name:String, description:String, iconImage:String,
                          quantity:uint, rarity:uint=0, quality:int=0,
                          dragEnabled:Boolean=true, showQty:Boolean=true):Slot
public function removeBySlotId(slotId:int):void       // splices slot out and compacts grid
public function updateBySlotId(slotId:int, qty:uint, rarity:uint):void
public function setSlotEquipStatus(slotId:int, isEquipped:Boolean):void
public function getSlotById(slotId:int):Slot
public function showTooltip(slotIndex:int):void
public function showTooltipAt(slotIndex:int, x:Number, y:Number):void
public function hideTooltip(slotIndex:int):void
public function activateSlot(slotIndex:int):void       // drives slotFrame to "activeCircle"
public function deactivateSlot(slotIndex:int):void     // drives slotFrame to "square"
public function disableHeader():void                   // hides heading, forces expanded, shifts rows up
public function clear():void
```

`maxRowCount` controls how many slots per `InventoryRow`; resizing the heading width to match is done in the setter. `capacity` shows/hides the `categoryCapacity` counter clip.

---

#### `DynamicRowView` — `extends ScrollableView`

The virtualised scrolling list system. Rows are never all instantiated at once; only those whose bounding rectangles intersect the current `viewportMovieClip.scrollRect` are live. A pool (`ObjectPool` seeded with `rowTemplate`, initial capacity 16) provides recycled row instances.

**Structure:** The view holds an ordered `sections:Array` of `DynamicRowViewSection` objects. Each section knows its `numRows` and `rowHeight` and calculates its own pixel height (header height + collapsed/expanded rows). The view computes which sections and which rows within those sections are visible on every scroll event.

**Setting up:**

```actionscript
rowView.rowTemplate = new MyRow();             // seeds the ObjectPool; triggers clear()
rowView.sectionHeaderTemplate = new MySection(); // optional custom section header
rowView.dataSource = new MyDataSource();        // wires the external-data bridge
```

**Key methods:**

```actionscript
public function addSection(numRows:int, category:String, infoText:String,
                           collapsed:Boolean, expandable:Boolean=true,
                           rowHeightPadding:Number=0):void
public function updateSection(index:int, numRows:int, category:String, infoText:String):void
public function removeSection(index:int):void
public function getSection(index:int):DynamicRowViewSection
public function sectionSize(index:int):int
public function setRowData(sectionIdx:int, rowIdx:int, data:Object):void   // called by DataSource
public function getRow(sectionIdx:int, rowIdx:int):DynamicRowViewRow
public function dataDirty():void                // marks sections stale, triggers re-fetch
public function refreshVisibleRows():void       // re-requests data for all currently visible rows
public function refreshLayout():void            // recalculates y positions of all sections
public function expandAllSections():void
public function collapseAllSections():void
public function setScrollPosition(pixelY:Number):void
public function setVScrollbarTo(sectionIdx:int, rowIdx:int):void   // scrolls to bring a row into view
public function editVisibleRows(fn:Function):void     // apply fn to each live DynamicRowViewRow
public function editVisibleSections(fn:Function):void
public function clear():void
public function GetViewportVScrollPosition():Number
```

**Data/recycling lifecycle:**

1. `dataDirty()` sets `sectionsDirty=true` and schedules a redraw.
2. On next `draw()`, if `sectionsDirty` and not already waiting, calls `dataSource.getSectionData()`.
3. The game engine responds synchronously (via `ExternalInterface` callback `setSectionData`) with one call per section, each calling `rowView.addSection(...)`.
4. After sections are laid out, `updateRows()` computes the visible section/row range and calls `dataSource.getData(sectionIdx, rowIdx)` for each visible cell not yet populated.
5. The engine calls the `updateData` or `setRowData` ExternalInterface callback, which resolves to `rowView.setRowData(section, row, dataObject)`.
6. `setRowData` checks out a row from `rowPool`, sets `sectionIndex`/`rowIndex`/data on it, calls `section.addRow(rowIdx, row)` (which positions `row.y = heading.height + rowHeight * rowIndex`), and calls the overridable hook `onRowMadeVisible(row)`.
7. When a row scrolls out of view, `removeVisibleRow` calls `row.willRemoveFromView(this)`, `row.reset()`, removes it from its section, and returns it to the pool.

---

#### `DynamicRowViewSection` — `extends UIComponent`

Manages one collapsible group header and the pixel geometry of its rows. Embedded from `symbol143`.

**Key members:**

```actionscript
public var heading:MovieClip          // contains titleText, infoText, collapsedIcon sub-MCs
public var numRows:int                // total logical rows (triggers recalculateHeight)
public var rowHeight:int              // pixel height of each row
public var category:String            // mapped to heading.titleText.text
public var infoText:String            // mapped to heading.infoText.text
public var collapsed:Boolean          // drives expand()/collapse(); guards expandable
public var expandable:Boolean         // if false, collapse() is always applied
public var disableCollapse:Boolean    // prevents click-to-toggle
public var expandCallback:Function    // called after toggle with `this` as argument
public function addRow(index:int, row:DynamicRowViewRow):void   // sets row.x=0, row.y=header+rowHeight*index
public function removeRow(row:DynamicRowViewRow):void
public function getRow(index:int):DynamicRowViewRow
public function getRowRectLocal(index:int):Rectangle            // for visibility intersection test
public function getRowAbsolutePosition(index:int):Number        // section.y + header + rowHeight*index
public function recalculateHeight():void
public function get headerHeight():int
```

Height is purely computed (`headerHeight + numRows * rowHeight` when expanded; `headerHeight` when collapsed). No scrolling occurs within a section.

---

#### `DynamicRowViewExternalDataSource` — `extends Object`

The glue layer between `DynamicRowView` and the game's JavaScript/Iggy layer. Registers four `ExternalInterface` callbacks at construction time (when `IggyFunctions.inIggy` is true):

| Callback name | Direction | Action |
|---|---|---|
| `setSectionData(total, numRows, category, infoText, collapsed)` | engine → Flash | Calls `rowView.addSection(numRows, category, infoText, collapsed)` |
| `setSectionDataEmpty()` | engine → Flash | Calls `rowView.clear()` |
| `setDataDirty()` | engine → Flash | Calls `rowView.dataDirty()` |
| `updateData()` | engine → Flash | Full refresh: clears, re-fetches sections and rows |
| `updateCurrentView()` | engine → Flash | Incremental update: iterates existing sections, calls `ExternalInterface.call("UpdateSectionData", i)`, reads result from the `updateSectionData` DTO, then calls `rowView.updateSection` or `rowView.removeSection` |

Outbound calls:

```actionscript
public function getSectionData():void   // ExternalInterface.call(externalSectionDataFunctionName)
public function getData(section:int, row:int):void  // ExternalInterface.call(externalRowDataFunctionName, section, row)
```

Default function names are `"GetSectionData"` and `"GetRowData"` but can be overridden via protected setters.

---

#### `DynamicRowViewUpdateSectionData` — `extends Object`

Simple DTO populated by the engine's `UpdateSectionData` JavaScript callback when `updateCurrentView()` runs an incremental refresh. Fields: `numRows:int`, `sectionName:String`, `sectionInfoText:String`. `clear()` zeroes them between section iterations.

---

#### `ScrollableView` — `extends UIComponent`

The foundational scroll container. Embedded from `symbol61`. All content is added as children of `viewportMovieClip`, which is clipped via its `scrollRect`. The scroll coordinate system uses abstract units: `scrollV` is a float in `[0, maxScrollV]`; pixel offset = `scrollV * verticalStep`. `SetContentSize(w, h)` recomputes `maxScrollV = (contentSize.y - viewportHeight) / verticalStep` and re-applies the `scrollRect`.

**Key API:**

```actionscript
public function SetContentSize(w:Number, h:Number):void
public function updateScrollbar(viewportRect:Rectangle):void  // syncs UIScrollBar and re-applies scrollRect
public function scrollTo(position:Number):Boolean             // sets vScrollbar.scrollPosition directly
public var mouseWheelSensitivity:int = 3
public var reverseAdd:Boolean = false   // inserts children at index 0 instead of end
// Properties: scrollV, scrollH, maxScrollV, maxScrollH, verticalStep, horizStep,
//             vertScrollbarVisible, horizScrollbarVisible, viewportWidth, viewportHeight,
//             contentSize:Point, viewportRect:Rectangle, bottomScrollV
```

`addChild`/`removeChild` are overridden to route through `viewportMovieClip`.

---

#### `ScrollableTileView` — `extends ScrollableView`

Wraps items into rows automatically. Items are placed left-to-right; when the next item would exceed `viewportWidth`, placement moves to the next row (using the tallest item in the current row as the row height). The patron-style layout mode (`_patronStyleNX` / `_patronView`) leaves a large first item in place and flows subsequent items around it.

```actionscript
public function setSpacing(xSpacing:Number, ySpacing:Number, xMargin:Number, yMargin:Number=-1):void
public function addItem(mc:MovieClip):int       // places and tracks mc; returns index
public function removeItem(mc:MovieClip):void   // splices and re-flows
public function getItem(index:int):MovieClip
public function sort(compareFn:Function):void   // sorts itemList then refreshLayout()
public function refreshLayout():void
public function clear():void
public function centerRowVertically(b:Boolean):void
public function centerRowHorizontally(b:Boolean):void
public var useCarouselView:Boolean   // disables row-wrapping
public function get numItems():int
```

When a single row is detected on `draw()` and `horizontallyCenterSingleRow` is true, all items are shifted so the row is centred within `viewportWidth`.

---

#### `StackList` — `extends UIComponent`

Simple vertical layout container. `addChild` appends directly below the last child (at `lastChild.y + lastChild.height + vertSpacing`), automatically updating `this.height`. `removeChild` shifts subsequent children upward. `swapChildrenAt` swaps two items and calls `RefreshLayout`. Has special console awareness in `RefreshLayout`: when an item is a `StatRow`, uses `nameTextField.height` rather than the full MC height.

```actionscript
public function RefreshLayout():void
public function clear():void
public var ItemSpacing:Number         // vertical gap between items
public var ChildCount:Number          // read-only, equals items.length
public function refreshLayoutOnRowsUpdated():void  // deferred layout via ENTER_FRAME once all StatRows report textFieldsSetUp
```

---

#### `Listbox` — `extends UIComponent`

A single-selection vertical list. Items are stored in an internal `items` array of `{movieclip, data, disabled}` structs and displayed via an embedded `StackList` (`itemsStackList`). Selection is visually rendered by hiding the clicked item's MC and floating a `ListboxItem_Selected` overlay over it at the same position.

```actionscript
public function addItem(text:String, data:Object, disabled:Boolean=false):void
public function get selectedIndex():int
public function set selectedIndex(i:int):void   // skips disabled items; dispatches Event.SELECT
public function getItemData(index:int):Object
public function getIndexForData(data:Object):int
```

Disabled items use `ListboxItem_Disabled` MC (grey text, no click handler).

---

#### `TabHeader` — `extends UIComponent`

A tab-button widget (symbol `symbol175`) that can carry both a text label and an `ObjectPreview` icon image. Timeline frames provide the selected/unselected visual states.

```actionscript
public function set selected(b:Boolean):void    // gotoAndStop("selected"/"unselected") on self and lightbox MC
public var label:String                          // written to labelField TextField
public var iconImage:String                      // drives embedded ObjectPreview
public var identifier:String                     // opaque string ID for the owning tab system
public var allowFontResize:Boolean               // enables automatic font-size reduction (to 14pt) when label overflows
```

On console, `labelField` is set to `wordWrap=true` and the class listens on `ENTER_FRAME` to detect frame changes and re-apply `Constraints` after timeline jumps.

---

#### `StatRow` — `extends UIComponent`

A single labelled stat entry for the character panel. Constructor signature:

```actionscript
public function StatRow(statName:String, statValue:String, description:String="",
                        isCharPanel:Boolean=false, isStatCapped:Boolean=false)
```

Exposes three `TextField` members: `nameTextField`, `valueTextField`, `bonusTextField`. The `bonusTextField` is positioned dynamically at `nameTextField.x + nameTextField.textWidth`. If `isStatCapped` is true the text is coloured red (`Colors.RED`). `description` enables a tooltip via `ShowTooltip` ExternalInterface call on mouse-over of `nameTextField`. `bonusLevel` drives a `bonusLevelClip` MC to a frame. `shrink()` trims the MC width to its text content. `textFieldsSetUp:Boolean` (read-only) lets `StackList.refreshLayoutOnRowsUpdated()` wait for console async frame readiness.

---

### Remaining classes — notable one-liners

**`RowView`** (`extends ScrollableView`) — non-virtualised vertically-stacked scrolling list; `addItem(mc)` positions each MC below the previous with `itemPadding`; `refreshLayout()` reflows. Used for simpler lists that do not need recycling.

**`SpliceableTileView`** (`extends ScrollableTileView`) — adds string-keyed identity to tile items; `removeItemById(id)` splices the item out of `viewportMovieClip`, updates all subsequent indices in `itemIndexById`, and re-places all shifted items.

**`ClipListContainer`** (`extends ResizeBroadcastMovieClip`) — vertical stack of `MovieClip` items with `rowMargin`; calls an `autoResizeCallback` whenever total height changes; used internally by `BagContainer` to hold `InventoryRow` objects.

**`GroupedItemList`** (`extends UIComponent`) — groups of items each with a heading label and a child `StackList`; scroll is driven by pixel offset applied to group `y` positions rather than `scrollRect`; requires an external `UIScrollBar` to be assigned via `set scrollbar`.

**`OptionsContainer`** (`extends UIComponent`) — two-column (label + secondary label) hover menu; highlight MC tracks the hovered row; calls `ExternalInterface.call("OnOptionSelected", index)` or an `onItemSelected` function.

**`AlignmentCellsRow`** (`extends UIComponent`) — three-cell row (left/centre/right); any `DisplayObject` can be assigned to any cell via `setCellChild(ALIGN_*, displayObject)`.

**`KiwiScrollBar`** (`extends UIScrollBar`) — adds `fixedThumbHeight` and a `variableThumbHeight` mode (`thumb.height = track.height² / (track.height + maxScrollPosition * lineScrollSize)`, minimum 12 px) for proportional thumb sizing.

**`ListboxItem_Unselected`** / **`ListboxItem_Selected`** / **`ListboxItem_Disabled`** — pure skin MCs (`dynamic class`, extends `MovieClip`) with a single `textfield:TextField`; two-frame timelines (`"stop"` + console frame at frame 10).

## com.kiwi.* — legacy kiwi namespace

`com.kiwi.*` is an older incarnation of the kiwi UI framework that predates the active `_kiwi.*` namespace used by the majority of Trove's SWFs. The two namespaces share the same conceptual architecture — constraint system, invalidation-driven component lifecycle, button state machine, event types — but they are distinct codebases: classes in `com.kiwi.*` are not subclasses of their `_kiwi.*` counterparts, and the packages do not cross-import each other (with the sole exception of `com.kiwi.Core.KiwiComponent`, which imports `_kiwi.Controls.Slot` to support the legacy slot system). In practice, only a small number of SWFs (notably **chat**, **info**, **activitytrackerui**, **delveselectorui**, **trade**, **reticle**) still compile the `com.kiwi.*` sources; all others use `_kiwi.*` exclusively.

### Class summary

| Class | Package | Purpose |
|---|---|---|
| `ButtonStates` | Constants | String constants for the button state machine: `up`, `over`, `down`, `release`, `out`, `disabled`, `selecting`, `toggle`, `kb_selecting`, `kb_release`, `kb_down` |
| `ConstrainMode` | Constants | Two scaling modes for the constraint system: `COUNTER_SCALE` (children counter-scale to stay pixel-exact as the parent scales) and `REFLOW` (children reflow their positions instead) |
| `InvalidationType` | Constants | Dirty-flag string tokens used by `KiwiComponent.invalidate()`: `ALL`, `SIZE`, `STATE`, `DATA`, `SETTINGS`, `RENDERERS`, `SCROLL_BAR`, `SELECTED_INDEX` |
| `KeyCodes` | Constants | Full keyboard keycode lookup table (A–Z, 0–9, navigation/modifier keys). Identical in scope to `_kiwi`'s equivalent. |
| `ConstrainedElement` | Constraints | Plain value object recording a child `DisplayObject` alongside its pinned edge bitmask and the initial left/top/right/bottom offsets and scaleX/scaleY captured at registration time |
| `Constraints` | Constraints | Anchor-based layout engine. Manages a `Dictionary` of `ConstrainedElement` entries and, on `update(width, height)`, repositions/resizes each child according to its edge bitmask (`LEFT`, `RIGHT`, `TOP`, `BOTTOM`, `CENTER_H`, `CENTER_V`). Supports both `COUNTER_SCALE` mode (children counter-scale their own `scaleX`/`scaleY` to cancel out parent scaling) and `REFLOW` mode (pure position/size adjustment without scale compensation). Automatically walks the display hierarchy on `ADDED_TO_STAGE` to chain itself to any ancestor `Constraints` instance via `ResizeEvent`. |
| `KiwiComponent` | Core | Base `MovieClip` subclass for all `com.kiwi.*` UI components. Maintains an internal `_width`/`_height` independent of Flash's raw display-list dimensions, owns a `Constraints` object, and implements an `invalidate()` / `validateNow()` deferred-draw cycle using `Event.RENDER` / `Event.ENTER_FRAME`. Exposes typed `invalidateSize()`, `invalidateData()`, `invalidateState()`, etc. convenience methods and hooks for focus handling (`focusable`, `focused`, `displayFocus`, `focusTarget`). Also holds up to six `Slot` references (imported from `_kiwi.Controls.Slot`) for legacy slot-based UI. |
| `ListItemData` | Data | Minimal value object: `index:uint`, `label:String`, `selected:Boolean`. Used by list renderers implementing `IListItemRenderer`. |
| `KiwiButtonEvent` | Events | Extends `Event` with button-interaction semantics. Constants: `PRESS`, `CLICK`, `DRAG_OVER`, `DRAG_OUT`, `RELEASE_OUTSIDE`. Carries `controllerIndex`, `buttonIndex`, `isKeyboard`, and `isRepeat` to distinguish mouse from keyboard/gamepad sources and to flag auto-repeat firings. |
| `KiwiEvent` | Events | General-purpose kiwi component event. Constants: `STATE_CHANGE`, `SHOW`, `HIDE`. Dispatched by `KiwiButton` when its visual state transitions, and by `KiwiComponent.visible` setter. |
| `ResizeEvent` | Events | Carries `scaleX:Number` and `scaleY:Number` payloads through the `Constraints` chain when a scope is resized. Constants: `RESIZE` and `SCOPE_ORIGINALS_UPDATED`. |
| `IKiwiComponent` | Interfaces | Interface contract for all kiwi components: x/y, width/height, enabled, tabEnabled, alpha, `focusTarget`, `validateNow()`, `handleInput()`. Extends `IEventDispatcher`. |
| `IListItemRenderer` | Interfaces | Extends `IKiwiComponent` with list-cell contract: `index`, `owner`, `selectable`, `selected`, `setListData(ListItemData)`, `setData(Object)`. |
| `KiwiTextfield` | Styles | Thin `TextFormat` subclass that hard-codes the **Comfortaa** typeface and exposes a single constructor `(size, color, bold, italic, align)`. Used by legacy chat SWF text fields. |
| `KiwiButton` | Templates | Full button implementation extending `KiwiComponent`. Owns the state machine driven by `_stateMap` (mapping state name strings to ordered frame-label arrays), supports toggle mode, auto-repeat (configurable `repeatDelay`/`repeatInterval` via a `Timer`), label text field auto-sizing (`LEFT`/`RIGHT`/`CENTER`), keyboard and mouse interaction, focus-indicator sub-clip, and an optional owner (`KiwiComponent`) for coordinated selection focus. Dispatches `KiwiButtonEvent.PRESS`, `.CLICK`, `.RELEASE_OUTSIDE`, `.DRAG_OVER`, `.DRAG_OUT`, and `KiwiEvent.STATE_CHANGE`. |
| `StatusBarVertical` | Templates | Arc-mask progress indicator extending `KiwiComponent`. Draws a vector arc into a mask `MovieClip` over a `_bar` clip to display a `percent` value (0–1). Arc geometry is inferred from child clips (`_empty`, `_full`, `_center`, `_half`) placed on the timeline. Supports a `flipped` flag to reverse direction. Fades the entire component to alpha 0 via an `IggyTween` when `percent` reaches 1. Receives updates via an `ExternalInterface` callback registered at `currentEvent`. Used for ability/mechanic charge indicators in the reticle HUD. |

### Notes

**Relationship to `_kiwi.*`:** The two namespaces are structurally parallel: `com.kiwi.Core.KiwiComponent` ↔ `_kiwi.Core.KiwiComponent`; `com.kiwi.Constraints.Constraints` ↔ `_kiwi.Constraints.Constraints`; `com.kiwi.Templates.KiwiButton` ↔ `_kiwi.Controls.BaseButton`; and so on. Because `com.kiwi.Core.KiwiComponent` imports `_kiwi.Controls.Slot`, any SWF that pulls in the `com.kiwi.*` sources must also have `_kiwi.Controls.Slot` available — meaning the two namespaces coexist in the same SWF rather than being mutually exclusive.

**Constraint engine detail:** `Constraints.addElement(name, clip, edgeMask)` captures the element's position and scale at registration time. `update(w, h)` iterates all registered elements and applies the edge rules. In `COUNTER_SCALE` mode the children's own `scaleX`/`scaleY` are adjusted to cancel out parent scale, keeping them at a fixed logical size; in `REFLOW` mode scales are left alone and only position/size is adjusted. Chaining to ancestor constraints (via `ResizeEvent`) propagates the effective scale ratio down the display tree.

**`KiwiButton` state machine detail:** Button visual states are driven by `gotoAndPlay()` calls into the host `MovieClip`'s timeline. `setState()` consults `_stateMap` to produce an ordered list of candidate frame labels (e.g. `release` → tries `release` then `over` as a fallback) and, for toggle-selected buttons, first tries a `selected_`-prefixed variant. If no matching label is found the frame does not change. This makes the state machine gracefully degrade if a skin lacks a given label.

## fl.* — Adobe Flash component framework (stock)

Everything under `fl.*` (and the single `flash.utils.natCaseCompare` free function) is **Adobe stock code**, not authored by Trove or the kiwi team. These files are emitted verbatim by the Flash CS/Flash Professional IDE when any built-in component (Button, List, ComboBox, etc.) is placed on a SWF's stage. They are included here because the manifest records them as shared across multiple SWFs; the descriptions below document how they are actually used by Trove screens rather than exhaustively covering the full Adobe API.

---

### fl.core — component base layer

These two classes underpin every other `fl.*` component.

**`UIComponent`** (`fl.core.UIComponent`, extends `flash.display.Sprite`)
The root base class for all Adobe FL components. Implements the same deferred-invalidation pattern used by the kiwi framework (`invalidate()` queues a `callLater` redraw, `draw()` is the override point) but through Flash's own `callLater` mechanism rather than `Event.RENDER`. Owns the style system integration (`getStyle`, `setStyle`, `StyleManager.registerInstance`), focus management wiring (`FocusManager` attach/detach on `ADDED_TO_STAGE`/`REMOVED_FROM_STAGE`), and the `version` constant (`"3.0.3.1"`). Default styles include a `_sans` 11pt text format and focus-rect skin references. Trove components that use the `fl.*` widget set (settings sliders, mod-loader lists, account-linking text inputs) all ultimately extend this class.

**`ComponentShim`** (`fl.core.ComponentShim`, extends `MovieClip`)
An embedded-asset shim (`[Embed(source="/_assets/assets.swf", symbol="symbol108")]`) that loads the shared symbol library used by all FL component skins. Without it the skin-name strings in `defaultStyles` would resolve to nothing. Its sole purpose is to force the asset SWF into the player's symbol registry.

**`InvalidationType`** (`fl.core.InvalidationType`)
String constants for the FL invalidation queue: `ALL`, `SIZE`, `STYLES`, `STATE`, `DATA`, `SCROLL`, `RENDERER`, `SELECTED_ITEM`. Parallel in purpose to the `com.kiwi.Constants.InvalidationType` constants.

---

### fl.controls — interactive widgets

The components Trove screens actually instantiate:

**`Button`** (extends `LabelButton`)
Standard push/toggle button with an optional `emphasized` border skin. Used in settings, mod loader, and account-linking screens. Inherits the full `LabelButton` state machine (up/over/down/disabled skins, label text field, auto-size).

**`LabelButton`** (extends `BaseButton`, not directly instantiated by Trove)
Intermediate class that adds a `label` text field, icon field, `toggle` mode, and `selected` state to `BaseButton`. The shared skin-style keys (`upSkin`, `downSkin`, `overSkin`, `disabledSkin`, `selectedUpSkin`, etc.) are defined here.

**`BaseButton`** (extends `UIComponent`)
Core mouse/keyboard event wiring for all button types: `ROLL_OVER`/`OUT`, `MOUSE_DOWN`/`UP`, auto-repeat timer (`repeatDelay`, `repeatInterval`), and dispatching of `ComponentEvent.BUTTON_DOWN`.

**`List`** (extends `SelectableList`)
Single-column scrollable list backed by a `DataProvider`. Key properties: `rowHeight` (default 20 px), `labelField`/`labelFunction`, `iconField`/`iconFunction`, `cellRenderer` (defaults to `CellRenderer`). Used by charsheet and other screens to display enumerable item sets. Keyboard navigation (Up/Down/Page/Home/End) is handled natively.

**`SelectableList`** (extends `BaseScrollPane`)
Intermediate class providing multi-item selection logic (single or multi-select), `DataProvider` binding, and the cell-renderer pool/recycling mechanism. `List` and `ComboBox`'s drop-down both extend this.

**`ComboBox`** (extends `UIComponent`)
Drop-down selector: closed state shows the selected item in a text field; clicking opens a `List` overlay. `dataProvider`, `selectedIndex`/`selectedItem`, `labelField`, `prompt` (placeholder text), `editable` (free-text mode), and `rowCount` (max visible rows before scrolling). Used by the settings screen for option pickers.

**`ScrollBar`** (extends `UIComponent`)
Standalone scroll bar with configurable `minScrollPosition`, `maxScrollPosition`, `pageSize`, and `lineScrollSize`. Emits `ScrollEvent` on thumb drag or arrow-button click. Arrow buttons use the auto-repeat timer. Fixed width constant `ScrollBar.WIDTH = 15`. Used internally by `BaseScrollPane` but also directly in the mod-loader screen.

**`UIScrollBar`** (extends `ScrollBar`)
Convenience wrapper that can be linked to any `DisplayObject` that exposes `scrollV`/`maxScrollV` (or `scrollH`/`maxScrollH`) properties via `scrollTarget`. Listens to `Event.SCROLL` on the target to keep in sync, and dispatches its own `ScrollEvent` upward. Used to scroll Flash `TextField` objects (e.g. chat, mod descriptions).

**`TextInput`** (extends `UIComponent`)
Single-line editable text field with up/disabled skins. Properties: `text`, `htmlText`, `maxChars`, `restrict`, `editable`, `displayAsPassword`. Exposes the inner `textField:TextField` for direct formatting. Used by account-linking forms and search/filter fields. Emits `ComponentEvent.ENTER` on Return key.

**`Label`** (extends `UIComponent`)
Non-interactive display-only text label. Supports `text` and `htmlText`; auto-sizes to content when `autoSize` is set. Used throughout crafting, charsheet, and other read-only display contexts.

**`Slider`** (extends `UIComponent`)
Horizontal or vertical drag-thumb value picker. Properties: `minimum`, `maximum`, `value`, `snapInterval`, `tickInterval`, `liveDragging`. Emits `SliderEvent.CHANGE` and `.THUMB_PRESS`/`.THUMB_RELEASE`. Used by the settings screen for audio/graphics sliders.

**`ButtonLabelPlacement`**, **`ScrollBarDirection`**, **`ScrollPolicy`**, **`SliderDirection`**
Enum-style string-constant classes controlling layout orientation and overflow policy on their respective components.

---

### fl.controls.listClasses — cell rendering

**`CellRenderer`** (extends `LabelButton`, implements `ICellRenderer`)
The default row renderer for `List` and `SelectableList`. Toggled-selected when its row is selected; never focus-enabled (focus stays on the parent list). Style keys add `selectedUpSkin`, `selectedDownSkin`, `selectedOverSkin` variants on top of the standard button skins. Text padding defaults to 5 px. Trove screens can substitute a custom renderer class via `List.cellRenderer`.

**`ICellRenderer`**
Interface contract: `data:Object`, `listData:ListData`, `setSize(w, h)`, standard `UIComponent` display properties. Any class assigned to `List.cellRenderer` must implement this.

**`ListData`**
Value object passed by `SelectableList` to each `ICellRenderer` on render: `label:String`, `icon:Object`, `owner:List`, `index:int`, `row:int`.

---

### fl.containers — scroll pane

**`BaseScrollPane`** (extends `UIComponent`)
Abstract base for any scrollable container. Manages `_verticalScrollBar` and `_horizontalScrollBar` (`ScrollBar` instances), a masked content clip, `ScrollPolicy` (ON / OFF / AUTO) for each axis, and `contentPadding`. Subclasses (`SelectableList`, and in principle `ScrollPane`) override `drawLayout()` to place their content. Emits `ScrollEvent` when scroll position changes.

---

### fl.data — data model

**`DataProvider`** (extends `EventDispatcher`)
Observable array wrapper consumed by `SelectableList`/`List`/`ComboBox`. Supports `addItem`, `addItemAt`, `removeItem`, `removeItemAt`, `replaceItemAt`, `sortOn`, and bulk operations. Dispatches `DataChangeEvent` (pre-change and post-change) for each mutation so the bound component can update only the affected rows.

**`SimpleCollectionItem`**
Plain Object subclass with `label:String` and `data:Object`; created internally by `DataProvider` when a raw string is added.

---

### fl.managers — focus and style singletons

**`FocusManager`** (implements `IFocusManager`)
Singleton-per-container that manages Tab-key focus traversal and the default button (`Button` that responds to Enter). Attaches to a `DisplayObjectContainer` (usually the SWF root or a modal panel) on `ADDED_TO_STAGE`, walks the display tree to build a sorted `focusableCandidates` list, and intercepts `KEY_DOWN` (Tab/Shift-Tab) to advance focus. Provides `showFocusIndicator:Boolean` to toggle the visual focus ring.

**`IFocusManager`**, **`IFocusManagerComponent`**, **`IFocusManagerGroup`**
Interfaces: `IFocusManager` defines `setFocus`/`getFocus`/`findFocusManagerComponent`/`showFocusIndicator`; `IFocusManagerComponent` marks a `UIComponent` as tab-stoppable; `IFocusManagerGroup` groups radio-button-style mutually-exclusive components.

**`StyleManager`** (singleton)
Central registry mapping style property names to the `UIComponent` subclasses that declare them, and those classes to their live instances. `UIComponent.setStyle()` calls through here so that a single `StyleManager.setStyle("textFormat", fmt)` affects every registered component of the matching class. Styles cascade: instance → class → global → default.

---

### fl.events — component events

Stock event classes; all extend `flash.events.Event`. No Trove-specific logic.

| Class | Key constants |
|---|---|
| `ComponentEvent` | `BUTTON_DOWN`, `ENTER`, `HIDE`, `LABEL_CHANGE`, `RESIZE`, `SHOW` |
| `DataChangeEvent` | `DATA_CHANGE`, `PRE_CHANGE` — carries `changeType`, `startIndex`, `endIndex`, `items` |
| `DataChangeType` | `ADD`, `INVALIDATE`, `INVALIDATE_ALL`, `MOVE`, `REMOVE`, `REPLACE_ITEM`, `SORT` |
| `ListEvent` | `ITEM_CLICK`, `ITEM_DOUBLE_CLICK`, `ITEM_ROLL_OVER`, `ITEM_ROLL_OUT` — carries `rowIndex`, `columnIndex`, `index`, `item` |
| `ScrollEvent` | `SCROLL` — carries `direction` and `position` |
| `SliderEvent` | `CHANGE`, `THUMB_DRAG`, `THUMB_PRESS`, `THUMB_RELEASE` — carries `value`, `clickTarget`, `triggerEvent` |
| `SliderEventClickTarget` | Constants: `THUMB`, `TRACK` |
| `InteractionInputType` | Constants: `KEYBOARD`, `MOUSE` — used by `SliderEvent` |

---

### fl.transitions — tween and transition engine

Stock Adobe tween library; used sparingly by Trove for clip fades and show/hide transitions (notably `kiwistore`, `questtracker`, `pvphud`, `geodeincubator`, `gems`).

**`Tween`** — frame- or time-based property interpolator. Constructor: `new Tween(obj, prop, easingFunc, begin, finish, duration, useSeconds)`. Drives itself via an internal `Timer` or `ENTER_FRAME` listener. Emits `TweenEvent.MOTION_START/CHANGE/FINISH/LOOP/RESUME`.

**`Transition`** / **`TransitionManager`** — higher-level clip transition system. `TransitionManager` attaches to a `MovieClip`, accepts multiple concurrent `Transition` instances (e.g. `Fade`), applies them to the clip's visual properties, and dispatches `IN_START`/`IN_END`/`OUT_START`/`OUT_END` events. `Fade` is the only `Transition` subclass present in the shared union.

**`TweenEvent`** — events dispatched by `Tween` and `TransitionManager`.

Easing functions (all stateless classes with static `easeIn`/`easeOut`/`easeInOut` methods):

| Class | Curve character |
|---|---|
| `easing.None` | Linear (no easing) |
| `easing.Regular` | Quadratic |
| `easing.Strong` | Quintic |
| `easing.Bounce` | Bouncing deceleration |
| `easing.Elastic` | Overshoot spring |

---

### fl.motion — Flash IDE motion XML animation runtime

Stock runtime for the keyframe-based motion paths exported by Flash Pro's Motion Editor. Present in the shared union because `companionforge` and `gems` SWFs use IDE-generated motion tweens.

**`AnimatorBase`** / **`Animator3D`** — apply a `MotionBase` keyframe sequence to a `DisplayObject` each `ENTER_FRAME`. Handle color transforms, filters, blend modes, caching, and visibility. `Animator3D` additionally drives `z`, `rotationX`/`Y`/`Z` for 2.5D motion.

**`AnimatorFactoryBase`** / **`AnimatorFactory3D`** — companion factories that instantiate `AnimatorBase`/`Animator3D` instances for a given target clip.

**`MotionBase`** / **`KeyframeBase`** — data model. `MotionBase` holds the ordered `keyframes:Array` and a duration; `KeyframeBase` stores per-frame transform properties (x, y, scaleX, scaleY, skewX, skewY, rotationConcat, filters, `Color`).

| Class | Role |
|---|---|
| `Color` | `ColorTransform` subclass with tint helpers (`tintColor`, `tintMultiplier`) and XML/interpolation factory methods |
| `ColorMatrix` / `AdjustColor` | Builds a 4×5 color-matrix filter for brightness/contrast/saturation/hue adjustments |
| `DynamicMatrix` | General-purpose resizable matrix; used internally by `AdjustColor` |
| `MatrixTransformer3D` | Decomposes/recomposes 3D transformation matrices |
| `MotionEvent` | `MOTION_START`, `MOTION_END`, `MOTION_LOOP`, `MOTION_CHANGE` dispatched by animators |
| `RotateDirection` | Constants: `AUTO`, `CLOCKWISE`, `COUNTER_CLOCKWISE` |
| `Tweenables` | String constants for tweeneable property names (`x`, `y`, `scaleX`, …, `brightness`, `tintMultiplier`, etc.) |
| `motion_internal` | Custom namespace used to gate internal animator APIs |
| `easing.Exponential` | Exponential easing function (present via `gems`) |

---

### flash.utils — utility

**`natCaseCompare(a, b, foldCase)`** (`flash.utils` package-level function) — natural-sort string comparator. Treats embedded digit runs as numeric values (so `"item10"` sorts after `"item9"`), collapses leading zeros, and optionally folds case. Used by `friendpicker` to sort player-name lists. Not part of the standard Flash player API; this is Trove-included helper code shipped inside the `flash.utils` package for convenience.

## mx.* — Adobe Flex framework (stock, partial)

The `mx.*` classes present in the shared union are unmodified Adobe Flex 4 SDK sources (version string `"4.0.0.0"` throughout), included because `chat.swf` is built on Flex-derived text and component infrastructure. Flex requires a set of core interfaces and service singletons (resource management, module loading, layout geometry) to be resolvable at runtime, so those classes were compiled into the SWF. No `mx.*` code was written by the Trove team; everything here is stock Adobe. The remaining SWFs do not use these classes — they appear only in the `chat` folder of the shared union.

---

### mx/flash — Flex Component Kit wrappers (concrete, most important)

These three classes form the **Flex Component Kit** bridge that lets a Flash-authored `MovieClip` symbol participate fully in a Flex component tree (layout, focus, states, styling, automation).

#### `UIMovieClip`

`public dynamic class UIMovieClip extends MovieClip` — the base bridge class. Implements `IDeferredInstantiationUIComponent`, `IToolTipManagerClient`, `IStateClient`, `IFocusManagerComponent`, `IConstraintClient`, `IAutomationObject`, `IVisualElement`, `ILayoutElement`, and `IFlexModule`. Key responsibilities:

- Tracks `explicitWidth`/`explicitHeight` and measured size; calls `setActualSize(w, h)` when Flex lays it out.
- Manages Flex states via a `stateMap` object: frame labels are mapped to state names; transitioning between states plays timeline frames between `transitionStartFrame` and `transitionEndFrame`.
- Propagates `MoveEvent`, `ResizeEvent`, `FlexEvent` (CREATION\_COMPLETE, INITIALIZE, etc.) at the correct Flex lifecycle points.
- Implements constraint properties (`left`, `right`, `top`, `bottom`, `horizontalCenter`, `verticalCenter`, `baseline`) for Flex anchor-based layout.
- Delegates 2D/3D transform arithmetic to `AdvancedLayoutFeatures` and `MatrixUtil`.
- `boundingBoxName` (default `"boundingBox"`) names a child clip whose bounds are used as the component's measured size, hiding the actual bounding box from Flex layout.
- `autoUpdateMeasuredSize` — when true, re-measures every frame; `autoUpdateCurrentState` — syncs `currentState` to the timeline's current label automatically.

#### `ContainerMovieClip`

`public dynamic class ContainerMovieClip extends UIMovieClip implements IVisualElementContainer`

Extends `UIMovieClip` to hold exactly **one** Flex `IUIComponent` child, exposed via the `content` property. At runtime it looks for a `FlexContentHolder` child clip (by scanning `getChildAt` in order); if found, delegation goes through that holder. `scaleContentWhenResized` (default `false`) controls whether stretch factors from `_layoutFeatures` are applied to the holder or left at 1. `addElement`/`removeElement` and related `IVisualElementContainer` mutations all throw `ArgumentError` — this container is single-child, set-only via `content`.

#### `FlexContentHolder`

`public dynamic class FlexContentHolder extends ContainerMovieClip`

The in-library symbol companion to `ContainerMovieClip`. It is the actual child clip placed inside the Flash symbol on the timeline; its job is to host and size the live Flex `IUIComponent`. On `initialize()` it hides the placeholder symbol at child index 0 by setting its `alpha` to 0, then calls `setFlexContent()` with any pending content. `setFlexContent()` adds the Flex component as a display child, walks up the parent chain to find a `UIComponent` ancestor (via `getDefinitionByName("mx.core::UIComponent")`), propagates `document`, `moduleFactory`, style cache, and nest level, then calls `flexContent.initialize()`. `sizeFlexContent()` resolves `percentWidth`/`percentHeight` against the holder's current pixel dimensions (adjusted for stretch factors when `scaleContentWhenResized` is false) and calls `setActualSize` on the content.

---

### mx/resources — Localization (concrete)

These classes implement Flex's locale-keyed string/value lookup system.

#### `ResourceManager`

Static facade. `ResourceManager.getInstance(): IResourceManager` returns the process-wide singleton, registering `ResourceManagerImpl` via `Singleton` on first call (with fallback `new ResourceManagerImpl()` if Singleton registration fails).

#### `ResourceManagerImpl`

The real implementation (`extends EventDispatcher implements IResourceManager`). Maintains `localeMap: Object` — a two-level map of `locale → bundleName → IResourceBundle`. Key API:

| Method | Description |
|---|---|
| `installCompiledResourceBundles(domain, locales, bundleNames)` | Iterates all locale×bundleName pairs and instantiates compiled bundle classes (named `locale$bundleName_properties`) from the ApplicationDomain. |
| `loadResourceModule(url, update, appDomain, secDomain): IEventDispatcher` | Loads a resource module SWF via `ModuleManager`; on `ModuleEvent.READY` extracts its bundles and optionally calls `update()`. |
| `unloadResourceModule(url, update)` | Removes all bundles contributed by the module, then unloads it. |
| `getString(bundle, key, params, locale): String` | Looks up a string; if `params` is provided, substitutes `{0}`, `{1}`, … via `StringUtil.substitute`. |
| `getObject/getNumber/getInt/getUint/getBoolean/getClass` | Type-coercing accessors for bundle entries. |
| `localeChain: Array` | Ordered list of locale codes for fallback search; setting it fires `Event.CHANGE`. |
| `initializeLocaleChain(compiledLocales)` | Calls `LocaleSorter.sortLocalesByPreference` against `Capabilities.languages` to pick the best match. |

Dispatches `Event.CHANGE` whenever the effective resource set changes (locale switch, module load/unload).

#### `ResourceBundle`

Base class for compiled bundle classes (`implements IResourceBundle`). Constructor: `ResourceBundle(locale, bundleName)` — calls `getContent()` (overridden by generated subclasses) to populate `content: Object` (a plain AS3 object mapping string keys to values). Properties: `locale`, `bundleName`, `content` (read-only). The static `mx_internal var locale` holds the current active locale string set during `processInfo`.

**Resource interfaces (stubs):** `IResourceManager` (contract implemented by `ResourceManagerImpl`), `IResourceBundle` (contract implemented by `ResourceBundle`), and `IResourceModule` (marker interface for a compiled resource module — exposes `resourceBundles`/`bundles` so a dynamically loaded module can hand its bundles to the manager). `LocaleSorter` is a stock helper that orders locale chains by preference.

---

### mx/utils — Utility helpers (concrete)

#### `StringUtil`

All-static utility class. Methods:

| Method | Signature | Description |
|---|---|---|
| `trim` | `(s: String): String` | Strips leading/trailing whitespace (space, tab, CR, LF, FF). |
| `trimArrayElements` | `(s: String, delim: String): String` | Splits on `delim`, trims each element, rejoins. |
| `substitute` | `(str: String, ...rest): String` | Replaces `{0}`, `{1}`, … tokens; `rest` may be varargs or a single Array. Used by `ResourceManagerImpl.getString`. |
| `repeat` | `(s: String, n: int): String` | Concatenates `s` to itself `n` times. |
| `restrict` | `(s: String, pattern: String): String` | Filters characters by a TextField `restrict`-style pattern (supports ranges `a-z`, negation `^`, escape `\`). |
| `isWhitespace` | `(c: String): Boolean` | Returns true for space/tab/CR/LF/FF. |

#### `MatrixUtil`

All-static (`public final class`). Provides 2D/3D matrix composition, decomposition, and transformation helpers used by `AdvancedLayoutFeatures`, `CompoundTransform`, and `UIMovieClip`. Key methods:

| Method | Description |
|---|---|
| `composeMatrix(x,y,scaleX,scaleY,rotation,transformX,transformY)` | Builds a 2D `Matrix` from decomposed properties around a transform point. |
| `decomposeMatrix(result, m, transformX, transformY)` | Extracts `[x, y, scaleX, scaleY, rotation]` from a `Matrix` into a `Vector.<Number>`. |
| `transformPoint(x, y, m): Point` | Applies matrix to a point (null matrix = identity); reuses a static `Point` for allocation-free calls. |
| `clampRotation(r): Number` | Normalises rotation to `(-180, 180]`. |
| Various 3D helpers | `fitBounds`, `getConcatenatedComputedMatrix3D`, `projectBounds`, etc. — used by `CompoundTransform` for perspective/3D layout. |

---

### mx/geom — Layout transform geometry (concrete)

#### `mx.geom.Transform`

Extends `flash.geom.Transform`. Holds a reference to an `IVisualElement` `target`; overrides `matrix`, `matrix3D`, `colorTransform`, `perspectiveProjection`, `pixelBounds`, and `concatenatedMatrix`/`concatenatedColorTransform` to delegate to the target's internal `$transform` (Flex's shadow transform) or its `displayObject` when available, falling back to the native Flash `Transform` otherwise. Setting `matrix` on a layout element calls `ILayoutElement.setLayoutMatrix(m, true)` instead of the raw Flash setter.

#### `mx.geom.TransformOffsets`

`extends EventDispatcher`. A set of additive/multiplicative post-layout deltas: `x`, `y`, `z`, `rotationX`, `rotationY`, `rotationZ`, `scaleX`, `scaleY`, `scaleZ`. Each setter dispatches `Event.CHANGE` and invalidates the cached 3D-flag. The `mx_internal is3D` getter checks lazily whether any Z-axis property is non-trivial. Referenced by `AdvancedLayoutFeatures` as `_postLayoutTransformOffsets`.

#### `mx.geom.CompoundTransform`

The authoritative store for a Flex component's layout transform. Maintains either decomposed properties (`x/y/z/rotationX/rotationY/rotationZ/scaleX/scaleY/scaleZ`) or raw `Matrix`/`Matrix3D`, tracking which is the canonical source (`SOURCE_PROPERTIES`, `SOURCE_MATRIX`, `SOURCE_MATRIX3D`). Lazily re-derives whichever representation is stale via `MatrixUtil.decomposeMatrix`/`composeMatrix`. Used exclusively by `AdvancedLayoutFeatures`.

---

### mx/core — Core interfaces and concrete helpers

Most `mx/core` classes are interfaces or thin stubs; two concrete helpers deserve detail.

#### `Singleton`

Global class-registry for Flex service singletons. `registerClass(interfaceName, cls)` maps a fully qualified interface string to its implementation class (no-op if already registered). `getInstance(interfaceName)` calls `cls.getInstance()` on the registered class; throws if none registered. Used by `ResourceManager` to obtain `ResourceManagerImpl`.

#### `AdvancedLayoutFeatures`

The component-side transform cache. Stores a `CompoundTransform layout` (the component's own placement) and a `TransformOffsets _postLayoutTransformOffsets` (applied after layout). Exposes `computedMatrix: Matrix` and `computedMatrix3D: Matrix3D`, invalidating lazily when `layout` or offsets change. Tracks `stretchX`/`stretchY` (applied by a Flex layout pass), `depth` (z-ordering), and `updatePending` for deferred commit. Consumed by `UIMovieClip` for all transform arithmetic.

**Remaining `mx/core` classes — interfaces and minor concretes:**

| Class | Purpose |
|---|---|
| `IFlexDisplayObject` | Interface: full `DisplayObject` shape plus `measuredWidth/Height`, `move()`, `setActualSize()`. |
| `IUIComponent` | Extends `IFlexDisplayObject`; adds `document`, `enabled`, `explicit*` sizing, `focusPane`, `systemManager`, `initialized`, `initialize()`, `owns()`. |
| `IVisualElement` | Extends `ILayoutElement`; adds `owner`, `displayObject`, `depth`, `designLayer`, `postLayoutTransformOffsets`, visibility. |
| `IVisualElementContainer` | Container that holds `IVisualElement` children; defines `numElements`, `addElement`, `removeElement`, `getElementAt`, `getElementIndex`, etc. |
| `ILayoutElement` | Layout constraint properties (`left/right/top/bottom/horizontalCenter/verticalCenter/baseline`, `percentWidth/Height`), explicit/measured/min/max sizing, `setLayoutMatrix*`, `getLayoutMatrix*`. |
| `IDeferredInstantiationUIComponent` | Extends `IUIComponent`; adds `cacheHeuristic`, `cachePolicy`, `createReferenceOnParentDocument`, `deleteReferenceOnParentDocument`. |
| `IChildList` | Read-only child enumeration: `numChildren`, `getChildAt`, `getChildByName`, `getChildIndex`, `contains`. |
| `IConstraintClient` | `getConstraintValue(name)` / `setConstraintValue(name, value)`. |
| `IFlexModule` | `moduleFactory: IFlexModuleFactory` get/set. |
| `IFlexModuleFactory` | `create(...args): Object`, `info(): Object`, `registerImplementation`, `getImplementation`. |
| `IInvalidating` | `invalidateDisplayList()`, `invalidateProperties()`, `invalidateSize()`, `validateNow()`. |
| `IMXMLObject` | `initialized(document, id)` — called by MXML compiler on non-visual objects. |
| `IStateClient` | `currentState: String` get/set. |
| `IToolTip` | Tooltip display interface: `x/y/width/height`, text. |
| `IVisualElement` | (see above) |
| `LayoutElementUIComponentUtils` | Static helpers bridging `ILayoutElement` and `IUIComponent` sizing conventions (preferred/min/max bounds, percent sizing). |
| `ComponentDescriptor` | Descriptor holding class + properties + events + effects for deferred MXML instantiation. |
| `UIComponentDescriptor` | Extends `ComponentDescriptor`; adds `document`, `id`, `stylesFactory`, `propertiesFactory`, `events`, `effects` for Flex component trees. |
| `DesignLayer` | `EventDispatcher`-based tree node for design-time layers; tracks `visible`, `alpha`, and a list of child `DesignLayer` instances; dispatches `PropertyChangeEvent` on mutation. |
| `DragSource` | Carries typed data payloads for a drag operation: `addData(obj, format)`, `addHandler(fn, format)`, `dataForFormat(format)`, `hasFormat(format)`. |
| `FlexVersion` | Constants for SDK version gates (`VERSION_2_0`, `VERSION_3_0`, `VERSION_4_0`); `compatibilityVersion` property allows older rendering behaviour. |
| `mx_internal` | Namespace declaration only — no logic. |

---

### mx/events — Flex event types

All are concrete `Event` subclasses carrying typed payloads; none contain business logic.

| Class | Key payload / type string |
|---|---|
| `FlexEvent` | 30+ static string constants for Flex lifecycle events: `CREATION_COMPLETE`, `INITIALIZE`, `APPLICATION_COMPLETE`, `SHOW`, `HIDE`, `RENDER`, `VALID`, `INVALID`, `NEW_CHILD_APPLICATION`, etc. |
| `PropertyChangeEvent` | `kind: String` (`PropertyChangeEventKind.UPDATE`/`DELETE`), `property`, `oldValue`, `newValue`, `source`. Factory method `createUpdateEvent`. |
| `PropertyChangeEventKind` | Constants: `UPDATE = "update"`, `DELETE = "delete"`. |
| `StateChangeEvent` | `currentState: String`, `newState: String` — fired before/after a Flex state transition (`CURRENT_STATE_CHANGING` / `CURRENT_STATE_CHANGE`). |
| `MoveEvent` | `oldX: Number`, `oldY: Number` — type `"move"`. |
| `ResizeEvent` | `oldWidth: Number`, `oldHeight: Number` — type `"resize"`. |
| `FlexMouseEvent` | Wraps a `MouseEvent` with a Flex component `relatedObject: IUIComponent`. |
| `DragEvent` | `dragInitiator: IUIComponent`, `dragSource: DragSource`, `action: String`. Types: `DRAG_START`, `DRAG_COMPLETE`, `DRAG_ENTER`, `DRAG_OVER`, `DRAG_EXIT`, `DRAG_DROP`. |
| `ModuleEvent` | `module: IModuleInfo`, `bytesLoaded`, `bytesTotal`, `errorText`. Types: `READY`, `PROGRESS`, `ERROR`, `UNLOAD`, `SETUP`. |
| `ResourceEvent` | `bytesLoaded`, `bytesTotal`, `errorText`. Types: `COMPLETE`, `PROGRESS`, `ERROR`. |
| `ToolTipEvent` | `toolTip: IToolTip`. Types: `TOOL_TIP_CREATE`, `TOOL_TIP_HIDE`, `TOOL_TIP_SHOW`, `TOOL_TIP_SHOWN`, `TOOL_TIP_START`. |
| `Request` | Generic request/response event: `value: *` — used for synchronous info requests via event bubbling. |

---

### mx/managers — System manager stubs

| Class | Purpose |
|---|---|
| `ISystemManager` | Interface for the Flex root system manager: child/popup/focus management, `document`, `stage`, `info()`, `isTopLevel()`. |
| `IFocusManagerComponent` | Marker interface for components that participate in Flex focus management: `focusEnabled`, `hasFocusableChildren`, `setFocus()`. |
| `IToolTipManagerClient` | `toolTip: String` get/set — implemented by components that display tooltips. |
| `SystemManagerGlobals` | Static fields shared across all system managers: `topLevelSystemManagers: Array`, `info: Object`, `parameters: Object`, `bootstrapLoaderInfoURL: String`. Read by `ResourceManagerImpl` during init. |

---

### mx/modules — Module loading

| Class | Purpose |
|---|---|
| `ModuleManager` | Static facade: `getModule(url): IModuleInfo`, `getAssociatedFactory(obj): IFlexModuleFactory`. Delegates to a `ModuleManagerImpl` singleton stored in `ModuleManagerGlobals`. `getModule` returns a handle that can be `.load()`-ed; fires `ModuleEvent.READY`/`PROGRESS`/`ERROR`. |
| `ModuleManagerGlobals` | Single static field `managerSingleton: Object` — stores the `ModuleManagerImpl` instance. |
| `IModuleInfo` | Interface on the handle returned by `ModuleManager.getModule`: `url`, `ready`, `loaded`, `setup`, `error`, `factory`, `load()`, `unload()`, `release()`, `publish()`. |

---

### mx/automation

| Class | Purpose |
|---|---|
| `IAutomationObject` | Interface implemented by `UIMovieClip` to expose the component to Flex automation/testing frameworks: `automationEnabled`, `automationName`, `automationOwner`, `automationParent`, `automationTabularData`, `automationValue`, `showInAutomationHierarchy`, `createAutomationIDPart`, `resolveAutomationIDPart`, `getAutomationChildAt`, `numAutomationChildren`. |
