/* ═══════════════════════════════════════════════════════════════════════
   /modpacks - Modpacks listing (Beta)
   ───────────────────────────────────────────────────────────────────────
   Search + sort over a paginated card grid, backed by the same-origin
   /site/modpacks/* proxies (tokenless). A modpack is a curated bundle of
   Mods Hub mods; cards link to /modpacks/<handle>/<slug>. Signed-in users
   (window.BTTAuth) get "Create a modpack" + "My modpacks". Images are shared
   with the Mods Hub CAS (served at /site/mods/image/<sha>).
   ═══════════════════════════════════════════════════════════════════════ */

(function () {
  'use strict';

  const { esc } = window.BTTUtil;

  const PAGE_SIZE = 30;

  const state = {
    q: '', sort: 'recent',
    offset: 0, items: [], total: 0, loading: false,
  };

  const $ = (id) => document.getElementById(id);
  const $grid = $('mh-grid');
  const $meta = $('mh-meta');
  const $foot = $('mh-foot');
  const $loadMore = $('mh-load-more');
  const $search = $('mh-search');
  const $sort = $('mh-sort');

  const imageUrl = (sha) => BTTUtil.apiUrl('/site/mods/image/' + encodeURIComponent(sha));
  // Modpacks are addressed as /modpacks/<owner_handle>/<slug>.
  const packUrl = (p) => '/modpacks/' + encodeURIComponent(p.handle) + '/' + encodeURIComponent(p.slug);
  const t = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);
  function rerunI18n() { if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh(); }

  // ─── Boot ──────────────────────────────────────────────────────────
  init().catch((err) => {
    console.error('[modpacks] boot failed', err);
    $grid.innerHTML = `<p class="mh-error">${esc(t('Failed to load modpacks.'))}</p>`;
    rerunI18n();
  });

  async function init() {
    wireEvents();
    wireAuth();
    wireCreate();
    await loadPage(true);
  }

  function wireEvents() {
    let timer = null;
    $search.addEventListener('input', () => {
      clearTimeout(timer);
      timer = setTimeout(() => { state.q = $search.value.trim(); loadPage(true); }, 250);
    });
    $sort.addEventListener('change', () => { state.sort = $sort.value; loadPage(true); });
    $loadMore.addEventListener('click', () => loadPage(false));
  }

  // Show "Create" / "My modpacks" for signed-in users, "Sign in" otherwise.
  function wireAuth() {
    const apply = (user) => {
      const authed = !!user;
      $('mh-create').hidden = !authed;
      $('mh-mine').hidden = !authed;
      $('mh-signin').hidden = authed;
      $('mh-mine').setAttribute('href', '#');
    };
    apply(window.BTTAuth && window.BTTAuth.getCachedUser ? window.BTTAuth.getCachedUser() : null);
    if (window.BTTAuth && window.BTTAuth.getMe) {
      window.BTTAuth.getMe().then(apply).catch(() => {});
    }
    $('mh-mine').addEventListener('click', async (e) => {
      e.preventDefault();
      try {
        const r = await fetch('/site/modpacks/me/projects', { credentials: 'include' });
        const data = r.ok ? await r.json() : { items: [] };
        if (data.items && data.items.length) {
          location.href = packUrl(data.items[0]);
        } else {
          openCreate();
        }
      } catch (_) { openCreate(); }
    });
  }

  // ─── Data ──────────────────────────────────────────────────────────
  async function loadPage(reset) {
    if (state.loading) return;
    state.loading = true;
    if (reset) { state.offset = 0; state.items = []; }
    const params = new URLSearchParams({
      sort: state.sort, limit: String(PAGE_SIZE), offset: String(state.offset),
    });
    if (state.q) params.set('q', state.q);
    try {
      const r = await fetch('/site/modpacks/projects?' + params.toString());
      if (!r.ok) throw new Error('HTTP ' + r.status);
      const data = await r.json();
      state.total = data.total || 0;
      state.items = reset ? data.items : state.items.concat(data.items);
      state.offset = state.items.length;
      render();
    } catch (err) {
      console.error('[modpacks] load failed', err);
      $grid.innerHTML = `<p class="mh-error">${esc(t('Failed to load modpacks.'))}</p>`;
    } finally {
      state.loading = false;
    }
  }

  // ─── Render ────────────────────────────────────────────────────────
  function render() {
    if (!state.items.length) {
      $grid.innerHTML = `<p class="mh-empty">${esc(t('No modpacks yet. Be the first to publish one!'))}</p>`;
      $meta.textContent = '';
      $foot.hidden = true;
      rerunI18n();
      return;
    }
    $meta.textContent = t('Showing') + ' ' + state.items.length + ' / ' + state.total;
    $grid.innerHTML = state.items.map(cardHTML).join('');
    $foot.hidden = state.items.length >= state.total;
    rerunI18n();
  }

  function cardHTML(p) {
    // No banner? Fall back to the first preview image for the card thumbnail.
    const cardSha = p.banner_sha || p.preview_sha || null;
    const banner = cardSha
      ? `<img class="mh-card-banner" src="${imageUrl(cardSha)}" alt="" loading="lazy">`
      : `<div class="mh-card-banner placeholder"><i class="fa-solid fa-box-open" aria-hidden="true"></i></div>`;
    const tags = (p.tags || []).slice(0, 4)
      .map((tg) => `<span class="mh-card-tag">${esc(tg)}</span>`).join('');
    const badge = p.visibility === 'draft'
      ? `<span class="mh-badge mh-badge-draft">${esc(t('Draft'))}</span>`
      : p.visibility === 'unlisted'
        ? `<span class="mh-badge mh-badge-unlisted">${esc(t('Unlisted'))}</span>` : '';
    const mods = Number(p.mod_count || 0);
    const variants = Number(p.variant_count || 1);
    const modLabel = mods === 1 ? t('mod') : t('mods');
    const variantChip = variants > 1
      ? `<span class="mhp-chip"><i class="fa-solid fa-layer-group" aria-hidden="true"></i> ${variants} ${esc(t('editions'))}</span>` : '';
    return `<a class="mh-card" href="${packUrl(p)}">
      ${banner}
      <div class="mh-card-body">
        <h3 class="mh-card-title">${esc(p.title)} ${badge}</h3>
        ${p.summary ? `<p class="mh-card-summary">${esc(p.summary)}</p>` : ''}
        <div class="mhp-card-chips">
          <span class="mhp-chip"><i class="fa-solid fa-cube" aria-hidden="true"></i> ${mods} ${esc(modLabel)}</span>
          ${variantChip}
        </div>
        ${tags ? `<div class="mh-card-tags">${tags}</div>` : ''}
        <div class="mh-card-foot">
          <span class="mh-card-author"><i class="fa-solid fa-user" aria-hidden="true"></i> ${esc(p.owner_username)}</span>
          <span class="mh-card-stats">
            <span class="mh-card-dl"><i class="fa-solid fa-download" aria-hidden="true"></i> ${Number(p.download_count || 0).toLocaleString()}</span>
            ${Number(p.star_count) > 0 ? `<span class="mh-card-dl" title="${esc(t('Favorites'))}"><i class="fa-solid fa-star" aria-hidden="true"></i> ${Number(p.star_count).toLocaleString()}<span class="sr-only">${esc(t('favorites'))}</span></span>` : ''}
          </span>
        </div>
      </div>
    </a>`;
  }

  // ─── Create modal ──────────────────────────────────────────────────
  function wireCreate() {
    const modal = $('mh-create-modal');
    $('mh-create').addEventListener('click', openCreate);
    modal.querySelectorAll('[data-close]').forEach((b) => b.addEventListener('click', closeCreate));
    document.addEventListener('keydown', (e) => { if (e.key === 'Escape') closeCreate(); });
    $('mh-create-form').addEventListener('submit', submitCreate);
  }

  let releaseCreateFocus = null;
  function openCreate() {
    if (!(window.BTTAuth && window.BTTAuth.getCachedUser && window.BTTAuth.getCachedUser())) {
      location.href = '/login';
      return;
    }
    const modal = $('mh-create-modal');
    modal.hidden = false;
    const card = modal.querySelector('.mh-modal-card');
    if (card && window.BTTUtil && window.BTTUtil.trapFocus) {
      releaseCreateFocus = window.BTTUtil.trapFocus(card, { onEscape: closeCreate });
    }
  }
  function closeCreate() {
    if (releaseCreateFocus) { releaseCreateFocus(); releaseCreateFocus = null; }
    $('mh-create-modal').hidden = true;
  }

  async function submitCreate(e) {
    e.preventDefault();
    const form = e.target;
    const err = $('mh-create-error');
    err.hidden = true;
    const tags = (form.tags.value || '').split(',').map((s) => s.trim()).filter(Boolean);
    const body = {
      title: form.title.value.trim(),
      summary: form.summary.value.trim(),
      tags,
      visibility: form.visibility.value,
    };
    const btn = form.querySelector('button[type=submit]');
    btn.disabled = true;
    try {
      const r = await window.BTTAuth.callJSON('/v1/modpacks/hub/projects', { json: body });
      if (r.ok && r.data && r.data.slug) {
        location.href = packUrl(r.data);
        return;
      }
      err.textContent = (r.data && r.data.error && r.data.error.message) || t('Could not create the modpack.');
      err.hidden = false;
    } catch (_) {
      err.textContent = t('Could not create the modpack.');
      err.hidden = false;
    } finally {
      btn.disabled = false;
    }
  }
})();
