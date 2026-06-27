# playerhealth.swf
> A minimal HUD element that displays the player's health (and potentially energy/resource) as a percentage bar with a numeric label. It contains no game-specific top-level or document class of its own — the functional work is done entirely by the shared `_kiwi.Controls.ResourceBar` framework component included in this SWF's library.

**Document/main class:** None (no top-level game-specific class found in scripts/)
**SWF-specific classes:** 0

---

## Framework class present: `_kiwi.Controls.ResourceBar`

Although `ResourceBar` is a shared `_kiwi` framework class (and is therefore not documented in detail per the skip rules), it is the only functional class in this SWF and warrants a brief description because the SWF contains no other logic at all.

`ResourceBar` extends `_kiwi.Core.UIComponent`. It exposes two settable properties:

- `percent : Number` — A 0–1 value. Setting it invalidates DATA and in `draw()` scales `bar.width` as `s_barWidth (200px) * percent`, and sets `textField.text` / `textFieldShadow.text` to `floor(100 * percent)`.
- `color : uint` — A tint color. Setting it invalidates STYLES and in `draw()` applies a 60%-strength `fl.motion.Color` tint transform to `bar`.

The symbol has three child display objects referenced in code: `bar : MovieClip`, `textField : TextField`, `textFieldShadow : TextField`.

No `ExternalInterface` calls, no translate keys, no IggyTween, no timers are present in this SWF.

---

## Notable logic

This SWF is essentially a pure framework/asset container. The engine likely sets `percent` and `color` directly on the `ResourceBar` instance to update the player health HUD. The shadow text field provides the drop-shadow effect on the percentage number without a Flash filter.
