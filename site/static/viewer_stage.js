/* Kiwi viewer stage tools - the backdrop and the snapshot button that the
   blueprint viewer and the assembled-model viewer both want.

   Two jobs, one place so the two viewers cannot drift apart:

     * BACKGROUND. The stage used to be a fixed radial gradient baked into each
       viewer's CSS. That gradient is still the default and still looks the same;
       this adds "no background at all", a flat colour, and an image from disk to
       stand the model in front of.

     * SNAPSHOT. Saves exactly what is on screen as a PNG - backdrop composited
       under the model, at the canvas' real device-pixel size.

   Compositing is why the two live together. A WebGL canvas here is transparent
   (`alpha: true`) and the backdrop is a CSS layer behind it, so reading the GL
   canvas alone would save a model floating on nothing. The snapshot paints the
   same backdrop into a 2D canvas first and draws the GL canvas over it, so the
   backdrop has to be describable in a form both CSS and canvas can draw.

   The DEFAULT backdrop therefore stays a host stylesheet's business - the stage
   is styled before this script has even run, and a viewer that flashed a flat
   panel while the model loaded would be a step backwards - but every host has to
   spell it exactly this way, because `rasterGradient()` below redraws it by hand:

     background: radial-gradient(circle at 50% 40%,
                                 var(--vs-bg-a, #1b2531), var(--vs-bg-b, #0c1118) 78%);

   Only the two stops may vary (the embed's light theme reassigns them), and this
   reads them back off the computed style. Every other mode is an inline
   background, which outranks the host's rule and needs no cooperation from it.

   Usage:
     var tools = window.ViewerStage.attach({
       stage: el, canvas: renderer.domElement,
       render: function () { renderer.render(scene, camera); },
       name: 'Dragon',
     });
     ...
     tools.dispose();

   Optional, so a host that hasn't loaded this script still works - both viewers
   check for `window.ViewerStage` before calling it. */
