/* Embeddable viewer (/embed/viewer) — the thin orchestrator around the three
   viewers a partner site can iframe.

   It does not render anything itself. It asks the server what this source can show
   (/site/embed/manifest), decides which viewer that means, and mounts it:

     blueprint  -> BlueprintViewer.mount   one .blueprint model
     assembled  -> ModelViewer.mount       the whole creature on its rig, with clips
     vfx        -> PkfxViewer.mount        a .pkfx particle effect

   The source is whatever the page was opened with - a Mods Hub release, an uploaded
   .tmod token, or a path in the game files - and is passed straight through as the
   query string, so this file never needs to know which one it is. */
(function () {
  'use strict';

  var shell = document.querySelector('.kv-shell');
  if (!shell) return;

  var els = {
    title: shell.querySelector('.kv-title'),
    tabs: shell.querySelector('.kv-tabs'),
    pick: shell.querySelector('.kv-pick'),
    select: shell.querySelector('.kv-select'),
    meta: shell.querySelector('.kv-meta'),
    stage: shell.querySelector('.kv-stage'),
    bar: shell.querySelector('.kv-bar'),
    hint: shell.querySelector('.kv-hint'),
  };

  var cfg = {
    path: shell.dataset.path || '',
    mode: shell.dataset.mode || 'auto',
  };

  // The one source param the page was opened with, re-encoded for our own fetches.
  var srcQuery = ['release', 'tmod', 'game'].reduce(function (acc, key) {
    var v = shell.dataset[key];
    return acc || (v ? key + '=' + encodeURIComponent(v) : '');
  }, '');

  var HINTS = {
    blueprint: 'Drag to rotate · scroll to zoom · right-drag to pan',
    assembled: 'Drag to rotate · scroll to zoom · pick a clip below',
    vfx: 'Drag to orbit · scroll to zoom',
  };
  var LABELS = { blueprint: 'Model', assembled: 'Creature', vfx: 'Effect' };

  var state = { manifest: null, mode: null, path: null, viewer: null };

  function api(endpoint, extra) {
    return '/site/embed/' + endpoint + '?' + srcQuery + (extra ? '&' + extra : '');
  }

  function message(text, isError) {
    disposeViewer();
    els.stage.textContent = '';
    els.stage.className = 'kv-stage';           // drop whatever the last viewer added
    var p = document.createElement('p');
    p.className = 'kv-msg' + (isError ? ' kv-error' : '');
    p.textContent = text;
    els.stage.appendChild(p);
  }

  function disposeViewer() {
    if (state.viewer) {
      try { state.viewer.dispose(); } catch (e) { /* already gone */ }
      state.viewer = null;
    }
    els.bar.textContent = '';
    els.bar.hidden = true;
    els.meta.textContent = '';
  }

  /* PkfxViewer is an ES module, so it can land after this deferred script. Wait a
     beat for it rather than telling the visitor the effect is unavailable. */
  function whenPkfxReady() {
    if (window.PkfxViewer) return Promise.resolve(window.PkfxViewer);
    return new Promise(function (resolve, reject) {
      var tries = 0;
      var t = setInterval(function () {
        if (window.PkfxViewer) { clearInterval(t); resolve(window.PkfxViewer); }
        else if (++tries > 60) { clearInterval(t); reject(new Error('The VFX viewer could not start.')); }
      }, 50);
    });
  }

  // ── what this source can show ────────────────────────────────────────────

  function modelItems(man) {
    /* When the parts assemble into a creature we list only the standalone
       blueprints - the component parts are reachable through the Creature tab, and
       showing "dragon_leg_l_01" next to the whole dragon is noise. */
    var items = (man.blueprints && man.blueprints.items) || [];
    if (!man.blueprints || !man.blueprints.rig) return items;
    var standalone = items.filter(function (i) { return !i.assembled; });
    return standalone.length ? standalone : items;
  }

  function availableModes(man) {
    var modes = [];
    if (man.blueprints && man.blueprints.rig) modes.push('assembled');
    if (modelItems(man).length) modes.push('blueprint');
    if (man.vfx && man.vfx.items.length) modes.push('vfx');
    return modes;
  }

  function initialMode(man, modes) {
    if (cfg.mode !== 'auto' && modes.indexOf(cfg.mode) >= 0) return cfg.mode;
    // An explicit ?path= tells us what the embedder actually wanted to show.
    if (cfg.path) {
      var wanted = /\.pkfx$/i.test(cfg.path) ? 'vfx' : 'blueprint';
      if (modes.indexOf(wanted) >= 0) return wanted;
    }
    return modes[0] || null;
  }

  function itemsFor(mode) {
    if (mode === 'vfx') return state.manifest.vfx.items;
    if (mode === 'blueprint') return modelItems(state.manifest);
    return [];
  }

  function defaultPath(mode) {
    var items = itemsFor(mode);
    if (!items.length) return null;
    if (cfg.path) {
      var hit = items.filter(function (i) {
        return i.path.toLowerCase() === cfg.path.toLowerCase()
          || i.path.toLowerCase().split('/').pop() === cfg.path.toLowerCase().split('/').pop();
      })[0];
      if (hit) return hit.path;
    }
    return items[0].path;
  }

  // ── chrome ───────────────────────────────────────────────────────────────

  function paintTabs(modes) {
    els.tabs.textContent = '';
    if (modes.length < 2) { els.tabs.hidden = true; return; }
    els.tabs.hidden = false;
    modes.forEach(function (mode) {
      var b = document.createElement('button');
      b.type = 'button';
      b.className = 'kv-tab';
      b.setAttribute('role', 'tab');
      b.setAttribute('aria-selected', String(mode === state.mode));
      b.setAttribute('aria-controls', 'kv-stage');   // one panel, re-rendered per tab
      b.textContent = LABELS[mode];
      b.addEventListener('click', function () { show(mode, null); });
      els.tabs.appendChild(b);
    });
  }

  function paintPicker() {
    var items = itemsFor(state.mode);
    els.select.textContent = '';
    if (items.length < 2) { els.pick.hidden = true; return; }
    els.pick.hidden = false;
    items.forEach(function (item) {
      var o = document.createElement('option');
      o.value = item.path;
      o.textContent = item.path.split('/').pop();
      o.selected = item.path === state.path;
      els.select.appendChild(o);
    });
  }

  els.select.addEventListener('change', function () {
    show(state.mode, els.select.value);
  });

  // ── mounting ─────────────────────────────────────────────────────────────

  function show(mode, path) {
    state.mode = mode;
    state.path = path || defaultPath(mode);
    disposeViewer();
    els.stage.textContent = '';
    els.stage.className = 'kv-stage';
    els.hint.textContent = HINTS[mode] || '';
    paintTabs(availableModes(state.manifest));
    paintPicker();

    var onMeta = function (text) { els.meta.textContent = text; };

    if (mode === 'assembled') {
      if (!window.ModelViewer) return message('The 3D viewer could not start.', true);
      els.bar.hidden = false;
      state.viewer = window.ModelViewer.mount(els.stage, {
        url: api('assembled'), bar: els.bar, onMeta: onMeta,
      });
      return;
    }

    if (mode === 'blueprint') {
      if (!window.BlueprintViewer) return message('The 3D viewer could not start.', true);
      state.viewer = window.BlueprintViewer.mount(els.stage, {
        url: api('blueprint', 'path=' + encodeURIComponent(state.path || '')),
        onMeta: onMeta,
      });
      return;
    }

    if (mode === 'vfx') {
      var stage = els.stage;
      whenPkfxReady().then(function (Pkfx) {
        if (state.mode !== 'vfx') return;        // switched tabs while loading
        state.viewer = Pkfx.mount(stage, {
          path: state.path,
          endpoint: { base: '/site/embed/vfx', query: srcQuery },
        });
      }).catch(function (e) { message(e.message, true); });
      return;
    }

    message('Nothing to preview here.');
  }

  // ── boot ─────────────────────────────────────────────────────────────────

  if (!srcQuery) {
    message('This embed is missing its source. Add a release, tmod or game parameter.', true);
    els.title.textContent = 'Nothing to preview';
    return;
  }

  fetch(api('manifest'), { credentials: 'same-origin' }).then(function (r) {
    return r.json().then(function (body) {
      if (!r.ok) {
        throw new Error((body && body.error && body.error.message)
          || 'This preview could not be loaded.');
      }
      return body;
    }, function () { throw new Error('This preview could not be loaded.'); });
  }).then(function (man) {
    state.manifest = man;
    els.title.textContent = man.title || 'Trove preview';
    document.title = (man.title || 'Trove preview') + ' — Kiwi';
    var modes = availableModes(man);
    if (!modes.length) {
      message('This mod has no 3D models or effects to preview.');
      return;
    }
    show(initialMode(man, modes), null);
  }).catch(function (err) {
    els.title.textContent = 'Preview unavailable';
    message(err.message || 'This preview could not be loaded.', true);
  });
})();
