/* Kiwi assembled-model viewer.
   Renders a Trove creature assembled from blueprint voxel parts placed on its
   skeleton (rest pose), with animation playback. Pure data at runtime - the model
   JSON is baked offline (Granny skeleton + animations -> per-bone transforms).

   Model payload:
     { voxel_scale, parts:[{name,x[],y[],z[],rgb[]}],
       rest:{part:[16]}, animations:{name:{fps,frames:N}} }
   Matrices are column-major (three.js order). `animations` is METADATA only; a clip's
   frames are fetched on demand from /site/rigs/<rig>/anim/<name> as a TANIM1 binary
   (see decodeAnim) - the attach-point transforms are rigid, so they ship as
   position+quaternion float32 rather than 4x4 matrices of JSON text.

   API: window.ModelViewer.open({ url, title })   -- modal
        window.ModelViewer.mount(el, { url, bar, onMeta, apiBase })  -- inline (embed page) */
(function () {
  'use strict';
  var THREE_URL = '/static/vendor/three.min.js';  // self-hosted (GDPR: no cdnjs IP leak)
  var _styles = false, _three = null;
  // Origin serving /site/rigs/* (the lazily-fetched animation clips). Empty = same
  // origin, which is the Mods Hub case. The embeddable viewer is served from the
  // website host while its data lives on the API, so it passes that origin in.
  var _apiBase = '';

  /* Decode one baked animation clip (TANIM1). Layout, little-endian:
       8s  magic "TANIM1\0\0"
       u32 ap_count, u32 frame_count, u32 fps, u32 name_blob_len
       ..  name_blob: NUL-separated attach-point keys, NUL-padded to a 4-byte boundary
       ..  frame_count * ap_count * 7 float32: position xyz then quaternion xyzw
     The float payload is 4-byte aligned by construction, so it wraps with no copy. */
  function decodeAnim(buf) {
    var dv = new DataView(buf);
    var head = new Uint8Array(buf, 0, 6), magic = '';
    for (var i = 0; i < 6; i++) magic += String.fromCharCode(head[i]);
    if (magic !== 'TANIM1') throw new Error('bad animation clip');
    var apCount = dv.getUint32(8, true), frameCount = dv.getUint32(12, true);
    var fps = dv.getUint32(16, true), nb = dv.getUint32(20, true);
    var raw = new Uint8Array(buf, 24, nb), s = '';
    for (var j = 0; j < nb; j++) s += String.fromCharCode(raw[j]);
    var keys = s.split('\0').filter(Boolean);
    var apIndex = {};
    for (var k = 0; k < keys.length; k++) apIndex[keys[k]] = k;
    return {
      fps: fps, frameCount: frameCount, apCount: apCount, apIndex: apIndex,
      data: new Float32Array(buf, 24 + nb, frameCount * apCount * 7),
    };
  }

  /* Clip names read <stance>_<action>_<detail>, e.g. "unarmed_ability_drink" or
     "mount_balance_idle". A companion ships a handful; a player rig ships 80+, which
     would bury the model under buttons, so those bucket by action and the bar shows one
     bucket at a time. Matching is prefix-based on the action token - anything unmatched
     lands in "Other" rather than being guessed into the wrong bucket. */
  var STANCES = ['unarmed_', 'ranged_', '1h_', '2h_'];
  var CLIP_GROUPS = [
    ['Idle', ['idle', 'sleep', 'pose', 'sit', 'afk', 'bob', 'stand', 'breathe', 'wait']],
    ['Move', ['walk', 'run', 'jump', 'roll', 'swim', 'glide', 'fly', 'climb', 'dash',
              'sprint', 'land', 'fall', 'turn', 'strafe', 'charge', 'move', 'hover',
              'crawl', 'slide', 'recall', 'leap', 'flip']],
    ['Mount', ['mount']],
    ['Combat', ['ability', 'melee', 'shoot', 'throw', 'attack', 'block', 'cast', 'mine',
                'shockwave', 'damage', 'death', 'die', 'hit', 'fling', 'instant', 'swing',
                'stab', 'punch', 'kick', 'slam', 'rampage', 'multispit', 'breath']],
    ['Emote', ['emote', 'dance', 'wave', 'laugh', 'cry', 'shrug', 'taunt', 'bow',
               'excite', 'point', 'clap', 'salute', 'prance']],
    ['Fishing', ['fishing']],
  ];
  var OTHER = 'Other';

  function stripStance(name) {
    for (var i = 0; i < STANCES.length; i++) {
      if (name.indexOf(STANCES[i]) === 0) return name.slice(STANCES[i].length);
    }
    return name;
  }
  /* -> { group, token }; token is the action word the bucket matched on, so the label
     can drop it (under "Mount", "mount balance idle" reads better as "balance idle"). */
  function clipInfo(name) {
    var b = stripStance(name);
    for (var i = 0; i < CLIP_GROUPS.length; i++) {
      var toks = CLIP_GROUPS[i][1];
      for (var j = 0; j < toks.length; j++) {
        if (b === toks[j] || b.indexOf(toks[j] + '_') === 0) {
          return { group: CLIP_GROUPS[i][0], token: toks[j] };
        }
      }
    }
    return { group: OTHER, token: null };
  }
  function groupRank(g) {
    for (var i = 0; i < CLIP_GROUPS.length; i++) if (CLIP_GROUPS[i][0] === g) return i;
    return CLIP_GROUPS.length;                     // Other sorts last
  }
  /* Tokens that are pure namespaces - the bucket chip already says "Mount", so
     "mount balance idle" reads better as "balance idle". Every other token carries
     meaning that the bucket name does not ("walk"/"run" both live under Move, and
     "idle class" must not collapse to a bare "class"), so it stays in the label. */
  var NAMESPACE_TOKENS = { mount: 1, ability: 1, emote: 1, fishing: 1 };

  function clipLabel(name, token) {
    var b = stripStance(name).replace(/_/g, ' ');
    if (token && NAMESPACE_TOKENS[token] && b.indexOf(token + ' ') === 0) {
      b = b.slice(token.length + 1);
    }
    return b;
  }

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
      // shrink-to-fit rather than push the buckets onto a second row and cost the model
      // a whole line of height
      '.mv-hint{color:#6b7480;font-size:.74rem;margin-left:auto;flex:0 1 auto;min-width:0;' +
        'overflow:hidden;text-overflow:ellipsis;white-space:nowrap}' +
      // grouped mode: a row of action buckets over a row of that bucket's clips.
      // Both rows take a full flex line so they stack inside the host's bar.
      '.mv-cats,.mv-clips{display:flex;flex-wrap:wrap;gap:7px;align-items:center;flex:1 0 100%;min-width:0}' +
      // hard cap so a 30-clip bucket scrolls instead of squeezing the model; the vh term
      // keeps it proportionate inside a short embed iframe
      '.mv-clips{max-height:min(5.6rem,26vh);overflow-y:auto;overscroll-behavior:contain}' +
      '.mv-cat{background:transparent;border:1px solid #2a323d;color:#9aa4b2;border-radius:999px;' +
        'padding:5px 12px;font-size:.78rem;cursor:pointer}' +
      '.mv-cat:hover{border-color:#4cc9f0;color:#cdd6e0}' +
      '.mv-cat.on{background:rgba(86,156,255,.16);border-color:#4cc9f0;color:#e6edf3}' +
      '.mv-cat-n{opacity:.6;font-variant-numeric:tabular-nums}' +
      '.mv-sep{width:1px;height:18px;background:#2a323d;flex:0 0 auto}';
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

  /* Fetch the payload, preferring the binary container (voxel_binary.js): an assembled
     creature is the biggest thing we ship, so skipping the JSON parse matters most
     here. Falls back to plain JSON when that script isn't on the page. */
  function loadModel(url) {
    if (window.VoxelBinary) return window.VoxelBinary.fetchModel(url);
    return fetch(url, { credentials: 'same-origin' }).then(function (r) {
      if (!r.ok) throw new Error('Could not load model (HTTP ' + r.status + ').');
      return r.json();
    });
  }

  /* Load an assembled model into an existing element (no modal) - used by the modal
     below and by the embeddable viewer (/embed/viewer). `bar` is where the animation
     buttons go; pass one so the host controls where they sit. Returns { dispose }. */
  function mount(container, opts) {
    injectStyles();
    container.classList.add('mv-stage');
    if (typeof opts.apiBase === 'string') _apiBase = opts.apiBase.replace(/\/$/, '');
    var msg = document.createElement('div');
    msg.className = 'mv-msg';
    msg.textContent = 'Loading model…';
    container.appendChild(msg);
    var bar = opts.bar || document.createElement('div');

    var viewer = null, alive = true;
    ensureThree().then(function (THREE) {
      return loadModel(opts.url).then(function (data) {
        if (!alive) return;
        msg.remove();
        var nv = data.parts.reduce(function (a, p) { return a + p.x.length; }, 0);
        if (opts.onMeta) opts.onMeta(data.parts.length + ' parts · ' + nv.toLocaleString() + ' voxels');
        viewer = build(THREE, container, bar, data);
      });
    }).catch(function (e) {
      if (!alive) return;
      // build() runs after msg.remove(), so re-attach or the failure is invisible
      if (!msg.parentNode) container.appendChild(msg);
      msg.textContent = e.message || 'Could not load this model.';
      msg.classList.add('err');
      if (window.console) console.error('model viewer:', e);
    });

    return {
      state: function () { return viewer ? viewer.state() : null; },
      poseFrame: function (n, f) { return viewer ? viewer.poseFrame(n, f) : null; },
      dispose: function () {
        alive = false;
        if (viewer) { viewer.dispose(); viewer = null; }
      },
    };
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
        '<div class="mv-stage"></div>' +
        '<div class="mv-bar"></div>' +
      '</div>';
    ov.querySelector('.mv-title').textContent = opts.title || 'Model';
    document.body.appendChild(ov);
    var stage = ov.querySelector('.mv-stage'),
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

    viewer = mount(stage, {
      url: opts.url, bar: bar,
      onMeta: function (text) { meta.textContent = text; },
    });
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

    var scaleM = new THREE.Matrix4().makeScale(s, s, s);
    /* Head slots (head/hat/hair/face) are modelled at DOUBLE resolution so a face can
       carry detail the body never needs, so they carry their own `scale` and are drawn
       at half the voxel size. Every voxel is kept - only the size of each one changes -
       and without it the head comes out twice the size of the character wearing it. */
    var scaleOf = {};
    data.parts.forEach(function (p) {
      var ps = s * (typeof p.scale === 'number' ? p.scale : 1);
      scaleOf[p.name] = ps === s ? scaleM : new THREE.Matrix4().makeScale(ps, ps, ps);
    });
    // One mesh per material group per part, with the faces you can't see culled
    // (voxel_mesh.js). Culling is per PART - two parts meeting at a joint are placed
    // by different bone matrices, so neither can know it's hidden by the other.
    var meshByPart = {};
    data.parts.forEach(function (p) {
      var partMeshes = window.VoxelMesh.build(THREE, p, {
        brdfUrl: _apiBase + '/site/render/brdf-map.png',
        lightDir: [0.6, 1.0, 0.5],                   // the key light, in world space
        onReady: function () { request(); },         // redraw when the atlas lands
      });
      partMeshes.forEach(function (mesh) {
        mesh.matrixAutoUpdate = false; mesh.frustumCulled = false;
        scene.add(mesh);
      });
      meshByPart[p.name] = partMeshes;
    });
    function applyPose(pose) {                       // pose = {part:[16]}
      data.parts.forEach(function (p) {
        var m = pose[p.name], meshes = meshByPart[p.name]; if (!m || !meshes) return;
        var sm = scaleOf[p.name] || scaleM;
        for (var i = 0; i < meshes.length; i++) meshes[i].matrix.fromArray(m).multiply(sm);
      });
    }
    applyPose(data.rest);

    // frame the camera on the rest-pose bounds
    var box = new THREE.Box3(), v = new THREE.Vector3();
    data.parts.forEach(function (p) {
      var M = new THREE.Matrix4().fromArray(data.rest[p.name]).multiply(scaleOf[p.name] || scaleM);
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
    /* three.js writes the canvas size as an INLINE style, which outranks the host's
       `height:100%` rule, so a canvas sized before the control bar existed keeps its
       old height, spills over the bar and swallows every click. The bar is built below
       (and grows again when a big bucket is picked), so watch the stage itself rather
       than relying on window resizes. */
    var ro = null;
    if (window.ResizeObserver) {
      ro = new ResizeObserver(function () { onResize(); });
      ro.observe(stage);
    }

    // render-on-demand; a rAF loop runs only while an animation plays. Animation CLIPS
    // are fetched lazily (the payload only carries metadata) and cached.
    var alive = true, anim = null, want = null, animStart = 0, raf = 0, pending = false;
    var loaded = {};   // animation name -> decodeAnim() result
    var _p = new THREE.Vector3(), _q = new THREE.Quaternion(), _one = new THREE.Vector3(1, 1, 1);
    function renderOnce() { renderer.render(scene, camera); }
    function request() { if (anim) return; if (!pending) { pending = true; requestAnimationFrame(function () { pending = false; renderOnce(); }); } }
    /* Pose the parts from frame `fi` of a decoded clip: rebuild each attach point's
       matrix from its position+quaternion instead of reading a stored 4x4. */
    function applyFrame(A, fi) {
      var base = fi * A.apCount * 7, d = A.data;
      data.parts.forEach(function (p) {
        var ai = A.apIndex[p.name], meshes = meshByPart[p.name];
        if (ai === undefined || !meshes) return;
        var o = base + ai * 7;
        _p.set(d[o], d[o + 1], d[o + 2]);
        _q.set(d[o + 3], d[o + 4], d[o + 5], d[o + 6]);
        var sm = scaleOf[p.name] || scaleM;
        for (var i = 0; i < meshes.length; i++) meshes[i].matrix.compose(_p, _q, _one).multiply(sm);
      });
    }
    function loop(ts2) {
      if (!alive || !anim) return;
      var A = loaded[anim]; if (!A || !A.frameCount) return;
      raf = requestAnimationFrame(loop);
      var dur = A.frameCount / A.fps;
      var t = ((ts2 - animStart) / 1000) % dur;
      var fi = Math.floor(t * A.fps) % A.frameCount;
      applyFrame(A, fi); renderOnce();
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
      fetch(_apiBase + '/site/rigs/' + encodeURIComponent(data.rig) + '/anim/' + encodeURIComponent(name), { credentials: 'same-origin' })
        .then(function (r) { if (!r.ok) throw new Error(); return r.arrayBuffer(); })
        .then(function (buf) {
          loaded[name] = decodeAnim(buf);
          if (btn) btn.classList.remove('mv-loading');
          if (want === name) startAnim(name);
        })
        .catch(function (e) {
          if (btn) btn.classList.remove('mv-loading');
          if (window.console) console.error('model viewer: animation "' + name + '":', e);
        });
    }

    // Control bar: Rest + one button per animation (names from the metadata). Up to
    // FLAT_MAX clips list flat; past that they bucket by action so the bar stays a
    // couple of rows instead of burying the model.
    var FLAT_MAX = 12;
    function mkBtn(parent, label, an) {
      var b = document.createElement('button');
      b.className = 'mv-btn'; b.textContent = label; b.dataset.anim = an;
      b.addEventListener('click', function () { play(an === 'rest' ? null : an); });
      parent.appendChild(b); return b;
    }
    var clipNames = Object.keys(data.animations || {});
    var hintHost = bar;
    if (clipNames.length <= FLAT_MAX) {
      mkBtn(bar, 'Rest pose', 'rest');
      clipNames.forEach(function (k) { mkBtn(bar, clipLabel(k, null), k); });
    } else {
      var buckets = {}, groups = [];
      clipNames.forEach(function (k) {
        var g = clipInfo(k).group;
        if (!buckets[g]) { buckets[g] = []; groups.push(g); }
        buckets[g].push(k);
      });
      groups.sort(function (a, b) { return groupRank(a) - groupRank(b); });

      var cats = document.createElement('div');
      cats.className = 'mv-cats';
      var clips = document.createElement('div');
      clips.className = 'mv-clips';
      clips.setAttribute('role', 'group');
      clips.setAttribute('aria-label', 'Animation clips');
      bar.appendChild(cats); bar.appendChild(clips);

      mkBtn(cats, 'Rest pose', 'rest');
      var sep = document.createElement('span');
      sep.className = 'mv-sep'; sep.setAttribute('aria-hidden', 'true');
      cats.appendChild(sep);

      function showGroup(g) {
        clips.textContent = '';
        buckets[g].forEach(function (k) { mkBtn(clips, clipLabel(k, clipInfo(k).token), k); });
        onResize();                               // bucket sizes differ -> bar height changed
        Array.prototype.forEach.call(cats.querySelectorAll('.mv-cat'), function (b) {
          var on = b.dataset.cat === g;
          b.classList.toggle('on', on);
          b.setAttribute('aria-pressed', on ? 'true' : 'false');
        });
        setActive(anim);                          // keep the playing clip highlighted
      }
      groups.forEach(function (g) {
        var b = document.createElement('button');
        b.className = 'mv-cat'; b.dataset.cat = g;
        b.setAttribute('aria-pressed', 'false');
        b.appendChild(document.createTextNode(g + ' '));
        var n = document.createElement('span');
        n.className = 'mv-cat-n'; n.textContent = buckets[g].length;
        b.appendChild(n);
        b.addEventListener('click', function () { showGroup(g); });
        cats.appendChild(b);
      });
      showGroup(groups[0]);
      hintHost = cats;                            // ride the category row, not a third line
    }
    var hint = document.createElement('span'); hint.className = 'mv-hint'; hint.textContent = 'drag rotate · scroll zoom · right-drag pan'; hintHost.appendChild(hint);
    // The canvas was sized against a stage that had no control bar under it yet. Re-fit
    // now that the bar occupies its real height, or the canvas overhangs it and eats
    // every click. Done explicitly rather than left to the ResizeObserver above, which
    // only delivers on a rendering tick.
    onResize();
    play(null);

    return {
      // test hook: what the playback loop currently sees
      state: function () {
        var A = anim ? loaded[anim] : null;
        return { anim: anim, want: want, alive: alive, cached: Object.keys(loaded),
                 clip: A ? { fps: A.fps, frameCount: A.frameCount, apCount: A.apCount,
                             dataLen: A.data && A.data.length } : null };
      },
      /* test hook: pose one specific frame of a loaded clip and report the resulting
         attach-point matrices. Playback itself is rAF-driven, which never runs in a
         non-compositing tab, so tests drive frames through here instead. */
      poseFrame: function (name, fi) {
        var A = loaded[name];
        if (!A || !A.frameCount) return null;
        applyFrame(A, ((fi % A.frameCount) + A.frameCount) % A.frameCount);
        renderOnce();
        var out = {};
        data.parts.forEach(function (p) {
          var m = meshByPart[p.name] && meshByPart[p.name][0];
          if (m) out[p.name] = Array.prototype.slice.call(m.matrix.elements);
        });
        return out;
      },
      dispose: function () {
      alive = false; cancelAnimationFrame(raf);
      el.removeEventListener('mousedown', down); window.removeEventListener('mousemove', move); window.removeEventListener('mouseup', upE);
      el.removeEventListener('wheel', wheel); el.removeEventListener('touchstart', ts); el.removeEventListener('touchmove', tm); el.removeEventListener('touchend', te);
      window.removeEventListener('resize', onResize);
      if (ro) { ro.disconnect(); ro = null; }
      Object.keys(meshByPart).forEach(function (k) {
        meshByPart[k].forEach(function (m) { m.geometry.dispose(); m.material.dispose(); });
      });
      renderer.dispose(); if (el.parentNode) el.parentNode.removeChild(el);
    } };
  }

  window.ModelViewer = { open: open, mount: mount };
})();
