/* Kiwi assembled-model viewer.
   Renders a Trove creature assembled from blueprint voxel parts placed on its
   skeleton (rest pose), with animation playback. Pure data at runtime - the model
   JSON is baked offline (Granny skeleton + animations -> per-bone transforms).

   Model payload:
     { voxel_scale, parts:[{name,x[],y[],z[],rgb[]}],
       rest:{part:[16]}, animations:{name:{fps,frames:[{part:[16]}]}} }
   Matrices are column-major (three.js order).

   API: window.ModelViewer.open({ url, title }) */
(function () {
  'use strict';
  var THREE_URL = 'https://cdnjs.cloudflare.com/ajax/libs/three.js/r134/three.min.js';
  var _styles = false, _three = null;

  function injectStyles() {
    if (_styles) return; _styles = true;
    var css =
      '.mv-overlay{position:fixed;inset:0;z-index:9999;background:rgba(4,7,12,.8);display:flex;align-items:center;justify-content:center;padding:20px}' +
      '.mv-modal{display:flex;flex-direction:column;width:min(1000px,95vw);height:min(760px,90vh);background:#10151c;border:1px solid #232a33;border-radius:14px;overflow:hidden;box-shadow:0 24px 60px rgba(0,0,0,.55)}' +
      '.mv-head{display:flex;align-items:center;gap:12px;padding:11px 14px;border-bottom:1px solid #232a33}' +
      '.mv-title{font-weight:700;color:#e6edf3;font-size:.98rem;flex:1;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}' +
      '.mv-meta{color:#6b7480;font-size:.75rem}' +
      '.mv-close{background:transparent;border:0;color:#9aa4b2;font-size:1.5rem;line-height:1;cursor:pointer;padding:0 4px}.mv-close:hover{color:#e6edf3}' +
      '.mv-stage{position:relative;flex:1;min-height:0;background:radial-gradient(circle at 50% 42%,#1b2531,#0c1118 80%);cursor:grab}.mv-stage.grab{cursor:grabbing}' +
      '.mv-stage canvas{display:block;width:100%;height:100%}' +
      '.mv-msg{position:absolute;inset:0;display:flex;align-items:center;justify-content:center;color:#9aa4b2;font-size:.9rem}.mv-msg.err{color:#f0997b}' +
      '.mv-bar{display:flex;flex-wrap:wrap;gap:7px;align-items:center;padding:10px 12px;border-top:1px solid #232a33}' +
      '.mv-btn{background:#1b2129;border:1px solid #2a323d;color:#cdd6e0;border-radius:8px;padding:6px 12px;font-size:.82rem;cursor:pointer}' +
      '.mv-btn:hover{border-color:#4cc9f0}.mv-btn.on{background:rgba(86,156,255,.16);border-color:#4cc9f0;color:#e6edf3}' +
      '.mv-btn.mv-loading{opacity:.55;cursor:progress}' +
      '.mv-hint{color:#6b7480;font-size:.74rem;margin-left:auto}';
    var s = document.createElement('style'); s.textContent = css; document.head.appendChild(s);
  }
  function ensureThree() {
    if (window.THREE) return Promise.resolve(window.THREE);
    if (_three) return _three;
    _three = new Promise(function (res, rej) {
      var s = document.createElement('script'); s.src = THREE_URL;
      s.onload = function () { window.THREE ? res(window.THREE) : rej(new Error('3D library failed to load.')); };
      s.onerror = function () { rej(new Error('Could not load the 3D library.')); };
      document.head.appendChild(s);
    });
    return _three;
  }

  function open(opts) {
    injectStyles();
    var ov = document.createElement('div'); ov.className = 'mv-overlay';
    ov.setAttribute('role', 'dialog');
    ov.setAttribute('aria-modal', 'true');
    ov.setAttribute('aria-label', (opts.title || 'Model') + ' — 3D model preview');
    ov.innerHTML =
      '<div class="mv-modal">' +
        '<div class="mv-head"><span class="mv-title"></span><span class="mv-meta"></span>' +
          '<button class="mv-close" type="button" aria-label="Close">×</button></div>' +
        '<div class="mv-stage"><div class="mv-msg">Loading model…</div></div>' +
        '<div class="mv-bar"></div>' +
      '</div>';
    ov.querySelector('.mv-title').textContent = opts.title || 'Model';
    document.body.appendChild(ov);
    var stage = ov.querySelector('.mv-stage'), msg = ov.querySelector('.mv-msg'),
        bar = ov.querySelector('.mv-bar'), meta = ov.querySelector('.mv-meta');
    var viewer = null;
    var releaseFocus = null;
    function close() { if (viewer) viewer.dispose(); document.removeEventListener('keydown', onKey); if (releaseFocus) { releaseFocus(); releaseFocus = null; } ov.remove(); }
    function onKey(e) { if (e.key === 'Escape') close(); }
    ov.querySelector('.mv-close').addEventListener('click', close);
    ov.addEventListener('mousedown', function (e) { if (e.target === ov) close(); });
    document.addEventListener('keydown', onKey);
    if (window.BTTUtil && window.BTTUtil.trapFocus) {
      releaseFocus = window.BTTUtil.trapFocus(ov.querySelector('.mv-modal'));
    }

    ensureThree().then(function (THREE) {
      return fetch(opts.url, { credentials: 'same-origin' }).then(function (r) {
        if (!r.ok) throw new Error('Could not load model (HTTP ' + r.status + ').');
        return r.json();
      }).then(function (data) {
        if (!ov.isConnected) return;
        msg.remove();
        var nv = data.parts.reduce(function (a, p) { return a + p.x.length; }, 0);
        meta.textContent = data.parts.length + ' parts · ' + nv.toLocaleString() + ' voxels';
        viewer = build(THREE, stage, bar, data);
      });
    }).catch(function (e) { msg.textContent = e.message || 'Could not load this model.'; msg.classList.add('err'); });
  }

  function build(THREE, stage, bar, data) {
    var W = stage.clientWidth || 900, H = stage.clientHeight || 560, s = data.voxel_scale;
    var renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
    renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
    renderer.setSize(W, H);
    renderer.domElement.setAttribute('role', 'img');
    renderer.domElement.setAttribute('aria-label', 'Interactive 3D creature model. Drag to rotate, scroll to zoom.');
    renderer.domElement.appendChild(document.createTextNode('3D model preview (requires a WebGL-capable browser).'));
    stage.appendChild(renderer.domElement);
    var scene = new THREE.Scene();
    scene.add(new THREE.AmbientLight(0xffffff, 0.68));
    var key = new THREE.DirectionalLight(0xffffff, 0.85); key.position.set(0.6, 1, 0.5); scene.add(key);
    var fill = new THREE.DirectionalLight(0xffffff, 0.32); fill.position.set(-0.5, 0.4, -0.6); scene.add(fill);
    var camera = new THREE.PerspectiveCamera(42, W / H, 0.001, 1000);

    var geo = new THREE.BoxGeometry(1, 1, 1), scaleM = new THREE.Matrix4().makeScale(s, s, s);
    var meshByPart = {};
    var obj = new THREE.Object3D(), col = new THREE.Color();
    // Material kinds (from the blueprint): 0 solid · 1 glass · 2 glow · 3 glow-glass.
    // level = glass alpha 16+32*w -> opacity (level/255)^2, matching the game/catalog.
    function makeMaterial(kind, level) {
      var opacity = Math.pow((level || 255) / 255, 2);
      if (kind === 2) return new THREE.MeshBasicMaterial();   // glow -> unlit/emissive
      if (kind === 3) return new THREE.MeshBasicMaterial({ transparent: true, opacity: opacity, depthWrite: false });
      if (kind === 1) return new THREE.MeshPhongMaterial({ transparent: true, opacity: opacity, depthWrite: false, shininess: 70, specular: 0x4d4d4d });
      return new THREE.MeshPhongMaterial({ shininess: 28, specular: 0x1c1c1c });   // solid -> specular sheen
    }
    data.parts.forEach(function (p) {
      var n = p.x.length, KIND = p.kind, LVL = p.level, groups = {};
      for (var i = 0; i < n; i++) {                 // split a part by material (+ glass level)
        var k = KIND ? (KIND[i] || 0) : 0;
        var lv = (k === 1 || k === 3) ? (LVL ? LVL[i] : 255) : 255;
        var gk = k + ':' + lv; (groups[gk] || (groups[gk] = [])).push(i);
      }
      var partMeshes = [];
      Object.keys(groups).forEach(function (gk) {
        var idx = groups[gk], kv = gk.split(':'), k = +kv[0], lv = +kv[1];
        var mesh = new THREE.InstancedMesh(geo, makeMaterial(k, lv), idx.length);
        for (var j = 0; j < idx.length; j++) {
          var vi = idx[j];
          obj.position.set(p.x[vi], p.y[vi], p.z[vi]); obj.updateMatrix(); mesh.setMatrixAt(j, obj.matrix);
          var c = p.rgb[vi]; col.setRGB(((c >> 16) & 255) / 255, ((c >> 8) & 255) / 255, (c & 255) / 255); mesh.setColorAt(j, col);
        }
        mesh.instanceMatrix.needsUpdate = true; if (mesh.instanceColor) mesh.instanceColor.needsUpdate = true;
        mesh.matrixAutoUpdate = false; mesh.frustumCulled = false;
        if (k === 1 || k === 3) mesh.renderOrder = 1;   // draw transparent after opaque
        partMeshes.push(mesh); scene.add(mesh);
      });
      meshByPart[p.name] = partMeshes;
    });
    function applyPose(pose) {                       // pose = {part:[16]}
      data.parts.forEach(function (p) {
        var m = pose[p.name], meshes = meshByPart[p.name]; if (!m || !meshes) return;
        for (var i = 0; i < meshes.length; i++) meshes[i].matrix.fromArray(m).multiply(scaleM);
      });
    }
    applyPose(data.rest);

    // frame the camera on the rest-pose bounds
    var box = new THREE.Box3(), v = new THREE.Vector3();
    data.parts.forEach(function (p) {
      var M = new THREE.Matrix4().fromArray(data.rest[p.name]).multiply(scaleM);
      for (var i = 0; i < p.x.length; i++) { v.set(p.x[i], p.y[i], p.z[i]).applyMatrix4(M); box.expandByPoint(v); }
    });
    var center = box.getCenter(new THREE.Vector3()), size = box.getSize(new THREE.Vector3());
    var modelR = Math.max(size.x, size.y, size.z) || 1;
    var target = center.clone(), sph = { r: modelR * 2.2, t: Math.PI * 0.25, p: Math.PI * 0.42 };
    function cam() {
      camera.position.set(target.x + sph.r * Math.sin(sph.p) * Math.sin(sph.t),
                          target.y + sph.r * Math.cos(sph.p),
                          target.z + sph.r * Math.sin(sph.p) * Math.cos(sph.t));
      camera.lookAt(target);
    }
    cam();

    // orbit controls
    var el = renderer.domElement, drag = 0, lx = 0, ly = 0, pinch = 0;
    var right = new THREE.Vector3(), up = new THREE.Vector3(), fwd = new THREE.Vector3();
    function rot(dx, dy) { sph.t -= dx * 0.01; sph.p = Math.max(0.05, Math.min(Math.PI - 0.05, sph.p - dy * 0.01)); }
    function pan(dx, dy) { var k = sph.r * 0.0016; camera.matrix.extractBasis(right, up, fwd); target.addScaledVector(right, -dx * k); target.addScaledVector(up, dy * k); }
    function zoom(f) { sph.r = Math.max(modelR * 0.4, Math.min(modelR * 9, sph.r * f)); }
    function down(e) { drag = (e.button === 2 || e.shiftKey) ? 2 : 1; lx = e.clientX; ly = e.clientY; stage.classList.add('grab'); e.preventDefault(); }
    function move(e) { if (!drag) return; var dx = e.clientX - lx, dy = e.clientY - ly; lx = e.clientX; ly = e.clientY; if (drag === 2) pan(dx, dy); else rot(dx, dy); cam(); request(); }
    function upE() { drag = 0; stage.classList.remove('grab'); }
    function wheel(e) { e.preventDefault(); zoom(e.deltaY > 0 ? 1.12 : 0.89); cam(); request(); }
    function tdist(e) { var a = e.touches[0], b = e.touches[1]; return Math.hypot(a.clientX - b.clientX, a.clientY - b.clientY); }
    function ts(e) { if (e.touches.length === 1) { drag = 1; lx = e.touches[0].clientX; ly = e.touches[0].clientY; } else if (e.touches.length === 2) { drag = 0; pinch = tdist(e); } }
    function tm(e) { if (e.touches.length === 1 && drag === 1) { var t = e.touches[0]; rot(t.clientX - lx, t.clientY - ly); lx = t.clientX; ly = t.clientY; cam(); request(); } else if (e.touches.length === 2) { var d = tdist(e); if (pinch) { zoom(pinch / d); cam(); request(); } pinch = d; } e.preventDefault(); }
    function te() { drag = 0; pinch = 0; }
    el.addEventListener('mousedown', down); window.addEventListener('mousemove', move); window.addEventListener('mouseup', upE);
    el.addEventListener('wheel', wheel, { passive: false }); el.addEventListener('contextmenu', function (e) { e.preventDefault(); });
    el.addEventListener('touchstart', ts, { passive: false }); el.addEventListener('touchmove', tm, { passive: false }); el.addEventListener('touchend', te);
    function onResize() { var w = stage.clientWidth, h = stage.clientHeight; if (!w || !h) return; camera.aspect = w / h; camera.updateProjectionMatrix(); renderer.setSize(w, h); request(); }
    window.addEventListener('resize', onResize);

    // render-on-demand; a rAF loop runs only while an animation plays. Animation FRAMES
    // are fetched lazily per clip (the payload only carries metadata) and cached.
    var alive = true, anim = null, want = null, animStart = 0, raf = 0, pending = false;
    var loaded = {};   // animation name -> {fps, frames:[{part:[16]}]}
    function renderOnce() { renderer.render(scene, camera); }
    function request() { if (anim) return; if (!pending) { pending = true; requestAnimationFrame(function () { pending = false; renderOnce(); }); } }
    function loop(ts2) {
      if (!alive || !anim) return;
      var A = loaded[anim]; if (!A || !A.frames || !A.frames.length) return;
      raf = requestAnimationFrame(loop);
      var dur = A.frames.length / A.fps;
      var t = ((ts2 - animStart) / 1000) % dur;
      var fi = Math.floor(t * A.fps) % A.frames.length;
      applyPose(A.frames[fi]); renderOnce();
    }
    function setActive(name) { Array.prototype.forEach.call(bar.querySelectorAll('.mv-btn'), function (b) { b.classList.toggle('on', b.dataset.anim === (name || 'rest')); }); }
    function startAnim(name) { anim = name; animStart = performance.now(); cancelAnimationFrame(raf); raf = requestAnimationFrame(loop); }
    function play(name) {
      cancelAnimationFrame(raf); anim = null; want = name;
      if (!name) { applyPose(data.rest); renderOnce(); setActive(null); return; }
      setActive(name);
      if (loaded[name]) { startAnim(name); return; }
      if (!data.rig) return;                                  // no skeleton -> can't fetch frames
      var btn = bar.querySelector('.mv-btn[data-anim="' + name + '"]');
      if (btn) btn.classList.add('mv-loading');
      fetch('/site/rigs/' + encodeURIComponent(data.rig) + '/anim/' + encodeURIComponent(name), { credentials: 'same-origin' })
        .then(function (r) { if (!r.ok) throw new Error(); return r.json(); })
        .then(function (a) { loaded[name] = a; if (btn) btn.classList.remove('mv-loading'); if (want === name) startAnim(name); })
        .catch(function () { if (btn) btn.classList.remove('mv-loading'); });
    }

    // control bar: Rest + one button per animation (names from the metadata)
    function mkBtn(label, an) { var b = document.createElement('button'); b.className = 'mv-btn'; b.textContent = label; b.dataset.anim = an; b.addEventListener('click', function () { play(an === 'rest' ? null : an); }); bar.appendChild(b); return b; }
    mkBtn('Rest pose', 'rest');
    Object.keys(data.animations || {}).forEach(function (k) { mkBtn(k.replace(/^unarmed_/, '').replace(/_/g, ' '), k); });
    var hint = document.createElement('span'); hint.className = 'mv-hint'; hint.textContent = 'drag rotate · scroll zoom · right-drag pan'; bar.appendChild(hint);
    play(null);

    return { dispose: function () {
      alive = false; cancelAnimationFrame(raf);
      el.removeEventListener('mousedown', down); window.removeEventListener('mousemove', move); window.removeEventListener('mouseup', upE);
      el.removeEventListener('wheel', wheel); el.removeEventListener('touchstart', ts); el.removeEventListener('touchmove', tm); el.removeEventListener('touchend', te);
      window.removeEventListener('resize', onResize);
      Object.keys(meshByPart).forEach(function (k) { meshByPart[k].forEach(function (m) { m.material.dispose(); }); });
      geo.dispose(); renderer.dispose(); if (el.parentNode) el.parentNode.removeChild(el);
    } };
  }

  window.ModelViewer = { open: open };
})();
