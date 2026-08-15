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
    report: null,        // last check result
    showAttach: true,
  };

  var ATTACH_COLOUR = 0xff3fd5;   // the pink Trove modders mark attachment points in
  var FINDING_COLOUR = { error: 0xff5555, warning: 0xffc857, info: 0x58a6ff };

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

  function loaded(file, payload, note) {
    state.file = file;
    state.data = payload;
    // Only colour and material can change on an existing voxel, so those are the only
    // arrays worth a pristine copy - the edit list is DERIVED by diffing against it,
    // which means a voxel painted back to how it started stops counting as an edit
    // without anyone having to notice.
    state.base = {
      rgb: payload.rgb.slice(), type: payload.type.slice(), w: payload.w.slice(),
    };
    // `live` is how erase works: a voxel is dropped from the render and the save
    // rather than spliced out of a dozen parallel arrays. `origin` maps each row back
    // to its index in the file, or -1 for one the user placed.
    payload.live = [];
    state.origin = [];
    for (var i = 0; i < payload.count; i++) {
      payload.live.push(1);
      state.origin.push(i);
    }
    state.history = [];
    state.selection = -1;
    rebuildIndex();

    state.report = null;
    $('bpe-report').hidden = true;
    $('bpe-empty').hidden = true;
    $('bpe-workspace').hidden = false;
    renderKinds();
    renderTransforms();
    renderToolHint();
    renderPalette();
    renderMaterialList();
    renderMeta();
    renderSelection();
    updateDirty();
    setStatus(note || '', note ? 'ok' : '');

    if (state.scene) { state.scene.dispose(); state.scene = null; }
    window.VoxelScene.create({
      stage: $('bpe-stage'),
      data: liveView(),
      name: payload.name,
      onPick: onPick,
      onHover: onHover,
    }).then(function (scene) {
      state.scene = scene;
      drawAttachment();
    }).catch(function (err) {
      setStatus(err.message || 'The 3D view couldn’t start.', 'error');
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
  function editList() {
    var d = state.data, b = state.base, out = [];
    for (var i = 0; i < d.count; i++) {
      var orig = state.origin[i];
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
    fd.append('file', state.file, state.file.name);
    fd.append('edits', JSON.stringify(list));
    fd.append('kind', state.kind);
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
      state.history.push(batch);
      rebuildIndex();
      if (state.scene) state.scene.rebuild(liveView());
      renderMaterialList();
      renderMeta();
      updateDirty();
      staleReport();
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

  function onPick(hit) {
    if (!hit || !state.data) return;
    var i = state.index.get(hit.x + ',' + hit.y + ',' + hit.z);
    if (i === undefined) return;
    var batch = [], refused = 0;

    if (state.tool === 'add') {
      // The face normal says which side was clicked, so the new voxel goes on the
      // outside of it - the gesture every voxel editor uses.
      addVoxel(hit.x + hit.nx, hit.y + hit.ny, hit.z + hit.nz, batch);
      commit(batch, 0);
      state.selection = state.index.get(
        (hit.x + hit.nx) + ',' + (hit.y + hit.ny) + ',' + (hit.z + hit.nz));
      if (state.selection === undefined) state.selection = -1;
      renderSelection();
      return;
    }

    if (state.tool === 'erase') {
      if (state.scope === 'material') {
        var et = state.data.type[i], ew = state.data.w[i];
        for (var k = 0; k < state.data.count; k++) {
          if (state.data.live[k] && state.data.type[k] === et && state.data.w[k] === ew) {
            eraseVoxel(k, batch);
          }
        }
      } else {
        eraseVoxel(i, batch);
      }
      commit(batch, 0);
      state.selection = -1;
      renderSelection();
      return;
    }

    state.selection = i;
    renderSelection();

    var c = change();
    if (!Object.keys(c).length) return;

    if (state.scope === 'material') {
      var t = state.data.type[i], w = state.data.w[i];
      for (var j = 0; j < state.data.count; j++) {
        if (!state.data.live[j]) continue;
        if (state.data.type[j] !== t || state.data.w[j] !== w) continue;
        if (!applyTo(j, c, batch)) refused++;
      }
    } else if (!applyTo(i, c, batch)) {
      refused = 1;
    }
    commit(batch, refused);
    renderSelection();
  }

  function onHover(hit) {
    var el = $('bpe-hover');
    if (!el) return;
    if (!hit || !state.data) { el.textContent = ''; return; }
    var i = state.index.get(hit.x + ',' + hit.y + ',' + hit.z);
    if (i === undefined) { el.textContent = ''; return; }
    el.textContent = hit.x + ', ' + hit.y + ', ' + hit.z + ' · ' + label(i);
  }

  function undo() {
    var batch = state.history.pop();
    if (!batch) return;
    var d = state.data;
    batch.forEach(function (rec) {
      var i = rec.index;
      if (rec.added) {
        // A row the user placed is retired rather than spliced out: removing it would
        // renumber every row after it, and a dead row costs nothing.
        d.live[i] = 0;
        return;
      }
      d.rgb[i] = rec.rgb; d.type[i] = rec.type; d.w[i] = rec.w; d.live[i] = rec.live;
      reshade(i);
    });
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
    fd.append('file', state.file, state.file.name);
    fd.append('edits', JSON.stringify(list));
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
        openFile(new File([out.blob], state.file.name), 'Applied ' + bits.join(' · ') + '.');
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
    fd.append('file', state.file, state.file.name);
    fd.append('edits', JSON.stringify(list));
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
    staff: 'Staff', bow: 'Bow', spear: 'Spear', mask: 'Mask / face',
    hat: 'Hat', hair: 'Hair', deco: 'Decoration',
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

  /* The paint controls only govern a paint click, and Add uses them for the new voxel -
     so Erase greys them out rather than leaving a colour picker that does nothing. */
  function renderToolHint() {
    var hints = {
      paint: 'Click a voxel to paint it.',
      add: 'Click a face to put a new voxel against it, in the colour and material above.',
      erase: 'Click a voxel to delete it.',
    };
    $('bpe-toolhint').textContent = hints[state.tool] || hints.paint;
    var faded = state.tool === 'erase';
    ['bpe-paint-colour', 'bpe-paint-material'].forEach(function (id) {
      var el = $(id);
      if (el) el.classList.toggle('bpe-faded', faded);
    });
    // "A click changes" only means anything while painting.
    var modes = $('bpe-modes');
    if (modes) modes.classList.toggle('bpe-faded', state.tool !== 'paint');
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
      if (e) { e.count++; } else { seen.set(key, { type: d.type[i], w: d.w[i], count: 1, i: i }); }
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
      return '<li class="bpe-matrow' + (locked ? ' bpe-locked' : '') + '">'
        + '<span class="bpe-swatch" style="background:' + hex(d.rgb[r.i]) + '"></span>'
        + '<span class="bpe-matname">' + esc(label(r.i)) + '</span>'
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
    $('bpe-filename').textContent = state.data.name;

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
    // Dropping .qb files compiles them; dropping a .blueprint opens it. Sorting by
    // extension rather than asking is the whole point of a drop target.
    drop.addEventListener('drop', function (e) {
      var dropped = e.dataTransfer && e.dataTransfer.files;
      if (!dropped || !dropped.length) return;
      var qbs = Array.prototype.slice.call(dropped).filter(function (f) {
        return /\.qb$/i.test(f.name);
      });
      if (qbs.length) importQb(qbs);
      else openFile(dropped[0]);
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
    $('bpe-export-qb').addEventListener('click', exportQb);

    $('bpe-undo').addEventListener('click', undo);
    $('bpe-revert').addEventListener('click', revertAll);
    $('bpe-save').addEventListener('click', save);

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
