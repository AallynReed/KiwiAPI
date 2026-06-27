# draghost.swf

> A lightweight drag-image host used in Trove's inventory or item UI. When the player begins dragging an item, the engine pushes a texture name to this SWF; it renders the texture as a `Bitmap` that follows the cursor, acting as the drag ghost/phantom image.

**Document/main class:** `DragHost` (extends `KiwiComponent`)
**SWF-specific classes:** 1

---

## Main class: `DragHost`

Minimal single-responsibility class. On `config()` it adds a `Bitmap` child to the display list and registers one ExternalInterface callback. The engine drives all state through that callback.

### Public methods / overrides

- (none beyond constructor)

### Key fields

- `image : Bitmap` — the bitmap display object that shows the dragged item's texture. Created at field-init time (`new Bitmap()`); added to the display list in `config()`.

### Runtime dependencies & integration

- **ExternalInterface callback registered**: `DRAGHOST.SETTEXTURENAME(param1:String)` — receives a texture name string from the engine.
- **`IggyFunctions.setTextureForBitmap(bitmap, name)`** — Iggy runtime call that populates `image` with the named texture. This is the only operation performed when the callback fires; there is no additional layout or transform logic.

---

## Notable logic

- The entire functional surface is a single Iggy API call: `setTextureForBitmap`. Positioning of the SWF itself (following the mouse cursor) is handled entirely by the game engine — this class only manages which texture is displayed.
- No hide/show, no alpha tweening, no size adjustment — the SWF is likely toggled visible/invisible or repositioned externally.
- Extends `KiwiComponent` for constraint/layout infrastructure, but none of those features appear to be used here.
