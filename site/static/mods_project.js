/* ═══════════════════════════════════════════════════════════════════════
   /mods/{slug} - single mod page + inline studio (Beta)
   ───────────────────────────────────────────────────────────────────────
   Public read (banner, previews, description, releases + download, file /
   commit browser) via the same-origin /site/mods/* proxies; when the signed-in
   site user owns the mod, the studio controls (edit, upload images, branch,
   commit files, cut releases, delete) post to the /v1/mods/hub/* write API via
   window.BTTAuth. Non-owners who are signed in can report the mod.
   ═══════════════════════════════════════════════════════════════════════ */

(function () {
  'use strict';
  const toast = window.BTTToast.show;

  const { esc } = window.BTTUtil;

  // Path is the source of truth (/mods/{handle}/{slug}); the server-rendered meta
  // is a fallback (skipped if it arrives unrendered, e.g. the static dev server).
  const _metaSlug = (document.querySelector('meta[name="mh-slug"]') || {}).content || '';
  const _metaHandle = (document.querySelector('meta[name="mh-handle"]') || {}).content || '';
  const _clean = (v) => (v && v.indexOf('{{') === -1 ? v : '');
  const _segs = location.pathname.replace(/^\/mods\//, '').split('/');
  const HANDLE = decodeURIComponent(_segs[0] || '') || _clean(_metaHandle);
  const SLUG = decodeURIComponent(_segs[1] || '') || _clean(_metaSlug);
  // Mods are addressed as <owner_handle>/<slug> on every API + page URL.
  const PROJ_PATH = encodeURIComponent(HANDLE) + '/' + encodeURIComponent(SLUG);
  const modUrl = (m) => '/mods/' + encodeURIComponent(m.handle) + '/' + encodeURIComponent(m.slug);

  const state = { detail: null, viewer: null, branch: null };

  const $root = document.getElementById('mp-root');
  const $modalRoot = document.getElementById('mp-modal-root');

  const t = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);
  const imageUrl = (sha) => '/site/mods/image/' + encodeURIComponent(sha);
  function rerunI18n() { if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh(); }

  // Mod categories (mirrors app/trove/mod_categories.py). Selected ones are saved
  // as tags; the server also encodes them as a numeric `flags` bitmask in the .tmod.
  const MOD_CATEGORIES = ['Allies', 'Banners', 'Boats and Sails', 'Cosmetics', 'Costumes',
    'Dragons', 'Fishing', 'GUI', 'Helmets', 'Language', 'Mag Riders', 'Mounts', 'NPCs',
    'Wings', 'Automation', 'Optimization', 'Reskin', 'Waypoint', 'Radar'];
  const _CAT_LOWER = new Set(MOD_CATEGORIES.map((c) => c.toLowerCase()));
  const isCategory = (tag) => _CAT_LOWER.has(String(tag).trim().toLowerCase());

  function authHeader() {
    const tok = window.BTTAuth && window.BTTAuth.tokens ? window.BTTAuth.tokens.access : null;
    return tok ? { Authorization: 'Bearer ' + tok } : {};
  }

  // Same-origin read with a one-shot token refresh, so an owner whose access
  // token aged out still sees their drafts after the silent refresh.
  async function siteGET(path) {
    let r = await fetch(path, { headers: authHeader() });
    if ((r.status === 401 || r.status === 404) && window.BTTAuth
        && window.BTTAuth.tokens && window.BTTAuth.tokens.refresh) {
      if (await window.BTTAuth.refresh()) r = await fetch(path, { headers: authHeader() });
    }
    return r;
  }

  // JSON write to the /v1 API (api.aallyn.net) through BTTAuth (adds bearer +
  // refresh-on-401).
  async function apiJSON(path, opts) { return window.BTTAuth.callJSON(path, opts); }

  // Multipart write to the /v1 API.
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
    await loadDetail();
  }

  async function loadDetail() {
    try {
      const r = await siteGET('/site/mods/projects/' + PROJ_PATH);
      if (!r.ok) {
        // A real draft the viewer can't see yet reads as "not public" (distinct
        // error code) rather than a generic "not found".
        let code = '';
        try { code = (await r.json()).error.code || ''; } catch (_) { /* no body */ }
        const msg = code === 'not_public'
          ? t('This mod is not public yet.')
          : t('This mod could not be found.');
        $root.innerHTML = `<p class="mp-error">${esc(msg)}</p>`;
        return;
      }
      state.detail = await r.json();
      state.branch = state.detail.default_branch || 'main';
      render();
    } catch (err) {
      console.error('[mod] load failed', err);
      $root.innerHTML = `<p class="mp-error">${esc(t('Failed to load this mod.'))}</p>`;
    }
  }

  // ─── Render ────────────────────────────────────────────────────────
  function render() {
    const d = state.detail;
    const ownerHasPreviews = (d.preview_shas && d.preview_shas.length) || d.is_owner;
    const parts = [headerHTML(d)];

    if (d.source_visible) {
      // GitHub-style: the file browser + a rendered README on the left (wide);
      // a right sidebar with About → releases → commit history → clone → forks.
      const mainCol = [];
      if (ownerHasPreviews) mainCol.push(previewsHTML(d));
      mainCol.push(filesHTML(d));
      mainCol.push(readmeHTML());                 // populated from the tree's README.md, if any
      const sideCol = [descriptionHTML(d), releasesHTML(d), modpacksHTML(), historyHTML(), cloneHTML(d)];
      if (d.fork_count) sideCol.push(forksHTML());
      parts.push(`<div class="mp-layout">
        <div class="mp-col-main">${mainCol.join('')}</div>
        <aside class="mp-col-side">${sideCol.filter(Boolean).join('')}</aside>
      </div>`);
    } else {
      // No file browser (releases-only / private source) -> single column.
      parts.push(descriptionHTML(d));
      if (ownerHasPreviews) parts.push(previewsHTML(d));
      parts.push(readmeTextHTML(d));        // releases-only README (saved text)
      parts.push(releasesHTML(d));
      parts.push(modpacksHTML());
      if (d.fork_count) parts.push(forksHTML());
    }

    $root.innerHTML = parts.join('');
    wireHeader();
    wireOwnerControls();          // contextual owner actions (banner/edit/commit/release/…)
    wireReleases();
    wireClone();
    wireFiles();
    if (d.source_visible) loadBranchViews();   // tree + commits (hidden if source private)
    if (d.fork_count) loadForks();
    loadModpacks();               // "Included in modpacks" backlink (hidden if none)
    rerunI18n();
  }

  // ─── "Included in modpacks" (backlink to packs that bundle this mod) ──
  function modpacksHTML() {
    // Rendered hidden; loadModpacks() reveals it only if there are packs.
    return `<section class="mp-section" id="mp-modpacks-section" hidden>
      <div class="mp-section-head"><h2 class="mp-section-title"><i class="fa-solid fa-box-open"></i> ${esc(t('Included in modpacks'))}</h2></div>
      <div id="mp-modpacks" class="mp-forklist"></div>
    </section>`;
  }

  async function loadModpacks() {
    const box = document.getElementById('mp-modpacks');
    const section = document.getElementById('mp-modpacks-section');
    if (!box || !section) return;
    try {
      const r = await fetch('/site/modpacks/for-mod/' + PROJ_PATH);
      const data = r.ok ? await r.json() : { items: [] };
      if (!data.items || !data.items.length) return;   // stay hidden
      box.innerHTML = data.items.map((p) => `<a class="mp-fork-item"
          href="/modpacks/${encodeURIComponent(p.handle)}/${encodeURIComponent(p.slug)}">
          <i class="fa-solid fa-box-open"></i>
          <span class="mp-fork-title">${esc(p.title)}</span>
          <span class="mp-muted">${esc(t('by'))} ${esc(p.owner_username)}</span>
        </a>`).join('');
      section.hidden = false;
      rerunI18n();
    } catch (_) { /* leave the section hidden */ }
  }

  function forksHTML() {
    return `<section class="mp-section">
      <div class="mp-section-head"><h2 class="mp-section-title"><i class="fa-solid fa-code-fork"></i> ${esc(t('Forks'))}</h2></div>
      <div id="mp-forks" class="mp-forklist"><p class="mp-muted">${esc(t('Loading…'))}</p></div>
    </section>`;
  }

  async function loadForks() {
    const box = document.getElementById('mp-forks');
    if (!box) return;
    try {
      const r = await fetch('/site/mods/projects/' + PROJ_PATH + '/forks');
      const data = r.ok ? await r.json() : { items: [] };
      box.innerHTML = data.items && data.items.length
        ? data.items.map((f) => `<a class="mp-fork-item" href="${modUrl(f)}">
            <i class="fa-solid fa-code-fork"></i>
            <span class="mp-fork-title">${esc(f.title)}</span>
            <span class="mp-muted">${esc(t('by'))} ${esc(f.owner_username)}</span>
          </a>`).join('')
        : `<p class="mp-muted">${esc(t('No public forks yet.'))}</p>`;
    } catch (_) { box.innerHTML = ''; }
  }

  function headerHTML(d) {
    const bannerInner = d.banner_sha
      ? `<img class="mp-banner" src="${imageUrl(d.banner_sha)}" alt="">`
      : `<div class="mp-banner placeholder"><i class="fa-solid fa-cube"></i></div>`;
    // The owner edits the banner by clicking it (no separate toolbar button).
    const banner = d.is_owner
      ? `<div class="mp-banner-wrap" id="mp-banner-btn" role="button" tabindex="0" title="${esc(t('Change banner'))}">${bannerInner}<span class="mp-banner-edit"><i class="fa-solid fa-camera"></i> ${esc(d.banner_sha ? t('Change banner') : t('Add banner'))}</span></div>`
      : bannerInner;
    const vis = d.visibility;
    const badge = vis === 'draft' ? `<span class="mp-badge mp-badge-draft">${esc(t('Draft'))}</span>`
      : vis === 'unlisted' ? `<span class="mp-badge mp-badge-unlisted">${esc(t('Unlisted'))}</span>`
      : `<span class="mp-badge mp-badge-public">${esc(t('Public'))}</span>`;
    const modeBadge = d.mode === 'releases'
      ? `<span class="mp-badge mp-badge-unlisted">${esc(t('Releases only'))}</span>` : '';
    const privBadge = (d.is_owner && d.source_visibility === 'private')
      ? `<span class="mp-badge mp-badge-draft">${esc(t('Private source'))}</span>` : '';
    const tags = (d.tags || []).map((x) => `<span class="mp-tag">${esc(x)}</span>`).join('');
    const taken = d.taken_down
      ? `<div class="mp-takedown"><i class="fa-solid fa-triangle-exclamation"></i> ${esc(t('This mod has been removed by a moderator.'))} ${d.takedown_reason ? esc(d.takedown_reason) : ''}</div>` : '';
    // Header download: a direct link for a single release, but a dropdown to pick
    // when there are several (so it never silently grabs just the latest).
    const published = (d.releases || []).filter((r) => r.status === 'published');
    const dlBranches = new Set(published.map((r) => r.branch || 'main'));
    const showDlBranch = dlBranches.size > 1;
    let dlBtn;
    if (!published.length) {
      dlBtn = `<span class="mp-muted">${esc(t('No release yet'))}</span>`;
    } else if (published.length === 1) {
      const r0 = published[0];
      dlBtn = `<a class="mp-btn mp-btn-primary" href="/site/mods/releases/${esc(r0.id)}/download"><i class="fa-solid fa-download"></i> ${esc(t('Download'))} <span class="mp-release-tagchip">${esc(r0.tag)}</span></a>`;
    } else {
      const items = published.map((r) => `<a class="mp-dl-item" href="/site/mods/releases/${esc(r.id)}/download" role="menuitem">
          <span class="mp-release-tagchip">${esc(r.tag)}</span>
          ${showDlBranch ? `<span class="mp-dl-branch">${esc(r.branch || 'main')}</span>` : ''}
          <span class="mp-dl-size">${fmtBytes(r.tmod_size)}</span>
        </a>`).join('');
      dlBtn = `<div class="mp-dl-split">
        <button type="button" class="mp-btn mp-btn-primary mp-dl-toggle" id="mp-dl-toggle" aria-haspopup="true" aria-expanded="false">
          <i class="fa-solid fa-download"></i> ${esc(t('Download'))} <span class="mp-dl-caret" aria-hidden="true"></span>
        </button>
        <div class="mp-dl-menu" id="mp-dl-menu" role="menu" hidden>${items}</div>
      </div>`;
    }
    // Stray = an imported mod uploaded via contributions, with no owner here yet.
    const isStray = !!d.is_stray;
    const reportBtn = (!d.is_owner && !isStray && state.viewer)
      ? `<button type="button" class="mp-btn mp-btn-sm" id="mp-report"><i class="fa-solid fa-flag"></i> ${esc(t('Report'))}</button>` : '';
    // Claim: only on stray mods. A logged-out click routes to /login.
    const claimBtn = isStray
      ? `<button type="button" class="mp-btn mp-btn-sm" id="mp-claim"><i class="fa-solid fa-hand-sparkles"></i> ${esc(t('This is my mod'))}</button>` : '';
    const strayBadge = isStray
      ? `<span class="mp-badge mp-badge-stray"><i class="fa-solid fa-paper-plane"></i> ${esc(t('Stray'))}</span>` : '';
    // Uploaded = an account shared a mod someone else made. Owned by the uploader
    // (not claimable), credited to d.author. Distinct from an authored mod.
    const isUploaded = !isStray && !!d.uploaded_on_behalf;
    const uploadedBadge = isUploaded
      ? `<span class="mp-badge mp-badge-uploaded"><i class="fa-solid fa-share-from-square"></i> ${esc(t('Uploaded'))}</span>` : '';
    // Star (favourite) - shown to everyone; a click while logged out routes to /login.
    const starred = !!d.starred;
    const starBtn = `<button type="button" class="mp-btn mp-btn-sm ${starred ? 'mp-starred' : ''}" id="mp-star" aria-pressed="${starred}">
        <i class="fa-${starred ? 'solid' : 'regular'} fa-star"></i> <span id="mp-star-count">${Number(d.star_count || 0).toLocaleString()}</span>
      </button>`;
    // Fork copies the source, so it's only offered when the source is visible.
    // A source-locked mod can instead be credited as inspiration.
    let forkBtn = '';
    // Uploaded-on-behalf mods can't be forked OR credited as inspiration - they're
    // not the uploader's work, so no lineage rides on them.
    if (!d.is_owner && !isStray && !isUploaded) {
      forkBtn = d.source_visible
        ? `<button type="button" class="mp-btn mp-btn-sm" id="mp-fork"><i class="fa-solid fa-code-fork"></i> ${esc(t('Fork'))}</button>`
        : `<button type="button" class="mp-btn mp-btn-sm" id="mp-inspire"><i class="fa-solid fa-lightbulb"></i> ${esc(t('Use as inspiration'))}</button>`;
    }
    const forkLink = (ref) => `<a href="${modUrl(ref)}">${esc(ref.title || ref.slug)}</a>`;
    let attribution = '';
    if (d.forked_from) {
      attribution = `<div class="mp-attribution"><i class="fa-solid fa-code-fork"></i> ${esc(t('Forked from'))} ${forkLink(d.forked_from)}${d.forked_from.owner ? ' ' + esc(t('by')) + ' ' + esc(d.forked_from.owner) : ''}</div>`;
    } else if (d.inspired_by) {
      attribution = `<div class="mp-attribution"><i class="fa-solid fa-lightbulb"></i> ${esc(t('Inspired by'))} ${forkLink(d.inspired_by)}${d.inspired_by.owner ? ' ' + esc(t('by')) + ' ' + esc(d.inspired_by.owner) : ''}</div>`;
    }
    const forkCount = d.fork_count
      ? `<span><i class="fa-solid fa-code-fork"></i> ${Number(d.fork_count)} ${esc(t('forks'))}</span>` : '';
    // Edit details + Settings live next to the title (right), not in a toolbar.
    const ownerTitleActions = d.is_owner ? `<div class="mp-title-actions">
        <button type="button" class="mp-btn mp-btn-sm" id="mp-edit"><i class="fa-solid fa-pen"></i> ${esc(t('Edit details'))}</button>
        ${d.is_primary_owner ? `<button type="button" class="mp-btn mp-btn-sm" id="mp-collab"><i class="fa-solid fa-user-group"></i> ${esc(t('Collaborate'))}</button>` : ''}
        <button type="button" class="mp-btn mp-btn-sm" id="mp-settings" aria-label="${esc(t('Settings'))}" title="${esc(t('Settings'))}"><i class="fa-solid fa-gear"></i></button>
      </div>` : '';
    // Owner-provided links (Discord invite / website / donation buttons).
    const links = [];
    if (d.discord_url) links.push(`<a class="mp-linkbtn" href="${esc(d.discord_url)}" target="_blank" rel="noopener"><i class="fa-brands fa-discord"></i> Discord</a>`);
    if (d.website_url) links.push(`<a class="mp-linkbtn" href="${esc(d.website_url)}" target="_blank" rel="noopener"><i class="fa-solid fa-globe"></i> ${esc(t('Website'))}</a>`);
    (d.donation_urls || []).forEach((u) => { const m = donateMeta(u); links.push(`<a class="mp-linkbtn mp-linkbtn-donate" href="${esc(u)}" target="_blank" rel="noopener nofollow"><i class="${m.cls}"></i> ${esc(m.label)}</a>`); });
    // No owner links (Discord / website / donations) on an uploaded mod - the
    // uploader isn't the author, so no soliciting or self-linking on their work.
    const linksRow = (links.length && !isUploaded) ? `<div class="mp-links">${links.join('')}</div>` : '';
    return `<header class="mp-header">
      ${banner}
      <div class="mp-header-body">
        ${taken}
        <div class="mp-titlerow">
          <h1 class="mp-title">${esc(d.title)}</h1> ${strayBadge} ${uploadedBadge} ${badge} ${modeBadge} ${privBadge}
          ${ownerTitleActions}
        </div>
        ${attribution}
        <div class="mp-meta">
          ${isStray
            ? `<span><i class="fa-solid fa-user"></i> ${esc(d.author || d.owner_username)}</span>`
            : isUploaded
              ? `<span><i class="fa-solid fa-share-from-square"></i> ${esc(t('Uploaded by'))} <a class="mp-author-link" href="/mods/${encodeURIComponent(d.handle || '')}">${esc(d.owner_username)}</a></span><span><i class="fa-solid fa-user"></i> ${esc(t('Created by'))} ${esc(d.author || '')}</span>`
              : `<span><i class="fa-solid fa-user"></i> <a class="mp-author-link" href="/mods/${encodeURIComponent(d.handle || '')}">${esc(d.owner_username)}</a></span>`}
          <span><i class="fa-solid fa-download"></i> ${Number(d.download_count || 0).toLocaleString()} ${esc(t('downloads'))}</span>
          ${(isStray || isUploaded) ? '' : `<span><i class="fa-solid fa-code-commit"></i> ${Number(d.commit_count || 0)} ${esc(t('commits'))}</span>`}
          ${forkCount}
        </div>
        ${isStray ? `<p class="mp-stray-note"><i class="fa-solid fa-circle-info"></i> ${esc(t('This mod was uploaded via contributions and hasn\'t been claimed by its author yet. If it\'s yours, claim it to manage it here.'))}</p>` : ''}
        ${isUploaded ? `<p class="mp-stray-note"><i class="fa-solid fa-circle-info"></i> ${esc(t('This mod was uploaded by a community member on the creator\'s behalf. It isn\'t an official release by the author.'))}</p>` : ''}
        ${d.summary ? `<p class="mp-summary">${esc(d.summary)}</p>` : ''}
        ${tags ? `<div class="mp-tags">${tags}</div>` : ''}
        ${linksRow}
        <div class="mp-actions">${dlBtn}${starBtn}${claimBtn}${forkBtn}${reportBtn}</div>
      </div>
    </header>`;
  }

  // Detect a donation platform from its URL for the button's icon + label.
  function donateMeta(url) {
    const u = (url || '').toLowerCase();
    if (u.includes('ko-fi') || u.includes('kofi')) return { cls: 'fa-solid fa-mug-hot', label: 'Ko-fi' };
    if (u.includes('patreon')) return { cls: 'fa-brands fa-patreon', label: 'Patreon' };
    if (u.includes('paypal')) return { cls: 'fa-brands fa-paypal', label: 'PayPal' };
    if (u.includes('buymeacoffee') || u.includes('buymeacoff.ee')) return { cls: 'fa-solid fa-mug-hot', label: 'Buy me a coffee' };
    if (u.includes('github.com/sponsors')) return { cls: 'fa-brands fa-github', label: 'Sponsor' };
    return { cls: 'fa-solid fa-heart', label: t('Donate') };
  }

  // Owner actions now live where they belong (banner click, title row, section
  // headers, branch dropdown) instead of a single toolbar - wire them all here.
  // Each lookup no-ops for non-owners (the buttons aren't rendered for them).
  function wireOwnerControls() {
    const w = (id, fn) => { const el = document.getElementById(id); if (el) el.addEventListener('click', fn); };
    w('mp-edit', openEdit);
    w('mp-collab', openCollab);
    w('mp-settings', openSettings);
    w('mp-edit-readme', openReadmeEdit);
    w('mp-banner-btn', openBanner);
    const bannerBtn = document.getElementById('mp-banner-btn');
    if (bannerBtn) bannerBtn.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); openBanner(); }
    });
    w('mp-commit', openCommit);
    w('mp-release', openRelease);
    w('mp-add-previews', openPreviews);
    document.querySelectorAll('[data-prev-del]').forEach((b) =>
      b.addEventListener('click', () => removePreview(b.getAttribute('data-prev-del'))));
    document.querySelectorAll('[data-zoom]').forEach((img) =>
      img.addEventListener('click', () => lightbox(img.getAttribute('data-zoom'))));
  }

  function descriptionHTML(d) {
    const body = d.description && d.description.trim()
      ? renderMarkdown(d.description)
      : `<p class="mp-muted">${esc(t('No description yet.'))}</p>`;
    return `<section class="mp-section">
      <div class="mp-section-head"><h2 class="mp-section-title"><i class="fa-solid fa-book"></i> ${esc(t('About'))}</h2></div>
      <div class="mp-markdown">${body}</div>
      ${warningsHTML(d)}
    </section>`;
  }

  // Owner-authored warnings: one highlighted block per `<br>`-separated segment.
  function warningsHTML(d) {
    const raw = (d.warnings || '').trim();
    if (!raw) return '';
    const blocks = raw.split(/<br\s*\/?>/i).map((s) => s.trim()).filter(Boolean);
    if (!blocks.length) return '';
    return `<div class="mp-warnings">${blocks.map((b) =>
      `<div class="mp-warnblock"><i class="fa-solid fa-triangle-exclamation"></i><div class="mp-warnblock-body">${sanitizeHTML(mdInline(b))}</div></div>`).join('')}</div>`;
  }

  function previewsHTML(d) {
    const shas = d.preview_shas || [];
    const cells = shas.map((sha) => `<div class="mp-preview">
      <img src="${imageUrl(sha)}" alt="" data-zoom="${imageUrl(sha)}" loading="lazy">
      ${d.is_owner ? `<button type="button" class="mp-preview-del" data-prev-del="${esc(sha)}" aria-label="Remove"><i class="fa-solid fa-xmark"></i></button>` : ''}
    </div>`).join('');
    const addBtn = d.is_owner
      ? `<button type="button" class="mp-btn mp-btn-sm" id="mp-add-previews"><i class="fa-solid fa-plus"></i> ${esc(t('Add previews'))}</button>` : '';
    return `<section class="mp-section">
      <div class="mp-section-head"><h2 class="mp-section-title"><i class="fa-solid fa-images"></i> ${esc(t('Previews'))}</h2>${addBtn}</div>
      ${cells ? `<div class="mp-previews">${cells}</div>` : `<p class="mp-muted">${esc(t('No previews yet.'))}</p>`}
    </section>`;
  }

  // Variant (branch) display order: the owner-set order first, then any variants
  // not in that list, alphabetically.
  function variantOrder(d) {
    const names = [...new Set((d.releases || []).map((r) => r.branch || 'main'))];
    const order = d.branch_order || [];
    return [
      ...order.filter((b) => names.includes(b)),
      ...names.filter((b) => !order.includes(b)).sort(),
    ];
  }

  async function moveVariant(branch, dir) {
    const ord = variantOrder(state.detail);
    const i = ord.indexOf(branch);
    const j = i + dir;
    if (i < 0 || j < 0 || j >= ord.length) return;
    [ord[i], ord[j]] = [ord[j], ord[i]];
    const r = await apiJSON('/v1/mods/hub/projects/' + PROJ_PATH,
      { method: 'PATCH', json: { branch_order: ord } });
    if (r.ok) { await loadDetail(); }
    else toast(errMsg(r, 'Could not reorder variants.'), true);
  }

  function releasesHTML(d) {
    const items = d.releases || [];
    let rows;
    if (!items.length) {
      rows = `<p class="mp-muted">${esc(t('No releases yet.'))} ${d.is_owner ? esc(t('Use “New release” to publish one.')) : ''}</p>`;
    } else {
      // Group by branch = variant; surface the latest of each, older collapsible.
      const groups = {};
      items.forEach((r) => { (groups[r.branch || 'main'] = groups[r.branch || 'main'] || []).push(r); });
      const hidden = new Set(d.hidden_release_branches || []);
      const ordered = variantOrder(d);          // owner-set order, then the rest alphabetically
      rows = ordered.map((branch, idx) => {
        const rels = groups[branch];            // latest first (server sorts -created_at)
        const older = rels.slice(1);
        const isHidden = hidden.has(branch);
        const toggle = d.is_owner
          ? `<button type="button" class="mp-btn mp-btn-sm" data-hide-branch="${esc(branch)}">${isHidden ? esc(t('Show publicly')) : esc(t('Hide from public'))}</button>` : '';
        const hiddenTag = (d.is_owner && isHidden)
          ? `<span class="mp-badge mp-badge-draft">${esc(t('Hidden'))}</span>` : '';
        // Owner reorders variants with up/down (first/last disabled).
        const reorder = (d.is_owner && ordered.length > 1) ? `<span class="mp-variant-move">
            <button type="button" class="mp-iconbtn" data-move-up="${esc(branch)}" ${idx === 0 ? 'disabled' : ''} aria-label="${esc(t('Move up'))}" title="${esc(t('Move up'))}"><i class="fa-solid fa-chevron-up"></i></button>
            <button type="button" class="mp-iconbtn" data-move-down="${esc(branch)}" ${idx === ordered.length - 1 ? 'disabled' : ''} aria-label="${esc(t('Move down'))}" title="${esc(t('Move down'))}"><i class="fa-solid fa-chevron-down"></i></button>
          </span>` : '';
        return `<div class="mp-variant">
          <div class="mp-variant-head">
            <span class="mp-variant-name"><i class="fa-solid fa-code-branch"></i> ${esc(branch)} ${hiddenTag}</span>
            <span class="mp-variant-actions">${reorder}${toggle}</span>
          </div>
          ${releaseRow(rels[0], d.is_owner)}
          ${older.length ? `<details class="mp-variant-older"><summary>${older.length + ' ' + esc(older.length === 1 ? t('older release') : t('older releases'))}</summary>${older.map((r) => releaseRow(r, d.is_owner)).join('')}</details>` : ''}
        </div>`;
      }).join('');
    }
    const newRelBtn = d.is_owner
      ? `<button type="button" class="mp-btn mp-btn-sm mp-btn-primary" id="mp-release"><i class="fa-solid fa-rocket"></i> ${esc(t('New release'))}</button>` : '';
    return `<section class="mp-section">
      <div class="mp-section-head"><h2 class="mp-section-title"><i class="fa-solid fa-rocket"></i> ${esc(t('Releases'))}</h2>${newRelBtn}</div>
      <p class="mp-muted" style="margin:0 0 12px">${esc(t('Each variant shows its latest release.'))}</p>
      <div id="mp-releases">${rows}</div>
    </section>`;
  }

  function releaseRow(r, owner) {
    const draft = r.status !== 'published'
      ? `<span class="mp-badge mp-badge-draft">${esc(t('Draft'))}</span>` : '';
    const ownerBtns = owner ? `
      <button type="button" class="mp-btn mp-btn-sm" data-rel-toggle="${esc(r.id)}" data-status="${esc(r.status)}">
        ${r.status === 'published' ? esc(t('Unpublish')) : esc(t('Publish'))}
      </button>
      <button type="button" class="mp-btn mp-btn-sm mp-btn-danger" data-rel-del="${esc(r.id)}"><i class="fa-solid fa-trash"></i></button>` : '';
    return `<div class="mp-release">
      <div class="mp-release-top">
        <span class="mp-release-tag"><span class="mp-release-tagchip">${esc(r.tag)}</span> ${esc(r.title || '')} ${draft}</span>
        <div class="mp-release-actions">
          ${r.status === 'published'
            ? `<a class="mp-btn mp-btn-sm mp-btn-primary" href="/site/mods/releases/${esc(r.id)}/download"><i class="fa-solid fa-download"></i> ${esc(t('Download'))}</a>`
            : `<button type="button" class="mp-btn mp-btn-sm mp-btn-primary" data-rel-dl="${esc(r.id)}" data-fn="${esc(r.tmod_filename)}"><i class="fa-solid fa-download"></i> ${esc(t('Download'))}</button>`}
          ${ownerBtns}
        </div>
      </div>
      <div class="mp-release-stats">
        <span><i class="fa-solid fa-download"></i> ${Number(r.download_count || 0).toLocaleString()}</span>
        <span><i class="fa-solid fa-file-zipper"></i> ${fmtBytes(r.tmod_size)}</span>
        ${r.published_at ? `<span><i class="fa-solid fa-clock"></i> ${fmtDate(r.published_at)}</span>` : ''}
      </div>
      ${r.changelog ? `<div class="mp-release-changelog">${esc(r.changelog)}</div>` : ''}
      ${r.format !== 'zip' ? `<div class="mp-release-3d" data-rel-bp="${esc(r.id)}" hidden></div>` : ''}
      ${r.format !== 'zip' ? `<div class="mp-release-vfx" data-rel-vfx="${esc(r.id)}" hidden></div>` : ''}
      ${r.format !== 'zip' ? `<details class="mp-release-files" data-rel-files="${esc(r.id)}">
        <summary class="mp-3d-summary"><i class="fa-solid fa-folder-open"></i> ${esc(t('Files'))}</summary>
        <div class="mp-release-files-list" data-files-box></div></details>` : ''}
    </div>`;
  }

  function filesHTML(d) {
    if (!d.source_visible) return '';   // releases-only or private-source: no files view
    const branches = d.branches || [];
    const opts = branches.map((b) =>
      `<option value="${esc(b.name)}" ${b.name === state.branch ? 'selected' : ''}>${esc(b.name)}</option>`).join('');
    // "New branch" is now an option in the branch dropdown (not a toolbar button).
    const newBranchOpt = d.is_owner ? `<option value="__newbranch__">+ ${esc(t('New branch…'))}</option>` : '';
    // "Commit files" sits right-aligned in the Files header (owner, files mode).
    const commitBtn = (d.is_owner && d.mode === 'files')
      ? `<button type="button" class="mp-btn mp-btn-sm mp-btn-primary" id="mp-commit"><i class="fa-solid fa-upload"></i> ${esc(t('Commit files'))}</button>` : '';
    return `<section class="mp-section">
      <div class="mp-section-head">
        <h2 class="mp-section-title"><i class="fa-solid fa-folder-tree"></i> ${esc(t('Files'))}</h2>
        ${commitBtn}
      </div>
      <div class="mp-fb-bar">
        <label class="mp-muted">${esc(t('Branch'))}</label>
        <select id="mp-branch-select">${opts || `<option>${esc(state.branch)}</option>`}${newBranchOpt}</select>
      </div>
      <div id="mp-placement"></div>
      <div id="mp-tree" class="mp-tree"></div>
    </section>`;
  }

  function historyHTML() {
    return `<section class="mp-section">
      <div class="mp-section-head"><h2 class="mp-section-title"><i class="fa-solid fa-clock-rotate-left"></i> ${esc(t('History'))}</h2></div>
      <div id="mp-commits"></div>
    </section>`;
  }

  function readmeHTML() {
    return `<section class="mp-section" id="mp-readme-section" hidden>
      <div class="mp-section-head"><h2 class="mp-section-title"><i class="fa-solid fa-book-open"></i> <span id="mp-readme-name">README</span></h2></div>
      <div id="mp-readme" class="mp-markdown"></div>
    </section>`;
  }

  // README for releases-only mode (saved as text - there are no files to hold a
  // README.md). In files mode the repo's README.md is rendered instead (see
  // loadReadme) and this text is ignored.
  function readmeTextHTML(d) {
    if (d.mode !== 'releases') return '';
    const has = d.readme_text && d.readme_text.trim();
    if (!has && !d.is_owner) return '';
    const editBtn = d.is_owner
      ? `<button type="button" class="mp-btn mp-btn-sm" id="mp-edit-readme"><i class="fa-solid fa-pen"></i> ${esc(has ? t('Edit README') : t('Add README'))}</button>` : '';
    const body = has ? renderMarkdown(d.readme_text)
      : `<p class="mp-muted">${esc(t('No README yet.'))}</p>`;
    return `<section class="mp-section">
      <div class="mp-section-head"><h2 class="mp-section-title"><i class="fa-solid fa-book-open"></i> ${esc(t('README'))}</h2>${editBtn}</div>
      <div class="mp-markdown">${body}</div>
    </section>`;
  }

  function openReadmeEdit() {
    const d = state.detail;
    const m = openModal(t('Edit README'), `<form class="mp-form" id="mp-readme-form">
      <label class="mp-form-field"><span>${esc(t('README (Markdown)'))}</span><textarea name="readme" rows="14" maxlength="60000">${esc(d.readme_text || '')}</textarea></label>
      <p class="mp-form-hint">${esc(t('Shown as the main content for releases-only mods. Markdown + safe HTML (badges, alignment, tables) supported.'))}</p>
      <p class="mp-form-error" hidden></p>
      <div class="mp-form-actions">
        <button type="button" class="mp-btn" data-close>${esc(t('Cancel'))}</button>
        <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Save'))}</button>
      </div></form>`, { wide: true });
    m.wrap.querySelector('#mp-readme-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const f = e.target;
      const r = await apiJSON('/v1/mods/hub/projects/' + PROJ_PATH,
        { method: 'PATCH', json: { readme_text: f.readme.value } });
      if (r.ok) { m.close(); toast(t('Saved.')); await loadDetail(); }
      else showFormError(f, errMsg(r, 'Could not save README.'));
    });
  }

  // Render the branch's README.md (root preferred) under the file browser.
  async function loadReadme(entries, commitId) {
    const sec = document.getElementById('mp-readme-section');
    if (!sec) return;
    const list = entries || [];
    const entry = list.find((e) => e.path.toLowerCase() === 'readme.md')
      || list.find((e) => e.path.toLowerCase().endsWith('/readme.md'));
    if (!entry || !commitId) { sec.hidden = true; return; }
    try {
      const url = `/site/mods/projects/${PROJ_PATH}/raw/${commitId}/`
        + entry.path.split('/').map(encodeURIComponent).join('/');
      const r = await siteGET(url);
      if (!r.ok) { sec.hidden = true; return; }
      document.getElementById('mp-readme').innerHTML = renderMarkdown(await r.text());
      const nameEl = document.getElementById('mp-readme-name');
      if (nameEl) nameEl.textContent = entry.path.split('/').pop();
      sec.hidden = false;
      rerunI18n();
    } catch (_) { sec.hidden = true; }
  }

  // ─── Clone with git ────────────────────────────────────────────────
  function cloneHTML(d) {
    if (!d.clone_url) return '';
    const tokensBtn = state.viewer
      ? `<button type="button" class="mp-btn mp-btn-sm" id="mp-git-tokens"><i class="fa-solid fa-key"></i> ${esc(t('Access tokens'))}</button>` : '';
    return `<section class="mp-section mp-clone-section">
      <details class="mp-clone">
        <summary class="mp-clone-summary">
          <i class="fa-brands fa-git-alt"></i> <span>${esc(t('Clone with git'))}</span>
          <i class="fa-solid fa-chevron-down mp-clone-caret" aria-hidden="true"></i>
        </summary>
        <div class="mp-clone-body">
          <div class="mp-clone-row">
            <code id="mp-clone-url">git clone ${esc(d.clone_url)}</code>
            <button type="button" class="mp-btn mp-btn-sm" id="mp-clone-copy" aria-label="Copy"><i class="fa-solid fa-copy"></i></button>
          </div>
          <p class="mp-muted">${esc(t('Use any username and a git access token as the password.'))} ${d.is_owner ? esc(t('Push to your branch to update the mod.')) : ''}</p>
          ${tokensBtn}
        </div>
      </details>
    </section>`;
  }

  function wireClone() {
    const copy = document.getElementById('mp-clone-copy');
    if (copy) copy.addEventListener('click', () => {
      const txt = (document.getElementById('mp-clone-url') || {}).textContent || '';
      navigator.clipboard.writeText(txt).then(() => toast(t('Copied.'))).catch(() => {});
    });
    const tk = document.getElementById('mp-git-tokens');
    if (tk) tk.addEventListener('click', openGitTokens);
  }

  async function openGitTokens() {
    const m = openModal(t('Git access tokens'), `<div id="mp-gt-body">
        <p class="mp-form-hint">${esc(t('A token is your git password (use any username). It is shown once - store it safely. Tokens work across all your mods.'))}</p>
        <div id="mp-gt-list"><p class="mp-muted">${esc(t('Loading…'))}</p></div>
        <form class="mp-form" id="mp-gt-form" style="margin-top:14px">
          <label class="mp-form-field"><span>${esc(t('New token label'))}</span><input name="name" maxlength="60" placeholder="laptop"></label>
          <p class="mp-form-error" hidden></p>
          <div class="mp-form-actions">
            <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Generate token'))}</button>
          </div>
        </form>
      </div>`);
    const listBox = m.wrap.querySelector('#mp-gt-list');
    const loadList = async () => {
      const r = await apiJSON('/v1/mods/hub/me/git-tokens');
      const items = (r.data && r.data.items) || [];
      listBox.innerHTML = items.length
        ? items.map((tk) => `<div class="mp-gt-row">
            <div><strong>${esc(tk.name || t('token'))}</strong> <code>${esc(tk.prefix)}…</code>
              <span class="mp-muted" style="font-size:.78rem">${tk.last_used_at ? esc(t('used')) + ' ' + fmtDate(tk.last_used_at) : esc(t('never used'))}</span></div>
            <button type="button" class="mp-btn mp-btn-sm mp-btn-danger" data-revoke="${esc(tk.id)}">${esc(t('Revoke'))}</button>
          </div>`).join('')
        : `<p class="mp-muted">${esc(t('No tokens yet.'))}</p>`;
      listBox.querySelectorAll('[data-revoke]').forEach((b) => b.addEventListener('click', async () => {
        if (!confirm(t('Revoke this token? Anything using it will stop working.'))) return;
        await apiJSON('/v1/mods/hub/me/git-tokens/' + encodeURIComponent(b.getAttribute('data-revoke')), { method: 'DELETE' });
        loadList();
      }));
      rerunI18n();
    };
    m.wrap.querySelector('#mp-gt-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const f = e.target;
      const r = await apiJSON('/v1/mods/hub/me/git-tokens', { json: { name: f.name.value.trim() } });
      if (r.ok && r.data && r.data.token) {
        f.name.value = '';
        showTokenOnce(m, r.data.token);
        loadList();
      } else {
        showFormError(f, errMsg(r, 'Could not create a token.'));
      }
    });
    loadList();
  }

  function showTokenOnce(m, token) {
    const note = document.createElement('div');
    note.className = 'mp-gt-new';
    note.innerHTML = `<p>${esc(t('Your new token (copied to clipboard, shown once):'))}</p>
      <code class="mp-gt-token">${esc(token)}</code>`;
    m.wrap.querySelector('#mp-gt-body').prepend(note);
    navigator.clipboard.writeText(token).catch(() => {});
  }

  // ─── Branch-scoped views (tree + commits) ──────────────────────────
  async function loadBranchViews() {
    const slug = PROJ_PATH;
    const treeBox = document.getElementById('mp-tree');
    const histBox = document.getElementById('mp-commits');
    try {
      const tr = await siteGET(`/site/mods/projects/${slug}/tree?ref=${encodeURIComponent(state.branch)}`);
      if (tr.ok) {
        const data = await tr.json();
        state._treeCommit = data.commit ? data.commit.id : null;
        treeBox.innerHTML = (data.entries && data.entries.length)
          ? data.entries.map(treeRow).join('')
          : `<div class="mp-tree-row"><span class="mp-muted">${esc(t('No files committed on this branch yet.'))}</span></div>`;
        wireTreeRows();
        loadReadme(data.entries, state._treeCommit);
      } else {
        treeBox.innerHTML = `<div class="mp-tree-row"><span class="mp-muted">${esc(t('No files committed on this branch yet.'))}</span></div>`;
        loadReadme([], null);
      }
    } catch (_) { treeBox.innerHTML = ''; }
    try {
      const cr = await siteGET(`/site/mods/projects/${slug}/commits?branch=${encodeURIComponent(state.branch)}&limit=50`);
      const data = cr.ok ? await cr.json() : { items: [] };
      histBox.innerHTML = data.items && data.items.length
        ? data.items.map(commitRow).join('')
        : `<p class="mp-muted">${esc(t('No commits yet.'))}</p>`;
    } catch (_) { histBox.innerHTML = ''; }
    loadPlacement();
  }

  async function loadPlacement() {
    const box = document.getElementById('mp-placement');
    if (!box) return;
    try {
      const r = await siteGET(`/site/mods/projects/${PROJ_PATH}/placement?ref=${encodeURIComponent(state.branch)}`);
      box.innerHTML = r.ok ? placementHTML(await r.json()) : '';
      const fix = box.querySelector('#mp-fix-placement');
      if (fix) fix.addEventListener('click', fixPlacement);
      rerunI18n();
    } catch (_) { box.innerHTML = ''; }
  }

  function placementHTML(p) {
    const owner = state.detail.is_owner;
    const blocks = [];
    if (p.misplaced && p.misplaced.length) {
      const rows = p.misplaced.map((m) => `<li><code>${esc(m.path)}</code> → <code>${esc(m.expected)}</code></li>`).join('');
      blocks.push(`<div class="mp-warn mp-warn-fix">
        <div class="mp-warn-head"><i class="fa-solid fa-triangle-exclamation"></i>
          <strong>${p.misplaced.length} ${esc(p.misplaced.length === 1 ? t('file is misplaced') : t('files are misplaced'))}</strong>
          ${owner ? `<button type="button" class="mp-btn mp-btn-sm mp-btn-primary" id="mp-fix-placement"><i class="fa-solid fa-wand-magic-sparkles"></i> ${esc(t('Fix file placement'))}</button>` : ''}
        </div>
        <p class="mp-muted">${esc(t("These match a game file but sit at the wrong path, so they won't override anything. They should be:"))}</p>
        <ul class="mp-warn-list">${rows}</ul>
      </div>`);
    }
    if (p.skipped && p.skipped.length) {
      const rows = p.skipped.map((s) => `<li><code>${esc(s.path)}</code> <span class="mp-muted">— ${esc(s.reason)}</span></li>`).join('');
      blocks.push(`<div class="mp-warn">
        <div class="mp-warn-head"><i class="fa-solid fa-circle-info"></i>
          <strong>${p.skipped.length} ${esc(p.skipped.length === 1 ? t('file will be skipped at build') : t('files will be skipped at build'))}</strong>
        </div>
        <p class="mp-muted">${esc(t('Only files inside a Trove folder (blueprints/, ui/, prefabs/…) are compiled - root files and folders like bin/ are ignored.'))}</p>
        <ul class="mp-warn-list">${rows}</ul>
      </div>`);
    }
    return blocks.join('');
  }

  async function fixPlacement() {
    if (!confirm(t('Move misplaced files to their correct game paths? This makes a new commit on this branch.'))) return;
    const r = await apiJSON(
      '/v1/mods/hub/projects/' + PROJ_PATH + '/fix-placement?branch=' + encodeURIComponent(state.branch),
      { method: 'POST' });
    if (r.ok && r.data) {
      toast(t('Fixed') + ' ' + (r.data.fixed || 0) + ' ' + t('file(s).'));
      await loadDetail();
    } else {
      toast(errMsg(r, 'Could not fix placement.'), true);
    }
  }

  function treeRow(e) {
    return `<div class="mp-tree-row">
      <span class="mp-tree-path"><i class="fa-solid fa-file"></i> ${esc(e.path)}</span>
      <span class="mp-tree-size">
        ${fmtBytes(e.size)}
        <button type="button" class="mp-btn mp-btn-sm" data-file="${esc(e.path)}" style="margin-left:8px"><i class="fa-solid fa-download"></i></button>
      </span>
    </div>`;
  }

  function commitRow(c) {
    return `<div class="mp-commit">
      <span class="mp-commit-dot"><i class="fa-solid fa-code-commit"></i></span>
      <div class="mp-commit-body">
        <div class="mp-commit-msg">${esc(c.message)}</div>
        <div class="mp-commit-meta">
          <span class="mp-commit-seq">${esc(c.short || (c.id || '').slice(0, 7))}</span>
          <span>${esc(c.author_username)}</span>
          <span>${esc(c.branch)}</span>
          <span>${c.file_count} ${esc(t('files'))}</span>
          <span>${fmtDate(c.created_at)}</span>
        </div>
      </div>
    </div>`;
  }

  // ─── Wiring ────────────────────────────────────────────────────────
  function wireHeader() {
    const rep = document.getElementById('mp-report');
    if (rep) rep.addEventListener('click', openReport);
    const fork = document.getElementById('mp-fork');
    if (fork) fork.addEventListener('click', forkProject);
    const inspire = document.getElementById('mp-inspire');
    if (inspire) inspire.addEventListener('click', createInspired);
    const star = document.getElementById('mp-star');
    if (star) star.addEventListener('click', toggleStar);
    const claim = document.getElementById('mp-claim');
    if (claim) claim.addEventListener('click', openClaim);

    const dlToggle = document.getElementById('mp-dl-toggle');
    if (dlToggle) {
      const menu = document.getElementById('mp-dl-menu');
      // The menu is position:fixed (escapes the header's overflow:hidden); place it
      // under the button, clamped to the viewport's right edge.
      const place = () => {
        const r = dlToggle.getBoundingClientRect();
        const w = Math.max(menu.offsetWidth, 230);
        const h = menu.offsetHeight;
        const left = Math.max(8, Math.min(r.left, window.innerWidth - w - 8));
        // Open below the button; flip above if it would run off the viewport bottom.
        let top = r.bottom + 6;
        if (top + h > window.innerHeight - 8 && r.top - h - 6 > 8) top = r.top - h - 6;
        menu.style.top = top + 'px';
        menu.style.left = left + 'px';
      };
      const onDoc = (e) => {
        if (e.target !== dlToggle && !dlToggle.contains(e.target) && !menu.contains(e.target)) close();
      };
      const close = () => {
        menu.hidden = true;
        dlToggle.setAttribute('aria-expanded', 'false');
        document.removeEventListener('click', onDoc);
        window.removeEventListener('scroll', close, true);
        window.removeEventListener('resize', close);
      };
      dlToggle.addEventListener('click', (e) => {
        e.stopPropagation();
        if (!menu.hidden) { close(); return; }
        menu.hidden = false;            // unhide so width is measurable, then place
        place();
        dlToggle.setAttribute('aria-expanded', 'true');
        setTimeout(() => {
          document.addEventListener('click', onDoc);
          window.addEventListener('scroll', close, true);
          window.addEventListener('resize', close);
        }, 0);
      });
      menu.querySelectorAll('.mp-dl-item').forEach((a) => a.addEventListener('click', close));
    }
  }

  // Manage co-owners (primary owner only).
  function openCollab() {
    const d = state.detail;
    const rows = (d.collaborators || []).map((c) => `
      <div class="mp-collab-row">
        <span><i class="fa-solid fa-user"></i> @${esc(c.username)}</span>
        <button type="button" class="mp-icon-btn mp-icon-danger" data-rm-collab="${esc(c.id)}" title="${esc(t('Remove'))}"><i class="fa-solid fa-xmark"></i></button>
      </div>`).join('') || `<p class="mp-muted">${esc(t('No collaborators yet.'))}</p>`;
    const m = openModal(t('Collaborators'), `
      <p class="mp-muted">${esc(t('Collaborators can edit this mod. Only you, the owner, can add or remove them or delete the mod.'))}</p>
      <div class="mp-collab-list">${rows}</div>
      <form id="mp-collab-form" class="mp-form" style="margin-top:12px">
        <label class="mp-form-field"><span>${esc(t('Add a collaborator by username'))}</span>
          <input type="text" name="username" maxlength="80" placeholder="username" autocomplete="off" required></label>
        <p class="mp-form-error" id="mp-collab-error" hidden></p>
        <div class="mp-form-actions">
          <button type="button" class="mp-btn mp-btn-ghost" data-close>${esc(t('Close'))}</button>
          <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Add'))}</button>
        </div>
      </form>`);
    const refresh = (data) => { state.detail = data; m.close(); render(); openCollab(); };
    m.wrap.querySelectorAll('[data-rm-collab]').forEach((b) => b.addEventListener('click', async () => {
      const r = await apiJSON('/v1/mods/hub/projects/' + PROJ_PATH + '/collaborators/' + encodeURIComponent(b.getAttribute('data-rm-collab')), { method: 'DELETE' });
      if (r.ok && r.data) refresh(r.data); else toast(errMsg(r, 'Could not remove.'), true);
    }));
    m.wrap.querySelector('#mp-collab-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const err = document.getElementById('mp-collab-error');
      const r = await apiJSON('/v1/mods/hub/projects/' + PROJ_PATH + '/collaborators',
        { json: { username: e.target.username.value.trim() } });
      if (r.ok && r.data) refresh(r.data);
      else { err.textContent = errMsg(r, t('Could not add that collaborator.')); err.hidden = false; }
    });
  }

  // Claim a stray (imported) mod: a short form -> request to the admins.
  function openClaim() {
    if (!state.viewer) { location.href = '/login'; return; }
    const m = openModal(t('Claim this mod'), `<form class="mp-form" id="mp-claim-form">
      <p class="mp-muted">${esc(t('Are you the author of this mod? Request to claim it. A moderator reviews and hands it over to you, after which you manage it like your own mods.'))}</p>
      <label class="mp-form-field"><span>${esc(t('Note to the moderators (optional)'))}</span>
        <textarea name="message" rows="3" maxlength="2000" placeholder="${esc(t('e.g. a link proving you are the author'))}"></textarea></label>
      <p class="mp-form-error" id="mp-claim-error" hidden></p>
      <div class="mp-form-actions">
        <button type="button" class="mp-btn mp-btn-ghost" data-close>${esc(t('Cancel'))}</button>
        <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Request claim'))}</button>
      </div>
    </form>`);
    document.getElementById('mp-claim-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const err = document.getElementById('mp-claim-error');
      const r = await apiJSON('/v1/mods/hub/projects/' + PROJ_PATH + '/claim',
        { json: { message: e.target.message.value.trim() } });
      if (r.ok || r.status === 202) {
        m.close();
        toast(r.data && r.data.already ? t('You already have a pending claim for this mod.')
                                       : t('Claim requested - a moderator will review it.'));
      } else {
        err.textContent = errMsg(r, t('Could not send your claim.'));
        err.hidden = false;
      }
    });
  }

  async function toggleStar() {
    if (!state.viewer) { location.href = '/login'; return; }
    const starred = !!state.detail.starred;
    const r = await apiJSON('/v1/mods/hub/projects/' + PROJ_PATH + '/star',
      { method: starred ? 'DELETE' : 'POST' });
    if (!r.ok || !r.data) { toast(errMsg(r, 'Could not update star.'), true); return; }
    state.detail.starred = r.data.starred;
    state.detail.star_count = r.data.star_count;
    const btn = document.getElementById('mp-star');
    const cnt = document.getElementById('mp-star-count');
    if (btn) {
      btn.classList.toggle('mp-starred', r.data.starred);
      btn.setAttribute('aria-pressed', String(r.data.starred));
      const icon = btn.querySelector('i');
      if (icon) icon.className = 'fa-' + (r.data.starred ? 'solid' : 'regular') + ' fa-star';
    }
    if (cnt) cnt.textContent = Number(r.data.star_count || 0).toLocaleString();
  }

  async function forkProject() {
    if (!state.viewer) { location.href = '/login'; return; }
    if (!confirm(t('Fork this mod into a new project of your own?'))) return;
    const r = await apiJSON('/v1/mods/hub/projects/' + PROJ_PATH + '/fork', { method: 'POST' });
    if (r.ok && r.data && r.data.slug) { location.href = modUrl(r.data); }
    else toast(errMsg(r, 'Could not fork this mod.'), true);
  }

  // Source-locked mods can't be forked; this starts a NEW project that credits
  // the original as inspiration (no file copy).
  async function createInspired() {
    if (!state.viewer) { location.href = '/login'; return; }
    if (!confirm(t('Start a new mod of your own that credits this one as inspiration?'))) return;
    const r = await apiJSON('/v1/mods/hub/projects', {
      json: { title: (state.detail.title || 'My mod') + ' (inspired)', inspired_by: HANDLE + '/' + SLUG },
    });
    if (r.ok && r.data && r.data.slug) { location.href = modUrl(r.data); }
    else toast(errMsg(r, 'Could not create the project.'), true);
  }

  function wireFiles() {
    const sel = document.getElementById('mp-branch-select');
    if (sel) sel.addEventListener('change', () => {
      if (sel.value === '__newbranch__') {   // the "+ New branch…" option
        sel.value = state.branch;            // revert the selection, open the modal
        openBranch();
        return;
      }
      state.branch = sel.value;
      loadBranchViews();
    });
  }

  function wireTreeRows() {
    document.querySelectorAll('#mp-tree [data-file]').forEach((b) =>
      b.addEventListener('click', () => downloadFile(b.getAttribute('data-file'))));
  }

  function wireReleases() {
    document.querySelectorAll('[data-rel-del]').forEach((b) =>
      b.addEventListener('click', () => deleteRelease(b.getAttribute('data-rel-del'))));
    document.querySelectorAll('[data-rel-toggle]').forEach((b) =>
      b.addEventListener('click', () => toggleRelease(b.getAttribute('data-rel-toggle'), b.getAttribute('data-status'))));
    // Draft releases need an auth-aware fetch (a plain link can't carry the bearer).
    document.querySelectorAll('[data-rel-dl]').forEach((b) =>
      b.addEventListener('click', () => downloadRelease(b.getAttribute('data-rel-dl'), b.getAttribute('data-fn'))));
    document.querySelectorAll('[data-hide-branch]').forEach((b) =>
      b.addEventListener('click', () => toggleHiddenBranch(b.getAttribute('data-hide-branch'))));
    document.querySelectorAll('[data-move-up]').forEach((b) =>
      b.addEventListener('click', () => moveVariant(b.getAttribute('data-move-up'), -1)));
    document.querySelectorAll('[data-move-down]').forEach((b) =>
      b.addEventListener('click', () => moveVariant(b.getAttribute('data-move-down'), 1)));
    // Per-release file list loads lazily the first time it's expanded.
    document.querySelectorAll('[data-rel-files]').forEach((d) =>
      d.addEventListener('toggle', () => { if (d.open && !d.dataset.loaded) { d.dataset.loaded = '1'; loadReleaseFiles(d); } }));
    loadReleaseBlueprints();
    loadReleaseVfx();
  }

  // List the files packed in a release's .tmod (preview excluded) with a per-file
  // download button. Lazy: only fetched when the user expands the release's Files.
  async function loadReleaseFiles(d) {
    const id = d.getAttribute('data-rel-files');
    const box = d.querySelector('[data-files-box]');
    box.textContent = t('Loading…');
    try {
      const r = await siteGET('/site/mods/releases/' + encodeURIComponent(id) + '/files');
      if (!r.ok) { box.textContent = t('Could not load files.'); return; }
      const items = ((await r.json()).items) || [];
      if (!items.length) { box.textContent = t('No downloadable files.'); return; }
      box.innerHTML = items.map((f) => `<div class="mp-release-file">
        <span class="mp-release-file-name" title="${esc(f.path)}">${esc(f.path)}</span>
        <span class="mp-release-file-size">${fmtBytes(f.size)}</span>
        <button type="button" class="mp-btn mp-btn-sm" data-rel-file="${esc(id)}" data-path="${esc(f.path)}" aria-label="${esc(t('Download'))}"><i class="fa-solid fa-download"></i></button>
      </div>`).join('');
      box.querySelectorAll('[data-rel-file]').forEach((b) =>
        b.addEventListener('click', () => downloadReleaseFile(b.getAttribute('data-rel-file'), b.getAttribute('data-path'))));
    } catch (_) { box.textContent = t('Could not load files.'); }
  }

  async function downloadReleaseFile(id, path) {
    try {
      const r = await siteGET('/site/mods/releases/' + encodeURIComponent(id) + '/file?path=' + encodeURIComponent(path));
      if (!r.ok) { toast(t('Could not download that file.'), true); return; }
      const blob = await r.blob();
      const a = document.createElement('a');
      a.href = URL.createObjectURL(blob);
      a.download = path.split('/').pop();
      a.click();
      setTimeout(() => URL.revokeObjectURL(a.href), 4000);
    } catch (_) { toast(t('Could not download that file.'), true); }
  }

  // For each .tmod release, lazily check whether it ships .blueprint models and,
  // if so, reveal a "3D view" button per model that opens the WebGL viewer.
  function loadReleaseBlueprints() {
    document.querySelectorAll('[data-rel-bp]').forEach(async (box) => {
      const relId = box.getAttribute('data-rel-bp');
      try {
        const r = await siteGET('/site/mods/releases/' + encodeURIComponent(relId) + '/blueprints');
        if (!r.ok) return;
        const body = await r.json();
        const items = (body && body.items) || [];
        if (!items.length) return;
        const btnFor = (bp) => {
          const name = bp.path.split('/').pop();
          return '<button type="button" class="mp-btn mp-btn-sm mp-3d-btn" data-bp-rel="' + esc(relId) +
            '" data-bp-path="' + esc(bp.path) + '" data-bp-name="' + esc(name) + '">' +
            '<i class="fa-solid fa-cube"></i> ' + esc(name) + '</button>';
        };
        const hasRig = !!(body && body.rig);
        // Component blueprints (the parts the assembled model is built from) fold UNDER
        // the assembled button; standalone blueprints stay as their own buttons. With no
        // rig, everything is standalone (a flat list, the original behaviour).
        const components = hasRig ? items.filter((bp) => bp.assembled) : [];
        const standalone = hasRig ? items.filter((bp) => !bp.assembled) : items;
        const assembled = hasRig
          ? '<button type="button" class="mp-btn mp-btn-sm mp-3d-btn mp-asm-btn" data-asm-rel="' + esc(relId) + '">' +
            '<i class="fa-solid fa-spider"></i> ' + esc(t('View assembled creature')) +
            (body.animations && body.animations.length ? ' <span class="mp-asm-anim">' + body.animations.length + ' ' + esc(t('animations')) + '</span>' : '') +
            '</button>' : '';
        const partsFold = (hasRig && components.length)
          ? '<details class="mp-3d-parts"><summary class="mp-3d-parts-summary">' +
            '<i class="fa-solid fa-cubes"></i> ' + esc(components.length + ' ' + t(components.length === 1 ? 'part' : 'parts')) +
            '</summary><div class="mp-3d-list">' + components.map(btnFor).join('') + '</div></details>'
          : '';
        const topCount = (hasRig ? 1 : 0) + standalone.length;
        const label = topCount + ' ' + t(topCount === 1 ? '3D model' : '3D models');
        box.innerHTML = '<details class="mp-3d-details"><summary class="mp-3d-summary">' +
          '<i class="fa-solid fa-cube"></i> ' + esc(label) + '</summary>' +
          (assembled ? '<div class="mp-3d-assembled">' + assembled + partsFold + '</div>' : '') +
          (standalone.length ? '<div class="mp-3d-list">' + standalone.map(btnFor).join('') + '</div>' : '') +
          '</details>';
        box.hidden = false;
        box.querySelectorAll('[data-bp-rel]').forEach((b) => b.addEventListener('click', () => {
          if (!window.BlueprintViewer) { toast(t('3D viewer is unavailable.'), true); return; }
          const path = b.getAttribute('data-bp-path');
          window.BlueprintViewer.open({
            url: '/site/mods/releases/' + encodeURIComponent(b.getAttribute('data-bp-rel')) +
                 '/blueprint?path=' + encodeURIComponent(path),
            title: b.getAttribute('data-bp-name'),
          });
        }));
        box.querySelectorAll('[data-asm-rel]').forEach((b) => b.addEventListener('click', () => {
          if (!window.ModelViewer) { toast(t('3D viewer is unavailable.'), true); return; }
          window.ModelViewer.open({
            url: '/site/mods/releases/' + encodeURIComponent(b.getAttribute('data-asm-rel')) + '/assembled',
            title: (state.detail && state.detail.title) || t('Assembled creature'),
          });
        }));
      } catch (e) { /* a release without parseable blueprints just stays hidden */ }
    });
  }

  // For each .tmod release, lazily check whether it ships .pkfx particle effects
  // and, if so, reveal a click-to-load WebGL preview per effect. Missing textures /
  // meshes are pulled from the live game tree server-side.
  function loadReleaseVfx() {
    document.querySelectorAll('[data-rel-vfx]').forEach(async (box) => {
      const relId = box.getAttribute('data-rel-vfx');
      try {
        const r = await siteGET('/site/mods/releases/' + encodeURIComponent(relId) + '/vfx');
        if (!r.ok) return;
        const items = ((await r.json()) || {}).items || [];
        if (!items.length) return;
        const btnFor = (it) => {
          const name = it.path.split('/').pop();
          return '<button type="button" class="mp-btn mp-btn-sm mp-vfx-btn" data-vfx-rel="' + esc(relId) +
            '" data-vfx-path="' + esc(it.path) + '" data-vfx-name="' + esc(name) + '">' +
            '<i class="fa-solid fa-fire-flame-curved"></i> ' + esc(name) + '</button>';
        };
        const label = items.length + ' ' + t(items.length === 1 ? 'VFX effect' : 'VFX effects');
        box.innerHTML = '<details class="mp-3d-details"><summary class="mp-3d-summary">' +
          '<i class="fa-solid fa-fire-flame-curved"></i> ' + esc(label) + '</summary>' +
          '<div class="mp-vfx-list mp-3d-list">' + items.map(btnFor).join('') + '</div></details>';
        box.hidden = false;
        box.querySelectorAll('.mp-vfx-btn').forEach((b) => b.addEventListener('click', () => {
          if (!window.PkfxViewer) { toast(t('VFX viewer is unavailable.'), true); return; }
          window.PkfxViewer.open({
            releaseId: b.getAttribute('data-vfx-rel'),
            path: b.getAttribute('data-vfx-path'),
            title: b.getAttribute('data-vfx-name'),
          });
        }));
      } catch (e) { /* a release without parseable VFX just stays hidden */ }
    });
  }

  async function toggleHiddenBranch(branch) {
    const cur = (state.detail.hidden_release_branches || []).slice();
    const i = cur.indexOf(branch);
    if (i >= 0) cur.splice(i, 1); else cur.push(branch);
    const r = await apiJSON('/v1/mods/hub/projects/' + PROJ_PATH,
      { method: 'PATCH', json: { hidden_release_branches: cur } });
    if (r.ok) { toast(i >= 0 ? t('Variant shown.') : t('Variant hidden.')); await loadDetail(); }
    else toast(errMsg(r, 'Could not update visibility.'), true);
  }

  async function downloadRelease(id, filename) {
    try {
      const r = await siteGET('/site/mods/releases/' + encodeURIComponent(id) + '/download');
      if (!r.ok) { toast(t('Could not download that release.'), true); return; }
      const blob = await r.blob();
      const a = document.createElement('a');
      a.href = URL.createObjectURL(blob);
      a.download = filename || 'mod.tmod';
      a.click();
      setTimeout(() => URL.revokeObjectURL(a.href), 4000);
    } catch (_) { toast(t('Could not download that release.'), true); }
  }

  async function downloadFile(path) {
    if (!state._treeCommit) return;
    const url = `/site/mods/projects/${PROJ_PATH}/raw/${state._treeCommit}/${path.split('/').map(encodeURIComponent).join('/')}`;
    try {
      const r = await siteGET(url);
      if (!r.ok) { toast(t('Could not load that file.'), true); return; }
      const blob = await r.blob();
      const a = document.createElement('a');
      a.href = URL.createObjectURL(blob);
      a.download = path.split('/').pop();
      a.click();
      setTimeout(() => URL.revokeObjectURL(a.href), 4000);
    } catch (_) { toast(t('Could not load that file.'), true); }
  }

  // ─── Modal helper ──────────────────────────────────────────────────
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

  // ─── Studio: edit details ──────────────────────────────────────────
  function openEdit() {
    const d = state.detail;
    // An uploaded-on-behalf mod stays bare: no owner links / donations / inspiration.
    const isUploaded = !d.is_stray && !!d.uploaded_on_behalf;
    const selectedCats = new Set((d.tags || []).filter(isCategory).map((c) => c.toLowerCase()));
    const freeTags = (d.tags || []).filter((tg) => !isCategory(tg));
    const catChips = MOD_CATEGORIES.map((c) =>
      `<button type="button" class="mp-catchip ${selectedCats.has(c.toLowerCase()) ? 'is-sel' : ''}" data-cat="${esc(c)}">${esc(c)}</button>`).join('');
    const m = openModal(t('Edit mod details'), `<form class="mp-form" id="mp-edit-form">
      <label class="mp-form-field"><span>${esc(t('Title'))}</span><input name="title" maxlength="120" value="${esc(d.title)}" required></label>
      <label class="mp-form-field"><span>${esc(t('Short summary'))}</span><input name="summary" maxlength="280" value="${esc(d.summary || '')}"></label>
      <label class="mp-form-field"><span>${esc(t('Description (Markdown)'))}</span><textarea name="description" maxlength="40000">${esc(d.description || '')}</textarea></label>
      <label class="mp-form-field"><span><i class="fa-solid fa-triangle-exclamation"></i> ${esc(t('Warnings'))}</span><textarea name="warnings" rows="3" maxlength="4000" placeholder="${esc(t('Highlighted below the description. <br> starts a new warning block.'))}">${esc(d.warnings || '')}</textarea></label>
      <div class="mp-form-field"><span>${esc(t('Categories'))}</span>
        <div class="mp-cats" id="mp-cats">${catChips}</div>
        <p class="mp-form-hint">${esc(t('Pick any that fit - saved as tags and embedded in the build.'))}</p></div>
      <label class="mp-form-field"><span>${esc(t('Extra tags (comma-separated)'))}</span><input name="tags" value="${esc(freeTags.join(', '))}"></label>
      <label class="mp-form-field"><span>${esc(t('Visibility'))}</span>
        <select name="visibility">
          <option value="draft" ${d.visibility === 'draft' ? 'selected' : ''}>${esc(t('Draft (only you)'))}</option>
          <option value="unlisted" ${d.visibility === 'unlisted' ? 'selected' : ''}>${esc(t('Unlisted (link only)'))}</option>
          <option value="public" ${d.visibility === 'public' ? 'selected' : ''}>${esc(t('Public'))}</option>
        </select></label>
      ${isUploaded ? '' : `<label class="mp-form-field"><span><i class="fa-brands fa-discord"></i> ${esc(t('Discord invite'))}</span><input name="discord_url" maxlength="300" value="${esc(d.discord_url || '')}" placeholder="https://discord.gg/…"></label>
      <label class="mp-form-field"><span><i class="fa-solid fa-globe"></i> ${esc(t('Website'))}</span><input name="website_url" maxlength="300" value="${esc(d.website_url || '')}" placeholder="https://…"></label>
      <label class="mp-form-field"><span><i class="fa-solid fa-heart"></i> ${esc(t('Donation links (one per line, up to 5)'))}</span><textarea name="donation_urls" rows="3" placeholder="https://ko-fi.com/you">${esc((d.donation_urls || []).join('\n'))}</textarea></label>
      <label class="mp-form-field"><span>${esc(t('Inspired by (handle/slug)'))}</span><input name="inspired_by" value="${esc(d.inspired_by ? (d.inspired_by.handle + '/' + d.inspired_by.slug) : '')}" placeholder="someuser/another-mod"></label>`}
      <p class="mp-form-error" hidden></p>
      <div class="mp-form-actions">
        <button type="button" class="mp-btn" data-close>${esc(t('Cancel'))}</button>
        <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Save'))}</button>
      </div></form>`);
    const chosen = new Set(selectedCats);   // lowercased category keys
    m.wrap.querySelectorAll('#mp-cats .mp-catchip').forEach((b) => b.addEventListener('click', () => {
      const k = b.getAttribute('data-cat').toLowerCase();
      if (chosen.has(k)) { chosen.delete(k); b.classList.remove('is-sel'); }
      else { chosen.add(k); b.classList.add('is-sel'); }
    }));
    m.wrap.querySelector('#mp-edit-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const f = e.target;
      const body = {
        title: f.title.value.trim(), summary: f.summary.value.trim(),
        description: f.description.value, warnings: f.warnings.value,
        visibility: f.visibility.value,
        // Selected categories (canonical labels) + free tags from the input.
        tags: [
          ...MOD_CATEGORIES.filter((c) => chosen.has(c.toLowerCase())),
          ...f.tags.value.split(',').map((s) => s.trim()).filter(Boolean),
        ],
      };
      // Owner links / inspiration only exist on non-uploaded mods (fields omitted above).
      if (!isUploaded) {
        body.discord_url = f.discord_url.value.trim();
        body.website_url = f.website_url.value.trim();
        body.donation_urls = f.donation_urls.value.split('\n').map((s) => s.trim()).filter(Boolean).slice(0, 5);
        body.inspired_by = f.inspired_by.value.trim();
      }
      const r = await apiJSON('/v1/mods/hub/projects/' + PROJ_PATH, { method: 'PATCH', json: body });
      if (r.ok) { m.close(); toast(t('Saved.')); await loadDetail(); }
      else showFormError(f, errMsg(r, 'Could not save changes.'));
    });
  }

  // Settings: structural options (mode / source visibility) + the Delete danger
  // zone, kept off the main page so deleting isn't a one-click slip.
  function openSettings() {
    const d = state.detail;
    const m = openModal(t('Settings'), `<form class="mp-form" id="mp-settings-form">
      <label class="mp-form-field"><span>${esc(t('Mode'))}</span>
        <select name="mode">
          <option value="files" ${d.mode === 'files' ? 'selected' : ''}>${esc(t('Files + releases (versioned)'))}</option>
          <option value="releases" ${d.mode === 'releases' ? 'selected' : ''}>${esc(t('Releases only'))}</option>
        </select></label>
      <label class="mp-form-field"><span>${esc(t('Source visibility'))}</span>
        <select name="source_visibility">
          <option value="public" ${d.source_visibility === 'public' ? 'selected' : ''}>${esc(t('Public — show files & allow cloning'))}</option>
          <option value="private" ${d.source_visibility === 'private' ? 'selected' : ''}>${esc(t('Private — hide files/clone, show only releases'))}</option>
        </select></label>
      <p class="mp-form-hint">${esc(t('Private source turns the hub into an internal tool: you keep version history + git, the public sees only releases.'))}</p>
      <p class="mp-form-error" hidden></p>
      <div class="mp-form-actions">
        <button type="button" class="mp-btn" data-close>${esc(t('Close'))}</button>
        <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Save'))}</button>
      </div>
      ${state.detail.is_primary_owner ? `<div class="mp-danger-zone">
        <strong><i class="fa-solid fa-triangle-exclamation"></i> ${esc(t('Danger zone'))}</strong>
        <p class="mp-muted">${esc(t('Permanently removes the project, its files, history and releases. This cannot be undone.'))}</p>
        <button type="button" class="mp-btn mp-btn-sm mp-btn-danger" id="mp-delete-go"><i class="fa-solid fa-trash"></i> ${esc(t('Delete this mod'))}</button>
      </div>` : ''}</form>`);
    m.wrap.querySelector('#mp-settings-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const f = e.target;
      const r = await apiJSON('/v1/mods/hub/projects/' + PROJ_PATH,
        { method: 'PATCH', json: { mode: f.mode.value, source_visibility: f.source_visibility.value } });
      if (r.ok) { m.close(); toast(t('Saved.')); await loadDetail(); }
      else showFormError(f, errMsg(r, 'Could not save settings.'));
    });
    const delGo = m.wrap.querySelector('#mp-delete-go');
    if (delGo) delGo.addEventListener('click', () => { m.close(); confirmDelete(); });
  }

  // ─── Studio: banner / previews ─────────────────────────────────────
  function openBanner() {
    const hasBanner = !!(state.detail && state.detail.banner_sha);
    const m = openModal(t(hasBanner ? 'Change banner' : 'Upload banner'),
      imageForm('banner', t('Pick a 16:9 image (PNG / JPEG / WebP / GIF, ≤ 5 MB).')));
    wireImageForm(m, false, '/banner');
    // When a banner is set, offer a Remove option alongside the upload form.
    if (hasBanner) {
      const actions = m.wrap.querySelector('.mp-form-actions');
      if (actions) {
        const rm = document.createElement('button');
        rm.type = 'button';
        rm.className = 'mp-btn mp-btn-danger';
        rm.style.marginRight = 'auto';   // sit on the left, away from Cancel/Upload
        rm.textContent = t('Remove banner');
        rm.addEventListener('click', async () => {
          rm.disabled = true;
          const r = await apiJSON('/v1/mods/hub/projects/' + PROJ_PATH + '/banner', { method: 'DELETE' });
          if (r.ok || r.status === 200) { m.close(); toast(t('Banner removed.')); await loadDetail(); }
          else { rm.disabled = false; showFormError(m.wrap.querySelector('#mp-img-form'), errMsg(r, 'Could not remove the banner.')); }
        });
        actions.insertBefore(rm, actions.firstChild);
      }
    }
  }
  function openPreviews() {
    const m = openModal(t('Add preview images'), imageForm('previews', t('Add up to a few screenshots (≤ 5 MB each).'), true));
    wireImageForm(m, true, '/previews');
  }
  function imageForm(name, hint, multiple) {
    return `<form class="mp-form" id="mp-img-form">
      <label class="mp-form-field"><span>${esc(t('Image file'))}</span>
        <input type="file" name="file" accept="image/png,image/jpeg,image/webp,image/gif" ${multiple ? 'multiple' : ''} required></label>
      <p class="mp-form-hint">${esc(hint)}</p>
      <p class="mp-form-error" hidden></p>
      <div class="mp-form-actions">
        <button type="button" class="mp-btn" data-close>${esc(t('Cancel'))}</button>
        <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Upload'))}</button>
      </div></form>`;
  }
  function wireImageForm(m, multiple, endpoint) {
    m.wrap.querySelector('#mp-img-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const f = e.target;
      const fd = new FormData();
      if (multiple) {
        for (const file of f.file.files) fd.append('files', file);
      } else {
        fd.append('file', f.file.files[0]);
      }
      const btn = f.querySelector('button[type=submit]'); btn.disabled = true;
      const r = await apiForm('/v1/mods/hub/projects/' + PROJ_PATH + endpoint, fd);
      btn.disabled = false;
      if (r.ok) { m.close(); toast(t('Uploaded.')); await loadDetail(); }
      else showFormError(f, errMsg(r, 'Upload failed.'));
    });
  }
  async function removePreview(sha) {
    const r = await apiJSON('/v1/mods/hub/projects/' + PROJ_PATH + '/previews/' + encodeURIComponent(sha), { method: 'DELETE' });
    if (r.ok || r.status === 200) { toast(t('Removed.')); await loadDetail(); }
    else toast(errMsg(r, 'Could not remove that image.'), true);
  }

  // ─── Studio: commit files ──────────────────────────────────────────
  function openCommit() {
    const branches = (state.detail.branches || []).map((b) =>
      `<option value="${esc(b.name)}" ${b.name === state.branch ? 'selected' : ''}>${esc(b.name)}</option>`).join('');
    const m = openModal(t('Commit files'), `<form class="mp-form" id="mp-commit-form">
      <label class="mp-form-field"><span>${esc(t('Branch'))}</span><select name="branch">${branches}</select></label>
      <div class="mp-form-field"><span>${esc(t('Files'))}</span>
        <div class="mp-drop" id="mp-drop"><i class="fa-solid fa-cloud-arrow-up"></i> ${esc(t('Drop files here or click to choose'))}</div>
        <input type="file" id="mp-files" multiple hidden>
        <div class="mp-droplist" id="mp-droplist"></div>
      </div>
      <label class="mp-form-field"><span>${esc(t('Commit message'))}</span><input name="message" maxlength="500" required></label>
      <p class="mp-form-hint">${esc(t('Folder paths from a drag-drop are kept as the in-mod path.'))}</p>
      <p class="mp-form-error" hidden></p>
      <div class="mp-form-actions">
        <button type="button" class="mp-btn" data-close>${esc(t('Cancel'))}</button>
        <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Commit'))}</button>
      </div></form>`, { wide: true });

    const picked = [];   // { file, path }
    const drop = m.wrap.querySelector('#mp-drop');
    const input = m.wrap.querySelector('#mp-files');
    const list = m.wrap.querySelector('#mp-droplist');
    const refresh = () => {
      list.innerHTML = picked.map((p, i) =>
        `<div class="mp-dropitem"><span>${esc(p.path)}</span><button type="button" data-i="${i}"><i class="fa-solid fa-xmark"></i></button></div>`).join('');
      list.querySelectorAll('button').forEach((b) => b.addEventListener('click', () => {
        picked.splice(Number(b.getAttribute('data-i')), 1); refresh();
      }));
    };
    const add = (files) => {
      for (const f of files) picked.push({ file: f, path: (f.webkitRelativePath || f.name) });
      refresh();
    };
    drop.addEventListener('click', () => input.click());
    input.addEventListener('change', () => add(input.files));
    ['dragover', 'dragenter'].forEach((ev) => drop.addEventListener(ev, (e) => { e.preventDefault(); drop.classList.add('drag'); }));
    ['dragleave', 'drop'].forEach((ev) => drop.addEventListener(ev, () => drop.classList.remove('drag')));
    drop.addEventListener('drop', (e) => { e.preventDefault(); if (e.dataTransfer && e.dataTransfer.files) add(e.dataTransfer.files); });

    m.wrap.querySelector('#mp-commit-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const f = e.target;
      if (!picked.length) { showFormError(f, t('Add at least one file.')); return; }
      const fd = new FormData();
      fd.append('branch', f.branch.value);
      fd.append('message', f.message.value.trim());
      picked.forEach((p) => { fd.append('files', p.file); fd.append('paths', p.path); });
      const btn = f.querySelector('button[type=submit]'); btn.disabled = true;
      const r = await apiForm('/v1/mods/hub/projects/' + PROJ_PATH + '/commits', fd);
      btn.disabled = false;
      if (r.ok) { m.close(); toast(t('Committed.')); await loadDetail(); }
      else showFormError(f, errMsg(r, 'Commit failed.'));
    });
  }

  // ─── Studio: new branch ────────────────────────────────────────────
  function openBranch() {
    const branches = (state.detail.branches || []).map((b) =>
      `<option value="${esc(b.name)}">${esc(b.name)}</option>`).join('');
    const m = openModal(t('New branch'), `<form class="mp-form" id="mp-branch-form">
      <label class="mp-form-field"><span>${esc(t('Branch name'))}</span><input name="name" maxlength="80" required placeholder="experimental"></label>
      <label class="mp-form-field"><span>${esc(t('Fork from'))}</span><select name="from_ref">${branches}</select></label>
      <p class="mp-form-error" hidden></p>
      <div class="mp-form-actions">
        <button type="button" class="mp-btn" data-close>${esc(t('Cancel'))}</button>
        <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Create'))}</button>
      </div></form>`);
    m.wrap.querySelector('#mp-branch-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const f = e.target;
      const r = await apiJSON('/v1/mods/hub/projects/' + PROJ_PATH + '/branches',
        { json: { name: f.name.value.trim(), from_ref: f.from_ref.value } });
      if (r.ok) { m.close(); toast(t('Branch created.')); state.branch = f.name.value.trim(); await loadDetail(); }
      else showFormError(f, errMsg(r, 'Could not create the branch.'));
    });
  }

  // ─── Studio: new release ───────────────────────────────────────────
  function openRelease() {
    const filesMode = state.detail.mode === 'files';
    const branches = (state.detail.branches || []).map((b) =>
      `<option value="${esc(b.name)}" ${b.name === state.branch ? 'selected' : ''}>${esc(b.name)}</option>`).join('');
    const modeRow = filesMode ? `
      <div class="mp-radio-row">
        <label><input type="radio" name="mode" value="compile" checked> ${esc(t('Compile from a branch'))}</label>
        <label><input type="radio" name="mode" value="upload"> ${esc(t('Upload a build'))}</label>
      </div>` : `<input type="hidden" name="mode" value="upload">`;
    const compileFields = filesMode ? `
      <label class="mp-form-field" data-mode="compile"><span>${esc(t('Branch to compile'))}</span><select name="ref">${branches}</select></label>
      <label class="mp-form-field" data-mode="compile"><span>${esc(t('Compile to'))}</span>
        <select name="format"><option value="tmod">.tmod (${esc(t('Trove mod'))})</option><option value="zip">.zip</option></select></label>` : '';
    // Author(s) stamped into the .tmod (compile + tmod only), default = the owner.
    const authorField = filesMode ? `
      <label class="mp-form-field" data-mode="compile" data-fmt="tmod"><span>${esc(t('Author(s)'))}</span>
        <input name="author" maxlength="200" value="${esc(state.detail.owner_username || '')}" placeholder="${esc(t('Comma-separated for several'))}">
        <p class="mp-form-hint">${esc(t("Stamped into the .tmod - shown in-game as the mod's author."))}</p>
      </label>` : '';
    // Pick a preview to embed in the .tmod (compile + tmod only). Releases-only
    // mode skips this - those builds come pre-compiled.
    const previewPicker = filesMode ? `
      <div class="mp-form-field" data-mode="compile" data-fmt="tmod"><span>${esc(t('Preview image'))}</span>
        <div class="mp-prevpick" id="mp-prevpick"></div>
        <p class="mp-form-hint">${esc(t('Embedded in the .tmod as the in-game / mod-site thumbnail. Stored only in the build, not your files.'))}</p>
      </div>` : '';
    const m = openModal(t('New release'), `<form class="mp-form" id="mp-rel-form">
      ${modeRow}
      <label class="mp-form-field"><span>${esc(t('Version tag'))}</span><input name="tag" maxlength="60" required placeholder="v1.0.0"></label>
      <label class="mp-form-field"><span>${esc(t('Title'))}</span><input name="title" maxlength="160"></label>
      ${compileFields}
      ${authorField}
      ${previewPicker}
      <label class="mp-form-field" data-mode="upload" ${filesMode ? 'hidden' : ''}><span>${esc(t('Build file (.tmod or .zip)'))}</span><input type="file" name="file" accept=".tmod,.zip,application/octet-stream,application/zip"></label>
      <label class="mp-form-field" data-mode="upload" ${filesMode ? 'hidden' : ''}><span>${esc(t('Variant (branch)'))}</span><input name="upload_branch" maxlength="80" value="${esc(state.detail.default_branch || 'main')}"></label>
      <label class="mp-form-field"><span>${esc(t('Changelog (Markdown)'))}</span><textarea name="changelog" maxlength="20000"></textarea></label>
      <label class="mp-form-field"><span>${esc(t('Status'))}</span>
        <select name="status"><option value="published">${esc(t('Published'))}</option><option value="draft">${esc(t('Draft'))}</option></select></label>
      <p class="mp-form-hint">${filesMode ? esc(t('Compiling builds the artifact server-side from the latest commit on the chosen branch.')) : esc(t('Upload an already-built .tmod or .zip.'))}</p>
      <p class="mp-form-error" hidden></p>
      <div class="mp-form-actions">
        <button type="button" class="mp-btn" data-close>${esc(t('Cancel'))}</button>
        <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Publish release'))}</button>
      </div></form>`, { wide: true });

    const f = m.wrap.querySelector('#mp-rel-form');
    const fmtSel = f.querySelector('select[name=format]');
    const toggleMode = () => {
      const mode = f.mode.value;
      const fmt = fmtSel ? fmtSel.value : 'tmod';
      f.querySelectorAll('[data-mode]').forEach((el) => {
        let show = el.getAttribute('data-mode') === mode;
        if (show && el.hasAttribute('data-fmt')) show = el.getAttribute('data-fmt') === fmt;
        el.hidden = !show;
      });
    };
    f.querySelectorAll('input[name=mode]').forEach((r) => r.addEventListener('change', toggleMode));
    if (fmtSel) fmtSel.addEventListener('change', toggleMode);

    // Preview picker: choose one of the project's previews (default the first), or
    // upload a new one (which also adds it to the project gallery), to embed in the build.
    let selectedPreview = (state.detail.preview_shas || [])[0] || null;
    const renderPicker = () => {
      const box = f.querySelector('#mp-prevpick');
      if (!box) return;
      const shas = state.detail.preview_shas || [];
      const tile = (sha) => `<button type="button" class="mp-prevtile ${sha === selectedPreview ? 'is-sel' : ''}" data-sha="${esc(sha)}"><img src="${imageUrl(sha)}" alt="" loading="lazy"></button>`;
      box.innerHTML =
        `<button type="button" class="mp-prevtile mp-prevtile-none ${selectedPreview === null ? 'is-sel' : ''}" data-sha=""><i class="fa-solid fa-ban"></i><span>${esc(t('None'))}</span></button>`
        + shas.map(tile).join('')
        + `<button type="button" class="mp-prevtile mp-prevtile-add" id="mp-prev-upload"><i class="fa-solid fa-plus"></i><span>${esc(t('Upload'))}</span></button>`;
      box.querySelectorAll('[data-sha]').forEach((b) => b.addEventListener('click', () => {
        selectedPreview = b.getAttribute('data-sha') || null; renderPicker();
      }));
      const up = box.querySelector('#mp-prev-upload');
      if (up) up.addEventListener('click', uploadPreviewInline);
    };
    const uploadPreviewInline = () => {
      const inp = document.createElement('input');
      inp.type = 'file'; inp.accept = 'image/png,image/jpeg,image/webp,image/gif';
      inp.addEventListener('change', async () => {
        if (!inp.files.length) return;
        const fd = new FormData(); fd.append('files', inp.files[0]);
        const before = new Set(state.detail.preview_shas || []);
        const r = await apiForm('/v1/mods/hub/projects/' + PROJ_PATH + '/previews', fd);
        if (r.ok && r.data && r.data.preview_shas) {
          state.detail.preview_shas = r.data.preview_shas;
          selectedPreview = r.data.preview_shas.find((s) => !before.has(s)) || selectedPreview;
          renderPicker();
        } else { toast(errMsg(r, 'Upload failed.'), true); }
      });
      inp.click();
    };
    renderPicker();

    f.addEventListener('submit', async (e) => {
      e.preventDefault();
      const mode = f.mode.value;
      const btn = f.querySelector('button[type=submit]'); btn.disabled = true;
      let r;
      if (mode === 'upload') {
        if (!f.file.files.length) { showFormError(f, t('Choose a .tmod or .zip file.')); btn.disabled = false; return; }
        const fd = new FormData();
        fd.append('tag', f.tag.value.trim());
        fd.append('title', f.title.value.trim());
        fd.append('changelog', f.changelog.value);
        fd.append('status', f.status.value);
        fd.append('branch', f.upload_branch ? f.upload_branch.value.trim() : '');
        fd.append('file', f.file.files[0]);
        r = await apiForm('/v1/mods/hub/projects/' + PROJ_PATH + '/releases/upload', fd);
      } else {
        r = await apiJSON('/v1/mods/hub/projects/' + PROJ_PATH + '/releases', {
          json: {
            tag: f.tag.value.trim(), title: f.title.value.trim(),
            changelog: f.changelog.value, ref: f.ref.value,
            format: f.format ? f.format.value : 'tmod', status: f.status.value,
            preview_sha: (f.format && f.format.value === 'tmod') ? selectedPreview : null,
            author: f.author ? f.author.value.trim() : null,
          },
        });
      }
      btn.disabled = false;
      if (r.ok) { m.close(); toast(t('Release published.')); await loadDetail(); }
      else showFormError(f, errMsg(r, 'Could not create the release.'));
    });
  }

  async function toggleRelease(id, status) {
    const next = status === 'published' ? 'draft' : 'published';
    const r = await apiJSON('/v1/mods/hub/releases/' + encodeURIComponent(id), { method: 'PATCH', json: { status: next } });
    if (r.ok) { toast(t('Updated.')); await loadDetail(); }
    else toast(errMsg(r, 'Could not update the release.'), true);
  }
  async function deleteRelease(id) {
    if (!confirm(t('Delete this release? The compiled file stays in storage but the release is removed.'))) return;
    const r = await apiJSON('/v1/mods/hub/releases/' + encodeURIComponent(id), { method: 'DELETE' });
    if (r.ok || r.status === 204) { toast(t('Release deleted.')); await loadDetail(); }
    else toast(errMsg(r, 'Could not delete the release.'), true);
  }

  // ─── Studio: delete project ────────────────────────────────────────
  function confirmDelete() {
    const name = state.detail.title;
    const m = openModal(t('Delete this mod?'), `<p class="mp-muted">${esc(t('This permanently removes the project, its branches, commits and releases. This cannot be undone.'))}</p>
      <p class="mp-form-hint">${esc(t('Copy or click the name, then type it below to confirm.'))}</p>
      <div class="mp-copyname">
        <code id="mp-del-name">${esc(name)}</code>
        <button type="button" class="mp-btn mp-btn-sm" id="mp-del-copy" aria-label="${esc(t('Copy'))}" title="${esc(t('Copy'))}"><i class="fa-solid fa-copy"></i></button>
      </div>
      <form class="mp-form" id="mp-del-form">
        <label class="mp-form-field"><span>${esc(t('Mod title'))}</span><input name="confirm" autocomplete="off" placeholder="${esc(name)}" required></label>
        <p class="mp-form-error" hidden></p>
        <div class="mp-form-actions">
          <button type="button" class="mp-btn" data-close>${esc(t('Cancel'))}</button>
          <button type="submit" class="mp-btn mp-btn-danger">${esc(t('Delete forever'))}</button>
        </div></form>`);
    const copyBtn = m.wrap.querySelector('#mp-del-copy');
    if (copyBtn) copyBtn.addEventListener('click', () => {
      navigator.clipboard.writeText(name).then(() => toast(t('Copied.'))).catch(() => {});
    });
    m.wrap.querySelector('#mp-del-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const f = e.target;
      if (f.confirm.value.trim() !== name) { showFormError(f, t('The title does not match.')); return; }
      const r = await apiJSON('/v1/mods/hub/projects/' + PROJ_PATH, { method: 'DELETE' });
      if (r.ok || r.status === 204) { location.href = '/mods'; }
      else showFormError(f, errMsg(r, 'Could not delete the mod.'));
    });
  }

  // ─── Report (non-owner) ────────────────────────────────────────────
  function openReport() {
    const m = openModal(t('Report this mod'), `<form class="mp-form" id="mp-report-form">
      <label class="mp-form-field"><span>${esc(t('What is the problem?'))}</span><textarea name="reason" maxlength="2000" required></textarea></label>
      <p class="mp-form-error" hidden></p>
      <div class="mp-form-actions">
        <button type="button" class="mp-btn" data-close>${esc(t('Cancel'))}</button>
        <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Send report'))}</button>
      </div></form>`);
    m.wrap.querySelector('#mp-report-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const f = e.target;
      const r = await apiJSON('/v1/mods/hub/projects/' + PROJ_PATH + '/report', { json: { reason: f.reason.value.trim() } });
      if (r.ok || r.status === 202) { m.close(); toast(t('Thanks - your report was sent.')); }
      else showFormError(f, errMsg(r, 'Could not send the report.'));
    });
  }

  // ─── Misc UI ───────────────────────────────────────────────────────
  function lightbox(url) {
    const box = document.createElement('div');
    box.className = 'mp-lightbox';
    box.innerHTML = `<img src="${esc(url)}" alt="">`;
    box.addEventListener('click', () => box.remove());
    document.body.appendChild(box);
  }

  function showFormError(form, msg) {
    const el = form.querySelector('.mp-form-error');
    if (el) { el.textContent = msg; el.hidden = false; }
  }

  // ─── Formatting ────────────────────────────────────────────────────
  function fmtBytes(n) {
    n = Number(n || 0);
    if (n < 1024) return n + ' B';
    if (n < 1024 * 1024) return (n / 1024).toFixed(1) + ' KB';
    return (n / 1024 / 1024).toFixed(2) + ' MB';
  }
  function fmtDate(iso) {
    if (!iso) return '';
    const d = new Date(iso);
    const diff = (Date.now() - d.getTime()) / 1000;
    if (diff < 60) return t('just now');
    if (diff < 3600) return Math.floor(diff / 60) + 'm ' + t('ago');
    if (diff < 86400) return Math.floor(diff / 3600) + 'h ' + t('ago');
    if (diff < 2592000) return Math.floor(diff / 86400) + 'd ' + t('ago');
    return d.toLocaleDateString();
  }

  // The GitHub-flavored renderer + DOM sanitizer live in md_render.js
  // (window.BTTMarkdown) so there is ONE copy of the XSS-safe HTML sanitizer,
  // shared with the modder-profile page. It always escape()s first, then
  // transforms the escaped text - so no user input can inject HTML.
  const renderMarkdown = (s) => window.BTTMarkdown.render(s);
  const sanitizeHTML = (h) => window.BTTMarkdown.sanitize(h);
  const mdInline = (s, refs) => window.BTTMarkdown.inline(s, refs);
})();
