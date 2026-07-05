/* ═══════════════════════════════════════════════════════════════════════
   /mods/{handle} - modder profile page (Beta)
   ───────────────────────────────────────────────────────────────────────
   A modder's home: avatar, banner, README (markdown), socials, and a grid of
   their mods. Client-rendered from the same-origin /site/mods/profile/<handle>
   proxy (which passes the site user as viewer, so the owner sees their drafts +
   edit controls). Writes go to /v1/mods/hub/me/profile* via window.BTTAuth.
   ═══════════════════════════════════════════════════════════════════════ */
(function () {
  'use strict';

  const { esc } = window.BTTUtil;

  const _metaHandle = (document.querySelector('meta[name="mh-handle"]') || {}).content || '';
  const _clean = (v) => (v && v.indexOf('{{') === -1 ? v : '');
  const HANDLE = decodeURIComponent(location.pathname.replace(/^\/mods\//, '').split('/')[0] || '')
    || _clean(_metaHandle);

  const state = { profile: null, viewer: null };
  const $root = document.getElementById('mpf-root');
  const $modalRoot = document.getElementById('mpf-modal-root');

  const t = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);
  const imageUrl = (sha) => '/site/mods/image/' + encodeURIComponent(sha);
  const md = (s) => (window.BTTMarkdown ? window.BTTMarkdown.render(s) : esc(s));
  const modUrl = (m) => '/mods/' + encodeURIComponent(m.handle) + '/' + encodeURIComponent(m.slug);
  function rerunI18n() { if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh(); }

  function authHeader() {
    const tok = window.BTTAuth && window.BTTAuth.tokens ? window.BTTAuth.tokens.access : null;
    return tok ? { Authorization: 'Bearer ' + tok } : {};
  }
  async function siteGET(path) {
    let r = await fetch(path, { headers: authHeader() });
    if ((r.status === 401) && window.BTTAuth && window.BTTAuth.tokens && window.BTTAuth.tokens.refresh) {
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
  const errMsg = (r, fallback) =>
    (r && r.data && r.data.error && r.data.error.message) || t(fallback);

  // ─── Boot ──────────────────────────────────────────────────────────
  boot();

  async function boot() {
    if (window.BTTAuth && window.BTTAuth.getMe) {
      try { state.viewer = await window.BTTAuth.getMe(); } catch (_) { state.viewer = null; }
    }
    await loadProfile();
  }

  async function loadProfile() {
    try {
      const r = await siteGET('/site/mods/profile/' + encodeURIComponent(HANDLE));
      if (r.status === 404) {
        $root.innerHTML = `<p class="mp-error">${esc(t('No modder found with that handle.'))}</p>`;
        rerunI18n(); return;
      }
      if (!r.ok) throw new Error('HTTP ' + r.status);
      state.profile = await r.json();
      render();
    } catch (err) {
      console.error('[profile] load failed', err);
      $root.innerHTML = `<p class="mp-error">${esc(t('Failed to load this profile.'))}</p>`;
      rerunI18n();
    }
  }

  // ─── Render ────────────────────────────────────────────────────────
  function render() {
    const p = state.profile;
    const main = readmeHTML(p) + modsHTML(p);
    const side = aboutHTML(p) + featuredHTML(p);
    $root.innerHTML = headerHTML(p) + `<div class="mpf-layout">
      <div class="mpf-col-main">${main}</div>
      <aside class="mpf-col-side">${side}</aside>
    </div>`;
    wire(p);
    rerunI18n();
  }

  // Owner-provided social buttons (shown in the sidebar About section).
  function socialLinks(p) {
    const links = [];
    if (p.discord_url) links.push(`<a class="mp-linkbtn" href="${esc(p.discord_url)}" target="_blank" rel="noopener"><i class="fa-brands fa-discord"></i> Discord</a>`);
    if (p.website_url) links.push(`<a class="mp-linkbtn" href="${esc(p.website_url)}" target="_blank" rel="noopener"><i class="fa-solid fa-globe"></i> ${esc(t('Website'))}</a>`);
    (p.donation_urls || []).forEach((u) => { const m = donateMeta(u); links.push(`<a class="mp-linkbtn mp-linkbtn-donate" href="${esc(u)}" target="_blank" rel="noopener nofollow"><i class="${m.cls}"></i> ${esc(m.label)}</a>`); });
    return links;
  }

  function donateMeta(url) {
    const u = (url || '').toLowerCase();
    if (u.includes('ko-fi') || u.includes('kofi')) return { cls: 'fa-solid fa-mug-hot', label: 'Ko-fi' };
    if (u.includes('patreon')) return { cls: 'fa-brands fa-patreon', label: 'Patreon' };
    if (u.includes('paypal')) return { cls: 'fa-brands fa-paypal', label: 'PayPal' };
    if (u.includes('buymeacoffee') || u.includes('buymeacoff.ee')) return { cls: 'fa-solid fa-mug-hot', label: 'Buy me a coffee' };
    if (u.includes('github.com/sponsors')) return { cls: 'fa-brands fa-github', label: 'Sponsor' };
    return { cls: 'fa-solid fa-heart', label: t('Donate') };
  }

  function headerHTML(p) {
    const bannerInner = p.banner_url
      ? `<img class="mpf-banner" src="${esc(p.banner_url)}" alt="">`
      : `<div class="mpf-banner placeholder"></div>`;
    const banner = p.is_owner
      ? `<div class="mpf-banner-wrap" id="mpf-banner-btn" role="button" tabindex="0" title="${esc(t('Change banner'))}">${bannerInner}<span class="mp-banner-edit"><i class="fa-solid fa-camera"></i> ${esc(p.banner_sha ? t('Change banner') : t('Add banner'))}</span></div>`
      : bannerInner;
    const avatarInner = p.avatar_url
      ? `<img class="mpf-avatar" src="${esc(p.avatar_url)}" alt="">`
      : `<div class="mpf-avatar placeholder"><i class="fa-solid fa-user"></i></div>`;
    const avatar = p.is_owner
      ? `<div class="mpf-avatar-wrap" id="mpf-avatar-btn" role="button" tabindex="0" title="${esc(t('Change picture'))}">${avatarInner}<span class="mpf-avatar-edit"><i class="fa-solid fa-camera"></i></span></div>`
      : `<div class="mpf-avatar-wrap">${avatarInner}</div>`;

    const editBtn = p.is_owner
      ? `<button type="button" class="mp-btn mp-btn-sm" id="mpf-edit"><i class="fa-solid fa-pen"></i> ${esc(t('Edit profile'))}</button>` : '';

    return `<header class="mpf-header">
      ${banner}
      <div class="mpf-id">
        ${avatar}
        <div class="mpf-id-text">
          <div class="mpf-namerow">
            <h1 class="mpf-name">${esc(p.display_name)}</h1>
            ${editBtn}
          </div>
          <div class="mpf-handle">@${esc(p.handle)}</div>
        </div>
      </div>
      ${p.tagline ? `<p class="mpf-tagline">${esc(p.tagline)}</p>` : ''}
    </header>`;
  }

  // Sidebar: About (meta + socials) and a single highlighted mod.
  function aboutHTML(p) {
    const links = socialLinks(p);
    const linksRow = links.length ? `<div class="mp-links mpf-side-links">${links.join('')}</div>`
      : (p.is_owner ? `<p class="mp-muted" style="margin:8px 0 0">${esc(t('Add your socials in Edit profile.'))}</p>` : '');
    return `<section class="mp-section mpf-side-section">
      <div class="mp-section-head"><h2 class="mp-section-title"><i class="fa-solid fa-circle-info"></i> ${esc(t('About'))}</h2></div>
      <div class="mpf-about-meta">
        <span><i class="fa-solid fa-cube"></i> ${Number(p.mod_count || 0) + ' ' + esc(p.mod_count === 1 ? t('mod') : t('mods'))}</span>
        ${p.joined_at ? `<span><i class="fa-solid fa-clock"></i> ${esc(t('Joined')) + ' ' + fmtDate(p.joined_at)}</span>` : ''}
      </div>
      ${linksRow}
    </section>`;
  }

  function featuredHTML(p) {
    if (!p.featured && !p.is_owner) return '';
    const inner = p.featured
      ? cardHTML(p.featured, {})
      : `<p class="mp-muted" style="margin:0">${esc(t('Pin one of your mods (the pin button on a card) to highlight it here.'))}</p>`;
    return `<section class="mp-section mpf-side-section">
      <div class="mp-section-head"><h2 class="mp-section-title"><i class="fa-solid fa-thumbtack"></i> ${esc(t('Highlighted'))}</h2></div>
      ${inner}
    </section>`;
  }

  function readmeHTML(p) {
    const has = p.readme && p.readme.trim();
    if (!has && !p.is_owner) return '';
    const editBtn = p.is_owner
      ? `<button type="button" class="mp-btn mp-btn-sm" id="mpf-edit-readme"><i class="fa-solid fa-pen"></i> ${esc(has ? t('Edit') : t('Add README'))}</button>` : '';
    const body = has ? md(p.readme) : `<p class="mp-muted">${esc(t('No README yet.'))}</p>`;
    return `<section class="mp-section mpf-section">
      <div class="mp-section-head"><h2 class="mp-section-title"><i class="fa-solid fa-book-open"></i> ${esc(t('About'))}</h2>${editBtn}</div>
      <div class="mp-markdown">${body}</div>
    </section>`;
  }

  function modsHTML(p) {
    const mods = p.mods || [];
    const grid = mods.length
      ? `<div class="mh-grid">${mods.map((m, i) => cardHTML(m, {
          owner: p.is_owner, first: i === 0, last: i === mods.length - 1,
          featured: p.featured_slug === m.slug,
        })).join('')}</div>`
      : `<p class="mp-muted">${p.is_owner ? esc(t('You have no mods yet.')) : esc(t('No public mods yet.'))}</p>`;
    const hint = (p.is_owner && mods.length > 1)
      ? `<p class="mp-muted" style="margin:0 0 12px">${esc(t('Use the arrows to set the order; the pin highlights one in the sidebar.'))}</p>` : '';
    return `<section class="mp-section mpf-section">
      <div class="mp-section-head"><h2 class="mp-section-title"><i class="fa-solid fa-cubes"></i> ${esc(t('Mods'))}</h2></div>
      ${hint}${grid}
    </section>`;
  }

  function cardHTML(m, opts) {
    opts = opts || {};
    const cardSha = m.banner_sha || m.preview_sha || null;   // banner, else first preview
    const banner = cardSha
      ? `<img class="mh-card-banner" src="${imageUrl(cardSha)}" alt="" loading="lazy">`
      : `<div class="mh-card-banner placeholder"><i class="fa-solid fa-cube" aria-hidden="true"></i></div>`;
    const tags = (m.tags || []).slice(0, 4).map((tg) => `<span class="mh-card-tag">${esc(tg)}</span>`).join('');
    const badge = m.visibility === 'draft' ? `<span class="mh-badge mh-badge-draft">${esc(t('Draft'))}</span>`
      : m.visibility === 'unlisted' ? `<span class="mh-badge mh-badge-unlisted">${esc(t('Unlisted'))}</span>` : '';
    const card = `<a class="mh-card" href="${modUrl(m)}">
      ${banner}
      <div class="mh-card-body">
        <h3 class="mh-card-title">${esc(m.title)} ${badge}</h3>
        ${m.summary ? `<p class="mh-card-summary">${esc(m.summary)}</p>` : ''}
        ${tags ? `<div class="mh-card-tags">${tags}</div>` : ''}
        <div class="mh-card-foot">
          <span class="mh-card-stats">
            <span class="mh-card-dl"><i class="fa-solid fa-download" aria-hidden="true"></i> ${Number(m.download_count || 0).toLocaleString()}</span>
            <span class="mh-card-dl"><i class="fa-solid fa-star" aria-hidden="true"></i> ${Number(m.star_count || 0).toLocaleString()}</span>
          </span>
        </div>
      </div>
    </a>`;
    if (!opts.owner) return card;
    // Owner controls live OUTSIDE the <a> (a wrapper) so clicking them doesn't navigate.
    const ctl = `<div class="mpf-card-ctl">
      <button type="button" class="mpf-cbtn ${opts.featured ? 'is-on' : ''}" data-feature="${esc(m.slug)}" title="${esc(opts.featured ? t('Unpin') : t('Highlight in sidebar'))}" aria-label="${esc(t('Highlight'))}"><i class="fa-solid fa-thumbtack"></i></button>
      <button type="button" class="mpf-cbtn" data-up="${esc(m.slug)}" ${opts.first ? 'disabled' : ''} title="${esc(t('Move up'))}" aria-label="${esc(t('Move up'))}"><i class="fa-solid fa-chevron-up"></i></button>
      <button type="button" class="mpf-cbtn" data-down="${esc(m.slug)}" ${opts.last ? 'disabled' : ''} title="${esc(t('Move down'))}" aria-label="${esc(t('Move down'))}"><i class="fa-solid fa-chevron-down"></i></button>
    </div>`;
    return `<div class="mpf-card-wrap">${card}${ctl}</div>`;
  }

  // ─── Owner controls ────────────────────────────────────────────────
  function wire(p) {
    if (!p.is_owner) return;
    const w = (id, fn) => { const el = document.getElementById(id); if (el) el.addEventListener('click', fn); };
    w('mpf-edit', openEditProfile);
    w('mpf-edit-readme', openEditReadme);
    w('mpf-avatar-btn', () => openImageUpload(false));
    w('mpf-banner-btn', () => openImageUpload(true));
    const banner = document.getElementById('mpf-banner-btn');
    if (banner) banner.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); openImageUpload(true); } });
    const av = document.getElementById('mpf-avatar-btn');
    if (av) av.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); openImageUpload(false); } });
    document.querySelectorAll('[data-up]').forEach((b) => b.addEventListener('click', () => moveMod(b.getAttribute('data-up'), -1)));
    document.querySelectorAll('[data-down]').forEach((b) => b.addEventListener('click', () => moveMod(b.getAttribute('data-down'), 1)));
    document.querySelectorAll('[data-feature]').forEach((b) => b.addEventListener('click', () => toggleFeature(b.getAttribute('data-feature'))));
  }

  async function moveMod(slug, dir) {
    const ord = (state.profile.mod_order || []).slice();
    const i = ord.indexOf(slug); const j = i + dir;
    if (i < 0 || j < 0 || j >= ord.length) return;
    [ord[i], ord[j]] = [ord[j], ord[i]];
    const r = await apiJSON('/v1/mods/hub/me/profile', { method: 'PATCH', json: { mod_order: ord } });
    if (r.ok) { await refresh(r.data); } else toast(errMsg(r, 'Could not reorder your mods.'), true);
  }

  async function toggleFeature(slug) {
    const cur = state.profile.featured_slug;
    const r = await apiJSON('/v1/mods/hub/me/profile',
      { method: 'PATCH', json: { featured_slug: cur === slug ? '' : slug } });
    if (r.ok) { await refresh(r.data); } else toast(errMsg(r, 'Could not update.'), true);
  }

  function openEditProfile() {
    const p = state.profile;
    const m = openModal(t('Edit profile'), `<form class="mp-form" id="mpf-form">
      <label class="mp-form-field"><span>${esc(t('Display name'))}</span><input name="display_name" maxlength="80" value="${esc(p.display_name || '')}"></label>
      <label class="mp-form-field"><span>${esc(t('Tagline'))}</span><input name="tagline" maxlength="160" value="${esc(p.tagline || '')}" placeholder="${esc(t('Trove modder & retexture artist'))}"></label>
      <label class="mp-form-field"><span><i class="fa-brands fa-discord"></i> ${esc(t('Discord invite'))}</span><input name="discord_url" maxlength="300" value="${esc(p.discord_url || '')}" placeholder="https://discord.gg/…"></label>
      <label class="mp-form-field"><span><i class="fa-solid fa-globe"></i> ${esc(t('Website'))}</span><input name="website_url" maxlength="300" value="${esc(p.website_url || '')}" placeholder="https://…"></label>
      <label class="mp-form-field"><span><i class="fa-solid fa-heart"></i> ${esc(t('Donation links (one per line, up to 5)'))}</span><textarea name="donation_urls" rows="3" placeholder="https://ko-fi.com/you">${esc((p.donation_urls || []).join('\n'))}</textarea></label>
      <p class="mp-form-error" hidden></p>
      <div class="mp-form-actions">
        <button type="button" class="mp-btn" data-close>${esc(t('Cancel'))}</button>
        <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Save'))}</button>
      </div></form>`);
    m.wrap.querySelector('#mpf-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const f = e.target;
      const body = {
        display_name: f.display_name.value.trim(),
        tagline: f.tagline.value.trim(),
        discord_url: f.discord_url.value.trim(),
        website_url: f.website_url.value.trim(),
        donation_urls: f.donation_urls.value.split('\n').map((s) => s.trim()).filter(Boolean).slice(0, 5),
      };
      const r = await apiJSON('/v1/mods/hub/me/profile', { method: 'PATCH', json: body });
      if (r.ok) { m.close(); toast(t('Saved.')); await refresh(r.data); }
      else showFormError(f, errMsg(r, 'Could not save your profile.'));
    });
  }

  function openEditReadme() {
    const p = state.profile;
    const m = openModal(t('Edit README'), `<form class="mp-form" id="mpf-readme-form">
      <label class="mp-form-field"><span>${esc(t('About you (Markdown)'))}</span><textarea name="readme" rows="14" maxlength="40000">${esc(p.readme || '')}</textarea></label>
      <p class="mp-form-hint">${esc(t('Markdown + safe HTML (badges, images, tables) supported - make it yours.'))}</p>
      <p class="mp-form-error" hidden></p>
      <div class="mp-form-actions">
        <button type="button" class="mp-btn" data-close>${esc(t('Cancel'))}</button>
        <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Save'))}</button>
      </div></form>`, { wide: true });
    m.wrap.querySelector('#mpf-readme-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const f = e.target;
      const r = await apiJSON('/v1/mods/hub/me/profile', { method: 'PATCH', json: { readme: f.readme.value } });
      if (r.ok) { m.close(); toast(t('Saved.')); await refresh(r.data); }
      else showFormError(f, errMsg(r, 'Could not save README.'));
    });
  }

  function openImageUpload(banner) {
    const m = openModal(banner ? t('Profile banner') : t('Profile picture'), `<form class="mp-form" id="mpf-img-form">
      <label class="mp-form-field"><span>${esc(t('Image file'))}</span>
        <input type="file" name="file" accept="image/png,image/jpeg,image/webp,image/gif" required></label>
      <p class="mp-form-hint">${esc(banner ? t('A wide image works best (PNG / JPEG / WebP / GIF, ≤ 5 MB).') : t('A square image works best (≤ 5 MB).'))}</p>
      <p class="mp-form-error" hidden></p>
      <div class="mp-form-actions">
        <button type="button" class="mp-btn" data-close>${esc(t('Cancel'))}</button>
        <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Upload'))}</button>
      </div></form>`);
    m.wrap.querySelector('#mpf-img-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const f = e.target;
      if (!f.file.files.length) { showFormError(f, t('Choose an image.')); return; }
      const fd = new FormData(); fd.append('file', f.file.files[0]);
      const btn = f.querySelector('button[type=submit]'); btn.disabled = true;
      const r = await apiForm('/v1/mods/hub/me/profile/' + (banner ? 'banner' : 'avatar'), fd);
      btn.disabled = false;
      if (r.ok) { m.close(); toast(t('Uploaded.')); await refresh(r.data); }
      else showFormError(f, errMsg(r, 'Upload failed.'));
    });
  }

  // The PATCH/upload endpoints return the fresh profile DTO, so re-render from it.
  async function refresh(data) {
    if (data && data.handle) { state.profile = data; render(); }
    else await loadProfile();
  }

  // ─── Shared UI helpers ─────────────────────────────────────────────
  function openModal(title, bodyHTML, { wide = false } = {}) {
    const wrap = document.createElement('div');
    wrap.className = 'mp-modal';
    wrap.innerHTML = `<div class="mp-modal-backdrop" data-close></div>
      <div class="mp-modal-card ${wide ? 'wide' : ''}">
        <button type="button" class="mp-modal-close" data-close aria-label="Close"><i class="fa-solid fa-xmark"></i></button>
        <h2 class="mp-modal-title">${esc(title)}</h2>
        ${bodyHTML}
      </div>`;
    $modalRoot.appendChild(wrap);
    const close = () => { wrap.remove(); document.removeEventListener('keydown', onKey); };
    const onKey = (e) => { if (e.key === 'Escape') close(); };
    document.addEventListener('keydown', onKey);
    wrap.querySelectorAll('[data-close]').forEach((b) => b.addEventListener('click', close));
    rerunI18n();
    return { wrap, close };
  }
  function showFormError(form, msg) {
    const el = form.querySelector('.mp-form-error');
    if (el) { el.textContent = msg; el.hidden = false; }
  }
  function toast(msg, isErr) {
    const el = document.createElement('div');
    el.className = 'mp-toast' + (isErr ? ' mp-toast-err' : '');
    el.textContent = msg;
    document.body.appendChild(el);
    setTimeout(() => el.classList.add('show'), 10);
    setTimeout(() => { el.classList.remove('show'); setTimeout(() => el.remove(), 300); }, 2600);
  }
  function fmtDate(iso) {
    try { return new Date(iso).toLocaleDateString(undefined, { year: 'numeric', month: 'short' }); }
    catch (_) { return ''; }
  }
})();
