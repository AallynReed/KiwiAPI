/* Kiwi embed loader — the one file a partner site includes.

     <div data-kiwi-embed data-game="dragon_head.blueprint"></div>
     <script src="https://trove.aallyn.net/static/embed.js" async></script>

   Every marked element becomes a responsive iframe pointing at /embed/viewer. The
   origin comes from this script's own src, so a partner never hard-codes our
   hostname and a move can't strand their pages.

   Attributes (all optional except the source):
     data-release / data-tmod / data-game   the source — exactly one
     data-path      which file inside the source to show
     data-mode      blueprint | assembled | vfx   (default: auto)
     data-theme     dark | light                  (default: dark)
     data-height    CSS height                    (default: 420px)
     data-title     iframe accessible name

   Writing the iframe is all this does — no cookies, no globals, no tracking, and
   nothing read back out of the host page. */
(function () {
  'use strict';

  var self = document.currentScript;
  var ORIGIN = (function () {
    try { return new URL(self.src).origin; } catch (e) { return ''; }
  })();

  var SOURCES = ['release', 'tmod', 'game'];
  var PASS = ['path', 'mode', 'theme'];

  function build(el) {
    if (el.getAttribute('data-kiwi-mounted')) return;

    var params = [];
    SOURCES.concat(PASS).forEach(function (key) {
      var v = el.getAttribute('data-' + key);
      if (v) params.push(key + '=' + encodeURIComponent(v));
    });
    if (!params.length) return;                 // nothing to point at

    var frame = document.createElement('iframe');
    frame.src = ORIGIN + '/embed/viewer?' + params.join('&');
    frame.title = el.getAttribute('data-title') || 'Trove 3D preview';
    frame.loading = 'lazy';
    frame.allowFullscreen = true;
    frame.setAttribute('frameborder', '0');
    var height = el.getAttribute('data-height') || '420px';
    frame.style.cssText = 'display:block;width:100%;border:0;border-radius:12px;'
      + 'height:' + height;

    // data-height sizes the 3D VIEW. A creature with animation clips also needs a
    // control bar, so the viewer reports how much chrome it added and we grow the
    // frame by that much - the model keeps the height you asked for instead of being
    // squeezed to make room. Only px heights can be grown; anything else is left be.
    var px = /^(\d+(?:\.\d+)?)px$/.exec(height.trim());
    if (px) { frames.push({ frame: frame, base: parseFloat(px[1]) }); }

    el.textContent = '';
    el.appendChild(frame);
    el.setAttribute('data-kiwi-mounted', '1');
  }

  // Frames we may auto-grow, paired with the height the host asked for.
  var frames = [];
  window.addEventListener('message', function (e) {
    if (e.origin !== ORIGIN || !e.data || e.data.source !== 'kiwi-embed') return;
    if (e.data.type !== 'chrome') return;
    var chrome = Number(e.data.chrome);
    if (!(chrome >= 0 && chrome < 2000)) return;          // ignore nonsense
    for (var i = 0; i < frames.length; i++) {
      if (frames[i].frame.contentWindow === e.source) {   // only the frame that spoke
        frames[i].frame.style.height = (frames[i].base + chrome) + 'px';
        return;
      }
    }
  });

  function scan() {
    var nodes = document.querySelectorAll('[data-kiwi-embed]');
    for (var i = 0; i < nodes.length; i++) build(nodes[i]);
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', scan);
  } else {
    scan();
  }

  // Exposed so a partner rendering mods client-side can mount new placeholders
  // after their own render pass.
  window.KiwiEmbed = { scan: scan, mount: build };
})();
