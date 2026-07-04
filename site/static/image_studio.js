/* ═══════════════════════════════════════════════════════════════════════
   Image Studio - freeform, server-rendered image designer.
   ───────────────────────────────────────────────────────────────────────
   A canvas + draggable layers (text / rect / image) over a background, with
   a live server preview (POST /v1/images/preview), variable binding to an
   event type, image uploads, and save/download. Designs render to a stable
   PNG URL usable standalone or as a customizable embed's image.

   window.ImageStudio.mount(container)
   ═══════════════════════════════════════════════════════════════════════ */
(function () {
  'use strict';

  const Auth = window.BTTAuth;
  const t = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);
  const esc = (s) => String(s == null ? '' : s).replace(/[&<>"']/g, (c) =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const clone = (o) => JSON.parse(JSON.stringify(o));
  const el = (tag, cls, html) => { const e = document.createElement(tag); if (cls) e.className = cls; if (html != null) e.innerHTML = html; return e; };

  const MAXW = 560;   // canvas display width cap (px)
  let host, S;

  function mount(container) {
    host = container;
    S = { designs: [], bindings: {}, bindingList: [], current: null, selected: -1, dirty: false, previewT: 0 };
    host.innerHTML = `<p class="dash-loading">${esc(t('Loading…'))}</p>`;
    loadAll();
  }

  async function loadAll() {
    const [list, binds] = await Promise.all([
      Auth.callJSON('/v1/images'), Auth.callJSON('/v1/images/bindings'),
    ]);
    if (binds.ok && binds.data) {
      S.bindingList = binds.data.bindings || [];
      S.bindingList.forEach((b) => { S.bindings[b.key] = b; });
    }
    if (!list.ok) { host.innerHTML = `<article class="dash-card"><p class="dash-error">${esc(Auth.errorMessage(list.data) || t('Failed to load'))}</p></article>`; return; }
    S.designs = (list.data && list.data.items) || [];
    renderShell();
  }

  function newDesign() {
    return {
      id: null, name: '', width: 600, height: 240, bind_type: null,
      background: { type: 'gradient', color1: '#1a1a2e', color2: '#16213e', angle: 90, image_sha: null, fit: 'cover' },
      layers: [
        { type: 'rect', x: 0, y: 0, w: 8, h: 240, color: '#F2A33C', radius: 0, opacity: 1 },
        { type: 'text', x: 40, y: 40, text: 'My title', font_size: 48, color: '#ffffff', bold: true, align: 'left', max_width: null, opacity: 1 },
      ],
    };
  }

  // ── shell: sidebar + main ─────────────────────────────────────────────
  function renderShell() {
    host.innerHTML = `<div class="is-wrap">
      <aside class="is-side">
        <button type="button" class="dash-btn dash-btn-mini" id="is-new">+ ${esc(t('New design'))}</button>
        <div class="is-list" id="is-list"></div>
      </aside>
      <div class="is-main" id="is-main"></div>
    </div>`;
    document.getElementById('is-new').addEventListener('click', () => {
      S.current = newDesign(); S.selected = 1; S.dirty = true; renderList(); renderEditor();
    });
    renderList();
    const main = document.getElementById('is-main');
    if (S.current) renderEditor();
    else main.innerHTML = `<p class="dash-empty">${esc(t('Pick a design on the left, or create a new one.'))}</p>`;
  }

  function renderList() {
    const box = document.getElementById('is-list');
    if (!box) return;
    if (!S.designs.length) { box.innerHTML = `<p class="dash-card-sub-mini">${esc(t('No designs yet.'))}</p>`; return; }
    box.innerHTML = S.designs.map((d) => {
      const active = S.current && S.current.id === d.id ? ' is-active' : '';
      const thumb = d.render_url ? `${d.render_url}?v=${encodeURIComponent(d.updated_at || '')}` : '';
      return `<button type="button" class="is-card${active}" data-id="${esc(d.id)}">
        ${thumb ? `<img src="${esc(thumb)}" alt="" loading="lazy">` : ''}
        <span>${esc(d.name || t('Untitled'))}</span>
      </button>`;
    }).join('');
    box.querySelectorAll('.is-card').forEach((b) =>
      b.addEventListener('click', () => selectDesign(b.dataset.id)));
  }

  function selectDesign(id) {
    const d = S.designs.find((x) => x.id === id);
    if (!d) return;
    S.current = clone(d); S.selected = -1; S.dirty = false;
    renderList(); renderEditor();
  }

  // ── editor ────────────────────────────────────────────────────────────
  function renderEditor() {
    const main = document.getElementById('is-main');
    const d = S.current;
    main.innerHTML = `
      <div class="is-toolbar">
        <input type="text" class="ee-input is-name" id="is-name" placeholder="${esc(t('Design name'))}" value="${esc(d.name || '')}">
        <button type="button" class="dash-btn dash-btn-mini" id="is-save">${esc(t('Save'))}</button>
        ${d.id ? `<button type="button" class="dash-btn dash-btn-mini dash-btn-ghost" id="is-download">${esc(t('Download'))}</button>
        <button type="button" class="dash-btn dash-btn-mini dash-btn-ghost" id="is-copy">${esc(t('Copy URL'))}</button>
        <button type="button" class="dash-btn dash-btn-mini dash-btn-danger" id="is-del">${esc(t('Delete'))}</button>` : ''}
        <span class="wh-cust-msg" id="is-msg"></span>
      </div>
      <div class="is-editor">
        <div class="is-canvas-col">
          <div class="is-canvas" id="is-canvas"></div>
          <p class="is-canvas-hint">${esc(t('Drag the dashed handles to position layers.'))}</p>
        </div>
        <div class="is-panels" id="is-panels"></div>
      </div>
      <div class="is-livepreview">
        <p class="is-section-h">${esc(t('Live preview'))}</p>
        <img class="is-live" id="is-live" alt="${esc(t('Live preview'))}">
      </div>`;
    document.getElementById('is-name').addEventListener('input', (e) => { d.name = e.target.value; S.dirty = true; });
    document.getElementById('is-save').addEventListener('click', save);
    if (d.id) {
      document.getElementById('is-download').addEventListener('click', () => window.open(d.render_url + '?v=' + Date.now(), '_blank'));
      document.getElementById('is-copy').addEventListener('click', () => copyText(d.render_url));
      document.getElementById('is-del').addEventListener('click', del);
    }
    renderCanvas();
    renderPanels();
    schedulePreview();
  }

  function scale() { return Math.min(MAXW / S.current.width, 1); }

  function renderCanvas() {
    const c = document.getElementById('is-canvas');
    const d = S.current, sc = scale();
    c.style.width = Math.round(d.width * sc) + 'px';
    c.style.height = Math.round(d.height * sc) + 'px';
    c.innerHTML = `<img class="is-preview" id="is-preview" alt="">`;
    d.layers.forEach((ly, i) => {
      const h = el('div', 'is-handle' + (i === S.selected ? ' is-sel' : ''));
      h.dataset.i = i;
      positionHandle(h, ly, sc);
      h.title = ly.type;
      c.appendChild(h);
      h.addEventListener('pointerdown', (e) => startDrag(e, i));
    });
  }

  function positionHandle(h, ly, sc) {
    h.style.left = (ly.x * sc) + 'px';
    h.style.top = (ly.y * sc) + 'px';
    if (ly.type === 'text') {
      h.style.width = ((ly.max_width || 160) * sc) + 'px';
      h.style.height = ((ly.font_size + 8) * sc) + 'px';
      h.classList.add('is-handle-text');
    } else {
      h.style.width = (ly.w * sc) + 'px';
      h.style.height = (ly.h * sc) + 'px';
    }
  }

  function startDrag(e, i) {
    e.preventDefault();
    S.selected = i; renderPanels();
    document.querySelectorAll('.is-handle').forEach((x) => x.classList.toggle('is-sel', +x.dataset.i === i));
    const ly = S.current.layers[i], sc = scale();
    const sx = e.clientX, sy = e.clientY, ox = ly.x, oy = ly.y;
    const move = (ev) => {
      ly.x = Math.round(ox + (ev.clientX - sx) / sc);
      ly.y = Math.round(oy + (ev.clientY - sy) / sc);
      const h = document.querySelector('.is-handle[data-i="' + i + '"]');
      if (h) positionHandle(h, ly, sc);
      syncLayerInputs(ly);
      S.dirty = true;
    };
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      schedulePreview();
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }

  function syncLayerInputs(ly) {
    const px = document.getElementById('is-f-x'), py = document.getElementById('is-f-y');
    if (px) px.value = ly.x; if (py) py.value = ly.y;
  }

  // ── panels: size, background, layers, selected props, binding ─────────
  function renderPanels() {
    const p = document.getElementById('is-panels');
    const d = S.current;
    p.innerHTML = '';
    p.appendChild(sizeBindPanel(d));
    p.appendChild(bgPanel(d));
    p.appendChild(layersPanel(d));
    if (S.selected >= 0 && d.layers[S.selected]) p.appendChild(layerPanel(d.layers[S.selected]));
    p.appendChild(palettePanel(d));
  }

  function section(title) { const s = el('div', 'is-section'); s.appendChild(el('p', 'is-section-h', esc(title))); return s; }
  function field(label, ctrl) { const w = el('label', 'ee-field'); w.appendChild(el('span', 'ee-label', esc(label))); w.appendChild(ctrl); return w; }
  function num(val, on, id) { const i = el('input', 'ee-input'); i.type = 'number'; i.value = val == null ? '' : val; if (id) i.id = id; i.addEventListener('input', () => { on(i.value === '' ? null : Number(i.value)); markPreview(); }); return i; }
  function txt(val, on, id) { const i = el('input', 'ee-input'); i.type = 'text'; i.value = val || ''; if (id) i.id = id; i.addEventListener('input', () => { on(i.value); markPreview(); }); return i; }
  function area(val, on, id) { const a = el('textarea', 'ee-input ee-textarea'); a.value = val || ''; if (id) a.id = id; a.addEventListener('input', () => { on(a.value); markPreview(); }); return a; }
  function color(val, on) { const r = el('div', 'ee-color-row'); const sw = el('input', 'ee-color'); sw.type = 'color'; sw.value = /^#[0-9a-f]{6}$/i.test(val || '') ? val : '#000000'; const tx = txt(val, (v) => { on(v); if (/^#[0-9a-f]{6}$/i.test(v)) sw.value = v; }); sw.addEventListener('input', () => { tx.value = sw.value; on(sw.value); markPreview(); }); r.appendChild(sw); r.appendChild(tx); return r; }
  function sel(val, opts, on) { const s = el('select', 'ee-input'); s.innerHTML = opts.map((o) => `<option value="${esc(o[0])}" ${o[0] === val ? 'selected' : ''}>${esc(o[1])}</option>`).join(''); s.addEventListener('change', () => { on(s.value); markPreview(); }); return s; }
  function check(label, val, on) { const w = el('label', 'ee-check'); const c = el('input'); c.type = 'checkbox'; c.checked = !!val; c.addEventListener('change', () => { on(c.checked); markPreview(); }); w.appendChild(c); w.appendChild(el('span', null, esc(label))); return w; }

  function sizeBindPanel(d) {
    const s = section(t('Canvas'));
    const row = el('div', 'is-row');
    row.appendChild(field(t('Width'), num(d.width, (v) => { d.width = v || 1; renderCanvas(); markPreview(); })));
    row.appendChild(field(t('Height'), num(d.height, (v) => { d.height = v || 1; renderCanvas(); markPreview(); })));
    s.appendChild(row);
    const opts = [['', t('None (static)')]].concat(S.bindingList.map((b) => [b.key, b.label]));
    s.appendChild(field(t('Fill variables from'), sel(d.bind_type || '', opts, (v) => { d.bind_type = v || null; renderPanels(); markPreview(); })));
    return s;
  }

  function bgPanel(d) {
    const s = section(t('Background'));
    s.appendChild(field(t('Type'), sel(d.background.type, [['solid', t('Solid')], ['gradient', t('Gradient')], ['image', t('Image')]], (v) => { d.background.type = v; renderPanels(); markPreview(); })));
    s.appendChild(field(d.background.type === 'gradient' ? t('Color 1') : t('Color'), color(d.background.color1, (v) => { d.background.color1 = v; })));
    if (d.background.type === 'gradient') {
      s.appendChild(field(t('Color 2'), color(d.background.color2, (v) => { d.background.color2 = v; })));
      s.appendChild(field(t('Angle'), num(d.background.angle, (v) => { d.background.angle = v || 0; })));
    }
    if (d.background.type === 'image') {
      s.appendChild(field(t('Fit'), sel(d.background.fit, [['cover', 'cover'], ['contain', 'contain'], ['stretch', 'stretch']], (v) => { d.background.fit = v; })));
      s.appendChild(uploadBtn(t('Upload background'), (sha) => { d.background.image_sha = sha; markPreview(); }));
    }
    return s;
  }

  function layersPanel(d) {
    const s = section(t('Layers'));
    const list = el('div', 'is-layers');
    d.layers.forEach((ly, i) => {
      const row = el('div', 'is-layer' + (i === S.selected ? ' is-sel' : ''));
      const label = ly.type === 'text' ? ('“' + (ly.text || '').slice(0, 16) + '”') : ly.type;
      row.appendChild(el('span', 'is-layer-name', esc((ly.type === 'text' ? 'T ' : ly.type === 'rect' ? '▭ ' : '🖼 ') + label)));
      const up = el('button', 'ee-btn ee-btn-ghost', '↑'); up.type = 'button'; up.addEventListener('click', () => moveLayer(i, -1));
      const dn = el('button', 'ee-btn ee-btn-ghost', '↓'); dn.type = 'button'; dn.addEventListener('click', () => moveLayer(i, 1));
      const rm = el('button', 'ee-btn-x', '✕'); rm.type = 'button'; rm.addEventListener('click', () => { d.layers.splice(i, 1); S.selected = -1; renderCanvas(); renderPanels(); markPreview(); });
      row.appendChild(up); row.appendChild(dn); row.appendChild(rm);
      row.addEventListener('click', (e) => { if (e.target.tagName !== 'BUTTON') { S.selected = i; renderCanvas(); renderPanels(); } });
      list.appendChild(row);
    });
    s.appendChild(list);
    const add = el('div', 'ee-ctrls');
    add.appendChild(addBtn('+ ' + t('Text'), () => addLayer({ type: 'text', x: 20, y: 20, text: 'Text', font_size: 28, color: '#ffffff', bold: false, align: 'left', max_width: null, opacity: 1 })));
    add.appendChild(addBtn('+ ' + t('Box'), () => addLayer({ type: 'rect', x: 20, y: 20, w: 120, h: 40, color: '#5865F2', radius: 6, opacity: 1 })));
    add.appendChild(addBtn('+ ' + t('Image'), () => addLayer({ type: 'image', x: 20, y: 20, w: 120, h: 120, radius: 0, image_sha: null, opacity: 1 })));
    s.appendChild(add);
    return s;
  }

  function addBtn(label, on) { const b = el('button', 'ee-btn ee-btn-ghost', esc(label)); b.type = 'button'; b.addEventListener('click', on); return b; }
  function addLayer(ly) { S.current.layers.push(ly); S.selected = S.current.layers.length - 1; renderCanvas(); renderPanels(); markPreview(); }
  function moveLayer(i, dir) { const a = S.current.layers; const j = i + dir; if (j < 0 || j >= a.length) return; [a[i], a[j]] = [a[j], a[i]]; S.selected = j; renderCanvas(); renderPanels(); markPreview(); }

  function layerPanel(ly) {
    const s = section(t('Selected layer'));
    const row = el('div', 'is-row');
    row.appendChild(field('X', num(ly.x, (v) => { ly.x = v || 0; repositionSel(); }, 'is-f-x')));
    row.appendChild(field('Y', num(ly.y, (v) => { ly.y = v || 0; repositionSel(); }, 'is-f-y')));
    s.appendChild(row);
    if (ly.type === 'text') {
      s.appendChild(field(t('Text'), area(ly.text, (v) => { ly.text = v; })));
      const r2 = el('div', 'is-row');
      r2.appendChild(field(t('Size'), num(ly.font_size, (v) => { ly.font_size = v || 12; repositionSel(); })));
      r2.appendChild(field(t('Wrap width'), num(ly.max_width, (v) => { ly.max_width = v; repositionSel(); })));
      s.appendChild(r2);
      s.appendChild(field(t('Color'), color(ly.color, (v) => { ly.color = v; })));
      const r3 = el('div', 'is-row');
      r3.appendChild(field(t('Align'), sel(ly.align, [['left', t('Left')], ['center', t('Center')], ['right', t('Right')]], (v) => { ly.align = v; })));
      r3.appendChild(check(t('Bold'), ly.bold, (v) => { ly.bold = v; }));
      s.appendChild(r3);
    } else {
      const r2 = el('div', 'is-row');
      r2.appendChild(field('W', num(ly.w, (v) => { ly.w = v || 1; repositionSel(); })));
      r2.appendChild(field('H', num(ly.h, (v) => { ly.h = v || 1; repositionSel(); })));
      s.appendChild(r2);
      s.appendChild(field(t('Corner radius'), num(ly.radius, (v) => { ly.radius = v || 0; })));
      if (ly.type === 'rect') s.appendChild(field(t('Color'), color(ly.color, (v) => { ly.color = v; })));
      if (ly.type === 'image') s.appendChild(uploadBtn(t('Upload image'), (sha) => { ly.image_sha = sha; markPreview(); }));
    }
    s.appendChild(field(t('Opacity'), num(ly.opacity, (v) => { ly.opacity = v == null ? 1 : v; })));
    return s;
  }

  function repositionSel() { const ly = S.current.layers[S.selected]; const h = document.querySelector('.is-handle[data-i="' + S.selected + '"]'); if (h && ly) positionHandle(h, ly, scale()); markPreview(); }

  function palettePanel(d) {
    const s = section(t('Variables'));
    const meta = d.bind_type && S.bindings[d.bind_type];
    if (!meta) { s.appendChild(el('p', 'dash-card-sub-mini', esc(t('Bind to an event type above to insert live variables.')))); return s; }
    const p = el('div', 'ee-palette');
    (meta.variables || []).forEach((v) => {
      const chip = el('button', 'ee-chip', '{' + v + '}'); chip.type = 'button';
      chip.addEventListener('click', () => insertVar('{' + v + '}'));
      p.appendChild(chip);
    });
    s.appendChild(p);
    return s;
  }

  function insertVar(token) {
    const ly = S.current.layers[S.selected];
    if (!ly || ly.type !== 'text') { toast(t('Select a text layer first.')); return; }
    ly.text = (ly.text || '') + token;
    renderPanels(); markPreview();
  }

  // ── upload ────────────────────────────────────────────────────────────
  function uploadBtn(label, on) {
    const w = el('div', 'is-upload');
    const inp = el('input'); inp.type = 'file'; inp.accept = 'image/png,image/jpeg,image/webp,image/gif'; inp.style.display = 'none';
    const b = el('button', 'ee-btn ee-btn-ghost', esc(label)); b.type = 'button';
    b.addEventListener('click', () => inp.click());
    inp.addEventListener('change', async () => {
      if (!inp.files || !inp.files[0]) return;
      const fd = new FormData(); fd.append('file', inp.files[0]);
      b.disabled = true; b.textContent = t('Uploading…');
      const res = await Auth.call('/v1/images/upload', { method: 'POST', body: fd });
      let data = null; try { data = await res.json(); } catch (e) { /* */ }
      b.disabled = false; b.textContent = label;
      if (res.ok && data && data.sha) on(data.sha);
      else toast(t('Upload failed.'), true);
    });
    w.appendChild(b); w.appendChild(inp);
    return w;
  }

  // ── live preview (debounced server render) ────────────────────────────
  function markPreview() { S.dirty = true; schedulePreview(); }
  function schedulePreview() {
    clearTimeout(S.previewT);
    S.previewT = setTimeout(doPreview, 300);
  }
  async function doPreview() {
    const d = S.current; if (!d) return;
    const q = d.bind_type ? ('?kind=' + encodeURIComponent(d.bind_type)) : '';
    const body = { name: d.name, width: d.width, height: d.height, background: d.background, layers: d.layers, bind_type: d.bind_type };
    try {
      const res = await Auth.call('/v1/images/preview' + q, { method: 'POST', json: body });
      if (!res.ok) return;
      const blob = await res.blob();
      if (S.previewUrl) URL.revokeObjectURL(S.previewUrl);
      S.previewUrl = URL.createObjectURL(blob);
      // Feed the same render to the edit canvas (under the handles) and the clean
      // live preview below the editor.
      ['is-preview', 'is-live'].forEach((id) => {
        const im = document.getElementById(id);
        if (im) im.src = S.previewUrl;
      });
    } catch (e) { /* preview is best-effort */ }
  }

  // ── persistence ───────────────────────────────────────────────────────
  function body(d) { return { name: d.name, width: d.width, height: d.height, background: d.background, layers: d.layers, bind_type: d.bind_type }; }

  async function save() {
    const d = S.current; setMsg('');
    const r = d.id
      ? await Auth.callJSON('/v1/images/' + encodeURIComponent(d.id), { method: 'PUT', json: body(d) })
      : await Auth.callJSON('/v1/images', { method: 'POST', json: body(d) });
    if (!r.ok) { setMsg(Auth.errorMessage(r.data) || t('Failed to save.')); return; }
    S.current = clone(r.data); S.dirty = false;
    const idx = S.designs.findIndex((x) => x.id === S.current.id);
    if (idx >= 0) S.designs[idx] = r.data; else S.designs.unshift(r.data);
    renderList(); renderEditor(); setMsg(t('Saved.'));
  }

  async function del() {
    if (!confirm(t('Delete this design? Any embed using its URL will lose the image.'))) return;
    const id = S.current.id;
    const r = await Auth.callJSON('/v1/images/' + encodeURIComponent(id), { method: 'DELETE' });
    if (!r.ok && r.status !== 204) { setMsg(Auth.errorMessage(r.data) || t('Failed to delete.')); return; }
    S.designs = S.designs.filter((x) => x.id !== id); S.current = null; S.selected = -1;
    renderShell();
  }

  function setMsg(m) { const e = document.getElementById('is-msg'); if (e) e.textContent = m; }
  function copyText(s) { (navigator.clipboard ? navigator.clipboard.writeText(s) : Promise.reject()).then(() => toast(t('Copied!'))).catch(() => toast(s)); }
  function toast(m, bad) { const e = document.getElementById('is-msg'); if (e) { e.textContent = m; e.style.color = bad ? '#e0795a' : ''; } }

  window.ImageStudio = { mount };
})();
