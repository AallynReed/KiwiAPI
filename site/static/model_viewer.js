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

  /* Where a /site/* asset actually answers. The embed is told its apiBase outright.
     On the site, /site/* moves to the API origin once the site is split off it, and
     `_site_util.js` rewrites fetch/XHR for you - but NOT an <img>, which is how the
     specular atlas is loaded, so that one has to ask `apiUrl` itself or 404 and
     leave every solid shaded as rough. */
  function assetUrl(path) {
    if (_apiBase) return _apiBase + path;
    var U = window.BTTUtil;
    return (U && U.apiUrl) ? U.apiUrl(path) : path;
  }

  /* Clip decoding, name grouping, the rig's state machine -> moves, the control bar
     and the playback maths all live in `anim_clips.js` now: the Blueprint Editor plays
     the same clips on the same bar, and two copies of the bucketing rules would mean
     two answers to "which button is this clip under". */
  var AC = function () { return window.AnimClips; };


  function injectStyles() {
    if (_styles) return; _styles = true;
    var css =
      '.mv-overlay{position:fixed;inset:0;z-index:9999;background:rgba(4,7,12,.8);display:flex;align-items:center;justify-content:center;padding:20px}' +
      '.mv-modal{display:flex;flex-direction:column;width:min(1000px,95vw);height:min(760px,90vh);background:#10151c;border:1px solid #232a33;border-radius:14px;overflow:hidden;box-shadow:0 24px 60px rgba(0,0,0,.55)}' +
      '.mv-head{display:flex;align-items:center;gap:12px;padding:11px 14px;border-bottom:1px solid #232a33}' +
      '.mv-title{font-weight:700;color:#e6edf3;font-size:.98rem;flex:1;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}' +
      '.mv-meta{color:#6b7480;font-size:.75rem}' +
      '.mv-close{background:transparent;border:0;color:#9aa4b2;font-size:1.5rem;line-height:1;cursor:pointer;padding:0 4px}.mv-close:hover{color:#e6edf3}' +
      // The two stops are custom properties, and the gradient is spelled exactly as
      // viewer_stage.js documents, because that script redraws it by hand when
      // saving a PNG (and can replace it with a colour or an image).
      '.mv-stage{position:relative;flex:1;min-height:0;cursor:grab;' +
        '--vs-bg-a:#1b2531;--vs-bg-b:#0c1118;' +
        'background:radial-gradient(circle at 50% 40%,var(--vs-bg-a),var(--vs-bg-b) 78%)}' +
      '.mv-stage.grab{cursor:grabbing}' +
      '.mv-stage canvas{display:block;width:100%;height:100%}' +
      // The animation bar's own paint ships with anim_clips.js, which draws it.
      '.mv-msg{position:absolute;inset:0;display:flex;align-items:center;justify-content:center;color:#9aa4b2;font-size:.9rem}.mv-msg.err{color:#f0997b}';
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

  /* The rig's animation state machine, or null. A rig without one (props, chests) just
     lists its clips, so a miss here is a normal outcome and never an error. */
  function loadGraph(rig) {
    if (!rig) return Promise.resolve(null);
    return fetch(_apiBase + '/site/rigs/' + encodeURIComponent(rig) + '/graph',
                 { credentials: 'same-origin' })
      .then(function (r) { return r.ok ? r.json() : null; })
      .catch(function () { return null; });
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
        if (!alive) return null;
        // the graph only decides which BUTTONS there are, so it is fetched alongside the
        // model rather than blocking on it
        return loadGraph(data.rig).then(function (graph) {
          if (!alive) return;
          msg.remove();
          var nv = data.parts.reduce(function (a, p) { return a + p.x.length; }, 0);
          if (opts.onMeta) opts.onMeta(data.parts.length + ' parts · ' + nv.toLocaleString() + ' voxels');
          viewer = build(THREE, container, bar, data, graph, opts.title);
        });
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
      url: opts.url, bar: bar, title: opts.title,
      onMeta: function (text) { meta.textContent = text; },
    });
  }

  function build(THREE, stage, bar, data, graph, title) {
    var W = stage.clientWidth || 900, H = stage.clientHeight || 560, s = data.voxel_scale;
    var renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
    renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
    renderer.setSize(W, H);
    renderer.domElement.setAttribute('role', 'img');
    renderer.domElement.setAttribute('aria-label', 'Interactive 3D creature model. Drag to rotate, scroll to zoom.');
    renderer.domElement.appendChild(document.createTextNode('3D model preview (requires a WebGL-capable browser).'));
    stage.appendChild(renderer.domElement);
    // No scene lights: the voxel materials run Trove's own object shader, which
    // carries its sun and ambient as uniforms (voxel_mesh.js).
    var scene = new THREE.Scene();
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
    var meshByPart = {}, pickable = [];             // flat list, for the pan raycast
    data.parts.forEach(function (p) {
      var partMeshes = window.VoxelMesh.build(THREE, p, {
        brdfUrl: assetUrl('/site/render/brdf-map.png'),
        lightDir: [0.6, 1.0, 0.5],                   // the key light, in world space
        onReady: function () { request(); },         // redraw when the atlas lands
      });
      partMeshes.forEach(function (mesh) {
        mesh.matrixAutoUpdate = false; mesh.frustumCulled = false;
        scene.add(mesh); pickable.push(mesh);
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
    /* The model spins around ITSELF, wherever you have dragged it to.

       Panning used to move the orbit centre through the world, so once you slid the
       model over to one side the camera was circling a point out in empty space
       beside it and a rotation threw the model clean out of frame. The pivot is the
       model's own centre and stays there; the pan is kept as an offset in CAMERA
       space (`panX`/`panY`, along the view's right and up axes) instead of being
       baked into a world position. Because that offset is rebuilt from the current
       angles on every frame, the model holds its place on screen while you orbit -
       it turns where it sits rather than swinging around the middle of the stage. */
    var pivot = center.clone();                    // the model's own centre
    var target = new THREE.Vector3();              // pivot + the pan offset
    var panX = 0, panY = 0;
    var sph = { r: modelR * 2.2, t: Math.PI * 0.25, p: Math.PI * 0.42 };

    // The orbit basis for the current angles. `dir` runs target -> camera; right and
    // up are what three's lookAt builds from it against a world +Y, so a pan offset
    // measured along them lands exactly where the cursor moved.
    var dir = new THREE.Vector3(), bRight = new THREE.Vector3(), bUp = new THREE.Vector3();
    var WORLD_UP = new THREE.Vector3(0, 1, 0);

    function cam() {
      dir.set(Math.sin(sph.p) * Math.sin(sph.t), Math.cos(sph.p),
              Math.sin(sph.p) * Math.cos(sph.t));
      bRight.crossVectors(WORLD_UP, dir).normalize();        // p is clamped off the poles
      bUp.crossVectors(dir, bRight);
      target.copy(pivot).addScaledVector(bRight, panX).addScaledVector(bUp, panY);
      camera.position.copy(target).addScaledVector(dir, sph.r);
      camera.lookAt(target);
    }
    cam();

    // orbit controls
    var el = renderer.domElement, drag = 0, lx = 0, ly = 0, pinch = 0;
    function rot(dx, dy) { sph.t -= dx * 0.01; sph.p = Math.max(0.05, Math.min(Math.PI - 0.05, sph.p - dy * 0.01)); }
    /* Panning drags the model by the point you grabbed, so that point has to stay
       under the cursor. Two things decide whether it does.

       The SCALE: a pixel of drag must move the world by one pixel's worth, which is
       the viewport's world height at the panning depth over the canvas height in CSS
       pixels. The same number serves horizontally, because world width and pixel
       width both scale by the aspect ratio.

       The DEPTH: that height is a function of how far away the grabbed thing is, and
       a voxel in front of or behind the orbit target is not at the target's distance.
       So one raycast on pointerdown finds what is actually under the cursor and keeps
       it as the anchor; grabbing empty space leaves no anchor and falls back to the
       target, which is the best guess available. Depth is measured perpendicular to
       the image plane rather than along the ray - the ray is longer off-axis, and it
       is the perpendicular distance the pixel scale depends on. It is recomputed per
       move so zooming mid-drag stays honest; a pure pan never changes it.

       One raycast per drag, not per move: an assembled creature is a lot of triangles
       and the anchor cannot change while the button is held. */
    var caster = new THREE.Raycaster(), ndc = new THREE.Vector2();
    var viewDir = new THREE.Vector3(), scratch = new THREE.Vector3();
    var anchor = null;                      // world point grabbed, or null

    function grabAnchor(e) {
      anchor = null;
      var box = el.getBoundingClientRect();
      if (!box.width || !box.height) return;
      ndc.set(((e.clientX - box.left) / box.width) * 2 - 1,
              -((e.clientY - box.top) / box.height) * 2 + 1);
      camera.updateMatrixWorld();
      scene.updateMatrixWorld();
      caster.setFromCamera(ndc, camera);
      var hit = caster.intersectObjects(pickable, false)[0];
      if (hit && hit.point) anchor = hit.point.clone();
    }

    function panDepth() {
      if (!anchor) return sph.r;
      camera.getWorldDirection(viewDir);
      return Math.max(1e-6, scratch.copy(anchor).sub(camera.position).dot(viewDir));
    }

    function pan(dx, dy) {
      camera.updateMatrixWorld();
      var k = 2 * panDepth() * Math.tan(camera.fov * Math.PI / 360) / (stage.clientHeight || 1);
      panX -= dx * k; panY += dy * k;
    }
    function zoom(f) { sph.r = Math.max(modelR * 0.4, Math.min(modelR * 9, sph.r * f)); }
    /* Pointer events WITH CAPTURE, not window-level mouse listeners. Framed into
       another site, letting go of the button outside the frame delivers the mouseup
       to the parent document - this window never sees it, so the drag never ends and
       the model keeps spinning with the cursor. `setPointerCapture` routes every
       later event for that pointer to this element wherever it travels, which is the
       only thing that survives the frame boundary. Touch keeps its own handlers below
       (pinch needs the whole touch list), so touch pointers are ignored here rather
       than handling the same gesture twice. */
    function down(e) {
      if (e.pointerType === 'touch') return;
      drag = (e.button === 2 || e.shiftKey) ? 2 : 1; lx = e.clientX; ly = e.clientY;
      if (drag === 2) grabAnchor(e);        // pan: pin the point under the cursor
      stage.classList.add('grab');
      try { el.setPointerCapture(e.pointerId); } catch (err) { /* pointer already released */ }
      e.preventDefault();
    }
    function move(e) { if (!drag || e.pointerType === 'touch') return; var dx = e.clientX - lx, dy = e.clientY - ly; lx = e.clientX; ly = e.clientY; if (drag === 2) pan(dx, dy); else rot(dx, dy); cam(); request(); }
    function upE() { drag = 0; anchor = null; stage.classList.remove('grab'); }
    function wheel(e) { e.preventDefault(); zoom(e.deltaY > 0 ? 1.12 : 0.89); cam(); request(); }
    function tdist(e) { var a = e.touches[0], b = e.touches[1]; return Math.hypot(a.clientX - b.clientX, a.clientY - b.clientY); }
    function ts(e) { if (e.touches.length === 1) { drag = 1; lx = e.touches[0].clientX; ly = e.touches[0].clientY; } else if (e.touches.length === 2) { drag = 0; pinch = tdist(e); } }
    function tm(e) { if (e.touches.length === 1 && drag === 1) { var t = e.touches[0]; rot(t.clientX - lx, t.clientY - ly); lx = t.clientX; ly = t.clientY; cam(); request(); } else if (e.touches.length === 2) { var d = tdist(e); if (pinch) { zoom(pinch / d); cam(); request(); } pinch = d; } e.preventDefault(); }
    function te() { drag = 0; pinch = 0; }
    el.addEventListener('pointerdown', down); el.addEventListener('pointermove', move); el.addEventListener('pointerup', upE);
    // capture can be lost without a pointerup (another element grabs it, the tab
    // hides, the gesture is cancelled) - each one has to end the drag too
    el.addEventListener('pointercancel', upE); el.addEventListener('lostpointercapture', upE);
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
    var loaded = {};     // animation name -> AnimClips.decode() result
    var programs = {};   // button key -> { clips:[name], blends:[seconds] } (see below)
    var prog = null;     // the compiled timeline currently playing
    var barCtl = null;   // the shared control bar, once it is built
    var _p = new THREE.Vector3(), _q = new THREE.Quaternion(), _one = new THREE.Vector3(1, 1, 1);
    function renderOnce() { renderer.render(scene, camera); }
    function request() { if (anim) return; if (!pending) { pending = true; requestAnimationFrame(function () { pending = false; renderOnce(); }); } }
    // Backdrop picker + "Save PNG". Optional, so a page that hasn't loaded the
    // script yet still gets a working viewer. A snapshot taken mid-animation
    // catches whatever pose is on screen, which is the point.
    var tools = window.ViewerStage ? window.ViewerStage.attach({
      stage: stage, canvas: renderer.domElement, render: renderOnce, name: title,
      // for the lighting guide: where to hang the rays, and how big to draw them
      scene: scene, focus: function () { return { center: pivot, radius: modelR }; },
    }) : null;
    /* Pose the parts at the moment `AnimClips.frameAt` describes - which clip, which
       fractional frame, and the cross-fade into the next one. The sampling is shared;
       what stays here is where a pose GOES: each part's meshes carry the matrix. */
    function applyFrame(at) {
      data.parts.forEach(function (p) {
        var pose = AC().sample(at, p.name), meshes = meshByPart[p.name];
        if (!pose || !meshes) return;
        _p.set(pose.p[0], pose.p[1], pose.p[2]);
        _q.set(pose.q[0], pose.q[1], pose.q[2], pose.q[3]);
        var sm = scaleOf[p.name] || scaleM;
        for (var i = 0; i < meshes.length; i++) meshes[i].matrix.compose(_p, _q, _one).multiply(sm);
      });
    }
    function loop(ts2) {
      if (!alive || !prog) return;
      raf = requestAnimationFrame(loop);
      applyFrame(AC().frameAt(prog, (ts2 - animStart) / 1000));
      renderOnce();
    }
    function setActive(name) { if (barCtl) barCtl.setActive(name); }
    function startProgram(key) {
      var p = AC().timeline(programs[key], loaded);
      if (!p) return;
      prog = p; anim = key; animStart = performance.now();
      cancelAnimationFrame(raf); raf = requestAnimationFrame(loop);
    }
    function fetchClip(name) {
      return fetch(_apiBase + '/site/rigs/' + encodeURIComponent(data.rig) + '/anim/' +
                   encodeURIComponent(name), { credentials: 'same-origin' })
        .then(function (r) { if (!r.ok) throw new Error(name); return r.arrayBuffer(); })
        .then(function (buf) { loaded[name] = AC().decode(buf); });
    }
    /* `key` names a program: one clip, or the several a move is made of. */
    function play(key) {
      cancelAnimationFrame(raf); anim = null; prog = null; want = key;
      if (!key) { applyPose(data.rest); renderOnce(); setActive(null); return; }
      var spec = programs[key];
      if (!spec) return;
      setActive(key);
      var need = spec.clips.filter(function (n) { return !loaded[n]; });
      if (!need.length) { startProgram(key); return; }
      if (!data.rig) return;                                  // no skeleton -> can't fetch frames
      if (barCtl) barCtl.setLoading(key, true);
      Promise.all(need.map(fetchClip))
        .then(function () {
          if (barCtl) barCtl.setLoading(key, false);
          if (want === key) startProgram(key);
        })
        .catch(function (e) {
          if (barCtl) barCtl.setLoading(key, false);
          if (window.console) console.error('model viewer: animation "' + key + '":', e);
        });
    }

    /* The control bar - Rest plus one button per animation, bucketed by action once
       there are too many to list flat - is `anim_clips.js`, shared with the Blueprint
       Editor so a clip sits under the same heading in both. One button = one PROGRAM:
       the rig's state machine folds the clips that only ever play as part of a move
       (jump_begin, jump_cycle, jump_end) into one, and the rest stay their own. */
    var kit = AC().programs(data.animations || {}, graph);
    programs = kit.programs;
    barCtl = AC().bar({
      host: bar, kit: kit, onPick: play, onResize: onResize,
      hint: 'drag rotate · scroll zoom · right-drag pan',
    });
    // The canvas was sized against a stage that had no control bar under it yet. Re-fit
    // now that the bar occupies its real height, or the canvas overhangs it and eats
    // every click. Done explicitly rather than left to the ResizeObserver above, which
    // only delivers on a rendering tick.
    onResize();
    play(null);

    return {
      // test hook: what the playback loop currently sees
      state: function () {
        var spec = anim ? programs[anim] : null;
        return { anim: anim, want: want, alive: alive, cached: Object.keys(loaded),
                 buttons: Object.keys(programs),
                 program: spec ? { clips: spec.clips.slice(), blends: spec.blends.slice() } : null,
                 timeline: prog ? { starts: prog.starts.slice(), blends: prog.blends.slice(),
                                    total: prog.total } : null,
                 clip: prog ? { fps: prog.A[0].fps, frameCount: prog.A[0].frameCount,
                                apCount: prog.A[0].apCount,
                                dataLen: prog.A[0].data && prog.A[0].data.length } : null };
      },
      /* test hook: pose one specific frame of a loaded clip and report the resulting
         attach-point matrices. Playback itself is rAF-driven, which never runs in a
         non-compositing tab, so tests drive frames through here instead. */
      poseFrame: function (name, fi) {
        var A = loaded[name];
        if (!A || !A.frameCount) return null;
        applyFrame({ A: A, f: ((fi % A.frameCount) + A.frameCount) % A.frameCount,
                     B: null, g: 0, u: 0 });
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
      if (tools) tools.dispose();
      el.removeEventListener('pointerdown', down); el.removeEventListener('pointermove', move); el.removeEventListener('pointerup', upE);
      el.removeEventListener('pointercancel', upE); el.removeEventListener('lostpointercapture', upE);
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
