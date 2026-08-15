/* Kiwi voxel scene - renderer, camera, orbit/pan/zoom and voxel picking.

   The turntable half of a voxel viewer, with nothing about *what* is being shown.
   `blueprint_viewer.js` and `model_viewer.js` each grew their own copy of this
   (identical down to the comments); the Blueprint Editor needs the same camera plus
   the ability to say which voxel is under the cursor, so it lives here once instead
   of being typed a third time. Those two still carry their own copies and can adopt
   this later - nothing here is viewer-specific.

   What it adds over the two originals is `pick`: a raycast that resolves to a voxel
   COORDINATE rather than a triangle. The mesher merges faces into a handful of
   material groups, so a hit gives no voxel index back - but every face sits exactly
   half a unit out from its voxel's centre along the face normal, so stepping back
   in along that normal and rounding lands on the voxel, and stepping out lands on
   the empty cell in front of it.

     var scene = window.VoxelScene.create({ stage: el, data: payload });
     scene.rebuild(payload);        // after an edit
     var hit = scene.pick(event);   // { x, y, z, nx, ny, nz } | null
     scene.dispose();

   Requires three.js plus voxel_mesh.js, and uses viewer_stage.js when present. */
(function () {
  'use strict';

  var THREE_URL = '/static/vendor/three.min.js';   // self-hosted: no third-party IP leak
  var _threePromise = null;

  function loadThree() {
    if (window.THREE) return Promise.resolve(window.THREE);
    if (_threePromise) return _threePromise;
    _threePromise = new Promise(function (resolve, reject) {
      var s = document.createElement('script');
      s.src = THREE_URL;
      s.onload = function () { window.THREE ? resolve(window.THREE) : reject(new Error('3D library failed to load.')); };
      s.onerror = function () { reject(new Error('3D library failed to load.')); };
      document.head.appendChild(s);
    });
    return _threePromise;
  }

  /* `opts`: { stage, data, name, brdfUrl, onPick, onHover, cursor }.
     Resolves to the scene handle once three.js is in. */
  function create(opts) {
    return loadThree().then(function (THREE) { return build(THREE, opts); });
  }

  function build(THREE, opts) {
    var stage = opts.stage;
    var W = stage.clientWidth || 640, H = stage.clientHeight || 420;

    var renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
    renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
    renderer.setSize(W, H);
    renderer.domElement.setAttribute('role', 'img');
    renderer.domElement.setAttribute('aria-label',
      'Interactive 3D voxel model. Drag to rotate, scroll to zoom.');
    stage.appendChild(renderer.domElement);

    // No scene lights: the voxel materials run Trove's own object shader, which
    // carries its sun and ambient as uniforms (voxel_mesh.js).
    var scene = new THREE.Scene();
    var camera = new THREE.PerspectiveCamera(42, W / H, 0.1, 8000);
    var el = renderer.domElement;

    var meshes = [];
    var data = null;
    var pivot = new THREE.Vector3();
    var modelR = 1;

    function disposeMeshes() {
      meshes.forEach(function (m) {
        scene.remove(m);
        if (m.geometry) m.geometry.dispose();
        if (m.material) m.material.dispose();
      });
      meshes = [];
    }

    /* Rebuild the geometry from a payload. Called on open and after every edit;
       `keepCamera` is the difference between the two - re-framing the model each
       time a voxel changed colour would yank the view out from under the user. */
    function rebuild(next, keepCamera) {
      data = next;
      disposeMeshes();
      meshes = window.VoxelMesh.build(THREE, data, {
        brdfUrl: opts.brdfUrl,
        lightDir: [0.7, 1.0, 0.55],
        onReady: function () { request(); },
      });
      meshes.forEach(function (m) { scene.add(m); });

      if (!keepCamera) {
        /* Frame the VOXELS, not the size the payload declares. A voxel sits on its
           integer coordinate and reaches half a unit either side, so a model N wide
           occupies [-0.5, N-0.5] and its middle is (N-1)/2 - half a voxel off from
           N/2, enough to visibly swing a small model as it rotates. */
        var bounds = new THREE.Box3();
        meshes.forEach(function (m) { bounds.expandByObject(m); });
        if (bounds.isEmpty() && data.size) {
          bounds.set(new THREE.Vector3(0, 0, 0),
                     new THREE.Vector3(data.size[0], data.size[1], data.size[2]));
        }
        var bSize = bounds.getSize(new THREE.Vector3());
        modelR = Math.max(bSize.x, bSize.y, bSize.z) || 1;
        bounds.getCenter(pivot);
        sph.radius = modelR * 2.1;
      }
      request();
    }

    /* The model spins around ITSELF, wherever it has been dragged to. The pivot is
       the model's own centre and stays there; the pan is kept as an offset in CAMERA
       space and rebuilt from the current angles every frame, so the model holds its
       place on screen while it orbits instead of swinging around the stage. */
    var target = new THREE.Vector3();
    var panX = 0, panY = 0;
    var sph = { radius: 2.1, theta: Math.PI * 0.27, phi: Math.PI * 0.36 };
    var dir = new THREE.Vector3(), bRight = new THREE.Vector3(), bUp = new THREE.Vector3();
    var WORLD_UP = new THREE.Vector3(0, 1, 0);

    function applyCamera() {
      var t = sph.theta, p = sph.phi;
      dir.set(Math.sin(p) * Math.sin(t), Math.cos(p), Math.sin(p) * Math.cos(t));
      bRight.crossVectors(WORLD_UP, dir).normalize();   // phi is clamped off the poles
      bUp.crossVectors(dir, bRight);
      target.copy(pivot).addScaledVector(bRight, panX).addScaledVector(bUp, panY);
      camera.position.copy(target).addScaledVector(dir, sph.radius);
      camera.lookAt(target);
    }

    /* ---- overlays: wireframe boxes drawn over the model --------------------
       Used for the attachment point (which is usually NOT a voxel - on a hat it
       floats below the model) and to point at voxels a check complained about.
       One merged LineSegments per overlay rather than a mesh each: a finding can
       cover thousands of voxels, and that has to stay one draw call. */
    var overlays = {};
    var CUBE_EDGES = [
      [0, 0, 0, 1, 0, 0], [1, 0, 0, 1, 1, 0], [1, 1, 0, 0, 1, 0], [0, 1, 0, 0, 0, 0],
      [0, 0, 1, 1, 0, 1], [1, 0, 1, 1, 1, 1], [1, 1, 1, 0, 1, 1], [0, 1, 1, 0, 0, 1],
      [0, 0, 0, 0, 0, 1], [1, 0, 0, 1, 0, 1], [1, 1, 0, 1, 1, 1], [0, 1, 0, 0, 1, 1],
    ];

    function clearOverlay(key) {
      var o = overlays[key];
      if (!o) return;
      scene.remove(o);
      o.geometry.dispose();
      o.material.dispose();
      delete overlays[key];
    }

    /* `points` is [[x,y,z], ...]; `scale` grows the box past the voxel so the
       wireframe reads as a highlight around it rather than z-fighting its faces. */
    function setOverlay(key, points, colour, scale) {
      clearOverlay(key);
      if (!points || !points.length) { request(); return; }
      scale = scale || 1.06;
      var half = scale / 2;
      var pos = new Float32Array(points.length * CUBE_EDGES.length * 6);
      var p = 0;
      for (var i = 0; i < points.length; i++) {
        var cx = points[i][0], cy = points[i][1], cz = points[i][2];
        for (var e = 0; e < CUBE_EDGES.length; e++) {
          var E = CUBE_EDGES[e];
          pos[p++] = cx + (E[0] ? half : -half);
          pos[p++] = cy + (E[1] ? half : -half);
          pos[p++] = cz + (E[2] ? half : -half);
          pos[p++] = cx + (E[3] ? half : -half);
          pos[p++] = cy + (E[4] ? half : -half);
          pos[p++] = cz + (E[5] ? half : -half);
        }
      }
      var geo = new THREE.BufferGeometry();
      geo.setAttribute('position', new THREE.BufferAttribute(pos, 3));
      var mat = new THREE.LineBasicMaterial({
        color: colour, transparent: true, opacity: 0.95, depthTest: false,
      });
      var lines = new THREE.LineSegments(geo, mat);
      lines.renderOrder = 5;          // always on top: a highlight you can't see is useless
      scene.add(lines);
      overlays[key] = lines;
      request();
    }

    var caster = new THREE.Raycaster(), ndc = new THREE.Vector2();
    var viewDir = new THREE.Vector3(), scratch = new THREE.Vector3();
    var anchor = null;

    function castAt(clientX, clientY) {
      var box = el.getBoundingClientRect();
      if (!box.width || !box.height) return null;
      ndc.set(((clientX - box.left) / box.width) * 2 - 1,
              -((clientY - box.top) / box.height) * 2 + 1);
      camera.updateMatrixWorld();
      scene.updateMatrixWorld();
      caster.setFromCamera(ndc, camera);
      return caster.intersectObjects(meshes, false)[0] || null;
    }

    /* Which voxel is under the cursor. The hit point lies ON a face, exactly half a
       unit from the voxel centre along the face normal, so nudging back along -n by
       a quarter unit and rounding is unambiguous (a quarter avoids landing on the
       .5 boundary where rounding could go either way). `n` is handed back too, so a
       caller that wants the empty cell in FRONT of the face can just add it. */
    function pick(e) {
      var hit = castAt(e.clientX, e.clientY);
      if (!hit || !hit.face) return null;
      var n = hit.face.normal;
      return {
        x: Math.round(hit.point.x - n.x * 0.25),
        y: Math.round(hit.point.y - n.y * 0.25),
        z: Math.round(hit.point.z - n.z * 0.25),
        nx: Math.round(n.x), ny: Math.round(n.y), nz: Math.round(n.z),
      };
    }

    var drag = 0, lx = 0, ly = 0, pinch = 0, moved = 0;

    function rotate(dx, dy) {
      sph.theta -= dx * 0.01;
      sph.phi = Math.max(0.04, Math.min(Math.PI - 0.04, sph.phi - dy * 0.01));
    }
    function panDepth() {
      if (!anchor) return sph.radius;
      camera.getWorldDirection(viewDir);
      return Math.max(1e-6, scratch.copy(anchor).sub(camera.position).dot(viewDir));
    }
    function pan(dx, dy) {
      camera.updateMatrixWorld();
      var s = 2 * panDepth() * Math.tan(camera.fov * Math.PI / 360) / (stage.clientHeight || 1);
      panX -= dx * s;
      panY += dy * s;
    }
    function zoom(f) {
      sph.radius = Math.max(modelR * 0.2, Math.min(modelR * 9, sph.radius * f));
    }

    /* Pointer events WITH CAPTURE: letting go outside the frame otherwise delivers
       the pointerup somewhere this window never sees, and the drag never ends. */
    function onDown(e) {
      if (e.pointerType === 'touch') return;
      // A plain left click is the edit gesture, so it must not also start a rotate
      // until the pointer actually travels - `moved` decides that on pointerup.
      drag = (e.button === 2 || e.shiftKey) ? 2 : 1;
      lx = e.clientX; ly = e.clientY; moved = 0;
      if (drag === 2) {
        var h = castAt(e.clientX, e.clientY);
        anchor = h && h.point ? h.point.clone() : null;
      }
      stage.classList.add('vsc-grabbing');
      try { el.setPointerCapture(e.pointerId); } catch (err) { /* already released */ }
      e.preventDefault();
    }
    function onMove(e) {
      if (e.pointerType === 'touch') return;
      if (!drag) {
        if (opts.onHover) opts.onHover(pick(e), e);
        return;
      }
      var dx = e.clientX - lx, dy = e.clientY - ly;
      lx = e.clientX; ly = e.clientY;
      moved += Math.abs(dx) + Math.abs(dy);
      if (drag === 2) pan(dx, dy); else rotate(dx, dy);
      applyCamera(); request();
    }
    function onUp(e) {
      // Under the drag threshold this was a click, not a rotation - report the voxel.
      if (drag === 1 && moved < 4 && opts.onPick && e && e.clientX !== undefined) {
        opts.onPick(pick(e), e);
      }
      drag = 0; anchor = null; moved = 0;
      stage.classList.remove('vsc-grabbing');
    }
    /* Leaving the canvas ends the hover. Without this the last move wins and a host
       showing a readout keeps displaying whatever was under the cursor on the way out. */
    function onLeave() {
      if (!drag && opts.onHover) opts.onHover(null);
    }
    function onWheel(e) { e.preventDefault(); zoom(e.deltaY > 0 ? 1.12 : 0.89); applyCamera(); request(); }
    function dist(e) {
      var a = e.touches[0], b = e.touches[1];
      return Math.hypot(a.clientX - b.clientX, a.clientY - b.clientY);
    }
    function onTStart(e) {
      if (e.touches.length === 1) { drag = 1; lx = e.touches[0].clientX; ly = e.touches[0].clientY; moved = 0; }
      else if (e.touches.length === 2) { drag = 0; pinch = dist(e); }
    }
    function onTMove(e) {
      if (e.touches.length === 1 && drag === 1) {
        var t = e.touches[0];
        moved += Math.abs(t.clientX - lx) + Math.abs(t.clientY - ly);
        rotate(t.clientX - lx, t.clientY - ly);
        lx = t.clientX; ly = t.clientY; applyCamera(); request();
      } else if (e.touches.length === 2) {
        var d = dist(e);
        if (pinch) { zoom(pinch / d); applyCamera(); request(); }
        pinch = d;
      }
      e.preventDefault();
    }
    function onTEnd(e) {
      // A tap (no travel) edits, the same as a click.
      if (drag === 1 && moved < 6 && opts.onPick && e.changedTouches && e.changedTouches[0]) {
        opts.onPick(pick(e.changedTouches[0]), e);
      }
      drag = 0; pinch = 0; moved = 0;
    }
    function onResize() {
      var w = stage.clientWidth, h = stage.clientHeight;
      if (!w || !h) return;
      camera.aspect = w / h; camera.updateProjectionMatrix();
      renderer.setSize(w, h); request();
    }

    el.addEventListener('pointerdown', onDown);
    el.addEventListener('pointermove', onMove);
    el.addEventListener('pointerup', onUp);
    el.addEventListener('pointerleave', onLeave);
    el.addEventListener('pointercancel', onUp);
    el.addEventListener('lostpointercapture', onUp);
    el.addEventListener('wheel', onWheel, { passive: false });
    el.addEventListener('contextmenu', function (e) { e.preventDefault(); });
    el.addEventListener('touchstart', onTStart, { passive: false });
    el.addEventListener('touchmove', onTMove, { passive: false });
    el.addEventListener('touchend', onTEnd);
    window.addEventListener('resize', onResize);

    // Render-on-demand: the scene is static between edits, so draw once and redraw
    // only on interaction rather than spinning a rAF loop forever.
    var raf = 0, alive = true, pending = false;
    function renderOnce() { if (alive) renderer.render(scene, camera); }
    function request() {
      if (pending) return;
      pending = true;
      raf = requestAnimationFrame(function () { pending = false; renderOnce(); });
    }

    if (opts.data) rebuild(opts.data, false);
    applyCamera();
    request();

    var tools = window.ViewerStage ? window.ViewerStage.attach({
      stage: stage, canvas: el, render: renderOnce, name: opts.name || 'Model',
      // for the lighting guide: where to hang the rays, and how big to draw them
      scene: scene, focus: function () { return { center: pivot, radius: modelR }; },
    }) : null;

    return {
      rebuild: function (next) { rebuild(next, true); },
      reframe: function () { rebuild(data, false); applyCamera(); request(); },
      pick: pick,
      request: request,
      setOverlay: setOverlay,
      clearOverlay: clearOverlay,
      dispose: function () {
        alive = false; cancelAnimationFrame(raf);
        Object.keys(overlays).forEach(clearOverlay);
        if (tools) tools.dispose();
        el.removeEventListener('pointerdown', onDown);
        el.removeEventListener('pointermove', onMove);
        el.removeEventListener('pointerup', onUp);
        el.removeEventListener('pointerleave', onLeave);
        el.removeEventListener('pointercancel', onUp);
        el.removeEventListener('lostpointercapture', onUp);
        el.removeEventListener('wheel', onWheel);
        el.removeEventListener('touchstart', onTStart);
        el.removeEventListener('touchmove', onTMove);
        el.removeEventListener('touchend', onTEnd);
        window.removeEventListener('resize', onResize);
        disposeMeshes();
        renderer.dispose();
        if (el.parentNode) el.parentNode.removeChild(el);
      },
    };
  }

  window.VoxelScene = { create: create };
})();
