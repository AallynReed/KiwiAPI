/* ═══════════════════════════════════════════════════════════════════════
   Embed editor - a reusable Discord-embed template editor.
   ───────────────────────────────────────────────────────────────────────
   Shared by the Webhooks and Discord-bot announcement menus. Edits an
   EmbedTemplate ({enabled, content, title, url, description, color,
   fields[], footer, show_image, image_url}) against a per-type "meta"
   ({variables[], default_template, sample}) with a variable palette, a
   raw-JSON view, and a live Discord-style preview.

   window.EmbedEditor.mount(container, {meta, template, hasImage}) -> controller
     controller.getTemplate()  -> the current template object
     controller.destroy()
   ═══════════════════════════════════════════════════════════════════════ */
(function () {
  'use strict';

  const t = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);
  const esc = (s) => String(s == null ? '' : s).replace(/[&<>"']/g, (c) =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

  const TS_STYLES = 'tTdDfFR';

  // ── client-side mirror of app/embed_templates.substitute ──────────────
  function subst(text, ctx, forPreview) {
    if (!text) return '';
    return String(text).replace(/\{([a-zA-Z0-9_]+)(?::([a-zA-Z]))?\}/g, (m, key, style) => {
      if (!(key in ctx)) return '';
      let v = ctx[key];
      if (style && TS_STYLES.indexOf(style) !== -1) {
        const n = parseInt(v, 10);
        if (isNaN(n)) return v == null ? '' : String(v);
        return forPreview ? previewTime(n, style) : '<t:' + n + ':' + style + '>';
      }
      return v == null ? '' : String(v);
    });
  }

  function previewTime(unix, style) {
    const d = new Date(unix * 1000);
    if (style === 'R') {
      const diff = unix - Math.floor(Date.now() / 1000);
      const a = Math.abs(diff); const fut = diff >= 0;
      let n, unit;
      if (a < 60) { n = a; unit = 'second'; }
      else if (a < 3600) { n = Math.round(a / 60); unit = 'minute'; }
      else if (a < 86400) { n = Math.round(a / 3600); unit = 'hour'; }
      else { n = Math.round(a / 86400); unit = 'day'; }
      const plural = n === 1 ? '' : 's';
      return fut ? ('in ' + n + ' ' + unit + plural) : (n + ' ' + unit + plural + ' ago');
    }
    if (style === 't') return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    if (style === 'T') return d.toLocaleTimeString();
    if (style === 'd') return d.toLocaleDateString();
    if (style === 'D') return d.toLocaleDateString([], { year: 'numeric', month: 'long', day: 'numeric' });
    if (style === 'F') return d.toLocaleString([], { weekday: 'long', year: 'numeric', month: 'long', day: 'numeric', hour: '2-digit', minute: '2-digit' });
    return d.toLocaleString();
  }

  // minimal Discord markdown -> HTML for the preview (links, bold, italics, code)
  function md(text) {
    let h = esc(text);
    h = h.replace(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>');
    h = h.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
    h = h.replace(/(^|[^*])\*([^*]+)\*/g, '$1<em>$2</em>');
    h = h.replace(/`([^`]+)`/g, '<code>$1</code>');
    return h.replace(/\n/g, '<br>');
  }

  function clone(o) { return JSON.parse(JSON.stringify(o || {})); }

  function normalize(tmpl) {
    const tt = clone(tmpl || {});
    tt.fields = Array.isArray(tt.fields) ? tt.fields : [];
    if (typeof tt.show_image !== 'boolean') tt.show_image = true;
    return tt;
  }

  // ── the component ─────────────────────────────────────────────────────
  function mount(container, opts) {
    const meta = opts.meta || { variables: [], default_template: {}, sample: {} };
    const sample = meta.sample || {};
    const hasImage = !!opts.hasImage;
    const designs = opts.designs || [];        // [{id, name, render_url}] for the image picker
    const designUrl = (id) => { const d = designs.find((x) => x.id === id); return d ? d.render_url : null; };
    // Working copy: the stored template, else a disabled copy of the default.
    let tmpl = normalize(
      (opts.template && Object.keys(opts.template).length)
        ? opts.template
        : Object.assign({ enabled: false }, meta.default_template));
    let lastFocused = null;
    let rawMode = false;

    function render() {
      container.innerHTML = '';
      container.className = 'ee';

      // enable toggle
      const head = el('label', 'ee-enable');
      const cb = el('input'); cb.type = 'checkbox'; cb.checked = !!tmpl.enabled;
      cb.addEventListener('change', () => { tmpl.enabled = cb.checked; render(); });
      head.appendChild(cb);
      head.appendChild(el('span', null, t('Customize this embed')));
      container.appendChild(head);

      if (!tmpl.enabled) {
        const hint = el('p', 'ee-hint', t('Off — the default embed is used. Turn on to edit the title, color, fields and more.'));
        container.appendChild(hint);
        container.appendChild(buildPreview(Object.assign({ enabled: true }, meta.default_template)));
        return;
      }

      const body = el('div', 'ee-body');

      if (rawMode) {
        body.appendChild(buildRaw());
      } else {
        body.appendChild(buildForm());
      }

      // palette
      body.appendChild(buildPalette());

      // controls row
      const ctrls = el('div', 'ee-ctrls');
      ctrls.appendChild(btn(rawMode ? t('Form view') : t('View raw JSON'), 'ee-btn-ghost', () => {
        if (rawMode) { rawMode = false; render(); }
        else { rawMode = true; render(); }
      }));
      ctrls.appendChild(btn(t('Reset to default'), 'ee-btn-ghost', () => {
        tmpl = normalize(Object.assign({ enabled: true }, meta.default_template));
        render();
      }));
      body.appendChild(ctrls);

      container.appendChild(body);
      container.appendChild(buildPreview(tmpl));
    }

    function buildForm() {
      const f = el('div', 'ee-form');
      f.appendChild(field(t('Message text (above embed)'), input(tmpl.content || '', (v) => tmpl.content = v, '@role mentions go here')));
      f.appendChild(field(t('Title'), input(tmpl.title || '', (v) => tmpl.title = v)));
      f.appendChild(field(t('Title link (URL)'), input(tmpl.url || '', (v) => tmpl.url = v, 'https://…')));

      const colorRow = el('div', 'ee-color-row');
      const swatch = el('input', 'ee-color'); swatch.type = 'color';
      swatch.value = toHex(tmpl.color) || '#5865F2';
      const colorText = input(tmpl.color || '', (v) => { tmpl.color = v; swatch.value = toHex(v) || swatch.value; updatePreview(); }, '#5865F2');
      swatch.addEventListener('input', () => { tmpl.color = swatch.value; colorText.value = swatch.value; updatePreview(); });
      colorRow.appendChild(swatch); colorRow.appendChild(colorText);
      f.appendChild(field(t('Color'), colorRow));

      f.appendChild(field(t('Description'), textarea(tmpl.description || '', (v) => tmpl.description = v)));

      // fields editor
      const fieldsBox = el('div', 'ee-fields');
      (tmpl.fields || []).forEach((fld, i) => fieldsBox.appendChild(fieldRow(fld, i)));
      const addBtn = btn('+ ' + t('Add field'), 'ee-btn-ghost', () => {
        if ((tmpl.fields || []).length >= 25) return;
        tmpl.fields.push({ name: '', value: '', inline: true }); render();
      });
      const fwrap = field(t('Fields'), fieldsBox); fwrap.appendChild(addBtn);
      f.appendChild(fwrap);

      f.appendChild(field(t('Footer'), input(tmpl.footer || '', (v) => tmpl.footer = v)));

      if (hasImage) {
        const imgWrap = el('div', 'ee-imgwrap');
        const imgCb = el('input'); imgCb.type = 'checkbox'; imgCb.checked = tmpl.show_image !== false;
        imgCb.addEventListener('change', () => { tmpl.show_image = imgCb.checked; updatePreview(); });
        const imgLabel = el('label', 'ee-check');
        imgLabel.appendChild(imgCb); imgLabel.appendChild(el('span', null, t('Include an image')));
        imgWrap.appendChild(imgLabel);
        f.appendChild(imgWrap);
        // Pick a saved Image Studio design (rendered + uploaded fresh per post).
        const sel = document.createElement('select');
        sel.className = 'ee-input';
        const optNone = new Option(t('Default / none'), '');
        sel.appendChild(optNone);
        designs.forEach((d) => sel.appendChild(new Option(d.name || t('Untitled'), d.id)));
        sel.value = tmpl.image_design_id || '';
        sel.addEventListener('change', () => { tmpl.image_design_id = sel.value || null; updatePreview(); });
        f.appendChild(field(t('Image'), sel));
        if (!designs.length) {
          f.appendChild(el('p', 'ee-hint', t('Tip: create designs in the Image Studio to use them here.')));
        }
      }
      return f;
    }

    function fieldRow(fld, i) {
      const row = el('div', 'ee-field-row');
      row.appendChild(input(fld.name || '', (v) => { fld.name = v; }, t('Field name')));
      row.appendChild(input(fld.value || '', (v) => { fld.value = v; }, t('Field value')));
      const inlineLbl = el('label', 'ee-check ee-inline');
      const ic = el('input'); ic.type = 'checkbox'; ic.checked = fld.inline !== false;
      ic.addEventListener('change', () => { fld.inline = ic.checked; updatePreview(); });
      inlineLbl.appendChild(ic); inlineLbl.appendChild(el('span', null, t('inline')));
      row.appendChild(inlineLbl);
      row.appendChild(btn('✕', 'ee-btn-x', () => { tmpl.fields.splice(i, 1); render(); }));
      return row;
    }

    function buildRaw() {
      const wrap = el('div', 'ee-raw');
      const ta = el('textarea', 'ee-raw-ta');
      const view = clone(tmpl); delete view.enabled;
      ta.value = JSON.stringify(view, null, 2);
      const err = el('p', 'ee-raw-err'); err.hidden = true;
      ta.addEventListener('input', () => {
        try {
          const parsed = JSON.parse(ta.value);
          parsed.enabled = true;
          tmpl = normalize(parsed);
          err.hidden = true;
          updatePreview();
        } catch (e) { err.hidden = false; err.textContent = t('Invalid JSON'); }
      });
      wrap.appendChild(ta); wrap.appendChild(err);
      return wrap;
    }

    function buildPalette() {
      const p = el('div', 'ee-palette');
      p.appendChild(el('span', 'ee-palette-label', t('Variables (click to insert):')));
      (meta.variables || []).forEach((v) => {
        const chip = el('button', 'ee-chip'); chip.type = 'button'; chip.textContent = '{' + v + '}';
        chip.addEventListener('click', () => insertVar('{' + v + '}'));
        p.appendChild(chip);
      });
      return p;
    }

    function insertVar(token) {
      const t0 = lastFocused;
      if (!t0 || rawMode) return;
      const s = t0.selectionStart || 0, e = t0.selectionEnd || 0;
      t0.value = t0.value.slice(0, s) + token + t0.value.slice(e);
      t0.dispatchEvent(new Event('input'));
      t0.focus();
      t0.selectionStart = t0.selectionEnd = s + token.length;
    }

    // ── live preview ────────────────────────────────────────────────────
    function buildPreview(tt) {
      const wrap = el('div', 'ee-preview');
      wrap.appendChild(el('p', 'ee-preview-label', t('Preview')));
      const content = subst(tt.content, sample, true);
      if (content) wrap.appendChild(el('div', 'ee-pv-content', null, md(content)));
      const card = el('div', 'ee-pv-card');
      const col = colorInt(tt.color);
      if (col != null) card.style.borderLeftColor = '#' + col.toString(16).padStart(6, '0');
      const title = subst(tt.title, sample, true);
      if (title) {
        const url = subst(tt.url, sample, true);
        const titleEl = el('div', 'ee-pv-title', null, md(title));
        if (/^https?:\/\//.test(url)) { const a = el('a'); a.href = url; a.target = '_blank'; a.rel = 'noopener'; a.innerHTML = md(title); titleEl.innerHTML = ''; titleEl.appendChild(a); }
        card.appendChild(titleEl);
      }
      const desc = subst(tt.description, sample, true);
      if (desc) card.appendChild(el('div', 'ee-pv-desc', null, md(desc)));
      const flds = (tt.fields || []).map((f) => ({
        name: subst(f.name, sample, true), value: subst(f.value, sample, true), inline: f.inline !== false,
      })).filter((f) => f.name || f.value);
      if (flds.length) {
        const grid = el('div', 'ee-pv-fields');
        flds.forEach((f) => {
          const fe = el('div', 'ee-pv-field' + (f.inline ? ' ee-pv-inline' : ''));
          fe.appendChild(el('div', 'ee-pv-fname', null, md(f.name || '​')));
          fe.appendChild(el('div', 'ee-pv-fval', null, md(f.value || '​')));
          grid.appendChild(fe);
        });
        card.appendChild(grid);
      }
      if (hasImage && tt.show_image !== false) {
        // Preview shows the design's render URL directly (caching is irrelevant here);
        // real delivery uploads it as an attachment.
        const imgUrl = (tt.image_design_id && designUrl(tt.image_design_id))
          || subst(tt.image_url, sample, true) || sample.image_url;
        if (imgUrl) { const im = el('img', 'ee-pv-img'); im.src = imgUrl; im.alt = ''; card.appendChild(im); }
      }
      const footer = subst(tt.footer, sample, true);
      if (footer) card.appendChild(el('div', 'ee-pv-footer', null, md(footer)));
      wrap.appendChild(card);
      return wrap;
    }

    function updatePreview() {
      const old = container.querySelector('.ee-preview');
      if (old) old.replaceWith(buildPreview(tmpl));
    }

    // ── small DOM helpers ───────────────────────────────────────────────
    function el(tag, cls, text, html) {
      const e = document.createElement(tag);
      if (cls) e.className = cls;
      if (text != null) e.textContent = text;
      if (html != null) e.innerHTML = html;
      return e;
    }
    function field(label, control) {
      const w = el('label', 'ee-field');
      w.appendChild(el('span', 'ee-label', label));
      w.appendChild(control);
      return w;
    }
    function input(val, onInput, placeholder) {
      const i = el('input', 'ee-input'); i.type = 'text'; i.value = val || '';
      if (placeholder) i.placeholder = placeholder;
      i.addEventListener('input', () => { onInput(i.value); updatePreview(); });
      i.addEventListener('focus', () => { lastFocused = i; });
      return i;
    }
    function textarea(val, onInput) {
      const ta = el('textarea', 'ee-input ee-textarea'); ta.value = val || '';
      ta.addEventListener('input', () => { onInput(ta.value); updatePreview(); });
      ta.addEventListener('focus', () => { lastFocused = ta; });
      return ta;
    }
    function btn(label, cls, onClick) {
      const b = el('button', 'ee-btn ' + (cls || '')); b.type = 'button'; b.textContent = label;
      b.addEventListener('click', onClick);
      return b;
    }

    render();
    return {
      getTemplate() {
        // Drop a wholly-empty enabled template so we don't store noise.
        return clone(tmpl);
      },
      isEnabled() { return !!tmpl.enabled; },
      destroy() { container.innerHTML = ''; },
    };
  }

  // color helpers shared at module scope
  function colorInt(c) {
    if (c == null || c === '') return null;
    if (typeof c === 'number') return c;
    let s = String(c).trim().replace(/^#/, '');
    if (/^[0-9a-fA-F]{6}$/.test(s)) return parseInt(s, 16);
    const n = parseInt(s, 10);
    return isNaN(n) ? null : n;
  }
  function toHex(c) {
    const n = colorInt(c);
    return n == null ? '' : '#' + n.toString(16).padStart(6, '0');
  }

  window.EmbedEditor = { mount };
})();
