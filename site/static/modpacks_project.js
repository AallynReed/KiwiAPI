/* ═══════════════════════════════════════════════════════════════════════
   /modpacks/<handle>/<slug> - a single modpack (Beta)
   ───────────────────────────────────────────────────────────────────────
   Client-rendered from /site/modpacks/*. Shows the pack banner, description,
   warnings, its variants and the mods each variant bundles (with the version
   resolved per mod). The owner (signed in) gets the inline editor: edit
   details, manage variants, add/remove/reorder mods, and lock a mod to a
   specific version. Downloads build a .zip (or .tpack) on the fly.

   Reuses the Mods Hub page styles (.mp-*) + window.BTTMarkdown + window.BTTAuth.
   Mod entries are edited by PUTting a variant's whole ordered list.
   ═══════════════════════════════════════════════════════════════════════ */

(function () {
  'use strict';
  const toast = window.BTTToast.show;

  const { esc } = window.BTTUtil;

  const _metaSlug = (document.querySelector('meta[name="mh-slug"]') || {}).content || '';
  const _metaHandle = (document.querySelector('meta[name="mh-handle"]') || {}).content || '';
  const _clean = (v) => (v && v.indexOf('{{') === -1 ? v : '');
  const _segs = location.pathname.replace(/^\/modpacks\//, '').split('/');
  const HANDLE = decodeURIComponent(_segs[0] || '') || _clean(_metaHandle);
  const SLUG = decodeURIComponent(_segs[1] || '') || _clean(_metaSlug);
  const PACK_PATH = encodeURIComponent(HANDLE) + '/' + encodeURIComponent(SLUG);

  const state = { detail: null, viewer: null, variant: null };
  const _modCache = {};   // "handle/slug" -> mod detail (branches + releases)

  const $root = document.getElementById('mp-root');
  let _modal = null;   // current { wrap, close } handle from window.BTTModal.open

  const t = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);
  const imageUrl = (sha) => BTTUtil.apiUrl('/site/mods/image/' + encodeURIComponent(sha));
  const modUrl = (h, s) => '/mods/' + encodeURIComponent(h) + '/' + encodeURIComponent(s);
  const renderMd = (s) => (window.BTTMarkdown ? window.BTTMarkdown.render(s || '') : esc(s || ''));
  function rerunI18n() { if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh(); }

  // ─── auth + transport ──────────────────────────────────────────────
  function authHeader() {
    const tok = window.BTTAuth && window.BTTAuth.tokens ? window.BTTAuth.tokens.access : null;
    return tok ? { Authorization: 'Bearer ' + tok } : {};
  }
  async function siteGET(path) {
    let r = await fetch(path, { headers: authHeader() });
    if ((r.status === 401 || r.status === 404) && window.BTTAuth
        && window.BTTAuth.tokens && window.BTTAuth.tokens.refresh) {
      if (await window.BTTAuth.refresh()) r = await fetch(path, { headers: authHeader() });
    }
    return r;
  }
  async function apiJSON(path, opts) { return window.BTTAuth.callJSON(path, opts); }
  async function apiForm(path, formData, method = 'POST') {
    const res = await window.BTTAuth.call(path, { method, body: formData });
    let data = null;
    try { data = await res.json(); } catch (_) { /* no body */ }
    return { ok: res.ok, status: res.status, data };
  }

  // ─── Boot ──────────────────────────────────────────────────────────
  boot();
  async function boot() {
    if (window.BTTAuth && window.BTTAuth.getMe) {
      try { state.viewer = await window.BTTAuth.getMe(); } catch (_) { state.viewer = null; }
    }
    await loadDetail();
  }

  async function loadDetail() {
    try {
      const r = await siteGET('/site/modpacks/projects/' + PACK_PATH);
      if (!r.ok) {
        $root.innerHTML = `<p class="mp-error">${esc(t('This modpack could not be found.'))}</p>`;
        rerunI18n();
        return;
      }
      setDetail(await r.json());
    } catch (err) {
      console.error('[modpack] load failed', err);
      $root.innerHTML = `<p class="mp-error">${esc(t('Failed to load this modpack.'))}</p>`;
      rerunI18n();
    }
  }

  function setDetail(d) {
    state.detail = d;
    // Keep the selected variant valid; default to the pack default.
    const names = (d.variants || []).map((v) => v.name);
    if (!state.variant || names.indexOf(state.variant) === -1) {
      state.variant = names.indexOf(d.default_variant) !== -1 ? d.default_variant : (names[0] || null);
    }
    render();
  }

  const activeVariant = () => (state.detail.variants || []).find((v) => v.name === state.variant)
    || (state.detail.variants || [])[0] || null;

  // ─── Render ────────────────────────────────────────────────────────
  function render() {
    const d = state.detail;
    if (!d) return;
    $root.innerHTML = headerHTML(d) + bodyHTML(d);
    const main = $root.closest('.mp-main');
    if (main) main.classList.add('mp-has-hero');
    wire();
    rerunI18n();
  }

  function headerHTML(d) {
    // Same hero as a mod page (mods_project.css): the banner is the backdrop
    // behind the title, bleeding to the viewport edges, not a framed image.
    const bannerInner = d.banner_sha
      ? `<img class="mp-banner" src="${imageUrl(d.banner_sha)}" alt="">`
      : `<div class="mp-banner placeholder"><i class="fa-solid fa-box-open"></i></div>`;
    const banner = `<div class="mp-banner-wrap">${bannerInner}`
      + '<span class="mp-banner-scrim" aria-hidden="true"></span></div>';
    const badge = d.visibility === 'draft'
      ? `<span class="mp-badge mp-badge-draft">${esc(t('Draft'))}</span>`
      : d.visibility === 'unlisted'
        ? `<span class="mp-badge mp-badge-unlisted">${esc(t('Unlisted'))}</span>` : '';
    const taken = d.taken_down
      ? `<div class="mp-takedown"><i class="fa-solid fa-triangle-exclamation"></i> ${esc(t('This modpack has been taken down.'))} ${d.takedown_reason ? esc(d.takedown_reason) : ''}</div>` : '';
    const ownerCtl = d.is_owner ? `
      <button type="button" class="mp-btn mp-btn-sm" id="mpk-edit"><i class="fa-solid fa-pen"></i> ${esc(t('Edit details'))}</button>
      <button type="button" class="mp-btn mp-btn-sm" id="mpk-banner"><i class="fa-solid fa-image"></i> ${esc(t('Banner'))}</button>
      ${d.is_primary_owner ? `<button type="button" class="mp-btn mp-btn-sm" id="mpk-collab"><i class="fa-solid fa-user-group"></i> ${esc(t('Collaborate'))}</button>` : ''}
      ${d.is_primary_owner ? `<button type="button" class="mp-btn mp-btn-sm mp-btn-danger" id="mpk-delete"><i class="fa-solid fa-trash"></i> ${esc(t('Delete'))}</button>` : ''}` : '';
    const likeCls = d.starred ? 'mpk-like active' : 'mpk-like';
    const stats = `<div class="mpk-head-stats">
        <span class="mpk-stat" title="${esc(t('Downloads'))}"><i class="fa-solid fa-download"></i> ${Number(d.download_count || 0).toLocaleString()}</span>
        <button type="button" class="${likeCls}" id="mpk-like" aria-pressed="${d.starred ? 'true' : 'false'}">
          <i class="fa-solid fa-heart"></i> <span id="mpk-like-count">${Number(d.star_count || 0).toLocaleString()}</span>
        </button>
      </div>`;
    return `<header class="mp-header is-doc mpk-header">
      ${banner}
      <div class="mp-header-body mpk-head-body">
        ${taken}
        <div class="mpk-head-titlerow">
          <h1 class="mp-title">${esc(d.title)} ${badge}</h1>
        </div>
        <p class="mpk-head-by">${esc(t('by'))} <a href="/mods/${encodeURIComponent(d.handle)}">${esc(d.owner_username)}</a></p>
        ${d.summary ? `<p class="mpk-head-summary">${esc(d.summary)}</p>` : ''}
        ${stats}
        <div class="mpk-head-actions">${ownerCtl}${!d.is_owner ? `<button type="button" class="mp-btn mp-btn-sm" id="mpk-report"><i class="fa-solid fa-flag"></i> ${esc(t('Report'))}</button>` : ''}</div>
      </div>
    </header>`;
  }

  function warningsHTML(d) {
    if (!d.warnings) return '';
    const blocks = String(d.warnings).split(/<br\s*\/?>/i).map((s) => s.trim()).filter(Boolean);
    if (!blocks.length) return '';
    return `<section class="mp-section mpk-warnings">
      ${blocks.map((b) => `<p class="mp-warning"><i class="fa-solid fa-triangle-exclamation"></i> ${esc(b)}</p>`).join('')}
    </section>`;
  }

  // Reading column on the left, the panel you act on in a rail beside it -
  // stacked full-width, the pack read as four identical blocks and the download
  // sat below the fold.
  function bodyHTML(d) {
    const desc = d.description
      ? `<section class="mp-section mpk-desc"><div class="mp-markdown">${renderMd(d.description)}</div></section>` : '';
    return `<div class="mp-doc">
      <div class="mp-doc-main">${warningsHTML(d)}${desc}${modsHTML(d)}</div>
      <aside class="mp-doc-side">${getPackHTML(d)}</aside>
    </div>`;
  }

  // Pick a variant, then take the pack: the choice and the buttons it drives
  // belong in one panel, not two cards a screen apart.
  function getPackHTML(d) {
    const variants = d.variants || [];
    const tabs = variants.map((v) => {
      const isDefault = v.name === d.default_variant;
      return `<button type="button" class="mpk-tab ${v.name === state.variant ? 'active' : ''}" data-variant="${esc(v.name)}">
        ${esc(v.label || v.name)} <span class="mpk-tab-count">${v.available_count}/${v.mod_count}</span>${isDefault ? ` <i class="fa-solid fa-star mpk-default-star" title="${esc(t('Default'))}"></i>` : ''}
      </button>`;
    }).join('');
    const addBtn = d.is_owner
      ? `<button type="button" class="mpk-tab mpk-tab-add" id="mpk-add-variant"><i class="fa-solid fa-plus"></i> ${esc(t('Edition'))}</button>` : '';
    // A single-variant pack has no choice to offer - don't show a picker of one.
    const picker = (variants.length > 1 || d.is_owner)
      ? `<div class="mpk-picker">
          <span class="mpk-picker-label">${esc(t('Editions'))}</span>
          <div class="mpk-tabs">${tabs}${addBtn}</div>
        </div>` : '';
    const v = activeVariant();
    return `<section class="mp-section mpk-get">
      <div class="mp-section-head"><h2 class="mp-section-title"><i class="fa-solid fa-download"></i> ${esc(t('Download'))}</h2></div>
      ${picker}
      ${variantOwnerCtl(d)}
      ${v ? downloadHTML(v) : ''}
    </section>`;
  }

  function variantOwnerCtl(d) {
    if (!d.is_owner || !activeVariant()) return '';
    return `<div class="mpk-variant-ctl">
      ${activeVariant().name !== d.default_variant ? `<button type="button" class="mp-btn mp-btn-sm" id="mpk-make-default"><i class="fa-solid fa-star"></i> ${esc(t('Make default'))}</button>` : ''}
      <button type="button" class="mp-btn mp-btn-sm" id="mpk-rename-variant"><i class="fa-solid fa-pen"></i> ${esc(t('Rename'))}</button>
      ${(d.variants || []).length > 1 ? `<button type="button" class="mp-btn mp-btn-sm mp-btn-danger" id="mpk-delete-variant"><i class="fa-solid fa-trash"></i> ${esc(t('Delete edition'))}</button>` : ''}
    </div>`;
  }

  function modsHTML(d) {
    const v = activeVariant();
    if (!v) return '';
    const rows = (v.entries || []).map((e, i) => entryRow(e, i, d.is_owner)).join('');
    const empty = !(v.entries || []).length
      ? `<p class="mpk-empty">${esc(t('No mods in this edition yet.'))}</p>` : '';
    const addMod = d.is_owner
      ? `<div class="mpk-add-row">
          <button type="button" class="mp-btn mp-btn-primary mpk-add-mod" id="mpk-add-mod"><i class="fa-solid fa-plus"></i> ${esc(t('Add a mod'))}</button>
          <button type="button" class="mp-btn mpk-add-mod" id="mpk-upload-mod"><i class="fa-solid fa-upload"></i> ${esc(t('Upload a .tmod'))}</button>
          <input type="file" id="mpk-upload-input" accept=".tmod" hidden>
        </div>` : '';
    return `<section class="mp-section mpk-mods">
      <div class="mp-section-head">
        <h2 class="mp-section-title"><i class="fa-solid fa-cubes"></i> ${esc(t('Included mods'))} <span class="mpk-count">${(v.entries || []).length}</span></h2>
      </div>
      <div class="mpk-entries">${rows}${empty}</div>
      ${addMod}
    </section>`;
  }

  function downloadHTML(v) {
    // Disabled when nothing in the variant resolves to a build.
    const ok = v.available_count > 0;
    const dis = ok ? '' : 'disabled';
    return `<div class="mpk-dl">
      <button type="button" class="mp-btn mp-btn-primary" data-dl="zip" ${dis}><i class="fa-solid fa-download"></i> ${esc(t('Download .zip'))}</button>
      <button type="button" class="mp-btn mp-btn-ghost mp-btn-sm" data-dl="tpack" ${dis} title=".tpack">.tpack</button>
    </div>`;
  }

  function entryRow(e, i, isOwner) {
    // A custom uploaded .tmod has no hub mod behind it: no link, no branch/version
    // controls - just an "Uploaded" badge.
    const meta = e.custom
      ? `<span class="mpk-badge"><i class="fa-solid fa-upload"></i> ${esc(t('Uploaded'))}</span>`
        + (e.available ? '' : `<span class="mpk-badge mpk-warn"><i class="fa-solid fa-triangle-exclamation"></i> ${esc(t('file missing'))}</span>`)
      : `<span class="mpk-badge"><i class="fa-solid fa-code-branch"></i> ${esc(e.branch)}</span>`
        + (e.available
          ? (e.version_locked
            ? `<span class="mpk-badge mpk-locked"><i class="fa-solid fa-lock"></i> ${esc(e.version || '')}</span>`
            : `<span class="mpk-badge"><i class="fa-solid fa-arrows-rotate"></i> ${esc(t('latest'))} · ${esc(e.version || '')}</span>`)
          : `<span class="mpk-badge mpk-warn"><i class="fa-solid fa-triangle-exclamation"></i> ${esc(reasonLabel(e.reason))}</span>`);
    // Custom entries have no version/branch to edit, so no sliders button.
    const editBtn = e.custom ? '' :
      `<button type="button" class="mpk-icon" data-edit-entry="${i}" title="${esc(t('Version'))}"><i class="fa-solid fa-sliders"></i></button>`;
    const ctl = isOwner ? `
      <div class="mpk-entry-ctl">
        <button type="button" class="mpk-icon" data-move="${i}" data-dir="-1" title="${esc(t('Move up'))}" ${i === 0 ? 'disabled' : ''}><i class="fa-solid fa-arrow-up"></i></button>
        <button type="button" class="mpk-icon" data-move="${i}" data-dir="1" title="${esc(t('Move down'))}"><i class="fa-solid fa-arrow-down"></i></button>
        ${editBtn}
        <button type="button" class="mpk-icon mpk-icon-danger" data-remove="${i}" title="${esc(t('Remove'))}"><i class="fa-solid fa-xmark"></i></button>
      </div>` : '';
    const titleEl = (e.custom || !e.handle)
      ? `<span class="mpk-entry-title">${esc(e.title || t('Uploaded mod'))}</span>`
      : `<a class="mpk-entry-title" href="${modUrl(e.handle, e.slug)}">${esc(e.title || e.slug)}</a>`;
    const by = e.author
      ? (e.custom
        ? `<span class="mpk-entry-by">${esc(t('by'))} ${esc(e.author)}</span>`
        : `<span class="mpk-entry-by">${esc(t('by'))} <a href="/mods/${encodeURIComponent(e.handle)}">${esc(e.author)}</a></span>`)
      : '';
    return `<div class="mpk-entry ${e.available ? '' : 'mpk-entry-warn'}">
      <div class="mpk-entry-main">
        <div class="mpk-entry-titlerow">
          ${titleEl}
          ${by}
        </div>
        <div class="mpk-entry-meta">${meta}</div>
      </div>
      ${ctl}
    </div>`;
  }

  function reasonLabel(reason) {
    return ({
      removed: t('mod removed'), unavailable: t('mod unavailable'),
      'no build': t('no published build'), 'not a .tmod': t('no .tmod build'),
    })[reason] || t('unavailable');
  }

  // ─── Wiring ────────────────────────────────────────────────────────
  function wire() {
    $root.querySelectorAll('[data-variant]').forEach((b) =>
      b.addEventListener('click', () => { state.variant = b.getAttribute('data-variant'); render(); }));
    $root.querySelectorAll('[data-dl]').forEach((b) =>
      b.addEventListener('click', () => { if (!b.disabled) downloadVariant(b.getAttribute('data-dl')); }));

    const on = (id, fn) => { const el = document.getElementById(id); if (el) el.addEventListener('click', fn); };
    on('mpk-like', toggleLike);
    on('mpk-report', openReport);
    on('mpk-edit', openEditDetails);
    on('mpk-banner', openBanner);
    on('mpk-collab', openCollab);
    on('mpk-delete', deletePack);
    on('mpk-add-variant', openAddVariant);
    on('mpk-rename-variant', openRenameVariant);
    on('mpk-delete-variant', deleteVariant);
    on('mpk-make-default', makeDefault);
    on('mpk-add-mod', openAddMod);
    on('mpk-upload-mod', () => { const inp = document.getElementById('mpk-upload-input'); if (inp) inp.click(); });
    const upInput = document.getElementById('mpk-upload-input');
    if (upInput) upInput.addEventListener('change', uploadMod);

    $root.querySelectorAll('[data-move]').forEach((b) => b.addEventListener('click', () =>
      moveEntry(+b.getAttribute('data-move'), +b.getAttribute('data-dir'))));
    $root.querySelectorAll('[data-remove]').forEach((b) => b.addEventListener('click', () =>
      removeEntry(+b.getAttribute('data-remove'))));
    $root.querySelectorAll('[data-edit-entry]').forEach((b) => b.addEventListener('click', () =>
      openEntryEditor(+b.getAttribute('data-edit-entry'))));
  }

  // ─── Download (fetch with auth so owners can pull their own drafts) ──
  async function downloadVariant(fmt) {
    const v = activeVariant();
    if (!v) return;
    const url = '/site/modpacks/projects/' + PACK_PATH + '/download?variant='
      + encodeURIComponent(v.name) + '&format=' + encodeURIComponent(fmt);
    try {
      const r = await siteGET(url);
      if (!r.ok) { toast(t('Download failed.'), 'error'); return; }
      const blob = await r.blob();
      const cd = r.headers.get('Content-Disposition') || '';
      const m = /filename="?([^"]+)"?/.exec(cd);
      const name = m ? m[1] : (state.detail.slug + '.' + fmt);
      const a = document.createElement('a');
      a.href = URL.createObjectURL(blob);
      a.download = name;
      document.body.appendChild(a);
      a.click();
      setTimeout(() => { URL.revokeObjectURL(a.href); a.remove(); }, 1000);
    } catch (_) { toast(t('Download failed.'), 'error'); }
  }

  // ─── Modals ────────────────────────────────────────────────────────
  function openModal(html) {
    _modal = window.BTTModal.open({ html });
    return _modal;
  }
  function closeModal() { if (_modal) { _modal.close(); _modal = null; } }

  // Anyone (no account needed) can report a modpack - DSA notice-and-action.
  function openReport() {
    openModal(`
      <button type="button" class="mp-modal-close" data-close aria-label="Close"><i class="fa-solid fa-xmark"></i></button>
      <h2 class="mp-modal-title">${esc(t('Report this modpack'))}</h2>
      <form id="mpk-report-form" class="mp-form">
        <label class="mp-form-field"><span>${esc(t('What is the problem?'))}</span><textarea name="reason" rows="4" maxlength="2000" required></textarea></label>
        <p class="mp-form-error" id="mpk-report-error" hidden></p>
        <div class="mp-form-actions">
          <button type="button" class="mp-btn mp-btn-ghost" data-close>${esc(t('Cancel'))}</button>
          <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Send report'))}</button>
        </div>
      </form>`);
    document.getElementById('mpk-report-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const r = await apiJSON('/v1/moderation/report', { json: {
        target_type: 'modpack', handle: HANDLE, slug: SLUG, reason: e.target.reason.value.trim(),
      } });
      if (r.ok || r.status === 202) { closeModal(); toast(t('Thanks - your report was sent.')); }
      else { const el = document.getElementById('mpk-report-error'); if (el) { el.textContent = t('Could not send the report.'); el.hidden = false; } }
    });
  }

  function openEditDetails() {
    const d = state.detail;
    openModal(`
      <button type="button" class="mp-modal-close" data-close aria-label="Close"><i class="fa-solid fa-xmark"></i></button>
      <h2 class="mp-modal-title">${esc(t('Edit modpack'))}</h2>
      <form id="mpk-edit-form" class="mp-form">
        <label class="mp-form-field"><span>${esc(t('Title'))}</span><input type="text" name="title" maxlength="120" value="${esc(d.title)}" required></label>
        <label class="mp-form-field"><span>${esc(t('Short summary'))}</span><input type="text" name="summary" maxlength="280" value="${esc(d.summary || '')}"></label>
        <label class="mp-form-field"><span>${esc(t('Description (markdown)'))}</span><textarea name="description" rows="5" maxlength="40000">${esc(d.description || '')}</textarea></label>
        <label class="mp-form-field"><span>${esc(t('Warnings'))}</span><textarea name="warnings" rows="2" maxlength="4000" placeholder="${esc(t('Use <br> to split blocks'))}">${esc(d.warnings || '')}</textarea></label>
        <label class="mp-form-field"><span>${esc(t('Tags'))}</span><input type="text" name="tags" value="${esc((d.tags || []).join(', '))}" placeholder="${esc(t('comma, separated'))}"></label>
        <label class="mp-form-field"><span>${esc(t('Visibility'))}</span>
          <select name="visibility">
            <option value="draft" ${d.visibility === 'draft' ? 'selected' : ''}>${esc(t('Draft (only you)'))}</option>
            <option value="unlisted" ${d.visibility === 'unlisted' ? 'selected' : ''}>${esc(t('Unlisted (link only)'))}</option>
            <option value="public" ${d.visibility === 'public' ? 'selected' : ''}>${esc(t('Public'))}</option>
          </select></label>
        <label class="mp-form-field"><span>${esc(t('Discord link'))}</span><input type="url" name="discord_url" maxlength="300" value="${esc(d.discord_url || '')}"></label>
        <label class="mp-form-field"><span>${esc(t('Website'))}</span><input type="url" name="website_url" maxlength="300" value="${esc(d.website_url || '')}"></label>
        <p class="mp-form-error" id="mpk-edit-error" hidden></p>
        <div class="mp-form-actions">
          <button type="button" class="mp-btn mp-btn-ghost" data-close>${esc(t('Cancel'))}</button>
          <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Save'))}</button>
        </div>
      </form>`);
    document.getElementById('mpk-edit-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const f = e.target;
      const body = {
        title: f.title.value.trim(), summary: f.summary.value.trim(),
        description: f.description.value, warnings: f.warnings.value,
        tags: f.tags.value.split(',').map((s) => s.trim()).filter(Boolean),
        visibility: f.visibility.value,
        discord_url: f.discord_url.value.trim(), website_url: f.website_url.value.trim(),
      };
      await patchPack(body, document.getElementById('mpk-edit-error'));
    });
  }

  function openBanner() {
    openModal(`
      <button type="button" class="mp-modal-close" data-close aria-label="Close"><i class="fa-solid fa-xmark"></i></button>
      <h2 class="mp-modal-title">${esc(t('Modpack banner'))}</h2>
      <form id="mpk-banner-form" class="mp-form">
        <label class="mp-form-field"><span>${esc(t('Image (PNG, JPEG, WebP, GIF)'))}</span><input type="file" name="file" accept="image/*" required></label>
        <p class="mp-form-error" id="mpk-banner-error" hidden></p>
        <div class="mp-form-actions">
          <button type="button" class="mp-btn mp-btn-ghost" data-close>${esc(t('Cancel'))}</button>
          <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Upload'))}</button>
        </div>
      </form>`);
    document.getElementById('mpk-banner-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const file = e.target.file.files[0];
      if (!file) return;
      const fd = new FormData(); fd.append('file', file);
      const err = document.getElementById('mpk-banner-error');
      const r = await apiForm('/v1/modpacks/hub/projects/' + PACK_PATH + '/banner', fd);
      if (r.ok) { closeModal(); await loadDetail(); toast(t('Banner updated.')); }
      else { err.textContent = errMsg(r, t('Upload failed.')); err.hidden = false; }
    });
  }

  function openAddVariant() {
    openModal(`
      <button type="button" class="mp-modal-close" data-close aria-label="Close"><i class="fa-solid fa-xmark"></i></button>
      <h2 class="mp-modal-title">${esc(t('New edition'))}</h2>
      <form id="mpk-variant-form" class="mp-form">
        <label class="mp-form-field"><span>${esc(t('Name'))}</span><input type="text" name="name" maxlength="80" required placeholder="${esc(t('e.g. Lite'))}"></label>
        <label class="mp-form-field"><span>${esc(t('Copy mods from'))}</span>
          <select name="copy_from">
            <option value="">${esc(t('Start empty'))}</option>
            ${(state.detail.variants || []).map((v) => `<option value="${esc(v.name)}">${esc(v.label || v.name)}</option>`).join('')}
          </select></label>
        <p class="mp-form-error" id="mpk-variant-error" hidden></p>
        <div class="mp-form-actions">
          <button type="button" class="mp-btn mp-btn-ghost" data-close>${esc(t('Cancel'))}</button>
          <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Create'))}</button>
        </div>
      </form>`);
    document.getElementById('mpk-variant-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const f = e.target;
      const err = document.getElementById('mpk-variant-error');
      const r = await apiJSON('/v1/modpacks/hub/projects/' + PACK_PATH + '/variants',
        { json: { name: f.name.value.trim(), copy_from: f.copy_from.value || null } });
      if (r.ok) { closeModal(); setDetail(r.data); toast(t('Edition created.')); }
      else { err.textContent = errMsg(r, t('Could not create the edition.')); err.hidden = false; }
    });
  }

  function openRenameVariant() {
    const v = activeVariant();
    openModal(`
      <button type="button" class="mp-modal-close" data-close aria-label="Close"><i class="fa-solid fa-xmark"></i></button>
      <h2 class="mp-modal-title">${esc(t('Rename edition'))}</h2>
      <form id="mpk-rv-form" class="mp-form">
        <label class="mp-form-field"><span>${esc(t('Label'))}</span><input type="text" name="label" maxlength="120" value="${esc(v.label || v.name)}" required></label>
        <div class="mp-form-actions">
          <button type="button" class="mp-btn mp-btn-ghost" data-close>${esc(t('Cancel'))}</button>
          <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Save'))}</button>
        </div>
      </form>`);
    document.getElementById('mpk-rv-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const r = await apiJSON('/v1/modpacks/hub/projects/' + PACK_PATH + '/variants/' + encodeURIComponent(v.name),
        { method: 'PATCH', json: { label: e.target.label.value.trim() } });
      if (r.ok) { closeModal(); setDetail(r.data); }
      else { toast(errMsg(r, t('Could not rename.')), 'error'); }
    });
  }

  async function deleteVariant() {
    const v = activeVariant();
    if (!confirm(t('Delete this edition? Its mod list is removed.'))) return;
    const r = await apiJSON('/v1/modpacks/hub/projects/' + PACK_PATH + '/variants/' + encodeURIComponent(v.name),
      { method: 'DELETE' });
    if (r.ok) { state.variant = null; setDetail(r.data); toast(t('Edition deleted.')); }
    else { toast(errMsg(r, t('Could not delete the edition.')), 'error'); }
  }

  async function makeDefault() {
    await patchPack({ default_variant: state.variant });
  }

  // Manage co-owners (primary owner only).
  function openCollab() {
    const rows = (state.detail.collaborators || []).map((c) => `
      <div class="mpk-collab-row">
        <span><i class="fa-solid fa-user"></i> @${esc(c.username)}</span>
        <button type="button" class="mpk-icon mpk-icon-danger" data-rm-collab="${esc(c.id)}" title="${esc(t('Remove'))}"><i class="fa-solid fa-xmark"></i></button>
      </div>`).join('') || `<p class="mp-muted">${esc(t('No collaborators yet.'))}</p>`;
    openModal(`
      <button type="button" class="mp-modal-close" data-close aria-label="Close"><i class="fa-solid fa-xmark"></i></button>
      <h2 class="mp-modal-title">${esc(t('Collaborators'))}</h2>
      <p class="mp-muted">${esc(t('Collaborators can edit this modpack. Only you, the owner, can add or remove them or delete the pack.'))}</p>
      <div class="mpk-collab-list">${rows}</div>
      <form id="mpk-collab-form" class="mp-form" style="margin-top:12px">
        <label class="mp-form-field"><span>${esc(t('Add a collaborator by username'))}</span>
          <input type="text" name="username" maxlength="80" placeholder="${esc(t('username'))}" autocomplete="off" required></label>
        <p class="mp-form-error" id="mpk-collab-error" hidden></p>
        <div class="mp-form-actions">
          <button type="button" class="mp-btn mp-btn-ghost" data-close>${esc(t('Close'))}</button>
          <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Add'))}</button>
        </div>
      </form>`);
    _modal.wrap.querySelectorAll('[data-rm-collab]').forEach((b) => b.addEventListener('click', async () => {
      const r = await apiJSON('/v1/modpacks/hub/projects/' + PACK_PATH + '/collaborators/' + encodeURIComponent(b.getAttribute('data-rm-collab')), { method: 'DELETE' });
      if (r.ok) { setDetail(r.data); openCollab(); } else { toast(errMsg(r, t('Could not remove.')), 'error'); }
    }));
    document.getElementById('mpk-collab-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const err = document.getElementById('mpk-collab-error');
      const r = await apiJSON('/v1/modpacks/hub/projects/' + PACK_PATH + '/collaborators',
        { json: { username: e.target.username.value.trim() } });
      if (r.ok) { setDetail(r.data); openCollab(); }
      else { err.textContent = errMsg(r, t('Could not add that collaborator.')); err.hidden = false; }
    });
  }

  async function toggleLike() {
    if (!state.viewer) { location.href = '/login'; return; }
    const next = !state.detail.starred;
    const r = await apiJSON('/v1/modpacks/hub/projects/' + PACK_PATH + '/star',
      { method: next ? 'POST' : 'DELETE' });
    if (r.ok && r.data) {
      state.detail.starred = r.data.starred;
      state.detail.star_count = r.data.star_count;
      const btn = document.getElementById('mpk-like');
      const cnt = document.getElementById('mpk-like-count');
      if (btn) { btn.classList.toggle('active', r.data.starred); btn.setAttribute('aria-pressed', r.data.starred ? 'true' : 'false'); }
      if (cnt) cnt.textContent = Number(r.data.star_count || 0).toLocaleString();
    } else {
      toast(errMsg(r, t('Could not update your like.')), 'error');
    }
  }

  async function deletePack() {
    if (!confirm(t('Delete this whole modpack? This cannot be undone.'))) return;
    const r = await apiJSON('/v1/modpacks/hub/projects/' + PACK_PATH, { method: 'DELETE' });
    if (r.ok || r.status === 204) { location.href = '/modpacks'; }
    else { toast(errMsg(r, t('Could not delete the modpack.')), 'error'); }
  }

  async function patchPack(body, errEl) {
    const r = await apiJSON('/v1/modpacks/hub/projects/' + PACK_PATH, { method: 'PATCH', json: body });
    if (r.ok) { closeModal(); setDetail(r.data); toast(t('Saved.')); }
    else if (errEl) { errEl.textContent = errMsg(r, t('Could not save.')); errEl.hidden = false; }
    else { toast(errMsg(r, t('Could not save.')), 'error'); }
  }

  // ─── Entry editing (PUT the variant's whole list) ───────────────────
  function currentEntries() {
    // The editable shape the API expects, from the resolved entry views. Custom
    // uploads round-trip by their stored sha; hub mods by handle/slug.
    return (activeVariant().entries || []).map((e) => e.custom
      ? { custom_sha: e.custom_sha, custom_filename: e.custom_filename, title: e.title, author: e.author }
      : {
          handle: e.handle, slug: e.slug, branch: e.branch,
          version_locked: !!e.version_locked, locked_tag: e.locked_tag || null,
        });
  }

  // Upload a custom .tmod into the active variant (conflicts rejected server-side).
  async function uploadMod(ev) {
    const input = ev.target;
    const file = input.files && input.files[0];
    input.value = '';   // allow re-selecting the same file later
    if (!file) return;
    const v = activeVariant();
    if (!v) return;
    const fd = new FormData();
    fd.append('file', file);
    const r = await apiForm('/v1/modpacks/hub/projects/' + PACK_PATH + '/variants/'
      + encodeURIComponent(v.name) + '/upload', fd);
    if (r.ok && r.data) {
      setDetail(r.data);
      toast(r.data.matched_existing ? t('Added to your pack.') : t('Mod uploaded.'));
    } else {
      toast(errMsg(r, t('Could not add that .tmod.')), 'error');
    }
  }
  async function putEntries(entries, errEl) {
    const v = activeVariant();
    const r = await apiJSON('/v1/modpacks/hub/projects/' + PACK_PATH + '/variants/'
      + encodeURIComponent(v.name) + '/entries', { method: 'PUT', json: { entries } });
    if (r.ok) { closeModal(); setDetail(r.data); return true; }
    if (errEl) { errEl.textContent = errMsg(r, t('Could not save the mods.')); errEl.hidden = false; }
    else { toast(errMsg(r, t('Could not save the mods.')), 'error'); }
    return false;
  }

  async function moveEntry(i, dir) {
    const list = currentEntries();
    const j = i + dir;
    if (j < 0 || j >= list.length) return;
    [list[i], list[j]] = [list[j], list[i]];
    await putEntries(list);
  }
  async function removeEntry(i) {
    const list = currentEntries();
    list.splice(i, 1);
    await putEntries(list);
  }

  async function loadModDetail(handle, slug) {
    const key = handle + '/' + slug;
    if (_modCache[key]) return _modCache[key];
    const r = await siteGET('/site/mods/projects/' + encodeURIComponent(handle) + '/' + encodeURIComponent(slug));
    if (!r.ok) return null;
    const d = await r.json();
    _modCache[key] = d;
    return d;
  }

  // Branch + version lock for one entry (existing or being added).
  async function openEntryEditor(i) {
    const e = activeVariant().entries[i];
    const mod = await loadModDetail(e.handle, e.slug);
    showEntryForm({ handle: e.handle, slug: e.slug, title: e.title, branch: e.branch,
      version_locked: e.version_locked, locked_tag: e.locked_tag, index: i, mod });
  }

  // The mod's variants a pack can actually track = the branches that have a
  // published .tmod release. We derive these from `mod.releases`, NOT from
  // `mod.branches`: that's the git branch list, which comes back EMPTY for
  // releases-only mods, private-source mods, or any mod the pack maker doesn't own
  // (project_detail only fills `branches` when the source is visible). A git branch
  // with no release is useless to a pack anyway. Ordered by the modder's
  // `branch_order` when they set one, else by recency of the release.
  function modVariants(mod) {
    if (!mod) return [];
    const list = [...new Set((mod.releases || [])
      .filter((r) => r.status === 'published' && r.format === 'tmod')
      .map((r) => r.branch))];
    const order = mod.branch_order || [];
    if (order.length) {
      const idx = (b) => { const i = order.indexOf(b); return i === -1 ? order.length : i; };
      list.sort((a, b) => idx(a) - idx(b));
    }
    return list;
  }

  function showEntryForm(opts) {
    const mod = opts.mod;
    let branches = modVariants(mod);
    // Keep the entry's current variant selectable even if it no longer has a build.
    if (opts.branch && branches.indexOf(opts.branch) === -1) branches.unshift(opts.branch);
    if (!branches.length) branches = [opts.branch || 'main'];
    const branchOpts = branches.map((b) =>
      `<option value="${esc(b)}" ${b === opts.branch ? 'selected' : ''}>${esc(b)}</option>`).join('');
    openModal(`
      <button type="button" class="mp-modal-close" data-close aria-label="Close"><i class="fa-solid fa-xmark"></i></button>
      <h2 class="mp-modal-title">${esc(opts.title || opts.slug)}</h2>
      <form id="mpk-entry-form" class="mp-form">
        <label class="mp-form-field"><span>${esc(t('Edition of the mod'))}</span><select name="branch">${branchOpts}</select></label>
        <label class="mp-check"><input type="checkbox" name="lock" ${opts.version_locked ? 'checked' : ''}> <span>${esc(t('Lock to a specific version (don\'t auto-update)'))}</span></label>
        <label class="mp-form-field" id="mpk-ver-wrap" ${opts.version_locked ? '' : 'hidden'}><span>${esc(t('Version'))}</span><select name="locked_tag"></select></label>
        <p class="mp-form-hint">${esc(t('Off by default: the mod tracks its latest published build on the chosen edition.'))}</p>
        <p class="mp-form-error" id="mpk-entry-error" hidden></p>
        <div class="mp-form-actions">
          <button type="button" class="mp-btn mp-btn-ghost" data-close>${esc(t('Cancel'))}</button>
          <button type="submit" class="mp-btn mp-btn-primary">${opts.index == null ? esc(t('Add')) : esc(t('Save'))}</button>
        </div>
      </form>`);
    const form = document.getElementById('mpk-entry-form');
    const verWrap = document.getElementById('mpk-ver-wrap');
    const verSel = form.locked_tag;
    const fillVersions = () => {
      const br = form.branch.value;
      const rels = (mod ? (mod.releases || []) : [])
        .filter((r) => r.branch === br && r.status === 'published' && r.format === 'tmod');
      verSel.innerHTML = rels.length
        ? rels.map((r) => `<option value="${esc(r.tag)}" ${r.tag === opts.locked_tag ? 'selected' : ''}>${esc(r.tag)}</option>`).join('')
        : `<option value="">${esc(t('No published .tmod versions'))}</option>`;
    };
    fillVersions();
    form.branch.addEventListener('change', fillVersions);
    form.lock.addEventListener('change', () => { verWrap.hidden = !form.lock.checked; });
    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      const list = currentEntries();
      const entry = {
        handle: opts.handle, slug: opts.slug, branch: form.branch.value,
        version_locked: form.lock.checked,
        locked_tag: form.lock.checked ? (verSel.value || null) : null,
      };
      if (opts.index == null) list.push(entry); else list[opts.index] = entry;
      await putEntries(list, document.getElementById('mpk-entry-error'));
    });
  }

  // Add a mod: search the hub, pick one, then choose branch/version.
  function openAddMod() {
    openModal(`
      <button type="button" class="mp-modal-close" data-close aria-label="Close"><i class="fa-solid fa-xmark"></i></button>
      <h2 class="mp-modal-title">${esc(t('Add a mod'))}</h2>
      <div class="mp-form">
        <label class="mp-form-field"><span>${esc(t('Search mods'))}</span>
          <input type="search" id="mpk-mod-search" placeholder="${esc(t('Search the Mods Hub…'))}" autocomplete="off"></label>
        <div class="mpk-search-results" id="mpk-mod-results"></div>
      </div>`);
    const input = document.getElementById('mpk-mod-search');
    const results = document.getElementById('mpk-mod-results');
    const existing = new Set((activeVariant().entries || []).map((e) => e.handle + '/' + e.slug));
    let timer = null;
    const run = async () => {
      const q = input.value.trim();
      const r = await fetch('/site/mods/projects?limit=20&sort=downloads' + (q ? '&q=' + encodeURIComponent(q) : ''));
      const data = r.ok ? await r.json() : { items: [] };
      results.innerHTML = (data.items || []).map((m) => {
        const already = existing.has(m.handle + '/' + m.slug);
        return `<div class="mpk-result">
          <div class="mpk-result-main">
            <span class="mpk-result-title">${esc(m.title)}</span>
            <span class="mpk-result-by">${esc(m.owner_username)}</span>
          </div>
          <button type="button" class="mp-btn mp-btn-sm ${already ? 'mp-btn-ghost' : 'mp-btn-primary'}"
            data-add-handle="${esc(m.handle)}" data-add-slug="${esc(m.slug)}" data-add-title="${esc(m.title)}" ${already ? 'disabled' : ''}>
            ${already ? esc(t('Added')) : esc(t('Choose'))}</button>
        </div>`;
      }).join('') || `<p class="mpk-empty">${esc(t('No mods found.'))}</p>`;
      results.querySelectorAll('[data-add-handle]').forEach((b) => b.addEventListener('click', async () => {
        const h = b.getAttribute('data-add-handle'), s = b.getAttribute('data-add-slug'), ti = b.getAttribute('data-add-title');
        const mod = await loadModDetail(h, s);
        // Default to the mod's first real variant (one with a build), not "main" -
        // a releases-only mod may have no "main" branch at all.
        const branch = modVariants(mod)[0] || mod.default_branch || 'main';
        showEntryForm({ handle: h, slug: s, title: ti, branch,
          version_locked: false, locked_tag: null, index: null, mod });
      }));
      rerunI18n();
    };
    input.addEventListener('input', () => { clearTimeout(timer); timer = setTimeout(run, 250); });
    run();
  }

  function errMsg(r, fallback) {
    return (r && r.data && r.data.error && r.data.error.message) || fallback;
  }
})();