(function () {
  'use strict';

  var STORE_KEY = 'kiwi.viewerBg';        // { mode, color } - never the image itself
  var GRAD_A = '#1b2531', GRAD_B = '#0c1118';   // fallbacks; the CSS vars are the source
  var _styles = false;

  /* The chosen image lives for the page, not for one attach. The dressing room
     tears the viewer down and rebuilds it on every costume change, and losing your
     backdrop each time you tried a different hat would be absurd. It stays out of
     localStorage though - a photo is megabytes, and that is the user's file to
     keep, not ours to stash. */
  var IMG = { url: '', name: '', el: null };

  function injectStyles() {
    if (_styles) return; _styles = true;
    var css =
      '.vs-tools{position:absolute;top:8px;right:8px;z-index:3;display:flex;gap:6px}' +
      '.vs-tool{display:flex;align-items:center;gap:6px;background:rgba(16,21,28,.82);' +
        'border:1px solid #2a323d;color:#cdd6e0;border-radius:8px;padding:5px 9px;' +
        'font:inherit;font-size:.76rem;line-height:1.2;cursor:pointer}' +
      '.vs-tool:hover{border-color:#4cc9f0;color:#e6edf3}' +
      '.vs-tool:focus-visible{outline:2px solid #4cc9f0;outline-offset:2px}' +
      '.vs-tool[aria-expanded="true"]{border-color:#4cc9f0;color:#e6edf3}' +
      '.vs-tool svg{width:14px;height:14px;flex:0 0 auto;fill:none;stroke:currentColor;' +
        'stroke-width:1.8;stroke-linecap:round;stroke-linejoin:round}' +
      '.vs-panel{position:absolute;top:38px;right:0;z-index:4;width:212px;padding:11px;' +
        'background:#10151c;border:1px solid #2a323d;border-radius:10px;' +
        'box-shadow:0 14px 34px rgba(0,0,0,.5)}' +
      '.vs-panel[hidden]{display:none}' +
      '.vs-legend{display:block;color:#9aa4b2;font-size:.72rem;text-transform:uppercase;' +
        'letter-spacing:.06em;margin:0 0 7px}' +
      '.vs-opts{display:flex;flex-wrap:wrap;gap:6px;margin:0 0 9px}' +
      '.vs-opt{background:transparent;border:1px solid #2a323d;color:#9aa4b2;border-radius:999px;' +
        'padding:4px 11px;font:inherit;font-size:.76rem;cursor:pointer}' +
      '.vs-opt:hover{border-color:#4cc9f0;color:#cdd6e0}' +
      '.vs-opt:focus-visible{outline:2px solid #4cc9f0;outline-offset:2px}' +
      '.vs-opt[aria-pressed="true"]{background:rgba(86,156,255,.16);border-color:#4cc9f0;color:#e6edf3}' +
      '.vs-row{display:flex;align-items:center;gap:8px;color:#9aa4b2;font-size:.76rem}' +
      '.vs-row[hidden]{display:none}' +
      '.vs-row input[type=color]{width:34px;height:24px;padding:0;background:none;' +
        'border:1px solid #2a323d;border-radius:6px;cursor:pointer}' +
      '.vs-file{color:#9aa4b2;font-size:.72rem;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}' +
      '.vs-clear{background:none;border:0;color:#8a93a3;font:inherit;font-size:.72rem;' +
        'text-decoration:underline;cursor:pointer;padding:0;margin-left:auto;flex:0 0 auto}' +
      '.vs-clear:hover{color:#e6edf3}';
    var s = document.createElement('style'); s.textContent = css; document.head.appendChild(s);
  }

  function read() {
    try {
      var v = JSON.parse(window.localStorage.getItem(STORE_KEY) || 'null');
      if (v && typeof v.mode === 'string') return v;
    } catch (e) { /* private mode, or someone else's key - fall through */ }
    return { mode: 'default', color: '#0c1118' };
  }
  function write(v) {
    try { window.localStorage.setItem(STORE_KEY, JSON.stringify(v)); } catch (e) { /* not worth failing over */ }
  }

  function icon(d) {
    return '<svg viewBox="0 0 24 24" aria-hidden="true">' + d + '</svg>';
  }
  var ICON_IMAGE = '<rect x="3" y="4" width="18" height="16" rx="2"/>' +
                   '<circle cx="8.5" cy="9.5" r="1.6"/><path d="M4 17l5-5 4 4 3-2 4 4"/>';
  var ICON_SAVE = '<path d="M12 3v11"/><path d="M8 11l4 4 4-4"/><path d="M4 20h16"/>';

  /* The default backdrop, painted into a 2D context. `radial-gradient(circle at
     50% 40%, A, B 78%)` with no explicit size means a farthest-corner circle, so
     the radius is the distance to whichever corner is furthest from the centre
     point, and the 78% stop scales off that. Canvas clamps to the final stop
     past the end radius, which is what CSS does beyond 78% too. */
  function rasterGradient(ctx, w, h, a, b) {
    var cx = w * 0.5, cy = h * 0.4;
    var r = Math.max(Math.hypot(cx, cy), Math.hypot(w - cx, cy),
                     Math.hypot(cx, h - cy), Math.hypot(w - cx, h - cy)) * 0.78;
    var g = ctx.createRadialGradient(cx, cy, 0, cx, cy, r);
    g.addColorStop(0, a); g.addColorStop(1, b);
    ctx.fillStyle = g; ctx.fillRect(0, 0, w, h);
  }

  // `background-size: cover` - scale until both axes are covered, then centre.
  function rasterCover(ctx, img, w, h) {
    var s = Math.max(w / img.naturalWidth, h / img.naturalHeight);
    var dw = img.naturalWidth * s, dh = img.naturalHeight * s;
    ctx.drawImage(img, (w - dw) / 2, (h - dh) / 2, dw, dh);
  }

  function slug(name) {
    return String(name || 'model').toLowerCase().replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '').slice(0, 60) || 'model';
  }

  function attach(opts) {
    injectStyles();
    var stage = opts.stage, canvas = opts.canvas, render = opts.render;
    var state = read();

    var tools = document.createElement('div');
    tools.className = 'vs-tools';
    tools.innerHTML =
      '<button class="vs-tool vs-bg-btn" type="button" aria-expanded="false">' +
        icon(ICON_IMAGE) + '<span>Background</span></button>' +
      '<button class="vs-tool vs-save-btn" type="button">' +
        icon(ICON_SAVE) + '<span>Save PNG</span></button>' +
      '<div class="vs-panel" hidden>' +
        '<span class="vs-legend">Backdrop</span>' +
        '<div class="vs-opts">' +
          '<button class="vs-opt" type="button" data-mode="default" aria-pressed="false">Default</button>' +
          '<button class="vs-opt" type="button" data-mode="none" aria-pressed="false">None</button>' +
          '<button class="vs-opt" type="button" data-mode="color" aria-pressed="false">Colour</button>' +
          '<button class="vs-opt" type="button" data-mode="image" aria-pressed="false">Image</button>' +
        '</div>' +
        '<div class="vs-row vs-color-row" hidden>' +
          '<label for="" class="vs-color-label">Colour</label>' +
          '<input type="color" class="vs-color">' +
        '</div>' +
        '<div class="vs-row vs-image-row" hidden>' +
          '<button class="vs-opt vs-pick" type="button">Choose file…</button>' +
          '<span class="vs-file"></span>' +
        '</div>' +
        '<input type="file" class="vs-input" accept="image/*" hidden>' +
      '</div>';
    stage.appendChild(tools);

    var bgBtn = tools.querySelector('.vs-bg-btn'),
        saveBtn = tools.querySelector('.vs-save-btn'),
        panel = tools.querySelector('.vs-panel'),
        colorRow = tools.querySelector('.vs-color-row'),
        colorIn = tools.querySelector('.vs-color'),
        colorLabel = tools.querySelector('.vs-color-label'),
        imageRow = tools.querySelector('.vs-image-row'),
        pickBtn = tools.querySelector('.vs-pick'),
        fileName = tools.querySelector('.vs-file'),
        fileIn = tools.querySelector('.vs-input');

    // `for` and `aria-controls` need unique ids, and a page can hold more than one viewer
    var uid = Math.random().toString(36).slice(2, 8);
    colorIn.id = 'vs-c-' + uid; colorLabel.setAttribute('for', colorIn.id);
    panel.id = 'vs-p-' + uid; bgBtn.setAttribute('aria-controls', panel.id);
    colorIn.value = state.color;

    function apply() {
      var m = state.mode;
      // Default hands the stage back to the host's own stylesheet; every other mode
      // sets an inline background, which outranks it.
      if (m === 'default') stage.style.background = '';
      else if (m === 'color') stage.style.background = state.color;
      else if (m === 'image' && IMG.url) stage.style.background = 'center/cover no-repeat url("' + IMG.url + '")';
      else stage.style.background = 'none';

      Array.prototype.forEach.call(tools.querySelectorAll('.vs-opt[data-mode]'), function (b) {
        b.setAttribute('aria-pressed', String(b.getAttribute('data-mode') === m));
      });
      colorRow.hidden = m !== 'color';
      imageRow.hidden = m !== 'image';
      fileName.textContent = IMG.name || 'No file chosen';
    }

    function setMode(m) { state.mode = m; write(state); apply(); }

    function openPanel(on) {
      panel.hidden = !on;
      bgBtn.setAttribute('aria-expanded', String(on));
    }

    function onBgClick() { openPanel(panel.hidden); }
    function onOptClick(e) {
      var b = e.target.closest ? e.target.closest('.vs-opt[data-mode]') : null;
      if (!b) return;
      var m = b.getAttribute('data-mode');
      setMode(m);
      if (m === 'image' && !IMG.el) fileIn.click();
    }
    function onColor() { state.color = colorIn.value; write(state); apply(); }
    function onPick() { fileIn.click(); }
    function onFile() {
      var f = fileIn.files && fileIn.files[0];
      if (!f) return;
      if (IMG.url) URL.revokeObjectURL(IMG.url);
      IMG.url = URL.createObjectURL(f);
      IMG.name = f.name;
      /* Decoded into an <img> as well as handed to CSS: the snapshot needs real
         pixels to draw, and a blob from the user's own disk keeps the export
         canvas untainted - a remote URL would taint it and `toBlob` would throw. */
      IMG.el = new Image();
      IMG.el.onload = function () { apply(); };
      IMG.el.onerror = function () { IMG.el = null; IMG.name = ''; setMode('default'); };
      IMG.el.src = IMG.url;
      fileIn.value = '';                       // so re-picking the same file still fires
      setMode('image');
    }
    /* Escape must close the picker without also closing the viewer's modal, which
       listens for it on `document`. */
    function onKey(e) {
      if (e.key !== 'Escape' || panel.hidden) return;
      e.stopPropagation();
      openPanel(false); bgBtn.focus();
    }
    function onDocDown(e) {
      if (!panel.hidden && !tools.contains(e.target)) openPanel(false);
    }

    /* The renderer runs without `preserveDrawingBuffer`, so the GL canvas is only
       readable in the same task as the draw that filled it - hence render, then
       composite, with nothing awaited in between. */
    function snapshot() {
      render();
      var w = canvas.width, h = canvas.height;
      var out = document.createElement('canvas');
      out.width = w; out.height = h;
      var ctx = out.getContext('2d');
      var cs = window.getComputedStyle(stage);
      if (state.mode === 'default') {
        rasterGradient(ctx, w, h,
          (cs.getPropertyValue('--vs-bg-a') || '').trim() || GRAD_A,
          (cs.getPropertyValue('--vs-bg-b') || '').trim() || GRAD_B);
      } else if (state.mode === 'color') {
        ctx.fillStyle = state.color; ctx.fillRect(0, 0, w, h);
      } else if (state.mode === 'image' && IMG.el) {
        rasterCover(ctx, IMG.el, w, h);
      }
      ctx.drawImage(canvas, 0, 0, w, h);     // "none" leaves the PNG transparent behind the model
      save(out);
    }

    function save(out) {
      var name = slug(opts.name) + '.png';
      if (!out.toBlob) {                     // ancient browser: one big data: URL instead
        return download(out.toDataURL('image/png'), name, false);
      }
      out.toBlob(function (blob) {
        if (!blob) return;
        download(URL.createObjectURL(blob), name, true);
      }, 'image/png');
    }
    function download(href, name, revoke) {
      var a = document.createElement('a');
      a.href = href; a.download = name; a.rel = 'noopener';
      document.body.appendChild(a); a.click(); a.remove();
      if (revoke) setTimeout(function () { URL.revokeObjectURL(href); }, 1000);
    }

    bgBtn.addEventListener('click', onBgClick);
    saveBtn.addEventListener('click', snapshot);
    panel.addEventListener('click', onOptClick);
    colorIn.addEventListener('input', onColor);
    pickBtn.addEventListener('click', onPick);
    fileIn.addEventListener('change', onFile);
    tools.addEventListener('keydown', onKey);
    document.addEventListener('mousedown', onDocDown);

    apply();

    return {
      snapshot: snapshot,
      dispose: function () {
        document.removeEventListener('mousedown', onDocDown);
        // IMG is deliberately left alone - see the note on its declaration.
        stage.style.background = '';
        if (tools.parentNode) tools.parentNode.removeChild(tools);
      },
    };
  }

  window.ViewerStage = { attach: attach };
})();
