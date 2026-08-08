/* ═══════════════════════════════════════════════════════════════════════
   /mods - Mods Hub listing (Beta)
   ───────────────────────────────────────────────────────────────────────
   Search + sort + tag filter over a paginated banner-card grid, backed by the
   same-origin /site/mods/* proxies (tokenless). When a site user is signed in
   (window.BTTAuth), the hero exposes "Create a mod" (POST to the /v1/mods/hub
   write API) and "My mods". No login is needed to browse or download.
   ═══════════════════════════════════════════════════════════════════════ */

(function () {
  'use strict';

  const { esc } = window.BTTUtil;

  const PAGE_SIZE = 30;

  // One icon per fixed category (the list in mods_project.js / mod_categories.py).
  // Anything the server sends that isn't on here falls back to a plain tag glyph.
  const CAT_ICONS = {
    Allies: 'fa-dove', Banners: 'fa-flag', 'Boats and Sails': 'fa-sailboat',
    Cosmetics: 'fa-wand-magic-sparkles', Costumes: 'fa-shirt', Dragons: 'fa-dragon',
    Fishing: 'fa-fish', GUI: 'fa-table-columns', Helmets: 'fa-hat-wizard',
    Language: 'fa-language', 'Mag Riders': 'fa-person-skating', Mounts: 'fa-horse',
    NPCs: 'fa-users', Wings: 'fa-feather-pointed', Automation: 'fa-robot',
    Optimization: 'fa-gauge-high', Reskin: 'fa-palette', Waypoint: 'fa-location-dot',
    Radar: 'fa-satellite-dish',
  };

  const state = {
    q: '', tag: '', sort: 'recent',
    offset: 0, items: [], total: 0, loading: false,
    facets: { categories: [], custom: [] },
  };

  const $ = (id) => document.getElementById(id);
  const $grid = $('mh-grid');
  const $meta = $('mh-meta');
  const $foot = $('mh-foot');
  const $loadMore = $('mh-load-more');
  const $search = $('mh-search');
  const $sort = $('mh-sort');
  const $tagbar = $('mh-tagbar');
  const $tags = $('mh-tags');

  const imageUrl = (sha) => BTTUtil.apiUrl('/site/mods/image/' + encodeURIComponent(sha));
  // Mods are addressed as /mods/<owner_handle>/<slug>.
  const modUrl = (m) => '/mods/' + encodeURIComponent(m.handle) + '/' + encodeURIComponent(m.slug);
  const t = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);
  function rerunI18n() { if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh(); }

  // A creator's own translations follow the site language, so re-render on it.
  document.addEventListener('btt-lang-changed', () => { if (state.items.length) render(); });

  // ─── Boot ──────────────────────────────────────────────────────────
  init().catch((err) => {
    console.error('[mods] boot failed', err);
    $grid.innerHTML = `<p class="mh-error">${esc(t('Failed to load mods.'))}</p>`;
    rerunI18n();
  });

  async function init() {
    wireEvents();
    wireAuth();
    wireCreate();
    // Deep link from elsewhere (e.g. the "Create a mod" CTA on /mods/why).
    // openCreate() itself redirects to /login if the visitor isn't signed in.
    if (location.hash === '#create') openCreate();
    await loadFacets();
    await loadPage(true);
  }

  // Tag facets (counts) for the filter bar: categories first, then custom tags.
  async function loadFacets() {
    try {
      const r = await fetch('/site/mods/tags');
      if (r.ok) state.facets = await r.json();
    } catch (_) { /* tagbar just stays hidden if this fails */ }
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

  // Show "Create" / "My mods" for signed-in users, "Sign in" otherwise.
  function wireAuth() {
    const apply = (user) => {
      const authed = !!user;
      $('mh-create').hidden = !authed;
      $('mh-mine').hidden = !authed;
      $('mh-signin').hidden = authed;
      $('mh-mine').setAttribute('href', '#');   // click handler routes dynamically
    };
    apply(window.BTTAuth && window.BTTAuth.getCachedUser ? window.BTTAuth.getCachedUser() : null);
    if (window.BTTAuth && window.BTTAuth.getMe) {
      window.BTTAuth.getMe().then(apply).catch(() => {});
    }
    // "My mods" routes to the user's most recent project (or the create modal).
    $('mh-mine').addEventListener('click', async (e) => {
      e.preventDefault();
      try {
        const r = await fetch('/site/mods/me/projects', { headers: authHeader() });
        const data = r.ok ? await r.json() : { items: [] };
        if (data.items && data.items.length) {
          location.href = modUrl(data.items[0]);
        } else {
          openCreate();
        }
      } catch (_) { openCreate(); }
    });
  }

  function authHeader() {
    const tok = window.BTTAuth && window.BTTAuth.tokens ? window.BTTAuth.tokens.access : null;
    return tok ? { Authorization: 'Bearer ' + tok } : {};
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
    if (state.tag) params.set('tag', state.tag);
    try {
      const r = await fetch('/site/mods/projects?' + params.toString());
      if (!r.ok) throw new Error('HTTP ' + r.status);
      const data = await r.json();
      state.total = data.total || 0;
      state.items = reset ? data.items : state.items.concat(data.items);
      state.offset = state.items.length;
      render();
    } catch (err) {
      console.error('[mods] load failed', err);
      $grid.innerHTML = `<p class="mh-error">${esc(t('Failed to load mods.'))}</p>`;
    } finally {
      state.loading = false;
    }
  }

  // ─── Render ────────────────────────────────────────────────────────
  function render() {
    renderTags();
    if (!state.items.length) {
      $grid.innerHTML = `<p class="mh-empty">${esc(t('No mods found. Be the first to publish one!'))}</p>`;
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

  function renderTags() {
    const cats = state.facets.categories || [];
    const custom = state.facets.custom || [];
    if (!cats.length && !custom.length) { $tagbar.hidden = true; return; }
    $tagbar.hidden = false;
    const cur = state.tag.toLowerCase();
    // A category is a fixed, known thing, so it gets a face. Anything not on the
    // list is a creator's own tag and renders in the quieter tier below.
    const catChip = (tg, n) => {
      const on = cur === String(tg).toLowerCase();
      return `<button type="button" class="mh-cat${on ? ' active' : ''}" data-tag="${esc(tg)}" aria-pressed="${on}">`
        + `<i class="fa-solid ${CAT_ICONS[tg] || 'fa-tag'}" aria-hidden="true"></i>`
        + `<span class="mh-cat-name">${esc(tg)}</span>`
        + `<span class="mh-cat-count">${Number(n).toLocaleString()}</span></button>`;
    };
    const allChip = `<button type="button" class="mh-cat mh-cat-all${state.tag ? '' : ' active'}" data-tag="" aria-pressed="${!state.tag}">`
      + '<i class="fa-solid fa-border-all" aria-hidden="true"></i>'
      + `<span class="mh-cat-name">${esc(t('All'))}</span></button>`;
    // Creator tags are free text, so they read as text: a hash, the word, the
    // count. No icon to invent, and visibly a lighter weight of thing than the
    // curated set above - which one dashed rule between identical pills never
    // managed to say (and never said at all to a screen reader, being hidden).
    const tagChip = (tg, n) => {
      const on = cur === String(tg).toLowerCase();
      return `<button type="button" class="mh-utag${on ? ' active' : ''}" data-tag="${esc(tg)}" aria-pressed="${on}">`
        + `<span class="mh-utag-hash" aria-hidden="true">#</span>${esc(tg)}`
        + `<span class="mh-utag-count">${Number(n).toLocaleString()}</span></button>`;
    };
    const tier = (label, inner) => `<div class="mh-tier" role="group" aria-label="${esc(label)}">
        <span class="mh-tier-label">${esc(label)}</span>
        <div class="mh-tags">${inner}</div>
      </div>`;
    let html;
    if (cats.length) {
      html = tier(t('Categories'), allChip + cats.map((c) => catChip(c.tag, c.count)).join(''));
      if (custom.length) html += tier(t('Tags'), custom.map((c) => tagChip(c.tag, c.count)).join(''));
    } else {
      html = tier(t('Tags'), allChip + custom.map((c) => tagChip(c.tag, c.count)).join(''));
    }
    $tags.innerHTML = html;
    $tags.querySelectorAll('[data-tag]').forEach((b) => b.addEventListener('click', () => {
      state.tag = b.getAttribute('data-tag');
      loadPage(true);
    }));
  }

  function cardHTML(p) {
    // No banner? Fall back to the first preview image for the card thumbnail.
    const cardSha = p.banner_sha || p.preview_sha || null;
    const banner = cardSha
      ? `<img class="mh-card-banner" src="${imageUrl(cardSha)}" alt="" loading="lazy">`
      : `<div class="mh-card-banner placeholder"><i class="fa-solid fa-cube" aria-hidden="true"></i></div>`;
    const tags = (p.tags || []).slice(0, 4)
      .map((tg) => `<span class="mh-card-tag">${esc(tg)}</span>`).join('');
    const badge = p.visibility === 'draft'
      ? `<span class="mh-badge mh-badge-draft">${esc(t('Draft'))}</span>`
      : p.visibility === 'unlisted'
        ? `<span class="mh-badge mh-badge-unlisted">${esc(t('Unlisted'))}</span>` : '';
    // "Uploaded" = shared on the creator's behalf (uploader isn't the author).
    const uploadedBadge = p.uploaded_on_behalf
      ? `<span class="mh-badge mh-badge-uploaded">${esc(t('Uploaded'))}</span>` : '';
    // Attribution: an uploaded mod credits the named creator, with the uploader as a
    // muted secondary line; otherwise the owner is the author.
    const authorLine = p.uploaded_on_behalf
      ? `<span class="mh-card-author"><i class="fa-solid fa-user" aria-hidden="true"></i> ${esc(p.author || '')}<small class="mh-card-uploader">${esc(t('Uploaded by'))} ${esc(p.owner_username)}</small></span>`
      : `<span class="mh-card-author"><i class="fa-solid fa-user" aria-hidden="true"></i> ${esc(p.owner_username)}</span>`;
    const lineage = p.forked_from
      ? `<p class="mh-card-lineage"><i class="fa-solid fa-code-fork"></i> ${esc(t('Forked from'))} ${esc(p.forked_from.title || p.forked_from.slug)}</p>`
      : p.inspired_by
        ? `<p class="mh-card-lineage"><i class="fa-solid fa-lightbulb"></i> ${esc(t('Inspired by'))} ${esc(p.inspired_by.title || p.inspired_by.slug)}</p>` : '';
    // The creator may have written the card's text in the reader's language.
    const localSummary = BTTUtil.localized(p.summary, p.summary_i18n);
    return `<a class="mh-card" href="${modUrl(p)}">
      ${banner}
      <div class="mh-card-body">
        <h3 class="mh-card-title">${esc(BTTUtil.localized(p.title, p.title_i18n))} ${badge}${uploadedBadge}</h3>
        ${lineage}
        ${localSummary ? `<p class="mh-card-summary">${esc(localSummary)}</p>` : ''}
        ${tags ? `<div class="mh-card-tags">${tags}</div>` : ''}
        <div class="mh-card-foot">
          ${authorLine}
          <span class="mh-card-stats">
            <span class="mh-card-dl"><i class="fa-solid fa-download" aria-hidden="true"></i> ${Number(p.download_count || 0).toLocaleString()}</span>
            <span class="mh-card-dl"><i class="fa-solid fa-star" aria-hidden="true"></i> ${Number(p.star_count || 0).toLocaleString()}</span>
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
    // "Made by someone else" reveals the creator-name field and forces releases-only
    // mode (you can't own the source of a mod you're just sharing on the author's behalf).
    const onBehalf = $('mh-create-onbehalf');
    if (onBehalf) onBehalf.addEventListener('change', () => {
      const on = onBehalf.checked;
      const credited = $('mh-create-credited');
      const modeField = $('mh-create-mode');
      if (credited) credited.hidden = !on;
      if (modeField) modeField.hidden = on;
      const modeSel = modeField && modeField.querySelector('select[name=mode]');
      if (modeSel && on) modeSel.value = 'releases';
      const creditedInput = credited && credited.querySelector('input[name=credited_author]');
      if (creditedInput) creditedInput.required = on;
    });
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
    const onBehalf = !!(form.on_behalf && form.on_behalf.checked);
    const body = {
      title: form.title.value.trim(),
      summary: form.summary.value.trim(),
      tags,
      mode: onBehalf ? 'releases' : form.mode.value,
      visibility: form.visibility.value,
      on_behalf: onBehalf,
      credited_author: onBehalf ? (form.credited_author.value || '').trim() : null,
    };
    if (onBehalf && !body.credited_author) {
      err.textContent = t('Name the creator this mod was made by.');
      err.hidden = false;
      return;
    }
    const btn = form.querySelector('button[type=submit]');
    btn.disabled = true;
    try {
      const r = await window.BTTAuth.callJSON('/v1/mods/hub/projects', { json: body });
      if (r.ok && r.data && r.data.slug) {
        location.href = modUrl(r.data);
        return;
      }
      err.textContent = (r.data && r.data.error && r.data.error.message) || t('Could not create the mod.');
      err.hidden = false;
    } catch (_) {
      err.textContent = t('Could not create the mod.');
      err.hidden = false;
    } finally {
      btn.disabled = false;
    }
  }
})();
