/* Kiwi blueprint 3D viewer.
   Opens a modal with a WebGL turntable of a decoded Trove .blueprint model.
   Self-contained: injects its own styles, lazy-loads three.js (cdnjs), and ships
   its own lightweight orbit controls (drag = rotate, wheel = zoom, right/shift-drag
   = pan; touch: one-finger rotate, pinch zoom).

   Voxel payload (from /site/mods/releases/<id>/blueprint?path=...):
     { count, size:[sx,sy,sz], x:[], y:[], z:[], rgb:[], kind:[], level:[] }
   kind: 0 solid · 1 glass · 2 glow · 3 glow-glass

   Public API: window.BlueprintViewer.open({ url, title })      -- modal
               window.BlueprintViewer.mount(el, { url, onMeta })  -- inline (embed page) */
(function () {
  'use strict';

  var THREE_URL = '/static/vendor/three.min.js';  // self-hosted (GDPR: no cdnjs IP leak)
  var _stylesDone = false;
  var _threePromise = null;

  function injectStyles() {
    if (_stylesDone) return; _stylesDone = true;
    var css =
      '.bpv-overlay{position:fixed;inset:0;z-index:9999;background:rgba(4,7,12,.78);' +
        'display:flex;align-items:center;justify-content:center;padding:20px;backdrop-filter:blur(2px)}' +
      '.bpv-modal{display:flex;flex-direction:column;width:min(960px,94vw);height:min(720px,88vh);' +
        'background:#10151c;border:1px solid #232a33;border-radius:14px;overflow:hidden;box-shadow:0 24px 60px rgba(0,0,0,.5)}' +
      '.bpv-head{display:flex;align-items:center;gap:12px;padding:11px 14px;border-bottom:1px solid #232a33;flex:0 0 auto}' +
      '.bpv-title{font-weight:700;color:#e6edf3;font-size:.98rem;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}' +
      '.bpv-hint{flex:1;color:#8a93a3;font-size:.78rem;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}' +
      '.bpv-meta{color:#6b7480;font-size:.75rem;flex:0 0 auto}' +
      '.bpv-close{flex:0 0 auto;background:transparent;border:0;color:#9aa4b2;font-size:1.5rem;line-height:1;cursor:pointer;padding:0 4px}' +
      '.bpv-close:hover{color:#e6edf3}' +
      '.bpv-stage{position:relative;flex:1;min-height:0;background:radial-gradient(circle at 50% 40%,#1b2531,#0c1118 78%);cursor:grab}' +
      '.bpv-stage.bpv-grabbing{cursor:grabbing}' +
      '.bpv-stage canvas{display:block;width:100%;height:100%}' +
      '.bpv-msg{position:absolute;inset:0;display:flex;align-items:center;justify-content:center;' +
        'color:#9aa4b2;font-size:.9rem;text-align:center;padding:20px}' +
      '.bpv-msg.bpv-error{color:#f0997b}';
    var s = document.createElement('style'); s.textContent = css; document.head.appendChild(s);
  }

  function ensureThree() {
    if (window.THREE) return Promise.resolve(window.THREE);
    if (_threePromise) return _threePromise;
    _threePromise = new Promise(function (resolve, reject) {
      var s = document.createElement('script');
      s.src = THREE_URL;
      s.onload = function () { window.THREE ? resolve(window.THREE) : reject(new Error('3D library failed to load.')); };
      s.onerror = function () { reject(new Error('Could not load the 3D library.')); };
      document.head.appendChild(s);
    });
    return _threePromise;
  }

  /* Fetch a payload, preferring the binary container (voxel_binary.js) - same object
     back either way, typed arrays instead of parsed numbers. If that script isn't on
     the page we fetch JSON exactly as before, so a template that hasn't been updated
     still works. */
  function loadModel(url) {
    if (window.VoxelBinary) return window.VoxelBinary.fetchModel(url);
    return fetch(url, { credentials: 'same-origin' }).then(function (r) {
      if (!r.ok) {
        return r.json().then(function (j) {
          throw new Error((j && j.error && j.error.message) || ('Could not load model (HTTP ' + r.status + ').'));
        }, function () { throw new Error('Could not load model (HTTP ' + r.status + ').'); });
      }
      return r.json();
    });
  }

  /* Load a model into an existing element (no modal). This is what the modal below
     uses, and what the embeddable viewer (/embed/viewer) mounts directly into its
     page. `onMeta` reports the voxel count so each host can label it its own way.
     Returns { dispose } - safe to call while the fetch is still in flight. */
  function mount(container, opts) {
    injectStyles();
    container.classList.add('bpv-stage');
    var msg = document.createElement('div');
    msg.className = 'bpv-msg';
    msg.textContent = 'Loading model…';
    container.appendChild(msg);

    var viewer = null, alive = true;
    ensureThree().then(function (THREE) {
      return loadModel(opts.url).then(function (data) {
        if (!alive) return;                     // disposed while loading
        msg.remove();
        if (opts.onMeta) opts.onMeta(data.count.toLocaleString() + ' voxels');
        viewer = buildViewer(THREE, container, data);
      });
    }).catch(function (err) {
      if (!alive) return;
      msg.textContent = err.message || 'Could not load this model.';
      msg.classList.add('bpv-error');
    });

    return { dispose: function () {
      alive = false;
      if (viewer) { viewer.dispose(); viewer = null; }
    } };
  }

  function open(opts) {
    injectStyles();
    var ov = document.createElement('div');
    ov.className = 'bpv-overlay';
    ov.setAttribute('role', 'dialog');
    ov.setAttribute('aria-modal', 'true');
    ov.setAttribute('aria-label', (opts.title || 'Blueprint') + ' — 3D model preview');
    ov.innerHTML =
      '<div class="bpv-modal">' +
        '<div class="bpv-head">' +
          '<span class="bpv-title"></span>' +
          '<span class="bpv-hint">Drag to rotate · scroll to zoom · right-drag to pan</span>' +
          '<span class="bpv-meta"></span>' +
          '<button class="bpv-close" type="button" aria-label="Close">×</button>' +
        '</div>' +
        '<div class="bpv-stage"></div>' +
      '</div>';
    ov.querySelector('.bpv-title').textContent = opts.title || 'Blueprint';
    document.body.appendChild(ov);

    var stage = ov.querySelector('.bpv-stage');
    var meta = ov.querySelector('.bpv-meta');
    var viewer = null;
    var releaseFocus = null;
    function close() {
      if (viewer) { viewer.dispose(); viewer = null; }
      document.removeEventListener('keydown', onKey);
      if (releaseFocus) { releaseFocus(); releaseFocus = null; }
      ov.remove();
    }
    function onKey(e) { if (e.key === 'Escape') close(); }
    ov.querySelector('.bpv-close').addEventListener('click', close);
    ov.addEventListener('mousedown', function (e) { if (e.target === ov) close(); });
    document.addEventListener('keydown', onKey);
    if (window.BTTUtil && window.BTTUtil.trapFocus) {
      releaseFocus = window.BTTUtil.trapFocus(ov.querySelector('.bpv-modal'));
    }

    viewer = mount(stage, {
      url: opts.url,
      onMeta: function (text) { meta.textContent = text; },
    });
  }

  function buildViewer(THREE, stage, data) {
    var W = stage.clientWidth || 800, H = stage.clientHeight || 560;
    var renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
    renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
    renderer.setSize(W, H);
    renderer.domElement.setAttribute('role', 'img');
    renderer.domElement.setAttribute('aria-label', 'Interactive 3D voxel model. Drag to rotate, scroll to zoom.');
    renderer.domElement.appendChild(document.createTextNode('3D model preview (requires a WebGL-capable browser).'));
    stage.appendChild(renderer.domElement);

    var scene = new THREE.Scene();
    scene.add(new THREE.AmbientLight(0xffffff, 0.66));
    var key = new THREE.DirectionalLight(0xffffff, 0.85); key.position.set(0.7, 1.0, 0.55); scene.add(key);
    var fill = new THREE.DirectionalLight(0xffffff, 0.32); fill.position.set(-0.6, 0.35, -0.7); scene.add(fill);

    var camera = new THREE.PerspectiveCamera(42, W / H, 0.1, 8000);
    var meshes = window.VoxelMesh.build(THREE, data);
    meshes.forEach(function (m) { scene.add(m); });

    var sx = data.size[0], sy = data.size[1], sz = data.size[2];
    var modelR = Math.max(sx, sy, sz) || 1;
    var target = new THREE.Vector3(sx / 2, sy / 2, sz / 2);
    var sph = { radius: modelR * 2.1, theta: Math.PI * 0.27, phi: Math.PI * 0.36 };

    function applyCamera() {
      var r = sph.radius, t = sph.theta, p = sph.phi;
      camera.position.set(
        target.x + r * Math.sin(p) * Math.sin(t),
        target.y + r * Math.cos(p),
        target.z + r * Math.sin(p) * Math.cos(t));
      camera.lookAt(target);
    }
    applyCamera();

    var el = renderer.domElement;
    var drag = 0, lx = 0, ly = 0, pinch = 0;
    var right = new THREE.Vector3(), up = new THREE.Vector3(), fwd = new THREE.Vector3();
    function rotate(dx, dy) {
      sph.theta -= dx * 0.01;
      sph.phi = Math.max(0.04, Math.min(Math.PI - 0.04, sph.phi - dy * 0.01));
    }
    function pan(dx, dy) {
      var s = sph.radius * 0.0016;
      camera.matrix.extractBasis(right, up, fwd);
      target.addScaledVector(right, -dx * s);
      target.addScaledVector(up, dy * s);
    }
    function zoom(f) { sph.radius = Math.max(modelR * 0.35, Math.min(modelR * 9, sph.radius * f)); }

    function onDown(e) { drag = (e.button === 2 || e.shiftKey) ? 2 : 1; lx = e.clientX; ly = e.clientY; stage.classList.add('bpv-grabbing'); e.preventDefault(); }
    function onMove(e) {
      if (!drag) return;
      var dx = e.clientX - lx, dy = e.clientY - ly; lx = e.clientX; ly = e.clientY;
      if (drag === 2) pan(dx, dy); else rotate(dx, dy);
      applyCamera(); request();
    }
    function onUp() { drag = 0; stage.classList.remove('bpv-grabbing'); }
    function onWheel(e) { e.preventDefault(); zoom(e.deltaY > 0 ? 1.12 : 0.89); applyCamera(); request(); }
    function dist(e) { var a = e.touches[0], b = e.touches[1]; return Math.hypot(a.clientX - b.clientX, a.clientY - b.clientY); }
    function onTStart(e) { if (e.touches.length === 1) { drag = 1; lx = e.touches[0].clientX; ly = e.touches[0].clientY; } else if (e.touches.length === 2) { drag = 0; pinch = dist(e); } }
    function onTMove(e) {
      if (e.touches.length === 1 && drag === 1) { var t = e.touches[0]; rotate(t.clientX - lx, t.clientY - ly); lx = t.clientX; ly = t.clientY; applyCamera(); request(); }
      else if (e.touches.length === 2) { var d = dist(e); if (pinch) { zoom(pinch / d); applyCamera(); request(); } pinch = d; }
      e.preventDefault();
    }
    function onTEnd() { drag = 0; pinch = 0; }
    function onResize() { var w = stage.clientWidth, h = stage.clientHeight; if (!w || !h) return; camera.aspect = w / h; camera.updateProjectionMatrix(); renderer.setSize(w, h); request(); }

    el.addEventListener('mousedown', onDown);
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
    el.addEventListener('wheel', onWheel, { passive: false });
    el.addEventListener('contextmenu', function (e) { e.preventDefault(); });
    el.addEventListener('touchstart', onTStart, { passive: false });
    el.addEventListener('touchmove', onTMove, { passive: false });
    el.addEventListener('touchend', onTEnd);
    window.addEventListener('resize', onResize);

    // Render-on-demand: the scene is static, so draw once and re-render only on
    // interaction (drag / zoom / resize) rather than spinning a rAF loop forever.
    var raf = 0, alive = true, pending = false;
    function renderOnce() { if (alive) renderer.render(scene, camera); }
    function request() { if (pending) return; pending = true; raf = requestAnimationFrame(function () { pending = false; renderOnce(); }); }
    request();

    return {
      dispose: function () {
        alive = false; cancelAnimationFrame(raf);
        el.removeEventListener('mousedown', onDown);
        window.removeEventListener('mousemove', onMove);
        window.removeEventListener('mouseup', onUp);
        el.removeEventListener('wheel', onWheel);
        el.removeEventListener('touchstart', onTStart);
        el.removeEventListener('touchmove', onTMove);
        el.removeEventListener('touchend', onTEnd);
        window.removeEventListener('resize', onResize);
        meshes.forEach(function (m) { if (m.geometry) m.geometry.dispose(); if (m.material) m.material.dispose(); });
        renderer.dispose();
        if (el.parentNode) el.parentNode.removeChild(el);
      }
    };
  }

  window.BlueprintViewer = { open: open, mount: mount };
})();
