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

   CONTROLS DEFAULT TO THE VIEWERS'. A plain left-drag orbits unless a host calls
   `setDragMode`, and only the Blueprint Editor does - so the editor's "left-drag works,
   Ctrl-drag turns the view" scheme is its own. Nothing here changes how
   `blueprint_viewer.js` or `model_viewer.js` behave; they carry their own copies of the
   camera and this file is not loaded on their pages. Keep it that way: a viewer where
   dragging suddenly painted would be a bad surprise.

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
    var modelGroup = new THREE.Group();
    scene.add(modelGroup);
    var data = null;
    var pivot = new THREE.Vector3();
    var modelR = 1;

    function disposeMeshes() {
      meshes.forEach(function (m) {
        modelGroup.remove(m);
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
      meshes.forEach(function (m) { modelGroup.add(m); });
      // The box belongs to the geometry, so an edit that changes the model's extent
      // has to redraw it - it is stale the moment the mesh is replaced.
      if (outlineColour != null) setModelOutline(outlineColour);

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
      modelGroup.remove(o);
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
      modelGroup.add(lines);
      overlays[key] = lines;
      request();
    }

    /* ---- layers: further models shown alongside the first -----------------
       For lining models up before flattening them together. Each lives in its own
       Group, so nudging one is a position change rather than a re-mesh - which is
       what makes dragging it around a voxel at a time feel instant on a big model.
       A wireframe box says which is which. */
    var layers = {};

    function clearLayer(id) {
      var L = layers[id];
      if (!L) return;
      scene.remove(L.group);
      L.group.traverse(function (o) {
        if (o.geometry) o.geometry.dispose();
        if (o.material) o.material.dispose();
      });
      delete layers[id];
      request();
    }

    function clearLayers() {
      Object.keys(layers).forEach(clearLayer);
    }

    /* Every layer gets a wireframe box around it, and that box is what makes a model of
       many parts readable: without it a creature is one lump of voxels and there is no
       telling where the leg stops and the foot starts. `opts2.opacity` dims the boxes of
       the parts you are NOT editing - sixteen boxes at full strength is a cage, sixteen
       faint ones with a bright one around the active part reads at a glance.
       `opts2.outline: false` drops it entirely. */
    function setLayer(id, payload, colour, opts2) {
      clearLayer(id);
      if (!payload || !payload.count) { request(); return; }
      opts2 = opts2 || {};
      var group = new THREE.Group();
      var built = window.VoxelMesh.build(THREE, payload, {
        brdfUrl: opts.brdfUrl,
        lightDir: [0.7, 1.0, 0.55],
        onReady: function () { request(); },
      });
      built.forEach(function (m) { group.add(m); });

      if (opts2.outline !== false) {
        var box = new THREE.Box3().setFromObject(group);
        if (!box.isEmpty()) {
          var helper = new THREE.Box3Helper(box, colour || 0x58a6ff);
          if (helper.material) {
            helper.material.depthTest = false;
            helper.material.transparent = true;
            helper.material.opacity = opts2.opacity == null ? 0.9 : opts2.opacity;
          }
          helper.renderOrder = 4;
          group.add(helper);
        }
      }
      scene.add(group);
      layers[id] = { group: group, meshes: built };
      request();
    }

    function moveLayer(id, x, y, z) {
      var L = layers[id];
      if (!L) return;
      L.group.matrixAutoUpdate = true;
      L.group.position.set(x, y, z);
      request();
    }

    /* Place a layer by MATRIX rather than by offset: a part of a creature sits where
       its bone puts it, which is a rotation and a scale as well as a position. `m` is
       16 numbers, column-major (the same layout the baked rigs store). */
    function setLayerMatrix(id, m) {
      var L = layers[id];
      if (!L) return;
      L.group.matrixAutoUpdate = false;
      L.group.matrix.fromArray(m);
      L.group.matrixWorldNeedsUpdate = true;
      request();
    }

    function setModelMatrix(m) {
      if (!m) {
        modelGroup.matrixAutoUpdate = true;
        modelGroup.position.set(0, 0, 0);
        modelGroup.scale.set(1, 1, 1);
        modelGroup.quaternion.identity();
      } else {
        modelGroup.matrixAutoUpdate = false;
        modelGroup.matrix.fromArray(m);
        modelGroup.matrixWorldNeedsUpdate = true;
      }
      request();
    }

    /* A box around the model being edited. With one model on screen it is noise; with a
       creature's worth of parts around it, it is the only thing saying which one a click
       will land on. Pass null to drop it. */
    var modelOutline = null, outlineColour = null;
    function setModelOutline(colour) {
      outlineColour = colour == null ? null : colour;
      if (modelOutline) {
        modelGroup.remove(modelOutline);
        modelOutline.geometry.dispose();
        modelOutline.material.dispose();
        modelOutline = null;
      }
      if (colour == null || !meshes.length) { request(); return; }
      var box = new THREE.Box3();
      meshes.forEach(function (m) { box.expandByObject(m); });
      if (box.isEmpty()) { request(); return; }
      modelOutline = new THREE.Box3Helper(box, colour);
      if (modelOutline.material) {
        modelOutline.material.depthTest = false;
        modelOutline.material.transparent = true;
        modelOutline.material.opacity = 0.75;
      }
      modelOutline.renderOrder = 4;
      modelGroup.add(modelOutline);
      request();
    }

    /* Frame everything on screen rather than the active model alone - opening a whole
       creature should show the creature, not its left foot filling the stage. */
    function frameAll() {
      scene.updateMatrixWorld(true);
      var box = new THREE.Box3();
      meshes.forEach(function (m) { box.expandByObject(m); });
      Object.keys(layers).forEach(function (k) {
        (layers[k].meshes || []).forEach(function (m) { box.expandByObject(m); });
      });
      if (box.isEmpty()) return;
      var size = box.getSize(new THREE.Vector3());
      modelR = Math.max(size.x, size.y, size.z) || 1;
      box.getCenter(pivot);
      sph.radius = modelR * 2.1;
      panX = 0; panY = 0;
      applyCamera();
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
       caller that wants the empty cell in FRONT of the face can just add it.

       Both steps happen in the MODEL's own space, not the world's. A part placed on a
       rig carries a rotation and a voxel size of 1/12, so a quarter of a world unit is
       three voxels in the wrong direction - the offset-only correction this used to do
       silently painted a different voxel than the one under the cursor. `face.normal`
       is already model-space (the meshes carry no transform of their own). */
    var _lp = new THREE.Vector3();
    function pick(e) {
      var hit = castAt(e.clientX, e.clientY);
      if (!hit || !hit.face) return null;
      var n = hit.face.normal;
      modelGroup.updateMatrixWorld();
      _lp.copy(hit.point);
      modelGroup.worldToLocal(_lp);
      return {
        x: Math.round(_lp.x - n.x * 0.25),
        y: Math.round(_lp.y - n.y * 0.25),
        z: Math.round(_lp.z - n.z * 0.25),
        nx: Math.round(n.x), ny: Math.round(n.y), nz: Math.round(n.z),
      };
    }

    /* Which OTHER model is under the cursor - the id passed to setLayer, or null.
       A creature's parts are layers, so this is how double-clicking a leg makes the leg
       the thing being edited. The active model is deliberately not in the answer: it is
       already what a click acts on. */
    function pickLayer(e) {
      var all = [];
      Object.keys(layers).forEach(function (id) {
        (layers[id].meshes || []).forEach(function (m) { all.push(m); });
      });
      if (!all.length) return null;
      var box = el.getBoundingClientRect();
      if (!box.width || !box.height) return null;
      ndc.set(((e.clientX - box.left) / box.width) * 2 - 1,
              -((e.clientY - box.top) / box.height) * 2 + 1);
      camera.updateMatrixWorld();
      scene.updateMatrixWorld();
      caster.setFromCamera(ndc, camera);
      var hit = caster.intersectObjects(all, false)[0];
      if (!hit) return null;
      var found = null;
      Object.keys(layers).forEach(function (id) {
        if ((layers[id].meshes || []).indexOf(hit.object) >= 0) found = id;
      });
      return found;
    }

    var drag = 0, lx = 0, ly = 0, pinch = 0, moved = 0;
    /* What a plain left-drag does: '' orbits (the viewer default), 'move' repositions
       what the host is placing, 'stroke' runs the host's tool along everything the
       cursor passes over. Ctrl (or a right-drag) always orbits, so the two gestures
       never compete for the same button. */
    var dragMode = '';
    var dragAccum = new THREE.Vector3();
    var strokeSeen = null;

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
      // In drag mode a left-drag moves the thing being positioned instead; rotating
      // stays available on right-drag, which would otherwise be pan.
      // Ctrl (or a right-drag) turns the view even while positioning, so the two
      // gestures never fight over the same button.
      if (dragMode && e.button !== 2 && !e.shiftKey && !e.ctrlKey && !e.metaKey) {
        drag = (dragMode === 'stroke') ? 4 : 3;
        lx = e.clientX; ly = e.clientY; moved = 0;
        dragAccum.set(0, 0, 0);
        // A move-drag is one action too - a host that offers undo needs to know where it
        // started and stopped, or dragging a part twenty voxels is twenty undos.
        if (drag === 3 && opts.onStroke) opts.onStroke('start');
        if (drag === 4) {
          // One stroke is one action: the host is told where it starts and ends so a
          // drag across fifty voxels undoes in one go rather than fifty.
          strokeSeen = Object.create(null);
          if (opts.onStroke) opts.onStroke('start');
          strokeAt(e);
        }
        stage.classList.add('vsc-grabbing');
        try { el.setPointerCapture(e.pointerId); } catch (err) { /* already released */ }
        e.preventDefault();
        return;
      }
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
      if (drag === 4) {
        /* Coalesced events, not just the one that woke us: a fast drag delivers a
           single pointermove covering a lot of ground, and sampling only its endpoint
           would leave gaps in the stroke. The list can come back EMPTY (a synthetic
           event, or a browser that doesn't fill it), and an empty array is truthy -
           so fall back on length, not existence, or the stroke silently stops. */
        var pts = (e.getCoalescedEvents && e.getCoalescedEvents()) || [];
        if (!pts.length) pts = [e];
        for (var pi = 0; pi < pts.length; pi++) strokeAt(pts[pi]);
        return;
      }
      if (drag === 3) { dragLayer(dx, dy); return; }
      if (drag === 2) pan(dx, dy); else rotate(dx, dy);
      applyCamera(); request();
    }

    /* Turn a screen drag into whole-voxel movement.
       The drag is measured along the camera's own right and up axes so the model
       follows the cursor whichever way the view is turned, then accumulated in world
       space and handed over only when it crosses a whole voxel - a model that slid
       by fractions would never sit on the grid it has to be saved on. The leftover
       fraction is kept, so a slow drag still moves smoothly rather than sticking. */
    function dragLayer(dx, dy) {
      camera.updateMatrixWorld();
      var s = 2 * sph.radius * Math.tan(camera.fov * Math.PI / 360) / (stage.clientHeight || 1);
      // Signs so the model FOLLOWS the cursor. (Pan uses the opposite, because there
      // it is the camera's target that moves, not the thing being looked at.)
      /* A host whose grid ISN'T the world's - a creature's part, which its bone has
         rotated and scaled to 1/12 - takes the raw world-space movement and does its own
         quantising, because a whole world unit is twelve voxels there and rounding here
         would move it a foot at a time. */
      if (opts.onDragWorld) {
        dragAccum.set(0, 0, 0).addScaledVector(bRight, dx * s).addScaledVector(bUp, -dy * s);
        opts.onDragWorld(dragAccum.x, dragAccum.y, dragAccum.z);
        return;
      }
      dragAccum.addScaledVector(bRight, dx * s).addScaledVector(bUp, -dy * s);
      var step = new THREE.Vector3(
        Math.round(dragAccum.x), Math.round(dragAccum.y), Math.round(dragAccum.z));
      if (step.x || step.y || step.z) {
        dragAccum.sub(step);
        if (opts.onDrag) opts.onDrag(step.x, step.y, step.z);
      }
    }
    /* One sample of a stroke. Each distinct cell+face is reported once - dragging back
       over ground already covered must not re-fire, or an "add" would climb its own
       output and an undo entry would fill with no-ops. */
    function strokeAt(e) {
      var hit = pick(e);
      if (!hit) return;
      var key = hit.x + ',' + hit.y + ',' + hit.z + ':' + hit.nx + ',' + hit.ny + ',' + hit.nz;
      if (strokeSeen[key]) return;
      strokeSeen[key] = 1;
      if (opts.onPick) opts.onPick(hit, e);
    }

    function onUp(e) {
      if (drag === 4) {
        strokeSeen = null;
        if (opts.onStroke) opts.onStroke('end');
      }
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
      frameAll: frameAll,
      pick: pick,
      pickLayer: pickLayer,
      request: request,
      setOverlay: setOverlay,
      setLayer: setLayer,
      moveLayer: moveLayer,
      setLayerMatrix: setLayerMatrix,
      setModelMatrix: setModelMatrix,
      setModelOutline: setModelOutline,
      clearLayer: clearLayer,
      clearLayers: clearLayers,
      setDragMode: function (mode) { dragMode = mode || ''; },
      setModelOffset: function (x, y, z) {
        modelGroup.matrixAutoUpdate = true;
        modelGroup.position.set(x, y, z);
        request();
      },
      clearOverlay: clearOverlay,
      dispose: function () {
        alive = false; cancelAnimationFrame(raf);
        Object.keys(overlays).forEach(clearOverlay);
        setModelOutline(null);
        clearLayers();
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
