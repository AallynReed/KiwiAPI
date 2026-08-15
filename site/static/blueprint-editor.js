/* Blueprint Editor - recolour and re-material a Trove .blueprint in the browser.

   The file never leaves the visitor's control for longer than a request: it is posted
   to be decoded, held in this tab while it is edited, and posted back with the edit
   list to be written. Nothing is stored server-side, so the original bytes have to
   stay here - `state.file` is the same File object the visitor picked.

   The model is held as the inspect payload's parallel arrays and edited in place. Two
   extra arrays make that work: `live`, so an erase can drop a voxel without renumbering
   every row after it, and `origin`, mapping each row back to its index in the file (-1
   for one the user placed). The save payload is then DERIVED by diffing against the
   pristine copy rather than journalled as it goes - so a voxel painted back to how it
   started stops counting as an edit, with nothing to keep in sync.

   Voxels the server marked `edit: 0` are Trove's own - deco placeholders, terrain,
   anything the material palette can't safely reinterpret. They are rendered, they can
   be inspected, and they are refused as material targets rather than being converted
   into plain blocks behind the user's back. Erasing one is allowed: deleting a voxel
   is not the same as claiming to know what it was. */
(function () {
  'use strict';

  var U = window.BTTUtil;
  var esc = U.esc;
  var apiUrl = U.apiUrl;

  var state = {
    file: null,          // the File the visitor opened; posted back on save
    data: null,          // inspect payload (arrays are the render source of truth)
    base: null,          // pristine rgb/type/w, straight from the file
    origin: [],          // row -> index in the file, or -1 if the user placed it
    history: [],         // one array of before-states per action, for undo
    scene: null,
    selection: -1,
    index: null,         // "x,y,z" -> row, live rows only
    paint: { rgb: 0xff6b6b, type: 21, w: 0 },
    tool: 'paint',       // paint | add | erase
    mode: 'both',        // what a paint click applies: colour, material, or both
    scope: 'voxel',      // one voxel, or every voxel sharing its material
    kind: 'other',       // creation type the checks are run against
    docs: [],            // the stack, bottom to top; each is its own blueprint
    active: 0,           // which one the tools edit
    anchor: 0,           // which one everything else is positioned against
    docSeq: 1,
    project: null,       // a whole model open at once: the archive + its rig
    ask: -1,             // a part just added that nobody has placed yet
    drawn: {},           // doc id -> its geometry is currently in the scene as a layer
    isolate: false,      // show only the layer being edited
    stroke: null,        // in-progress drag-edit: one batch, one undo entry
    strokeNormal: null,  // the face an 'add' stroke started on
    report: null,        // last check result
    showAttach: true,
  };

  var ATTACH_COLOUR = 0xff3fd5;   // the pink Trove modders mark attachment points in
  var FINDING_COLOUR = { error: 0xff5555, warning: 0xffc857, info: 0x58a6ff };
  // A model's parts each get a box, the one being edited gets a brighter one - which is
  // the only thing telling you where the leg ends and the foot begins.
  var PART_BOX = 0x8b95a5, ACTIVE_BOX = 0x58a6ff, PART_BOX_OPACITY = 0.3;

  function $(id) { return document.getElementById(id); }

  function setStatus(msg, kind) {
    var el = $('bpe-status');
    if (!el) return;
    el.textContent = msg || '';
    el.className = 'bpe-status' + (kind ? ' bpe-' + kind : '');
    el.hidden = !msg;
  }

  function hex(rgb) {
    return '#' + ('000000' + (rgb >>> 0).toString(16)).slice(-6);
  }
  function unhex(s) {
    return parseInt(String(s || '').replace('#', ''), 16) & 0xFFFFFF;
  }

  // ---- opening ------------------------------------------------------------ //

  /* `note` survives the open: a .qb import has something to say about the conversion,
     and loading the result would otherwise clear the status line out from under it. */
  function openFile(file, note) {
    if (!file) return;
    if (!/\.blueprint$/i.test(file.name)) {
      setStatus('That isn’t a .blueprint file.', 'error');
      return;
    }
    setStatus('Opening ' + file.name + '…');
    var fd = new FormData();
    fd.append('file', file, file.name);
    fetch(apiUrl('/site/blueprint-editor/inspect'), { method: 'POST', body: fd })
      .then(function (res) {
        return res.json().then(function (body) {
          if (!res.ok) throw new Error((body && body.detail) || 'That blueprint couldn’t be opened.');
          return body;
        });
      })
      .then(function (payload) { loaded(file, payload, note); })
      .catch(function (err) { setStatus(err.message || String(err), 'error'); });
  }

  /* Opening a file starts a FRESH stack with it at the bottom. Everything a document
     needs in order to be edited independently - its pristine copy, its live flags, its
     own undo stack - is built by makeDoc and swapped in by setActive. */
  function loaded(file, payload, note) {
    state.project = null;
    state.docs = [makeDoc(file, payload)];
    state.active = 0;
    state.anchor = 0;
    mount(note);
  }

  /* Everything that has to happen once a fresh set of documents is in place, however
     they got there - one opened file, or a whole model's worth of parts. */
  function mount(note) {
    state.isolate = false;
    state.report = null;
    state.drawn = {};            // a fresh scene holds none of the old geometry
    $('bpe-report').hidden = true;
    $('bpe-empty').hidden = true;
    $('bpe-workspace').hidden = false;
    document.body.classList.toggle('bpe-project-mode', !!state.project);

    var d0 = state.docs[state.active];
    state.file = d0.file;
    state.data = d0.payload;
    state.base = d0.base;
    state.origin = d0.origin;
    state.history = d0.history;
    state.selection = -1;
    rebuildIndex();

    renderMode();
    renderKinds();
    renderTransforms();
    renderToolHint();
    renderPalette();
    renderMaterialList();
    renderMeta();
    renderSelection();
    renderLayers();
    updateDirty();
    setStatus(note || '', note ? 'ok' : '');

    if (state.scene) { state.scene.dispose(); state.scene = null; }
    window.VoxelScene.create({
      stage: $('bpe-stage'),
      data: liveView(),
      name: d0.payload.name,
      /* The specular atlas. Without it every solid shades as rough, so metal, water,
         iridescent, waxy and wave all look identical and the Finish control appears
         to do nothing. Loaded through an <img>, which the global fetch rewriter never
         sees, so it asks apiUrl itself. */
      brdfUrl: apiUrl('/site/render/brdf-map.png'),
      onPick: onPick,
      onHover: onHover,
      onDrag: function (dx, dy, dz) { nudgeActive(dx, dy, dz); },
      onStroke: onStroke,
    }).then(function (scene) {
      state.scene = scene;
      scene.setDragMode(dragModeFor(state.tool));
      drawStack();
      drawAttachment();
      // A creature opens on the creature, not on whichever part happens to be first.
      if (state.project) scene.frameAll();
    }).catch(function (err) {
      setStatus(err.message || 'The 3D view could not start.', 'error');
    });
  }

  // ---- the live model ----------------------------------------------------- //

  /* The mesher wants dense arrays, and erased voxels are still sitting in ours, so
     compact them out on the way through. One O(n) pass per rebuild, which the re-mesh
     costs anyway. */
  function liveView() {
    var d = state.data;
    var v = { count: 0, size: d.size, x: [], y: [], z: [], rgb: [],
              kind: [], level: [], spec: [] };
    for (var i = 0; i < d.count; i++) {
      if (!d.live[i]) continue;
      v.x.push(d.x[i]); v.y.push(d.y[i]); v.z.push(d.z[i]); v.rgb.push(d.rgb[i]);
      v.kind.push(d.kind[i]); v.level.push(d.level[i]); v.spec.push(d.spec[i]);
      v.count++;
    }
    return v;
  }

  function rebuildIndex() {
    var d = state.data;
    state.index = new Map();
    for (var i = 0; i < d.count; i++) {
      if (d.live[i]) state.index.set(d.x[i] + ',' + d.y[i] + ',' + d.z[i], i);
    }
  }

  function liveCount() {
    var d = state.data, n = 0;
    for (var i = 0; i < d.count; i++) if (d.live[i]) n++;
    return n;
  }

  /* The save payload, derived rather than journalled: every row is compared with the
     file it came from. An original voxel that changed emits an edit, one that was
     erased emits a delete, and a row the user placed emits an add. Nothing to keep in
     sync, and painting a voxel back to its starting colour simply stops appearing. */
  function editList() { return editListOf(state.docs[state.active]); }

  function editListOf(docu) {
    var d = docu.payload, b = docu.base, out = [];
    for (var i = 0; i < d.count; i++) {
      var orig = docu.origin[i];
      if (orig < 0) {
        if (d.live[i]) {
          out.push({ add: [d.x[i], d.y[i], d.z[i]],
                     type: d.type[i], w: d.w[i], rgb: d.rgb[i] });
        }
        continue;
      }
      if (!d.live[i]) { out.push({ i: orig, del: true }); continue; }
      var e = null;
      if (d.type[i] !== b.type[orig] || d.w[i] !== b.w[orig]) {
        e = { i: orig, type: d.type[i], w: d.w[i] };
      }
      if (d.rgb[i] !== b.rgb[orig]) {
        e = e || { i: orig };
        e.rgb = d.rgb[i];
      }
      if (e) out.push(e);
    }
    return out;
  }

  // ---- attachment point + check highlights -------------------------------- //

  function drawAttachment() {
    if (!state.scene) return;
    var a = state.data && state.data.attachment;
    state.scene.setOverlay('attach',
      (a && state.showAttach) ? [a] : [], ATTACH_COLOUR, 1.25);
  }

  function highlightFinding(finding) {
    if (!state.scene || !state.data) return;
    var d = state.data;
    var pts = (finding && finding.voxels || []).map(function (i) {
      return [d.x[i], d.y[i], d.z[i]];
    });
    state.scene.setOverlay('lint', pts, FINDING_COLOUR[finding && finding.level] || 0xff5555, 1.1);
  }

  function runCheck() {
    if (!state.file) return;
    var list = editList();
    var btn = $('bpe-check');
    btn.disabled = true;
    setStatus('Checking…');
    var fd = new FormData();
    anchorParts(fd);
    fd.append('kind', state.kind);
    stackParts(fd);
    fetch(apiUrl('/site/blueprint-editor/check'), { method: 'POST', body: fd })
      .then(function (res) {
        return res.json().then(function (b) {
          if (!res.ok) throw new Error((b && b.detail) || 'The check couldn’t run.');
          return b;
        });
      })
      .then(function (report) {
        state.report = report;
        renderReport();
        setStatus('');
      })
      .catch(function (err) { setStatus(err.message || String(err), 'error'); })
      .finally(function () { btn.disabled = false; });
  }

  /* An edit invalidates the last check. Rather than silently leaving a stale verdict
     on screen - the one thing a checker must never do - drop it and say why. */
  function staleReport() {
    if (!state.report) return;
    state.report = null;
    if (state.scene) state.scene.clearOverlay('lint');
    var box = $('bpe-report');
    box.hidden = false;
    box.innerHTML = '<p class="bpe-hint">You’ve changed the model since the last check. '
      + 'Run it again to see where it stands.</p>';
  }

  function renderReport() {
    var box = $('bpe-report');
    var r = state.report;
    if (!r) { box.innerHTML = ''; box.hidden = true; return; }
    box.hidden = false;
    var c = r.counts;
    var head = c.error
      ? '<strong class="bpe-r-error">' + c.error + ' to fix</strong>'
      : '<strong class="bpe-r-ok"><i class="fa-solid fa-check" aria-hidden="true"></i> Nothing blocking</strong>';
    if (c.warning) head += '<span>' + c.warning + ' to look at</span>';

    var items = r.findings.map(function (f, i) {
      return '<li class="bpe-finding bpe-f-' + f.level + '">'
        + '<div class="bpe-f-head"><span class="bpe-f-dot"></span><strong>' + esc(f.title) + '</strong>'
        + (f.voxels.length
            ? '<button type="button" class="bpe-f-show" data-finding="' + i + '">Show '
              + f.voxels.length.toLocaleString() + '</button>'
            : '')
        + '</div><p>' + esc(f.body) + '</p></li>';
    }).join('');

    box.innerHTML = '<div class="bpe-r-head">' + head + '</div>'
      + (items ? '<ul class="bpe-findings">' + items + '</ul>' : '')
      + '<ul class="bpe-satisfied">'
      + r.satisfied.map(function (s) {
          return '<li><i class="fa-solid fa-check" aria-hidden="true"></i> ' + esc(s) + '</li>';
        }).join('')
      + '</ul>'
      + '<p class="bpe-hint">These are the Trove Creations submission guidelines. '
      + 'Plenty of official game items bend them — they describe what gets accepted, '
      + 'not what the game can load.</p>';
  }

  // ---- editing ------------------------------------------------------------ //

  /* Recompute the render arrays for one voxel from its current (type, w, rgb).
     This mirrors `material_for` on the server: the viewer needs to know a voxel is
     glass at 50% before the file is ever saved, or the preview would lie. */
  function reshade(i) {
    var d = state.data;
    var t = d.type[i], w = d.w[i];
    var glass = (t === 18 || t === 54 || t === 56);
    var glow = (t === 55 || t === 56);
    d.kind[i] = glow ? (glass ? 3 : 2) : (glass ? 1 : 0);
    d.level[i] = glass ? (16 + 32 * Math.max(0, Math.min(w, 7))) : 255;
    d.spec[i] = (d.kind[i] === 0) ? Math.max(0, Math.min(w, 7)) : 0;
  }

  /* Apply a change to one voxel, recording what it was so undo can put it back.
     Returns false when nothing could be applied.

     Two independent permissions, matching the server exactly: `edit` governs the
     MATERIAL (the palette must not reinterpret a voxel whose meaning we don't know)
     and `paint` governs the COLOUR (a procedural voxel is tinted by the game, so a
     colour written there would change the file and nothing in game). A voxel can
     allow one and refuse the other, so a click that does both applies the half it
     is allowed to. */
  function applyTo(i, change, batch) {
    var d = state.data;
    var wantsColour = change.rgb !== undefined;
    var wantsMaterial = change.type !== undefined;
    var canColour = wantsColour && d.paint[i];
    var canMaterial = wantsMaterial && d.edit[i];
    if (!canColour && !canMaterial) return false;

    batch.push({ index: i, rgb: d.rgb[i], type: d.type[i], w: d.w[i], live: d.live[i] });
    if (canColour) d.rgb[i] = change.rgb;
    if (canMaterial) { d.type[i] = change.type; d.w[i] = change.w; }
    reshade(i);
    return true;
  }

  /* Erase: the row stays put and stops being live, which keeps every other index
     stable. Nothing is refused here - a placeholder or a terrain voxel may be deleted
     outright; what the editor won't do is reinterpret one as something else. */
  function eraseVoxel(i, batch) {
    var d = state.data;
    if (!d.live[i]) return false;
    batch.push({ index: i, rgb: d.rgb[i], type: d.type[i], w: d.w[i], live: 1 });
    d.live[i] = 0;
    return true;
  }

  /* Add: a new row appended with origin -1, or an erased row at that cell brought back
     to life. Re-using the row matters - two live rows on one cell would draw twice and
     save twice. */
  function addVoxel(x, y, z, batch) {
    var d = state.data;
    for (var i = 0; i < d.count; i++) {
      if (d.x[i] === x && d.y[i] === y && d.z[i] === z) {
        batch.push({ index: i, rgb: d.rgb[i], type: d.type[i], w: d.w[i], live: d.live[i] });
        d.live[i] = 1;
        d.rgb[i] = state.paint.rgb;
        d.type[i] = state.paint.type;
        d.w[i] = state.paint.w;
        reshade(i);
        return true;
      }
    }
    var n = d.count;
    d.x.push(x); d.y.push(y); d.z.push(z);
    d.rgb.push(state.paint.rgb);
    d.type.push(state.paint.type); d.w.push(state.paint.w);
    d.kind.push(0); d.level.push(255); d.spec.push(0);
    d.edit.push(1); d.paint.push(1);      // a voxel the user placed is always theirs
    d.live.push(1);
    state.origin.push(-1);
    d.count = n + 1;
    reshade(n);
    batch.push({ index: n, added: true });
    return true;
  }

  function commit(batch, refused) {
    if (batch.length) {
      rebuildIndex();
      if (state.scene) state.scene.rebuild(liveView());
      if (state.stroke) {
        // Mid-drag: fold into the stroke's single undo entry and skip the panel work
        // until it ends - re-rendering the material list per voxel would crawl.
        state.stroke.push.apply(state.stroke, batch);
      } else {
        state.history.push(batch);
        renderMaterialList();
        renderMeta();
        renderLayers();
        updateDirty();
        staleReport();
      }
    }
    if (refused) {
      setStatus(refused + (refused === 1 ? ' voxel is' : ' voxels are')
        + ' controlled by the game — a deco placeholder, terrain that’s coloured'
        + ' in-game, or a material outside the modding palette — so they were left'
        + ' as they are.', 'warn');
    } else if (batch.length) {
      setStatus('');
    }
  }

  function change() {
    var c = {};
    if (state.mode === 'colour' || state.mode === 'both') c.rgb = state.paint.rgb;
    if (state.mode === 'material' || state.mode === 'both') {
      c.type = state.paint.type; c.w = state.paint.w;
    }
    return c;
  }

  /* Which rows a click acts on.

     `material` matches (type, w) - which on a typical model is nearly every voxel,
     because most of a model is plain solid rough. That was the only bulk option, and
     it made "everything like it" mean "almost everything". `colour` matches the exact
     stored RGB, which is what you want when the intent is "recolour all the grey". */
  function targetsFor(i) {
    var d = state.data;
    if (state.scope === 'voxel') return [i];
    var out = [];
    if (state.scope === 'colour') {
      var rgb = d.rgb[i];
      for (var j = 0; j < d.count; j++) if (d.live[j] && d.rgb[j] === rgb) out.push(j);
      return out;
    }
    var t = d.type[i], w = d.w[i];
    for (var k = 0; k < d.count; k++) {
      if (d.live[k] && d.type[k] === t && d.w[k] === w) out.push(k);
    }
    return out;
  }

  function onPick(hit, ev) {
    if (!hit || !state.data) return;
    var i = state.index.get(hit.x + ',' + hit.y + ',' + hit.z);
    if (i === undefined) return;
    var batch = [], refused = 0;

    // Eyedropper: take the colour AND the material, so the next paint reproduces the
    // voxel you sampled rather than half of it. Alt-click does it from any tool, which
    // is the gesture every other editor uses.
    if (state.tool === 'pick' || (ev && ev.altKey)) {
      state.paint.rgb = state.data.rgb[i];
      if (state.data.edit[i]) {
        state.paint.type = state.data.type[i];
        state.paint.w = state.data.w[i];
      }
      state.selection = i;
      syncPaintUI();
      renderSelection();
      setStatus('Picked up ' + hex(state.paint.rgb).toUpperCase()
        + (state.data.edit[i] ? ' · ' + label(i) : '')
        + (state.data.edit[i] ? '' : ' (its material is one the game manages, so only the colour was taken)'),
        'ok');
      return;
    }

    if (state.tool === 'add') {
      /* The face normal says which side was clicked, so the new voxel goes on the
         outside of it - the gesture every voxel editor uses.

         Across a DRAG the normal is locked to the face the stroke began on. Without
         that, each voxel added becomes new geometry for the next raycast to hit, and
         the stroke climbs its own output into a wall coming at the camera instead of
         running along the surface you were tracing. */
      if (state.stroke) {
        if (!state.strokeNormal) state.strokeNormal = [hit.nx, hit.ny, hit.nz];
        var n = state.strokeNormal;
        if (n[0] !== hit.nx || n[1] !== hit.ny || n[2] !== hit.nz) return;
      }
      addVoxel(hit.x + hit.nx, hit.y + hit.ny, hit.z + hit.nz, batch);
      commit(batch, 0);
      state.selection = state.index.get(
        (hit.x + hit.nx) + ',' + (hit.y + hit.ny) + ',' + (hit.z + hit.nz));
      if (state.selection === undefined) state.selection = -1;
      renderSelection();
      return;
    }

    var targets = targetsFor(i);

    if (state.tool === 'erase') {
      targets.forEach(function (j) { eraseVoxel(j, batch); });
      commit(batch, 0);
      state.selection = -1;
      renderSelection();
      return;
    }

    state.selection = i;
    renderSelection();

    var c = change();
    if (!Object.keys(c).length) return;

    targets.forEach(function (j) { if (!applyTo(j, c, batch)) refused++; });
    commit(batch, refused);
    renderSelection();
  }

  /* The readout is hidden outright when the cursor isn't over a voxel, rather than
     blanked - an empty chip floating on the model reads as a rendering glitch. */
  function onHover(hit) {
    var el = $('bpe-hover');
    if (!el) return;
    var i = (hit && state.data)
      ? state.index.get(hit.x + ',' + hit.y + ',' + hit.z)
      : undefined;
    if (i === undefined) {
      if (!el.hidden) { el.hidden = true; el.textContent = ''; }
      return;
    }
    var text = hit.x + ', ' + hit.y + ', ' + hit.z + ' · ' + label(i);
    if (el.textContent !== text) el.textContent = text;
    el.hidden = false;
  }

  function undo() {
    var batch = state.history.pop();
    if (!batch) return;
    var d = state.data;
    /* BACKWARDS. A stroke can touch the same voxel more than once - a drag across a
       corner crosses two of its faces, and an add can place a cell and then paint it -
       so a batch may hold several records for one row, each holding the state at the
       moment it was touched. Replaying forwards leaves the LAST one applied, which is a
       mid-stroke state, not the original: the undo appears to do nothing and the model
       stays dirty with no history left to fix it. Going backwards ends on the earliest
       record, which is the one that was true before the stroke began. */
    for (var k = batch.length - 1; k >= 0; k--) {
      var rec = batch[k];
      var i = rec.index;
      if (rec.added) {
        // A row the user placed is retired rather than spliced out: removing it would
        // renumber every row after it, and a dead row costs nothing.
        d.live[i] = 0;
        continue;
      }
      d.rgb[i] = rec.rgb; d.type[i] = rec.type; d.w[i] = rec.w; d.live[i] = rec.live;
      reshade(i);
    }
    rebuildIndex();
    if (state.scene) state.scene.rebuild(liveView());
    renderMaterialList();
    renderMeta();
    renderSelection();
    updateDirty();
    setStatus('');
  }

  function revertAll() {
    var d = state.data, b = state.base;
    for (var i = 0; i < d.count; i++) {
      var orig = state.origin[i];
      if (orig < 0) { d.live[i] = 0; continue; }     // drop everything user-placed
      d.rgb[i] = b.rgb[orig]; d.type[i] = b.type[orig]; d.w[i] = b.w[orig];
      d.live[i] = 1;
      reshade(i);
    }
    state.history = [];
    rebuildIndex();
    if (state.scene) state.scene.rebuild(liveView());
    renderMaterialList();
    renderMeta();
    renderSelection();
    updateDirty();
    setStatus('');
  }

  // ---- saving ------------------------------------------------------------- //

  function save() {
    if (!state.file) return;
    var list = editList();
    setStatus('Saving…');
    var fd = new FormData();
    anchorParts(fd);
    stackParts(fd);
    fetch(apiUrl('/site/blueprint-editor/save'), { method: 'POST', body: fd })
      .then(function (res) {
        if (!res.ok) {
          return res.json().then(function (b) {
            throw new Error((b && b.detail) || 'The blueprint couldn’t be saved.');
          });
        }
        var ignored = parseInt(res.headers.get('X-Kiwi-Ignored') || '0', 10);
        return res.blob().then(function (blob) { return { blob: blob, ignored: ignored }; });
      })
      .then(function (out) {
        var url = URL.createObjectURL(out.blob);
        var a = document.createElement('a');
        a.href = url;
        a.download = state.file.name;
        document.body.appendChild(a);
        a.click();
        document.body.removeChild(a);
        setTimeout(function () { URL.revokeObjectURL(url); }, 4000);
        setStatus(out.ignored
          ? 'Saved. ' + out.ignored + ' protected voxels were left untouched.'
          : 'Saved — check your downloads.', 'ok');
      })
      .catch(function (err) { setStatus(err.message || String(err), 'error'); });
  }

  // ---- transforms --------------------------------------------------------- //

  /* A rotation renumbers every voxel, so it can't be an entry in the edit map the way a
     recolour is. It goes to the server, comes back as a new blueprint, and the page
     reopens it - which also folds in whatever was pending, so nothing is lost. */
  function runTransform(op) {
    if (!state.file) return;
    var list = editList();
    setStatus('Turning the model…');
    var fd = new FormData();
    fd.append('file', state.file, state.file.name);
    fd.append('edits', JSON.stringify(list));
    fd.append('ops', JSON.stringify([op]));
    fetch(apiUrl('/site/blueprint-editor/transform'), { method: 'POST', body: fd })
      .then(function (res) {
        if (!res.ok) {
          return res.json().then(function (b) {
            throw new Error((b && b.detail) || 'The model couldn’t be turned.');
          });
        }
        var summary = {};
        try { summary = JSON.parse(res.headers.get('X-Kiwi-Summary') || '{}'); } catch (e) { /* optional */ }
        return res.blob().then(function (blob) { return { blob: blob, summary: summary }; });
      })
      .then(function (out) {
        var s = out.summary;
        var label = (state.data.transforms || []).reduce(function (acc, t) {
          return t.op === op ? t.label : acc;
        }, op);
        var bits = [label.toLowerCase()];
        if (s.size_before && s.size_after && s.size_before.join() !== s.size_after.join()) {
          bits.push('now ' + s.size_after.join('×'));
        }
        if (s.attachment) bits.push('attachment point moved with it');
        if (s.entities) bits.push(s.entities.toLocaleString() + ' placed object'
          + (s.entities === 1 ? '' : 's') + ' moved too');
        var file = new File([out.blob], state.file.name);
        var note = 'Applied ' + bits.join(' · ') + '.';
        // In a project only THIS part turned; reopening would throw the model away.
        if (inProject()) replaceActive(file, note);
        else openFile(file, note);
      })
      .catch(function (err) { setStatus(err.message || String(err), 'error'); });
  }

  /* Swap the part being edited for a new version of itself - what a rotation produces,
     since a transform renumbers every voxel and no edit index survives it. The part
     keeps its place on the rig, and is marked as carrying its own bytes so the save
     posts them: the archive's copy is the un-rotated one. */
  function replaceActive(file, note) {
    var old = active();
    var fd = new FormData();
    fd.append('file', file, file.name);
    fetch(apiUrl('/site/blueprint-editor/inspect'), { method: 'POST', body: fd })
      .then(function (res) {
        return res.json().then(function (b) {
          if (!res.ok) throw new Error((b && b.detail) || 'That part couldn’t be reopened.');
          return b;
        });
      })
      .then(function (payload) {
        state.docs[state.active] = makeDoc(file, payload, {
          path: old.path, ap: old.ap, added: old.added, row: old.row, replaced: true,
        });
        if (state.scene) state.scene.clearLayer(old.id);
        setActive(state.active);
        setStatus(note || '', note ? 'ok' : '');
      })
      .catch(function (err) { setStatus(err.message || String(err), 'error'); });
  }

  function renderTransforms() {
    var ops = state.data.transforms || [];
    $('bpe-transforms').innerHTML = ops.map(function (t) {
      return '<button type="button" class="bpe-xform" data-op="' + t.op + '">'
        + esc(t.label) + '</button>';
    }).join('');
  }

  // ---- the layer stack ------------------------------------------------------ //

  /* Every layer is its own blueprint, with its own voxels, its own edits and its own
     position. One of them is ACTIVE: that is the one you paint, add to and erase from,
     and the only one a click can hit. The rest are drawn alongside it so you can line
     things up against them.

     Layering is NON-DESTRUCTIVE. A layer sitting over another hides what is underneath
     rather than replacing it, so sliding it back off brings the covered voxels straight
     back. The stack is resolved only when something is OUTPUT - download, the checks,
     the .qb export - and that is where the order decides which voxel wins a shared
     cell: the topmost layer that has one. Index 0 is the bottom of the stack.

     The alignment maths is mirrored from merge.py so the live preview and the server
     agree on where a layer sits; merge.py is the authority. */

  function doc(i) { return state.docs[i] || null; }
  function active() { return state.docs[state.active] || null; }

  function anchorDoc() { return state.docs[state.anchor] || null; }

  function alignOffset(d, mode) {
    var b = anchorDoc();
    if (!b || !d || d === b) return [0, 0, 0];
    if (mode === 'corner') return [0, 0, 0];
    if (mode === 'centre') {
      return [0, 1, 2].map(function (i) {
        return Math.round((b.payload.size[i] - 1) / 2 - (d.payload.size[i] - 1) / 2);
      });
    }
    if (!b.payload.attachment || !d.payload.attachment) return null;
    return [0, 1, 2].map(function (i) {
      return b.payload.attachment[i] - d.payload.attachment[i];
    });
  }

  /* Where a layer's grid sits in the stack's shared space. The bottom layer defines
     that space, so it is always at zero. */
  function placement(d) {
    if (!d || d === anchorDoc()) return [0, 0, 0];
    var base = alignOffset(d, d.mode);
    if (!base) return null;
    return [base[0] + d.offset[0], base[1] + d.offset[1], base[2] + d.offset[2]];
  }

  /* Compact a document's live voxels for the mesher. */
  function viewOf(d) {
    var p = d.payload;
    var v = { count: 0, size: p.size, x: [], y: [], z: [], rgb: [],
              kind: [], level: [], spec: [] };
    for (var i = 0; i < p.count; i++) {
      if (!p.live[i]) continue;
      v.x.push(p.x[i]); v.y.push(p.y[i]); v.z.push(p.z[i]); v.rgb.push(p.rgb[i]);
      v.kind.push(p.kind[i]); v.level.push(p.level[i]); v.spec.push(p.spec[i]);
      v.count++;
    }
    return v;
  }

  /* Draw the stack: the active document as the main (pickable) model at its own
     offset, every other visible one as a layer beside it. Isolate hides the rest
     without changing what any of them are - overlapping voxels make a model
     impossible to read, and this is how you get at the one you mean. */
  function drawStack() {
    if (!state.scene || !state.docs.length) return;
    if (inProject()) return drawProject();
    var a = active();
    var at = placement(a) || [0, 0, 0];
    state.scene.setModelOffset(at[0], at[1], at[2]);
    state.scene.clearLayers();
    state.docs.forEach(function (d, i) {
      if (i === state.active) return;
      if (!d.visible || state.isolate) return;
      var t = placement(d);
      if (!t) return;
      state.scene.setLayer(d.id, viewOf(d), 0x6e7785);
      state.scene.moveLayer(d.id, t[0], t[1], t[2]);
    });
  }

  /* What the flattened output would contain, so the numbers are honest before anything
     is downloaded. Bottom to top, last writer wins - the rule the server applies. */
  function stackStats() {
    var cells = new Set();
    var hidden = 0;
    state.docs.forEach(function (d) {
      var t = placement(d);
      if (!t) return;
      var p = d.payload;
      for (var i = 0; i < p.count; i++) {
        if (!p.live[i]) continue;
        var k = (p.x[i] + t[0]) + ',' + (p.y[i] + t[1]) + ',' + (p.z[i] + t[2]);
        if (cells.has(k)) hidden++;
        cells.add(k);
      }
    });
    return { total: cells.size, hidden: hidden };
  }

  function makeDoc(file, payload, extra) {
    payload.live = [];
    var origin = [];
    for (var i = 0; i < payload.count; i++) { payload.live.push(1); origin.push(i); }
    var d = {
      id: 'D' + (state.docSeq++),
      file: file,
      payload: payload,
      base: { rgb: payload.rgb.slice(), type: payload.type.slice(), w: payload.w.slice() },
      origin: origin,
      history: [],
      mode: 'attachment',
      offset: [0, 0, 0],
      visible: true,
      // Model projects: where this part lives in the mod, which bone it hangs off, and
      // where it sits in the row when it hangs off none.
      path: '',
      ap: null,
      added: false,
      row: [0, 0, 0],
    };
    return Object.assign(d, extra || {});
  }

  /* Switching which document is being edited swaps the whole working set - the arrays
     the tools read, the pristine copy the save diff is taken against, and the undo
     stack, which belongs to the document rather than to the session. */
  function setActive(i) {
    if (i < 0 || i >= state.docs.length) return;
    // Whatever we're leaving may have been painted on, so its cached layer geometry is
    // stale from here on. One re-mesh on the way out; nothing else is touched.
    if (state.docs[state.active]) state.docs[state.active].dirtyMesh = true;
    state.active = i;
    var d = state.docs[i];
    state.file = d.file;
    state.data = d.payload;
    state.base = d.base;
    state.origin = d.origin;
    state.history = d.history;
    state.selection = -1;
    rebuildIndex();
    renderKinds();
    renderTransforms();
    renderPalette();
    renderMaterialList();
    renderMeta();
    renderSelection();
    renderLayers();
    updateDirty();
    if (state.scene) {
      state.scene.rebuild(viewOf(d));
      drawStack();
      drawAttachment();
    }
  }

  function addLayer(file) {
    if (!file) return;
    if (state.docs.length >= 8) {
      setStatus('That is as many layers as one stack takes.', 'error');
      return;
    }
    setStatus('Reading ' + file.name + '...');
    var fd = new FormData();
    fd.append('file', file, file.name);
    fetch(apiUrl('/site/blueprint-editor/inspect'), { method: 'POST', body: fd })
      .then(function (res) {
        return res.json().then(function (b) {
          if (!res.ok) throw new Error((b && b.detail) || 'That blueprint could not be opened.');
          return b;
        });
      })
      .then(function (payload) {
        var d = makeDoc(file, payload);
        // Line the grips up by default; fall back when one side hasn't got one.
        var anc = anchorDoc();
        d.mode = (anc && anc.payload.attachment && payload.attachment)
          ? 'attachment' : 'centre';
        state.docs.push(d);
        setActive(state.docs.length - 1);
        setStatus('Layered in ' + payload.name
          + '. It is the layer you are editing - use Move to drag it about.', 'ok');
      })
      .catch(function (err) { setStatus(err.message || String(err), 'error'); });
  }

  function removeLayer(i) {
    if (!state.docs[i] || state.docs.length < 2) return;
    if (i === state.anchor) {
      setStatus('That layer is the anchor everything else is placed against. Make '
        + 'another layer the anchor first.', 'error');
      return;
    }
    if (state.scene) state.scene.clearLayer(state.docs[i].id);
    state.docs.splice(i, 1);
    if (state.anchor > i) state.anchor--;
    setActive(Math.min(state.active, state.docs.length - 1));
  }

  /* Reordering only changes who wins a shared cell. The anchor is a separate idea, so
     it follows its document rather than staying at an index. */
  function reorderLayer(i, dir) {
    var j = i + dir;
    if (i < 0 || j < 0 || i >= state.docs.length || j >= state.docs.length) return;
    var tmp = state.docs[i];
    state.docs[i] = state.docs[j];
    state.docs[j] = tmp;
    if (state.anchor === i) state.anchor = j;
    else if (state.anchor === j) state.anchor = i;
    setActive(state.active === i ? j : (state.active === j ? i : state.active));
  }

  /* Make a layer the frame. Everything else keeps the position it is SHOWN at, so
     changing the anchor re-describes the arrangement rather than rearranging it -
     otherwise picking a new anchor would scatter the models you had just lined up. */
  function setAnchor(i) {
    if (!state.docs[i] || i === state.anchor) return;
    var world = state.docs.map(placement);
    state.anchor = i;
    state.docs.forEach(function (d, k) {
      if (k === i) { d.offset = [0, 0, 0]; return; }
      var here = world[k], there = world[i];
      if (!here || !there) { d.offset = [0, 0, 0]; return; }
      var base = alignOffset(d, d.mode) || [0, 0, 0];
      d.offset = [here[0] - there[0] - base[0],
                  here[1] - there[1] - base[1],
                  here[2] - there[2] - base[2]];
    });
    renderLayers();
    drawStack();
  }

  function nudgeActive(dx, dy, dz) {
    var d = active();
    if (!d || d === anchorDoc()) return;
    d.offset = [d.offset[0] + dx, d.offset[1] + dy, d.offset[2] + dz];
    renderLayers();
    drawStack();
  }

  /* The parts of a model, with the socket each one hangs off. This is the layer list's
     job done for a creature: the same documents, listed by where they attach rather than
     by what covers what, because a model's parts don't stack - they sit on bones. */
  function renderParts() {
    var r = rig();
    var opts = sockets();
    var rows = state.docs.map(function (d, i) {
      var placed = !!(r && d.ap && r.rest[d.ap]);
      var asking = (state.ask === i && !placed);
      var pick = '<label class="sr-only" for="bpe-ap-' + d.id + '">Where '
        + esc(d.payload.name) + ' attaches</label>'
        + '<select class="bpe-apsel" id="bpe-ap-' + d.id + '" data-ap="' + i + '"'
        + (opts.length ? '' : ' disabled') + '>'
        + '<option value="">' + (opts.length ? 'Not placed — pick a socket'
                                             : 'No rig for this model') + '</option>'
        + opts.map(function (k) {
            return '<option value="' + esc(k) + '"' + (d.ap === k ? ' selected' : '') + '>'
              + esc(k.replace(/_/g, ' ')) + '</option>';
          }).join('')
        + '</select>';
      return '<li class="bpe-partrow' + (i === state.active ? ' active' : '')
        + (asking ? ' asking' : '') + (placed ? '' : ' unplaced') + '">'
        + '<button type="button" class="bpe-layerpick" data-pick="' + i + '">'
        + '<strong>' + esc(d.payload.name) + '</strong><span>'
        + liveCountOf(d).toLocaleString() + ' voxels'
        + (d.added ? ' · added' : '') + '</span></button>'
        + '<button type="button" class="bpe-layerbtn" data-vis="' + i
        + '" aria-label="Show or hide" aria-pressed="' + (!d.visible)
        + '"><i class="fa-solid fa-eye' + (d.visible ? '' : '-slash')
        + '" aria-hidden="true"></i></button>'
        + '<button type="button" class="bpe-layerbtn" data-drop="' + i
        + '" aria-label="Remove this part"' + (state.docs.length < 2 ? ' disabled' : '')
        + '><i class="fa-solid fa-xmark" aria-hidden="true"></i></button>'
        + '<div class="bpe-aprow">' + pick + '</div>'
        + '</li>';
    });
    $('bpe-layerlist').innerHTML = rows.join('');
    $('bpe-isolate').disabled = state.docs.length < 2;
    $('bpe-isolate').setAttribute('aria-pressed', String(!!state.isolate));
    $('bpe-flatten').disabled = true;
    $('bpe-layerctl').hidden = true;

    var unplaced = state.docs.filter(function (d) {
      return !(r && d.ap && r.rest[d.ap]);
    }).length;
    var p = state.project;
    $('bpe-layerinfo').textContent = r
      ? (r.name.replace(/_/g, ' ') + ' rig · ' + state.docs.length + ' parts'
         + (unplaced ? ' · ' + unplaced + ' waiting to be placed' : '')
         + (p.extras ? ' · ' + p.extras + ' other file'
            + (p.extras === 1 ? '' : 's') + ' carried through' : ''))
      : ('No matching creature in the game data, so the parts are laid out side by '
         + 'side. They still save back exactly where they came from.');
    /* Focus the question rather than only colouring it: a part was just added and the
       next thing to do is say where it goes.

       On a timeout, and aimed at the TRIGGER. The site's shared dropdown swaps every
       native <select> for a button and hides the select behind it, and it does that from
       a MutationObserver - so the button doesn't exist yet at the end of this function,
       and focusing the select we just wrote loses the focus the moment it is hidden. */
    if (state.ask >= 0 && opts.length) {
      setTimeout(function () {
        var sel = document.querySelector('.bpe-partrow.asking select.bpe-apsel');
        var dd = sel && sel.closest('.btt-dd');
        var target = dd ? dd.querySelector('.btt-dd-trigger') : sel;
        if (target) target.focus();
      }, 0);
    }
    renderToolHint();
  }

  function renderLayers() {
    if (!state.docs.length) return;
    if (inProject()) return renderParts();
    var multi = state.docs.length > 1;
    $('bpe-flatten').disabled = !multi;
    $('bpe-isolate').disabled = !multi;
    $('bpe-isolate').setAttribute('aria-pressed', String(!!state.isolate));

    // Topmost first, because that is the one that wins.
    var rows = [];
    for (var i = state.docs.length - 1; i >= 0; i--) {
      var d = state.docs[i];
      var t = placement(d);
      var isAnchor = (i === state.anchor);
      rows.push('<li class="bpe-layerrow' + (i === state.active ? ' active' : '')
        + (isAnchor ? ' anchored' : '') + '">'
        + '<button type="button" class="bpe-layerpick" data-pick="' + i + '">'
        + '<strong>' + esc(d.payload.name) + '</strong><span>'
        + liveCountOf(d).toLocaleString() + ' voxels'
        + (isAnchor ? ' - anchor'
                    : (t ? ' at ' + t.join(', ') : ' - cannot line up this way'))
        + '</span></button>'
        + '<button type="button" class="bpe-layerbtn bpe-anchorbtn'
        + (isAnchor ? ' on' : '') + '" data-anchor="' + i + '" aria-pressed="' + isAnchor
        + '" aria-label="Use as the anchor"><i class="fa-solid fa-thumbtack"'
        + ' aria-hidden="true"></i></button>'
        + '<button type="button" class="bpe-layerbtn" data-vis="' + i
        + '" aria-label="Show or hide"' + (isAnchor ? ' disabled' : '')
        + '><i class="fa-solid fa-eye' + (d.visible ? '' : '-slash')
        + '" aria-hidden="true"></i></button>'
        + '<button type="button" class="bpe-layerbtn" data-up="' + i + '" aria-label="Move up"'
        + (i === state.docs.length - 1 ? ' disabled' : '') + '>&uarr;</button>'
        + '<button type="button" class="bpe-layerbtn" data-down="' + i + '" aria-label="Move down"'
        + (i === 0 ? ' disabled' : '') + '>&darr;</button>'
        + '<button type="button" class="bpe-layerbtn" data-drop="' + i
        + '" aria-label="Remove layer"' + (isAnchor ? ' disabled' : '')
        + '><i class="fa-solid fa-xmark" aria-hidden="true"></i></button>'
        + '</li>');
    }
    $('bpe-layerlist').innerHTML = rows.join('');

    var a = active();
    var movable = a && a !== anchorDoc();
    $('bpe-layerctl').hidden = !movable;
    if (movable) { renderAlignModes(a); renderNudge(a); }

    var st = stackStats();
    $('bpe-layerinfo').textContent = multi
      ? ('Flattens to ' + st.total.toLocaleString() + ' voxels'
         + (st.hidden ? ' - ' + st.hidden.toLocaleString()
            + ' hidden underneath, still there until you download' : ''))
      : '';
    renderToolHint();
  }

  function liveCountOf(d) {
    var n = 0;
    for (var i = 0; i < d.payload.count; i++) if (d.payload.live[i]) n++;
    return n;
  }

  function renderAlignModes(d) {
    var modes = (anchorDoc() || state.docs[0]).payload.align_modes || [];
    $('bpe-align').innerHTML = modes.map(function (m) {
      return '<option value="' + m.mode + '"' + (m.mode === d.mode ? ' selected' : '')
        + '>' + esc(m.label) + '</option>';
    }).join('');
  }

  function renderNudge(d) {
    ['x', 'y', 'z'].forEach(function (a, i) {
      var v = d.offset[i];
      $('bpe-nudge-' + a).textContent = (v > 0 ? '+' : '') + v;
    });
  }

  /* The stack as the output endpoints want it: layer 0 is the base file, the rest ride
     as `layers` bottom to top with a spec each - placement AND their own edits, since
     every layer is a blueprint someone may have painted on. */
  function stackParts(fd) {
    // A model's parts aren't a stack - they sit on bones, and every per-part tool
    // (the checks, the .qb export) runs on the one part you're editing.
    if (inProject() || state.docs.length < 2) return;
    var others = state.docs.filter(function (_, i) { return i !== state.anchor; });
    others.forEach(function (d) { fd.append('layers', d.file, d.file.name); });
    fd.append('stack', JSON.stringify(others.map(function (d) {
      return { mode: d.mode, offset: d.offset, edits: editListOf(d) };
    })));
    // Where the anchor sits in the stacking order - separate from being the frame.
    fd.append('anchor_at', String(state.anchor));
  }

  function anchorParts(fd) {
    var a = inProject() ? active() : anchorDoc();
    fd.append('file', a.file, a.file.name);
    fd.append('edits', JSON.stringify(editListOf(a)));
  }

  function flattenStack() {
    if (state.docs.length < 2) return;
    setStatus('Flattening...');
    var fd = new FormData();
    anchorParts(fd);
    stackParts(fd);
    fetch(apiUrl('/site/blueprint-editor/flatten'), { method: 'POST', body: fd })
      .then(function (res) {
        if (!res.ok) {
          return res.json().then(function (b) {
            throw new Error((b && b.detail) || 'The stack could not be flattened.');
          });
        }
        var summary = {};
        try { summary = JSON.parse(res.headers.get('X-Kiwi-Summary') || '{}'); } catch (e) { /* optional */ }
        return res.blob().then(function (blob) { return { blob: blob, summary: summary }; });
      })
      .then(function (out) {
        var s = out.summary;
        var bits = [s.voxels.toLocaleString() + ' voxels from ' + ((s.layers || 0) + 1) + ' models'];
        if (s.hidden) bits.push(s.hidden.toLocaleString() + ' overlapping cell'
          + (s.hidden === 1 ? '' : 's') + ' went to the layer above');
        if (s.entities) bits.push(s.entities);
        var name = anchorDoc().file.name;
        openFile(new File([out.blob], name),
                 'Flattened into one blueprint - ' + bits.join(' - ') + '.');
      })
      .catch(function (err) { setStatus(err.message || String(err), 'error'); });
  }

  // ---- model projects ----------------------------------------------------- //

  /* A Trove model is not one file. A mount is a head, a jaw, four legs, a body and a
     tail, each its own .blueprint, assembled onto a skeleton by the game. Opening them
     one at a time meant recolouring a dragon sixteen times over without once seeing the
     dragon, so a project opens the whole set.

     Each part is one of the same documents the layer stack uses - its own voxels, its
     own edits, its own undo - and the ONLY difference is how it is placed: by the bone
     it attaches to rather than by a grid offset. `matrixFor` is that difference.

     WHERE A PART GOES IS NEVER GUESSED. The rig and each part's attach point come from
     the game's own prefab bindings, and a part the game doesn't place sits beside the
     model until somebody says where it goes. A part dropped in later is somebody
     saying: they pick the socket, we place it. */

  function b64bytes(s) {
    var bin = atob(s), n = bin.length, out = new Uint8Array(n);
    for (var i = 0; i < n; i++) out[i] = bin.charCodeAt(i);
    return out;
  }

  /* Open a .tmod / .zip, or a pile of loose .blueprint files, as one model. */
  function openModel(input) {
    var many = !(input instanceof File);
    var fd = new FormData();
    if (many) {
      Array.prototype.forEach.call(input, function (f) { fd.append('files', f, f.name); });
    } else {
      fd.append('file', input, input.name);
    }
    setStatus('Opening ' + (many ? input.length + ' parts' : input.name) + '…');
    fetch(apiUrl('/site/blueprint-editor/model'), { method: 'POST', body: fd })
      .then(function (res) {
        return res.json().then(function (b) {
          if (!res.ok) throw new Error((b && b.detail) || 'That model couldn’t be opened.');
          return b;
        });
      })
      .then(function (payload) { loadProject(payload, many ? null : input); })
      .catch(function (err) { setStatus(err.message || String(err), 'error'); });
  }

  function loadProject(payload, archive) {
    state.project = {
      file: archive || null,          // null for a project of loose files
      name: payload.name,
      source: payload.source,
      rig: payload.rig || null,
      skipped: payload.skipped || [],
      extras: payload.extras || 0,
    };
    state.docs = payload.parts.map(function (p) {
      // Each part carries its own bytes, so every single-file tool - the checks, the
      // .qb export, a rotation - keeps working on one part of a project unchanged.
      var file = new File([b64bytes(p.blueprint)], p.name);
      return makeDoc(file, p.model, { path: p.path, ap: p.ap || null });
    });
    state.active = 0;
    state.anchor = 0;
    mount(projectNote(payload));
  }

  function projectNote(payload) {
    var bits = [payload.parts.length + ' part' + (payload.parts.length === 1 ? '' : 's')];
    var placed = payload.parts.filter(function (p) { return p.ap; }).length;
    if (payload.rig) {
      bits.push(placed + ' of them on the ' + payload.rig.name.replace(/_/g, ' ') + ' rig');
    } else {
      bits.push('no matching creature in the game data, so they’re laid out side by side');
    }
    if (payload.skipped.length) {
      bits.push(payload.skipped.length + ' couldn’t be opened');
    }
    return 'Opened ' + payload.name + ' — ' + bits.join(', ') + '.';
  }

  /* The panel says "Layers" for a stack and "Parts" for a model, because they are not
     the same idea: layers cover each other and flatten into one file, parts sit on
     different bones and stay separate files for ever. Same controls, different truth. */
  function renderMode() {
    var on = inProject();
    var set = function (id, text) { var el = $(id); if (el) el.textContent = text; };
    set('bpe-save-label', on ? 'Download the model' : 'Download');
    set('bpe-layers-title', on ? 'Parts' : 'Layers');
    set('bpe-layer-open-label', on ? 'Add a part' : 'Add a layer');
    set('bpe-layers-hint', on
      ? 'Every part is its own blueprint on its own bone. Double-click one in the view '
        + 'to work on it, or pick it here — and tell a part you have added where it goes.'
      : 'Each layer is its own blueprint. Pick one to edit it, drag it about with the '
        + 'Move tool, and reorder them — on download the top layer wins wherever two '
        + 'overlap.');
    // Nothing to drag when a bone decides where a part sits.
    var move = document.querySelector('[data-tool="move"]');
    if (move) {
      move.disabled = on;
      if (on && state.tool === 'move') {
        state.tool = 'paint';
        document.querySelectorAll('[data-tool]').forEach(function (o) {
          o.setAttribute('aria-pressed', String(o.dataset.tool === 'paint'));
        });
      }
    }
  }

  function inProject() { return !!state.project; }
  function rig() { return state.project && state.project.rig; }

  /* Every socket this skeleton has, whether or not the mod fills it - the list you pick
     from when you add a part. A rig that carries no `hat` point can't be given a hat,
     and that is exactly the thing to say rather than let someone place one nowhere. */
  function sockets() {
    var r = rig();
    return r ? Object.keys(r.rest).sort() : [];
  }

  // 4x4 matrices, column-major like the baked rigs and three.js both store them.
  function matMul(a, b) {
    var o = new Array(16);
    for (var c = 0; c < 4; c++) {
      for (var r = 0; r < 4; r++) {
        var v = 0;
        for (var k = 0; k < 4; k++) v += a[k * 4 + r] * b[c * 4 + k];
        o[c * 4 + r] = v;
      }
    }
    return o;
  }
  function matScale(s) { return [s,0,0,0, 0,s,0,0, 0,0,s,0, 0,0,0,1]; }
  function matTrans(x, y, z) { return [1,0,0,0, 0,1,0,0, 0,0,1,0, x,y,z,1]; }
  function matPoint(m, x, y, z) {
    return [m[0]*x + m[4]*y + m[8]*z + m[12],
            m[1]*x + m[5]*y + m[9]*z + m[13],
            m[2]*x + m[6]*y + m[10]*z + m[14]];
  }

  /* Where a part sits. On its bone if it has one - the rig's rest matrix times the voxel
     size, which is what the 3D viewers do with the same numbers - and otherwise in the
     row `layoutUnplaced` works out, off to one side of the model. */
  function matrixFor(d) {
    var r = rig();
    var vs = r ? r.voxel_scale : 1;
    if (r && d.ap && r.rest[d.ap]) {
      return matMul(r.rest[d.ap], matScale(vs * (r.scales[d.ap] || 1)));
    }
    return matMul(matTrans(d.row[0], d.row[1], d.row[2]), matScale(vs));
  }

  /* Unplaced parts go in a row beside the model rather than all at the origin on top of
     each other. Measured off whatever IS placed, so the row clears the creature however
     big it is, and each part gets its own width of space. */
  function layoutUnplaced() {
    var r = rig();
    var vs = r ? r.voxel_scale : 1;
    var lo = [Infinity, Infinity, Infinity], hi = [-Infinity, -Infinity, -Infinity];
    state.docs.forEach(function (d) {
      if (!(r && d.ap && r.rest[d.ap])) return;
      var m = matrixFor(d), s = d.payload.size;
      [[0,0,0],[s[0],0,0],[0,s[1],0],[0,0,s[2]],[s[0],s[1],0],[s[0],0,s[2]],
       [0,s[1],s[2]],[s[0],s[1],s[2]]].forEach(function (c) {
        var p = matPoint(m, c[0], c[1], c[2]);
        for (var i = 0; i < 3; i++) {
          if (p[i] < lo[i]) lo[i] = p[i];
          if (p[i] > hi[i]) hi[i] = p[i];
        }
      });
    });
    var placed = isFinite(lo[0]);
    var gap = placed ? (hi[0] - lo[0]) * 0.12 + vs * 2 : vs * 2;
    var x = placed ? hi[0] + gap : 0;
    var y = placed ? lo[1] : 0;
    var z = placed ? (lo[2] + hi[2]) / 2 : 0;
    state.docs.forEach(function (d) {
      if (r && d.ap && r.rest[d.ap]) { d.row = [0, 0, 0]; return; }
      var s = d.payload.size;
      d.row = [x / vs, y / vs, (z - s[2] * vs / 2) / vs];   // row is in the part's own units
      x += s[0] * vs + gap;
    });
  }

  /* Draw the model. Only the parts that CHANGED are re-meshed: switching to another
     part redraws two of them, not sixteen, which is the difference between an instant
     switch and a visible stall on a creature. A part's geometry can only change while
     it is the one being edited, so `dirtyMesh` is set as it stops being that. */
  function drawProject() {
    layoutUnplaced();
    var a = active();
    state.scene.setModelMatrix(matrixFor(a));
    state.scene.setModelOutline(ACTIVE_BOX);

    var want = {};
    state.docs.forEach(function (d, i) {
      if (i !== state.active && d.visible && !state.isolate) want[d.id] = d;
    });
    Object.keys(state.drawn).forEach(function (id) {
      if (!want[id]) { state.scene.clearLayer(id); delete state.drawn[id]; }
    });
    Object.keys(want).forEach(function (id) {
      var d = want[id];
      if (!state.drawn[id] || d.dirtyMesh) {
        // Qubicle draws a box around every part of a multi-part model, and it is right:
        // without them a creature is one lump of voxels with no seams.
        state.scene.setLayer(d.id, viewOf(d), PART_BOX, { opacity: PART_BOX_OPACITY });
        d.dirtyMesh = false;
        state.drawn[id] = 1;
      }
      state.scene.setLayerMatrix(d.id, matrixFor(d));
    });
  }

  /* Add blueprints to an open model. They are decoded through the ordinary single-file
     path, so anything the editor can open can be added, and then we ASK where each one
     goes: the game's map places the parts a mod already had, but a file someone just
     dropped in is by definition not in it yet. */
  function addParts(fileList) {
    var picked = Array.prototype.slice.call(fileList).filter(function (f) {
      return /\.blueprint$/i.test(f.name);
    });
    if (!picked.length) return;
    if (state.docs.length + picked.length > 48) {
      setStatus('That is as many parts as one model holds.', 'error');
      return;
    }
    setStatus('Reading ' + picked.length + ' part' + (picked.length === 1 ? '' : 's') + '…');
    Promise.all(picked.map(function (f) {
      var fd = new FormData();
      fd.append('file', f, f.name);
      return fetch(apiUrl('/site/blueprint-editor/inspect'), { method: 'POST', body: fd })
        .then(function (res) {
          return res.json().then(function (b) {
            if (!res.ok) throw new Error((b && b.detail) || (f.name + ' couldn’t be opened.'));
            return { file: f, payload: b };
          });
        });
    })).then(function (opened) {
      opened.forEach(function (o) {
        state.docs.push(makeDoc(o.file, o.payload, { path: '', ap: null, added: true }));
      });
      // The first one we need an answer about. setActive redraws the list, and that is
      // also what puts the cursor in its socket picker - so nothing may redraw after it,
      // or the focus lands on an element that has already been replaced.
      state.ask = state.docs.length - opened.length;
      setActive(state.ask);
      var one = opened.length === 1;
      setStatus((one ? opened[0].payload.name + ' is in' : opened.length + ' parts are in')
        + '. ' + (rig()
            ? 'Tell me where ' + (one ? 'it' : 'each one') + ' attaches and '
              + (one ? 'it' : 'they') + ' will snap onto the model.'
            : 'There’s no rig for this model, so ' + (one ? 'it sits' : 'they sit')
              + ' beside the others.'), 'ok');
    }).catch(function (err) { setStatus(err.message || String(err), 'error'); });
  }

  /* Somebody answering "where does this go". The socket list is the rig's own, so the
     only thing that can be chosen is a bone the skeleton actually has. */
  function setPartAp(i, ap) {
    var d = doc(i);
    if (!d) return;
    d.ap = ap || null;
    if (state.ask === i) state.ask = -1;
    renderLayers();
    drawStack();
    if (state.scene && ap) state.scene.request();
  }

  function removePart(i) {
    if (!state.docs[i] || state.docs.length < 2) return;
    if (state.scene) state.scene.clearLayer(state.docs[i].id);
    delete state.drawn[state.docs[i].id];
    state.docs.splice(i, 1);
    if (state.ask >= i) state.ask = -1;
    setActive(Math.min(state.active, state.docs.length - 1));
  }

  /* The whole model back out as the file it came in as. The archive is posted again
     alongside the per-part edits - the server holds nothing between opening a model and
     writing it - and any part added since rides along with the path it should be packed
     at, so a mod comes back complete rather than as a folder of loose blueprints. */
  function saveModel() {
    var p = state.project;
    if (!p) return;
    var fd = new FormData();
    var edits = {};
    /* Where each part lives inside the mod. One already in it keeps its path; a part
       added since goes to blueprints/, which is the only folder Trove reads models out
       of. A project of loose files has no archive to keep anything else in, so every
       part is posted and the server names them by their own filenames. */
    var pathOf = new Map();
    state.docs.forEach(function (d) {
      pathOf.set(d, d.path || (p.file ? 'blueprints/' + d.file.name : d.file.name));
    });
    if (p.file) fd.append('file', p.file, p.file.name);
    var posted = [];
    state.docs.forEach(function (d) {
      // Already in the archive and still the bytes that came out of it -> nothing to
      // post; the server edits its own copy. A rotated part is NOT that, so it goes.
      if (p.file && d.path && !d.replaced) return;
      fd.append('files', d.file, d.file.name);
      posted.push(pathOf.get(d));
    });
    // ONE field holding the whole list, lined up with the files - a repeated form field
    // would arrive as whichever copy the server kept.
    fd.append('paths', JSON.stringify(posted));
    state.docs.forEach(function (d) {
      var list = editListOf(d);
      if (list.length) edits[pathOf.get(d)] = list;
    });
    fd.append('edits', JSON.stringify(edits));
    fd.append('name', p.name);
    setStatus('Building ' + p.name + '…');
    fetch(apiUrl('/site/blueprint-editor/model-save'), { method: 'POST', body: fd })
      .then(function (res) {
        if (!res.ok) {
          return res.json().then(function (b) {
            throw new Error((b && b.detail) || 'The model couldn’t be saved.');
          });
        }
        var summary = {};
        try { summary = JSON.parse(res.headers.get('X-Kiwi-Summary') || '{}'); } catch (e) { /* optional */ }
        var name = (res.headers.get('Content-Disposition') || '').match(/filename="([^"]+)"/);
        return res.blob().then(function (blob) {
          return { blob: blob, summary: summary, name: name ? name[1] : p.name };
        });
      })
      .then(function (out) {
        download(out.blob, out.name);
        var s = out.summary;
        var bits = [];
        if (s.parts) bits.push(s.parts + ' part' + (s.parts === 1 ? '' : 's') + ' changed');
        if (s.added_parts) bits.push(s.added_parts + ' added');
        if (s.ignored) bits.push(s.ignored + ' protected voxels left as they were');
        setStatus('Saved' + (bits.length ? ' — ' + bits.join(', ') : '') + '.', 'ok');
      })
      .catch(function (err) { setStatus(err.message || String(err), 'error'); });
  }

  // ---- starting from a model that's already in the game -------------------- //

  /* Most mods start from something the game already has: a mount recoloured, a dragon
     restyled. The codex knows every one of those by name, and the prefab behind the name
     is exactly what binds its blueprints to its bones - so a search of the codex IS a
     search of the editable models, and the card's picture is the creature itself. */

  var searchTimer = 0, searchSeq = 0;

  function runSearch() {
    var q = $('bpe-search').value.trim();
    var type = $('bpe-searchtype').value;
    var note = $('bpe-resultnote');
    if (q.length < 2 && !type) {
      $('bpe-results').innerHTML = '';
      note.hidden = true;
      return;
    }
    var seq = ++searchSeq;
    var url = '/site/codexes/search?limit=24&q=' + encodeURIComponent(q)
      + (type ? '&type=' + encodeURIComponent(type) : '');
    U.getJSON(url).then(function (body) {
      if (seq !== searchSeq) return;      // a later keystroke already answered
      renderResults((body && body.items) || [], body && body.total);
    }).catch(function () {
      if (seq !== searchSeq) return;
      note.hidden = false;
      note.textContent = 'The game data couldn’t be searched just now.';
    });
  }

  function renderResults(items, total) {
    var note = $('bpe-resultnote');
    // Only a creature the game binds to a skeleton can be opened as a model, and that
    // is not knowable from a codex row - so everything is offered and the one that
    // isn't bound says so when you pick it, rather than being silently missing.
    $('bpe-results').innerHTML = items.map(function (e) {
      var img = e.blueprint
        ? '<img loading="lazy" decoding="async" alt="" src="' + esc(apiUrl(
            '/site/codexes/render?dim=96&blueprint=' + encodeURIComponent(e.blueprint)
            + (e.path ? '&prefab=' + encodeURIComponent(e.path) : ''))) + '">'
        : '';
      return '<li><button type="button" class="bpe-result" data-prefab="' + esc(e.path || '')
        + '"><span class="bpe-result-img">' + img + '</span>'
        + '<span class="bpe-result-name">' + esc(e.name || e.path || 'Unnamed')
        + '<small>' + esc(e.codex_type || '') + '</small></span></button></li>';
    }).join('');
    note.hidden = !items.length && !total;
    note.textContent = items.length
      ? (total > items.length ? 'Showing ' + items.length + ' of ' + total.toLocaleString()
         + ' — keep typing to narrow it down.' : '')
      : 'Nothing in the game data matches that.';
    if (items.length && !note.textContent) note.hidden = true;
  }

  function openGameModel(prefab, label) {
    setStatus('Opening ' + (label || prefab) + '…');
    U.fetchJSON('/site/blueprint-editor/game-model?prefab=' + encodeURIComponent(prefab))
      .then(function (payload) { loadProject(payload, null); })
      .catch(function (err) {
        // A bare status code says nothing to whoever clicked a picture of a dragon, and
        // the one thing that goes wrong here has a real explanation: plenty of codex
        // entries aren't creatures at all.
        var msg = (err && err.message) || '';
        setStatus(/^HTTP \d+$/.test(msg) || !msg
          ? (label || 'That one') + ' isn’t a model the game builds from blueprint parts,'
            + ' so there’s nothing to open in the editor.'
          : msg, 'error');
      });
  }

  // ---- Qubicle interop ---------------------------------------------------- //

  function download(blob, filename) {
    var url = URL.createObjectURL(blob);
    var a = document.createElement('a');
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    setTimeout(function () { URL.revokeObjectURL(url); }, 4000);
  }

  /* Out to the authoring format: the base model plus the _a / _s / _t material maps,
     which is what Qubicle and MagicaVoxel users actually work in. */
  function exportQb() {
    if (!state.file) return;
    var list = editList();
    setStatus('Building the .qb files…');
    var fd = new FormData();
    anchorParts(fd);
    stackParts(fd);
    fetch(apiUrl('/site/blueprint-editor/export-qb'), { method: 'POST', body: fd })
      .then(function (res) {
        if (!res.ok) {
          return res.json().then(function (b) {
            throw new Error((b && b.detail) || 'The export failed.');
          });
        }
        var notes = [];
        try { notes = JSON.parse(res.headers.get('X-Kiwi-Notes') || '[]'); } catch (e) { /* header is optional */ }
        return res.blob().then(function (blob) { return { blob: blob, notes: notes }; });
      })
      .then(function (out) {
        var stem = state.file.name.replace(/\.blueprint$/i, '') || 'model';
        download(out.blob, stem + '_qb.zip');
        // Anything the conversion had to flatten is said out loud here rather than
        // left for the user to notice in Qubicle.
        setStatus(out.notes.length
          ? 'Exported four .qb files. ' + out.notes.join(' ')
          : 'Exported four .qb files — model, alpha, specular and type.', 'ok');
      })
      .catch(function (err) { setStatus(err.message || String(err), 'error'); });
  }

  /* In from the authoring format: compile the .qb set to a blueprint on the server,
     then open the result through the ordinary path so everything downstream - editing,
     checks, saving - behaves exactly as it does for a file that arrived as a blueprint. */
  function importQb(fileList) {
    var picked = Array.prototype.slice.call(fileList).filter(function (f) {
      return /\.qb$/i.test(f.name);
    });
    if (!picked.length) {
      setStatus('Pick a .qb file — and its _a / _s / _t maps too, if you have them.', 'error');
      return;
    }
    if (picked.length > 4) {
      setStatus('Send the model and up to three material maps.', 'error');
      return;
    }
    setStatus('Compiling ' + picked.length + ' .qb file' + (picked.length === 1 ? '' : 's') + '…');
    var fd = new FormData();
    picked.forEach(function (f) { fd.append('files', f, f.name); });
    fetch(apiUrl('/site/blueprint-editor/import-qb'), { method: 'POST', body: fd })
      .then(function (res) {
        if (!res.ok) {
          return res.json().then(function (b) {
            throw new Error((b && b.detail) || 'Those .qb files couldn’t be compiled.');
          });
        }
        var summary = {};
        try { summary = JSON.parse(res.headers.get('X-Kiwi-Summary') || '{}'); } catch (e) { /* optional */ }
        return res.blob().then(function (blob) { return { blob: blob, summary: summary }; });
      })
      .then(function (out) {
        var base = picked.find(function (f) { return !/_[ast]\.qb$/i.test(f.name); }) || picked[0];
        var stem = base.name.replace(/\.qb$/i, '');
        var file = new File([out.blob], stem + '.blueprint');
        var s = out.summary;
        var bits = [];
        if (s.maps && s.maps.length) {
          bits.push('read the ' + s.maps.join(', ') + ' map' + (s.maps.length === 1 ? '' : 's'));
        } else {
          bits.push('no material maps were supplied, so everything is a rough solid');
        }
        if (s.attachment_source === 'marker') bits.push('found the attachment point');
        else if (s.attachment_source === 'offset') bits.push('took the attachment point from the matrix offset');
        // No marker means the model was centred, the same as a decoration. Say that
        // rather than "no attachment point", because one is shown either way - the
        // format always stores an origin, and a centred one is a real answer for a deco.
        else bits.push('no attachment marker, so it was centred — add a magenta '
          + '(255, 0, 255) voxel if it needs a specific grip point');
        var lossy = (s.unknown_type || 0) + (s.unknown_alpha || 0) + (s.unknown_specular || 0);
        if (lossy) bits.push(lossy.toLocaleString() + ' voxels had map colours outside the palette and fell back to defaults');
        // With a model open, a compiled .qb joins it as a part rather than replacing it.
        if (inProject()) { addParts([file]); return; }
        openFile(file, 'Compiled — ' + bits.join('; ') + '.');
      })
      .catch(function (err) { setStatus(err.message || String(err), 'error'); });
  }

  // ---- rendering ---------------------------------------------------------- //

  function label(i) {
    var d = state.data, t = d.type[i], w = d.w[i];
    var pal = (d.palette.types || []).find(function (p) { return p.type === t; });
    if (!pal) {
      var known = (d.materials || []).find(function (m) { return m.type === t && m.w === w; });
      return known ? known.label : 'Unknown type ' + t;
    }
    var opt = (pal.options || []).find(function (o) { return o.w === w; });
    return pal.label + (opt ? ' · ' + opt.label : '');
  }

  var KIND_LABELS = {
    other: 'Not sure / something else', melee: 'Melee weapon', gun: 'Gun',
    staff: 'Staff', bow: 'Bow', spear: 'Spear', fist: 'Fist weapon',
    mask: 'Mask / face', hat: 'Hat', hair: 'Hair / helmet', deco: 'Decoration',
  };

  function renderKinds() {
    var types = state.data.creation_types || ['other'];
    // "Not sure" first: it's the honest default, and it runs only the checks that
    // don't depend on knowing what the model is.
    types = types.slice().sort(function (a, b) {
      return (a === 'other' ? -1 : b === 'other' ? 1 : a.localeCompare(b));
    });
    $('bpe-kind').innerHTML = types.map(function (t) {
      return '<option value="' + t + '"' + (t === state.kind ? ' selected' : '') + '>'
        + esc(KIND_LABELS[t] || t) + '</option>';
    }).join('');
  }

  /* Push the current paint settings back into the controls. The eyedropper changes them
     from outside the panel, and a swatch that didn't follow would be lying. */
  function syncPaintUI() {
    $('bpe-colour').value = hex(state.paint.rgb);
    $('bpe-colour-hex').textContent = hex(state.paint.rgb).toUpperCase();
    renderPalette();
  }

  /* The paint controls only govern a paint click, and Add uses them for the new voxel -
     so Erase and Pick grey them out rather than leaving a colour picker that does
     nothing. */
  /* Paint, add and erase run along the drag; move repositions; pick stays a click.
     Ctrl-drag turns the view in every one of them, so the gesture is the same
     everywhere in the editor. */
  function dragModeFor(tool) {
    if (tool === 'move') return 'move';
    if (tool === 'paint' || tool === 'add' || tool === 'erase') return 'stroke';
    return '';
  }

  /* A drag is ONE action. Edits accumulate into a single batch so the whole streak
     undoes in one step, and the expensive panel redraws wait for the end rather than
     running per voxel. */
  function onStroke(phase) {
    if (phase === 'start') {
      state.stroke = [];
      state.strokeNormal = null;
      return;
    }
    var batch = state.stroke;
    state.stroke = null;
    state.strokeNormal = null;
    if (batch && batch.length) {
      state.history.push(batch);
      renderMaterialList();
      renderMeta();
      renderLayers();
      renderSelection();
      updateDirty();
      staleReport();
    }
  }

  function renderToolHint() {
    var hints = {
      paint: 'Click or drag across voxels to paint them. Ctrl-drag turns the view.',
      pick: 'Click a voxel to copy its colour and material into the settings below.',
      add: 'Drag along a face to lay voxels against it. Ctrl-drag turns the view.',
      erase: 'Click or drag across voxels to delete them. Ctrl-drag turns the view.',
      move: 'Drag to slide this layer around. Ctrl-drag turns the view.',
    };
    var hint = hints[state.tool] || hints.paint;
    // Which layer a click lands on is the thing to be unambiguous about once there is
    // more than one model on screen.
    if (state.docs.length > 1 && active()) {
      hint += ' Working on ' + active().payload.name + '.';
    }
    if (inProject() && state.docs.length > 1) {
      hint += ' Double-click another part to switch to it.';
    }
    if (inProject() && state.tool === 'move') {
      hint = 'A part of a model sits where its bone puts it, so there is nothing to '
        + 'drag. Change where it attaches in the Parts list instead.';
    } else if (state.tool === 'move' && active() === anchorDoc()) {
      hint = 'This layer is the anchor - the frame the others are placed against, so it '
        + 'does not move. Pick another layer, or pin a different one as the anchor.';
    }
    $('bpe-toolhint').textContent = hint;
    var faded = state.tool === 'erase' || state.tool === 'pick' || state.tool === 'move';
    ['bpe-paint-colour', 'bpe-paint-material'].forEach(function (id) {
      var el = $(id);
      if (el) el.classList.toggle('bpe-faded', faded);
    });
    // "A click changes" only means anything while painting.
    var modes = $('bpe-modes');
    if (modes) modes.classList.toggle('bpe-faded', state.tool !== 'paint');
    // Scope governs paint and erase; pick and add are always a single voxel.
    var scopes = $('bpe-scopes');
    var single = state.tool === 'pick' || state.tool === 'add' || state.tool === 'move';
    if (scopes) scopes.classList.toggle('bpe-faded', single);
    var lbl = $('bpe-scope-label');
    if (lbl) lbl.classList.toggle('bpe-faded', single);
    renderScopeHint();
  }

  /* Say how many voxels the current scope would hit BEFORE the click, using whatever is
     selected as the sample. "Every voxel of that material" on a plain model can mean
     hundreds, and finding that out by doing it is the wrong order. */
  function renderScopeHint() {
    var el = $('bpe-scopehint');
    if (!el || !state.data) return;
    if (state.tool === 'pick' || state.tool === 'add' || state.scope === 'voxel') {
      el.textContent = '';
      el.hidden = true;
      return;
    }
    el.hidden = false;
    if (state.selection < 0) {
      el.textContent = state.scope === 'colour'
        ? 'Matches on the exact colour of whichever voxel you click.'
        : 'Matches on material — on a plain model that can be most of it.';
      return;
    }
    var n = targetsFor(state.selection).length;
    el.textContent = 'The selected voxel matches ' + n.toLocaleString()
      + (n === 1 ? ' voxel' : ' voxels') + '.';
  }

  function renderPalette() {
    var pal = state.data.palette;
    var html = (pal.types || []).map(function (p) {
      return '<button type="button" class="bpe-mat" data-type="' + p.type + '"'
        + ' aria-pressed="' + (state.paint.type === p.type) + '">'
        + esc(p.label) + '</button>';
    }).join('');
    $('bpe-types').innerHTML = html;
    renderWOptions();
  }

  function renderWOptions() {
    var pal = state.data.palette;
    var p = (pal.types || []).find(function (t) { return t.type === state.paint.type; });
    if (!p) { $('bpe-finish').innerHTML = ''; return; }
    $('bpe-finish-label').textContent = p['class'] === 'glass' ? 'Opacity' : 'Finish';
    $('bpe-finish').innerHTML = (p.options || []).map(function (o) {
      return '<button type="button" class="bpe-w" data-w="' + o.w + '"'
        + ' aria-pressed="' + (state.paint.w === o.w) + '">' + esc(o.label) + '</button>';
    }).join('');
  }

  /* Every distinct material in the model, largest first. This doubles as the bulk
     tool: it is how you recolour "all the glass" without hunting for a voxel. */
  function renderMaterialList() {
    var d = state.data;
    var seen = new Map();
    for (var i = 0; i < d.count; i++) {
      if (!d.live[i]) continue;
      var key = d.type[i] + ':' + d.w[i];
      var e = seen.get(key);
      if (e) { e.count++; e.colours.add(d.rgb[i]); }
      else {
        seen.set(key, { type: d.type[i], w: d.w[i], count: 1, i: i,
                        colours: new Set([d.rgb[i]]) });
      }
    }
    var rows = Array.from(seen.values()).sort(function (a, b) { return b.count - a.count; });
    $('bpe-materials').innerHTML = rows.map(function (r) {
      // Apply is offered whenever SOMETHING can change - a placeable-colour voxel
      // takes a new colour even though its material is fixed, and the click applies
      // whichever half it's allowed. Only a voxel that can take neither is locked.
      var canMaterial = !!d.edit[r.i], canColour = !!d.paint[r.i];
      var locked = !canMaterial && !canColour;
      var title = canMaterial && canColour ? 'Apply the current paint to all of these'
        : canColour ? 'These can be recoloured, but their material is fixed'
        : 'Their material can change, but Trove colours these itself';
      // A material group can span many colours, and a single swatch implied it didn't -
      // which is half of why "everything of that material" surprised people. Show up to
      // three of the colours actually in the group, and say how many there are.
      var cols = Array.from(r.colours);
      var swatch = cols.length === 1
        ? 'background:' + hex(cols[0])
        : 'background:linear-gradient(135deg,' + cols.slice(0, 3).map(function (c, k, a) {
            var from = Math.round(k / a.length * 100), to = Math.round((k + 1) / a.length * 100);
            return hex(c) + ' ' + from + '%,' + hex(c) + ' ' + to + '%';
          }).join(',') + ')';
      var colnote = cols.length > 1 ? ' · ' + cols.length + ' colours' : '';
      return '<li class="bpe-matrow' + (locked ? ' bpe-locked' : '') + '">'
        + '<span class="bpe-swatch" style="' + swatch + '" title="'
        + esc(cols.length === 1 ? hex(cols[0]).toUpperCase() : cols.length + ' different colours')
        + '"></span>'
        + '<span class="bpe-matname" title="' + esc(label(r.i) + colnote) + '">'
        + esc(label(r.i)) + '<small class="bpe-colnote">' + esc(colnote) + '</small></span>'
        + '<span class="bpe-matcount">' + r.count.toLocaleString() + '</span>'
        + (locked
            ? '<span class="bpe-lock" title="Controlled by the game — preserved on save">'
              + '<i class="fa-solid fa-lock" aria-hidden="true"></i></span>'
            : '<button type="button" class="bpe-applyall' + ((canMaterial && canColour) ? '' : ' bpe-partial')
              + '" data-type="' + r.type + '" data-w="' + r.w + '" title="' + esc(title) + '">Apply</button>')
        + '</li>';
    }).join('');
  }

  function renderMeta() {
    var s = state.data.stats;
    var live = liveCount();
    var bits = [
      live.toLocaleString() + ' voxels'
        + (live !== s.voxels ? ' (was ' + s.voxels.toLocaleString() + ')' : ''),
      'v' + state.data.version,
      state.data.size.join('×'),
    ];
    $('bpe-meta').textContent = bits.join(' · ');
    // In a model the part's name alone is half an answer - which model it belongs to is
    // the other half, and it is what the download is named after.
    $('bpe-filename').textContent = inProject()
      ? state.project.name + ' · ' + state.data.name
      : state.data.name;

    var notes = [];
    if (s.placeholders) {
      notes.push(s.placeholders.toLocaleString() + ' deco placeholder'
        + (s.placeholders === 1 ? '' : 's') + ' — these mark where furniture goes'
        + ' and are kept exactly as they are.');
    }
    if (s.procedural) {
      notes.push(s.procedural.toLocaleString() + ' voxel'
        + (s.procedural === 1 ? ' is' : 's are') + ' coloured by the game at runtime'
        + ' (terrain and the like), so repainting them here would have no effect in game.');
    }
    var other = s.locked - s.placeholders;
    if (other > 0) {
      notes.push(other.toLocaleString() + (other === 1 ? ' more voxel uses a material' : ' more voxels use materials')
        + ' outside the modding palette and ' + (other === 1 ? 'is' : 'are') + ' preserved as-is.');
    }
    if (s.entities) {
      notes.push(s.entities.toLocaleString() + ' placed object'
        + (s.entities === 1 ? '' : 's') + ' travel with the model untouched.');
    }
    // The attachment point is where the game grips or seats the model. It is usually
    // OUTSIDE the model for a hat or mask (that gap is the head), which looks wrong
    // until it's explained, so explain it rather than just printing coordinates.
    var a = state.data.attachment;
    if (a) {
      var outside = a[0] < 0 || a[1] < 0 || a[2] < 0
        || a[0] >= state.data.size[0] || a[1] >= state.data.size[1] || a[2] >= state.data.size[2];
      notes.push('Attaches at ' + a.join(', ') + (outside
        ? ' — outside the model, which is right for a hat or mask: the gap is where the head goes.'
        : ' — the pink marker in the view, where the game grips it.'));
    } else if (state.data.version !== 5) {
      notes.push('This is an older blueprint (v' + state.data.version + '), which stores no '
        + 'attachment point, so there is none to show.');
    }
    var box = $('bpe-notes');
    box.innerHTML = notes.map(function (n) { return '<li>' + esc(n) + '</li>'; }).join('');
    box.hidden = !notes.length;
  }

  function renderSelection() {
    var box = $('bpe-selection');
    var i = state.selection;
    renderScopeHint();
    if (i < 0 || !state.data) {
      box.innerHTML = '<p class="bpe-hint">Click a voxel to select it.</p>';
      return;
    }
    var d = state.data;
    // Say which half is unavailable, not just "locked" - a placeable-colour voxel
    // takes a new colour but not a new material, and the two read very differently.
    var note = '';
    if (!d.edit[i] && !d.paint[i]) {
      note = 'The game controls this voxel’s colour and material. It’s saved exactly as it is.';
    } else if (!d.edit[i]) {
      note = 'You can recolour this voxel, but its material is one the game manages and is kept as-is.';
    } else if (!d.paint[i]) {
      note = 'Trove colours this voxel itself, so only its material can be changed here.';
    }
    box.innerHTML =
      '<div class="bpe-selrow"><span class="bpe-swatch" style="background:'
        + hex(d.rgb[i]) + '"></span>'
      + '<div><strong>' + esc(label(i)) + '</strong>'
      + '<span class="bpe-selcoords">' + d.x[i] + ', ' + d.y[i] + ', ' + d.z[i]
      + ' · ' + hex(d.rgb[i]).toUpperCase() + '</span></div></div>'
      + (note
          ? '<p class="bpe-hint bpe-warn"><i class="fa-solid fa-lock" aria-hidden="true"></i> '
            + esc(note) + '</p>'
          : '');
  }

  function updateDirty() {
    var n = editList().length;
    $('bpe-save').disabled = false;
    $('bpe-undo').disabled = !state.history.length;
    $('bpe-revert').disabled = !n;
    if (inProject()) {
      // Across the whole model, not just the part in front of you - the download
      // carries all of it, so the count has to describe all of it.
      var total = 0, parts = 0;
      state.docs.forEach(function (d) {
        var c = editListOf(d).length;
        if (c) { total += c; parts++; }
      });
      $('bpe-dirty').textContent = total
        ? total.toLocaleString() + ' voxel' + (total === 1 ? '' : 's') + ' changed across '
          + parts + ' part' + (parts === 1 ? '' : 's')
        : 'No changes yet';
      return;
    }
    $('bpe-dirty').textContent = n
      ? n.toLocaleString() + ' voxel' + (n === 1 ? '' : 's') + ' changed'
      : 'No changes yet';
  }

  // ---- wiring ------------------------------------------------------------- //

  function ready() {
    var input = $('bpe-input');
    $('bpe-open').addEventListener('click', function () { input.click(); });
    $('bpe-open2').addEventListener('click', function () { input.click(); });
    input.addEventListener('change', function () {
      if (input.files && input.files[0]) openFile(input.files[0]);
      input.value = '';
    });

    var drop = $('bpe-page');
    ['dragenter', 'dragover'].forEach(function (t) {
      drop.addEventListener(t, function (e) { e.preventDefault(); drop.classList.add('bpe-dragging'); });
    });
    ['dragleave', 'drop'].forEach(function (t) {
      drop.addEventListener(t, function (e) { e.preventDefault(); drop.classList.remove('bpe-dragging'); });
    });
    /* Sorting the drop by extension rather than asking is the whole point of a drop
       target, and with model projects there is more to sort: a .tmod or .zip is a whole
       model, several blueprints at once are a model too, and one blueprint dropped onto
       an open model joins it as a part. */
    drop.addEventListener('drop', function (e) {
      var dropped = e.dataTransfer && e.dataTransfer.files;
      if (!dropped || !dropped.length) return;
      var all = Array.prototype.slice.call(dropped);
      var qbs = all.filter(function (f) { return /\.qb$/i.test(f.name); });
      var bps = all.filter(function (f) { return /\.blueprint$/i.test(f.name); });
      var mod = all.find(function (f) { return /\.(tmod|zip)$/i.test(f.name); });
      if (qbs.length) importQb(qbs);
      else if (mod) openModel(mod);
      else if (bps.length && inProject()) addParts(bps);
      else if (bps.length > 1) openModel(bps);
      // A .qbcl is a Qubicle project holding several models, not one grid, so it has
      // nothing to compile from - say that rather than failing as a bad blueprint.
      else if (/\.qbcl$/i.test(all[0].name)) {
        setStatus('That’s a Qubicle project file, which holds several models at once. '
          + 'Export the model as .qb and drop that instead.', 'error');
      } else openFile(all[0]);
    });

    $('bpe-colour').addEventListener('input', function (e) {
      state.paint.rgb = unhex(e.target.value);
      $('bpe-colour-hex').textContent = hex(state.paint.rgb).toUpperCase();
    });

    $('bpe-types').addEventListener('click', function (e) {
      var b = e.target.closest('.bpe-mat');
      if (!b) return;
      state.paint.type = parseInt(b.dataset.type, 10);
      state.paint.w = 0;
      renderPalette();
    });
    $('bpe-finish').addEventListener('click', function (e) {
      var b = e.target.closest('.bpe-w');
      if (!b) return;
      state.paint.w = parseInt(b.dataset.w, 10);
      renderWOptions();
    });

    document.querySelectorAll('[data-tool]').forEach(function (b) {
      b.addEventListener('click', function () {
        state.tool = b.dataset.tool;
        document.querySelectorAll('[data-tool]').forEach(function (o) {
          o.setAttribute('aria-pressed', String(o.dataset.tool === state.tool));
        });
        if (state.scene) state.scene.setDragMode(dragModeFor(state.tool));
        renderToolHint();
      });
    });

    document.querySelectorAll('[data-mode]').forEach(function (b) {
      b.addEventListener('click', function () {
        state.mode = b.dataset.mode;
        document.querySelectorAll('[data-mode]').forEach(function (o) {
          o.setAttribute('aria-pressed', String(o.dataset.mode === state.mode));
        });
      });
    });
    document.querySelectorAll('[data-scope]').forEach(function (b) {
      b.addEventListener('click', function () {
        state.scope = b.dataset.scope;
        document.querySelectorAll('[data-scope]').forEach(function (o) {
          o.setAttribute('aria-pressed', String(o.dataset.scope === state.scope));
        });
        renderScopeHint();
      });
    });

    // "Apply" on a material row is the same edit as clicking one of its voxels,
    // aimed at every voxel that shares the material.
    $('bpe-materials').addEventListener('click', function (e) {
      var b = e.target.closest('.bpe-applyall');
      if (!b || !state.data) return;
      var t = parseInt(b.dataset.type, 10), w = parseInt(b.dataset.w, 10);
      var c = change();
      if (!Object.keys(c).length) return;
      var batch = [], refused = 0;
      for (var i = 0; i < state.data.count; i++) {
        if (state.data.type[i] !== t || state.data.w[i] !== w) continue;
        if (!applyTo(i, c, batch)) refused++;
      }
      commit(batch, refused);
    });

    $('bpe-kind').addEventListener('change', function (e) {
      state.kind = e.target.value;
      staleReport();
    });
    $('bpe-check').addEventListener('click', runCheck);
    $('bpe-report').addEventListener('click', function (e) {
      var b = e.target.closest('.bpe-f-show');
      if (!b || !state.report) return;
      var f = state.report.findings[parseInt(b.dataset.finding, 10)];
      var already = b.classList.contains('active');
      [...document.querySelectorAll('.bpe-f-show')].forEach(function (o) {
        o.classList.remove('active');
      });
      if (already) {
        state.scene && state.scene.clearOverlay('lint');
      } else {
        b.classList.add('active');
        highlightFinding(f);
      }
    });
    $('bpe-show-attach').addEventListener('change', function (e) {
      state.showAttach = e.target.checked;
      drawAttachment();
    });

    var qbInput = $('bpe-qb-input');
    $('bpe-import-qb').addEventListener('click', function () { qbInput.click(); });
    $('bpe-import-qb2').addEventListener('click', function () { qbInput.click(); });
    qbInput.addEventListener('change', function () {
      if (qbInput.files && qbInput.files.length) importQb(qbInput.files);
      qbInput.value = '';
    });
    $('bpe-transforms').addEventListener('click', function (e) {
      var b = e.target.closest('.bpe-xform');
      if (b) runTransform(b.dataset.op);
    });
    var layerInput = $('bpe-layer-input');
    $('bpe-layer-open').addEventListener('click', function () { layerInput.click(); });
    layerInput.addEventListener('change', function () {
      if (layerInput.files && layerInput.files.length) {
        // The same button adds a layer to a stack and a part to a model - which of
        // those you are doing is decided by what is open, not by picking the right one.
        if (inProject()) addParts(layerInput.files);
        else addLayer(layerInput.files[0]);
      }
      layerInput.value = '';
    });

    var search = $('bpe-search');
    if (search) {
      var go = U.debounce(runSearch, 250);
      search.addEventListener('input', go);
      search.addEventListener('search', runSearch);
      $('bpe-searchtype').addEventListener('change', runSearch);
      $('bpe-results').addEventListener('click', function (e) {
        var b = e.target.closest('.bpe-result');
        if (!b || !b.dataset.prefab) return;
        openGameModel(b.dataset.prefab, b.querySelector('.bpe-result-name').textContent);
      });
      // A codex row can name a blueprint the renderer has nothing to draw for, and a
      // broken-image box is worse than none. Captured, because `error` doesn't bubble.
      $('bpe-results').addEventListener('error', function (e) {
        if (e.target && e.target.tagName === 'IMG') e.target.remove();
      }, true);
    }

    var modelInput = $('bpe-model-input');
    ['bpe-open-model', 'bpe-open-model2'].forEach(function (id) {
      var b = $(id);
      if (b) b.addEventListener('click', function () { modelInput.click(); });
    });
    modelInput.addEventListener('change', function () {
      var picked = Array.prototype.slice.call(modelInput.files || []);
      if (picked.length) {
        var mod = picked.find(function (f) { return /\.(tmod|zip)$/i.test(f.name); });
        openModel(mod || picked);
      }
      modelInput.value = '';
    });
    $('bpe-save-part').addEventListener('click', save);

    // Double-click another part to work on it - Qubicle's gesture, and the one that
    // makes a sixteen-part creature a single workspace rather than sixteen files.
    $('bpe-stage').addEventListener('dblclick', function (e) {
      if (!inProject() || !state.scene) return;
      var id = state.scene.pickLayer(e);
      if (!id) return;
      for (var i = 0; i < state.docs.length; i++) {
        if (state.docs[i].id === id) {
          setActive(i);
          setStatus('Editing ' + state.docs[i].payload.name + '.', 'ok');
          return;
        }
      }
    });
    $('bpe-flatten').addEventListener('click', flattenStack);
    $('bpe-isolate').addEventListener('click', function () {
      state.isolate = !state.isolate;
      renderLayers();
      drawStack();
    });
    $('bpe-layerlist').addEventListener('click', function (e) {
      var b = e.target.closest('button[data-pick],button[data-vis],button[data-up],' +
                               'button[data-down],button[data-drop],button[data-anchor]');
      if (!b) return;
      var ds = b.dataset;
      if (ds.pick !== undefined) return setActive(+ds.pick);
      if (ds.anchor !== undefined) return setAnchor(+ds.anchor);
      if (ds.vis !== undefined) {
        var d = doc(+ds.vis);
        if (d) { d.visible = !d.visible; renderLayers(); drawStack(); }
        return;
      }
      if (ds.up !== undefined) return reorderLayer(+ds.up, 1);
      if (ds.down !== undefined) return reorderLayer(+ds.down, -1);
      if (ds.drop !== undefined) {
        return inProject() ? removePart(+ds.drop) : removeLayer(+ds.drop);
      }
    });
    // Answering "where does this part attach". The options are the rig's own sockets,
    // so the only thing that can be picked is a bone the skeleton actually has.
    // The shared dropdown copies the select's classes onto its trigger button, so match
    // the element that actually carries the value and the index.
    $('bpe-layerlist').addEventListener('change', function (e) {
      var sel = e.target.closest('select.bpe-apsel');
      if (sel) setPartAp(+sel.dataset.ap, sel.value);
    });
    $('bpe-align').addEventListener('change', function (e) {
      var d = active();
      if (!d) return;
      d.mode = e.target.value;
      d.offset = [0, 0, 0];
      renderLayers();
      drawStack();
    });
    $('bpe-nudge').addEventListener('click', function (e) {
      var b = e.target.closest('[data-nudge]');
      if (!b) return;
      var v = b.dataset.nudge.split(',').map(Number);
      nudgeActive(v[0], v[1], v[2]);
    });
    $('bpe-export-qb').addEventListener('click', exportQb);

    $('bpe-undo').addEventListener('click', undo);
    $('bpe-revert').addEventListener('click', revertAll);
    $('bpe-save').addEventListener('click', function () {
      if (inProject()) saveModel(); else save();
    });

    document.addEventListener('keydown', function (e) {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'z' && state.data) {
        e.preventDefault(); undo();
      }
    });
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', ready);
  } else {
    ready();
  }
})();
