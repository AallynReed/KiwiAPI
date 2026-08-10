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

  // contentLang: the language the reader picked for this mod's own text (About +
  // README). null = follow the site language (and fall back to English).
  const state = { detail: null, viewer: null, branch: null, contentLang: null, repoReadme: null };

  const $root = document.getElementById('mp-root');

  const t = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);
  const imageUrl = (sha) => BTTUtil.apiUrl('/site/mods/image/' + encodeURIComponent(sha));
  function rerunI18n() { if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh(); }

  // Mod categories (mirrors app/trove/mod_categories.py). Selected ones are saved
  // as tags; the server also encodes them as a numeric `flags` bitmask in the .tmod.
  const MOD_CATEGORIES = ['Allies', 'Banners', 'Boats and Sails', 'Cosmetics', 'Costumes',
    'Dragons', 'Fishing', 'GUI', 'Helmets', 'Language', 'Mag Riders', 'Mounts', 'NPCs',
    'Wings', 'Automation', 'Optimization', 'Reskin', 'Waypoint', 'Radar'];
  const _CAT_LOWER = new Set(MOD_CATEGORIES.map((c) => c.toLowerCase()));
  const isCategory = (tag) => _CAT_LOWER.has(String(tag).trim().toLowerCase());

  // ─── Content languages ───────────────────────────────────────────────
  // Every piece of prose on a mod - title, summary, About, warnings, README and
  // each release's title + changelog - can be written again in any language the
  // site speaks. English is always the base and the fallback, so a partial
  // translation never leaves a blank. One switch at the top of the page drives
  // the lot. In files mode the README's translations live in the repo instead,
  // as README.<code>.md files next to README.md.
  const LANG_BY_LOWER = ModsI18n.BY_LOWER;      // lowercased code -> canonical
  const sortLangs = ModsI18n.sortLangs;
  const textVersions = BTTUtil.textVersions;
  // The text to show for one field, honouring the reader's pick.
  const local = (base, translations) => BTTUtil.localized(base, translations, state.contentLang);
  const pickLang = (versions) => BTTUtil.pickLang(versions, state.contentLang);

  // Every language this mod has *something* in, so the one switch covers the
  // whole page (a section without that language quietly stays English).
  function pageLangs(d) {
    const codes = new Set();
    [[d.title, d.title_i18n], [d.summary, d.summary_i18n],
      [d.description, d.description_i18n], [d.warnings, d.warnings_i18n],
      [d.readme_text, d.readme_i18n]].forEach(([base, map]) => {
      Object.keys(textVersions(base, map)).forEach((c) => codes.add(c));
    });
    (d.releases || []).forEach((r) => {
      Object.keys(textVersions(r.title, r.title_i18n)).forEach((c) => codes.add(c));
      Object.keys(textVersions(r.changelog, r.changelog_i18n)).forEach((c) => codes.add(c));
    });
    // Files mode: the README translations are repo files, found after the tree loads.
    if (state.repoReadme) Object.keys(state.repoReadme.files).forEach((c) => codes.add(c));
    codes.add('en');                       // the base language is always readable
    return sortLangs([...codes]);
  }

  // The switch itself - nothing at all when the mod is English-only.
  function pageLangTabsHTML(d) {
    const codes = pageLangs(d);
    return ModsI18n.tabsHTML(codes, pickLang(Object.fromEntries(codes.map((c) => [c, true]))));
  }

  // Same-origin read with a one-shot token refresh, so an owner whose access
  // token aged out still sees their drafts after the silent refresh. The
  // HttpOnly session cookie is the credential - there is no header to add.
  async function siteGET(path) {
    const init = { credentials: 'include' };
    let r = await fetch(path, init);
    if ((r.status === 401 || r.status === 404) && window.BTTAuth && window.BTTAuth.hasSession && window.BTTAuth.hasSession()) {
      if (await window.BTTAuth.refresh()) r = await fetch(path, init);
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
      // No file browser (releases-only / private source). Reads like a document
      // rather than a stack of identical panels: prose and gallery run
      // chrome-free down the left, and the panels that do something (releases,
      // packs, forks) sit in a rail on the right - stacked, they pushed the
      // download list a screen and a half below the fold.
      const docMain = [descriptionHTML(d)];
      if (ownerHasPreviews) docMain.push(previewsHTML(d));
      docMain.push(readmeTextHTML(d));    // releases-only README (saved text)
      const docSide = [releasesHTML(d), modpacksHTML()];
      if (d.fork_count) docSide.push(forksHTML());
      parts.push(`<div class="mp-doc">
        <div class="mp-doc-main">${docMain.filter(Boolean).join('')}</div>
        <aside class="mp-doc-side">${docSide.filter(Boolean).join('')}</aside>
      </div>`);
    }

    $root.innerHTML = parts.join('');
    // Lets the page pull its top padding and float the back link over the hero.
    const main = $root.closest('.mp-main');
    if (main) main.classList.add('mp-has-hero');
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
      <div class="mp-section-head"><h2 class="mp-section-title"><i class="fa-solid fa-wand-magic-sparkles"></i> ${esc(t('Remixes'))}</h2></div>
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
            <i class="fa-solid fa-wand-magic-sparkles"></i>
            <span class="mp-fork-title">${esc(f.title)}</span>
            <span class="mp-muted">${esc(t('by'))} ${esc(f.owner_username)}</span>
          </a>`).join('')
        : `<p class="mp-muted">${esc(t('No remixes yet.'))}</p>`;
    } catch (_) { box.innerHTML = ''; }
  }

  function headerHTML(d) {
    // The banner is the hero's backdrop (it fills the header behind the title
    // and fades into the page), not a framed image sitting inside a card.
    const bannerInner = d.banner_sha
      ? `<img class="mp-banner" src="${imageUrl(d.banner_sha)}" alt="">`
      : `<div class="mp-banner placeholder"><i class="fa-solid fa-cube"></i></div>`;
    // The owner edits the banner by clicking it (no separate toolbar button).
    const banner = `<div class="mp-banner-wrap${d.is_owner ? ' is-editable' : ''}"${d.is_owner
      ? ` id="mp-banner-btn" role="button" tabindex="0" title="${esc(t('Change banner'))}"` : ''}>`
      + bannerInner
      + '<span class="mp-banner-scrim" aria-hidden="true"></span>'
      + (d.is_owner ? `<span class="mp-banner-edit"><i class="fa-solid fa-camera"></i> ${esc(d.banner_sha ? t('Change banner') : t('Add banner'))}</span>` : '')
      + '</div>';
    const vis = d.visibility;
    // Visibility and mode are owner settings. A visitor already knows the mod is
    // public - they're reading it - so the badges only speak to the owner.
    const badge = !d.is_owner ? ''
      : vis === 'draft' ? `<span class="mp-badge mp-badge-draft">${esc(t('Draft'))}</span>`
      : vis === 'unlisted' ? `<span class="mp-badge mp-badge-unlisted">${esc(t('Unlisted'))}</span>`
      : `<span class="mp-badge mp-badge-public">${esc(t('Public'))}</span>`;
    const modeBadge = (d.is_owner && d.mode === 'releases')
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
      dlBtn = `<a class="mp-btn mp-btn-primary" href="${BTTUtil.apiUrl('/site/mods/releases/' + r0.id + '/download')}"><i class="fa-solid fa-download"></i> ${esc(t('Download'))} <span class="mp-release-tagchip">${esc(r0.tag)}</span></a>`;
    } else {
      const items = published.map((r) => `<a class="mp-dl-item" href="${BTTUtil.apiUrl('/site/mods/releases/' + r.id + '/download')}" role="menuitem">
          <span class="mp-release-tagchip">${esc(r.tag)}</span>
          ${showDlBranch ? `<span class="mp-dl-branch">${esc(variantLabel(r.branch || 'main'))}</span>` : ''}
          <span class="mp-dl-size">${fmtBytes(r.tmod_size)}</span>
        </a>`).join('');
      dlBtn = `<div class="mp-dl-split">
        <button type="button" class="mp-btn mp-btn-primary mp-dl-toggle" id="mp-dl-toggle" aria-haspopup="true" aria-expanded="false">
          <i class="fa-solid fa-download"></i> ${esc(t('Download'))} <span class="mp-dl-caret" aria-hidden="true"></span>
        </button>
        <div class="mp-dl-menu" id="mp-dl-menu" role="menu" hidden>${items}</div>
      </div>`;
    }
    // A mod's config file goes in the game's ModCfgs folder, not the mods folder,
    // so it gets its own button - filled in after we've looked inside the .tmod.
    const headRel = published.find((r) => r.format !== 'zip');
    const cfgSlot = headRel ? `<span data-rel-cfg="${esc(headRel.id)}"></span>` : '';
    // Hand off to the desktop app's Mods Hub tab over the btt:// protocol it
    // registers on install. Only for mods the app can actually find: the hub
    // browses published, public mods.
    const appHref = 'btt://mods?handle=' + encodeURIComponent(d.handle || '')
      + '&slug=' + encodeURIComponent(d.slug || '')
      + '&q=' + encodeURIComponent(d.title || '');
    const appBtn = (vis === 'public' && published.length && !d.taken_down)
      ? `<a class="mp-btn" id="mp-open-app" href="${esc(appHref)}" title="${esc(t('Requires the Better Trove Tools desktop app'))}"><i class="fa-solid fa-desktop"></i> ${esc(t('Open in app'))}</a>`
      : '';
    // Stray = an imported mod uploaded via contributions, with no owner here yet.
    const isStray = !!d.is_stray;
    // Anyone can report (no account needed) - DSA notice-and-action; only the
    // owner and unclaimed stray imports don't show the control.
    // Quiet by design: reporting stays one click away for DSA notice-and-action,
    // but it isn't a visual peer of the download.
    const reportBtn = (!d.is_owner && !isStray)
      ? `<button type="button" class="mp-btn mp-btn-sm mp-btn-quiet" id="mp-report"><i class="fa-solid fa-flag"></i> ${esc(t('Report'))}</button>` : '';
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
    // A lone "0" beside a star reads as a rating, not a count - so it only shows
    // once someone has actually favourited the mod.
    const starN = Number(d.star_count || 0);
    const starBtn = `<button type="button" class="mp-btn mp-btn-sm ${starred ? 'mp-starred' : ''}" id="mp-star" aria-pressed="${starred}" aria-label="${esc(t('Favourite'))}" title="${esc(t('Favourite'))}">
        <i class="fa-${starred ? 'solid' : 'regular'} fa-star"></i> <span id="mp-star-count"${starN ? '' : ' hidden'}>${starN.toLocaleString()}</span>
      </button>`;
    // Fork copies the source, so it's only offered when the source is visible.
    // A source-locked mod can instead be credited as inspiration.
    let forkBtn = '';
    // Uploaded-on-behalf mods can't be forked OR credited as inspiration - they're
    // not the uploader's work, so no lineage rides on them.
    if (!d.is_owner && !isStray && !isUploaded) {
      forkBtn = d.source_visible
        ? `<button type="button" class="mp-btn mp-btn-sm" id="mp-fork"><i class="fa-solid fa-wand-magic-sparkles"></i> ${esc(t('Remix'))}</button>`
        : `<button type="button" class="mp-btn mp-btn-sm" id="mp-inspire"><i class="fa-solid fa-lightbulb"></i> ${esc(t('Make your own version'))}</button>`;
    }
    const forkLink = (ref) => `<a href="${modUrl(ref)}">${esc(ref.title || ref.slug)}</a>`;
    let attribution = '';
    if (d.forked_from) {
      attribution = `<div class="mp-attribution"><i class="fa-solid fa-wand-magic-sparkles"></i> ${esc(t('Remixed from'))} ${forkLink(d.forked_from)}${d.forked_from.owner ? ' ' + esc(t('by')) + ' ' + esc(d.forked_from.owner) : ''}</div>`;
    } else if (d.inspired_by) {
      attribution = `<div class="mp-attribution"><i class="fa-solid fa-lightbulb"></i> ${esc(t('Inspired by'))} ${forkLink(d.inspired_by)}${d.inspired_by.owner ? ' ' + esc(t('by')) + ' ' + esc(d.inspired_by.owner) : ''}</div>`;
    }
    const forkCount = d.fork_count
      ? `<span><i class="fa-solid fa-wand-magic-sparkles"></i> ${Number(d.fork_count)} ${esc(d.fork_count === 1 ? t('remix') : t('remixes'))}</span>` : '';
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
    // A first-timer leaves here with a .tmod and no idea what to do with it, so the
    // answer sits at the download rather than on a page they'd have to go find.
    // Deliberately no absolute path: Glyph and Steam installs differ, and the
    // desktop app is the thing that actually resolves the folder.
    const installHelp = published.length ? `<details class="mp-install">
      <summary class="mp-install-summary"><i class="fa-solid fa-circle-question" aria-hidden="true"></i> ${esc(t('Where does this file go?'))}</summary>
      <div class="mp-install-body">
        <p>${esc(t('Drop the downloaded file into your Trove {folder} folder - nothing else to set up.')).replace('{folder}', '<code>mods</code>')}</p>
        <p>${esc(t('If the mod also comes with a settings file, that one goes in {folder} instead.')).replace('{folder}', '<code>ModCfgs</code>')}</p>
        <p class="mp-install-app">${esc(t('Not sure where that is? The desktop app finds the folder and installs for you.'))} <a href="/app">${esc(t('Get the app'))}</a></p>
      </div>
    </details>` : '';
    // The owner's own picture on the byline; strays and owners without one keep an icon.
    const ownerFace = d.owner_avatar_url
      ? `<img class="mp-author-av" src="${esc(d.owner_avatar_url)}" alt="" referrerpolicy="no-referrer">`
      : `<i class="fa-solid fa-${isUploaded ? 'share-from-square' : 'user'}"></i>`;
    return `<header class="mp-header${d.source_visible ? '' : ' is-doc'}">
      ${banner}
      <div class="mp-header-body">
        ${taken}
        <div class="mp-titlerow">
          <h1 class="mp-title" id="mp-title">${esc(local(d.title, d.title_i18n))}</h1> ${strayBadge} ${uploadedBadge} ${badge} ${modeBadge} ${privBadge}
          ${ownerTitleActions}
        </div>
        <div id="mp-langswitch">${pageLangTabsHTML(d)}</div>
        ${attribution}
        <div class="mp-meta">
          ${isStray
            ? `<span><i class="fa-solid fa-user"></i> ${esc(d.author || d.owner_username)}</span>`
            : isUploaded
              ? `<span>${ownerFace}${esc(t('Uploaded by'))} <a class="mp-author-link" href="/mods/${encodeURIComponent(d.handle || '')}">${esc(d.owner_username)}</a></span><span><i class="fa-solid fa-user"></i> ${esc(t('Created by'))} ${esc(d.author || '')}</span>`
              : `<span>${ownerFace}<a class="mp-author-link" href="/mods/${encodeURIComponent(d.handle || '')}">${esc(d.owner_username)}</a></span>`}
          <span><i class="fa-solid fa-download"></i> ${Number(d.download_count || 0).toLocaleString()} ${esc(t('downloads'))}</span>
          ${d.source_visible ? `<span><i class="fa-solid fa-clock-rotate-left"></i> ${Number(d.commit_count || 0)} ${esc(d.commit_count === 1 ? t('update') : t('updates'))}</span>` : ''}
          ${forkCount}
        </div>
        ${isStray ? `<p class="mp-stray-note"><i class="fa-solid fa-circle-info"></i> ${esc(t('This mod was uploaded via contributions and hasn\'t been claimed by its author yet. If it\'s yours, claim it to manage it here.'))}</p>` : ''}
        ${isUploaded ? `<p class="mp-stray-note"><i class="fa-solid fa-circle-info"></i> ${esc(t('This mod was uploaded by a community member on the creator\'s behalf. It isn\'t an official release by the author.'))}</p>` : ''}
        <p class="mp-summary" id="mp-summary"${local(d.summary, d.summary_i18n) ? '' : ' hidden'}>${esc(local(d.summary, d.summary_i18n))}</p>
        ${tags ? `<div class="mp-tags">${tags}</div>` : ''}
        ${linksRow}
        <div class="mp-actions">${dlBtn}${appBtn}${cfgSlot}${starBtn}${claimBtn}${forkBtn}${reportBtn}</div>
        ${installHelp}
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
    return `<section class="mp-section" id="mp-description-section">${descriptionInnerHTML(d)}</section>`;
  }

  // Head + body of the About text, rebuilt on every language switch.
  function descriptionInnerHTML(d) {
    const text = local(d.description, d.description_i18n);
    const body = text ? renderMarkdown(text)
      : `<p class="mp-muted">${esc(t('No description yet.'))}</p>`;
    return `<div class="mp-section-head">
        <h2 class="mp-section-title"><i class="fa-solid fa-book"></i> ${esc(t('About'))}</h2>
      </div>
      <div class="mp-markdown">${body}</div>
      ${warningsHTML(d)}`;
  }

  // Owner-authored warnings: one highlighted block per `<br>`-separated segment.
  function warningsHTML(d) {
    const raw = local(d.warnings, d.warnings_i18n).trim();
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
      ${d.is_owner ? `<button type="button" class="mp-preview-del" data-prev-del="${esc(sha)}" aria-label="${esc(t('Remove'))}"><i class="fa-solid fa-xmark"></i></button>` : ''}
    </div>`).join('');
    const addBtn = d.is_owner
      ? `<button type="button" class="mp-btn mp-btn-sm" id="mp-add-previews"><i class="fa-solid fa-plus"></i> ${esc(t('Add previews'))}</button>` : '';
    return `<section class="mp-section" id="mp-previews-section">
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

  // A creator's editions are their own words ("lite", "full"); only the git default
  // needs translating into something a player recognises.
  function variantLabel(branch) {
    return branch === 'main' ? t('Standard') : branch;
  }

  function releasesHTML(d) {
    const items = d.releases || [];
    // The "each edition…" hint only earns its line when there's more than one.
    const multiVariant = new Set(items.map((r) => r.branch || 'main')).size > 1;
    let rows;
    if (!items.length) {
      rows = `<p class="mp-muted">${esc(t('No versions yet.'))} ${d.is_owner ? esc(t('Use “New version” to publish one.')) : ''}</p>`;
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
        // A lone default edition has nothing to say to a visitor - "main" is a git
        // default, not a choice the creator made. The owner still needs the row for
        // its hide/reorder controls.
        const bareDefault = ordered.length === 1 && branch === 'main' && !d.is_owner;
        return `<div class="mp-variant">
          ${bareDefault ? '' : `<div class="mp-variant-head">
            <span class="mp-variant-name"><i class="fa-solid fa-layer-group"></i> ${esc(variantLabel(branch))} ${hiddenTag}</span>
            <span class="mp-variant-actions">${reorder}${toggle}</span>
          </div>`}
          ${releaseRow(rels[0], d.is_owner)}
          ${older.length ? `<details class="mp-variant-older"><summary>${older.length + ' ' + esc(older.length === 1 ? t('older version') : t('older versions'))}</summary>${older.map((r) => releaseRow(r, d.is_owner)).join('')}</details>` : ''}
        </div>`;
      }).join('');
    }
    const newRelBtn = d.is_owner
      ? `<button type="button" class="mp-btn mp-btn-sm mp-btn-primary" id="mp-release"><i class="fa-solid fa-rocket"></i> ${esc(t('New version'))}</button>` : '';
    return `<section class="mp-section">
      <div class="mp-section-head"><h2 class="mp-section-title"><i class="fa-solid fa-rocket"></i> ${esc(t('Versions'))}</h2>${newRelBtn}</div>
      ${multiVariant ? `<p class="mp-muted" style="margin:0 0 12px">${esc(t('Each edition shows its latest version.'))}</p>` : ''}
      <div id="mp-releases">${rows}</div>
    </section>`;
  }

  function releaseRow(r, owner) {
    const draft = r.status !== 'published'
      ? `<span class="mp-badge mp-badge-draft">${esc(t('Draft'))}</span>` : '';
    const ownerBtns = owner ? `
      <button type="button" class="mp-btn mp-btn-sm" data-rel-cfgset="${esc(r.id)}" hidden></button>
      <button type="button" class="mp-btn mp-btn-sm" data-rel-toggle="${esc(r.id)}" data-status="${esc(r.status)}">
        ${r.status === 'published' ? esc(t('Unpublish')) : esc(t('Publish'))}
      </button>
      <button type="button" class="mp-btn mp-btn-sm" data-rel-edit="${esc(r.id)}" aria-label="${esc(t('Edit release'))}" title="${esc(t('Edit release'))}"><i class="fa-solid fa-pen"></i></button>
      <button type="button" class="mp-btn mp-btn-sm mp-btn-danger" data-rel-del="${esc(r.id)}"><i class="fa-solid fa-trash"></i></button>` : '';
    return `<div class="mp-release">
      <div class="mp-release-top">
        <span class="mp-release-tag"><span class="mp-release-tagchip">${esc(r.tag)}</span> <span data-rel-title="${esc(r.id)}">${esc(local(r.title, r.title_i18n))}</span> ${draft}</span>
        <div class="mp-release-actions">
          ${r.status === 'published'
            ? `<a class="mp-btn mp-btn-sm mp-btn-primary" href="${BTTUtil.apiUrl('/site/mods/releases/' + r.id + '/download')}"><i class="fa-solid fa-download"></i> ${esc(t('Download'))}</a>`
            : `<button type="button" class="mp-btn mp-btn-sm mp-btn-primary" data-rel-dl="${esc(r.id)}" data-fn="${esc(r.tmod_filename)}"><i class="fa-solid fa-download"></i> ${esc(t('Download'))}</button>`}
          ${r.format !== 'zip' ? `<span data-rel-cfg="${esc(r.id)}"></span>` : ''}
          <button type="button" class="mp-btn mp-btn-sm" data-rel-inspect="${esc(r.id)}"><i class="fa-solid fa-folder-tree"></i> ${esc(t('Contents'))}</button>
          ${ownerBtns}
        </div>
      </div>
      <div class="mp-release-stats">
        <span><i class="fa-solid fa-download"></i> ${Number(r.download_count || 0).toLocaleString()}</span>
        <span><i class="fa-solid fa-file-zipper"></i> ${fmtBytes(r.tmod_size)}</span>
        ${r.published_at ? `<span><i class="fa-solid fa-clock"></i> ${fmtDate(r.published_at)}</span>` : ''}
      </div>
      <div class="mp-release-changelog" data-rel-changelog="${esc(r.id)}"${local(r.changelog, r.changelog_i18n) ? '' : ' hidden'}>${esc(local(r.changelog, r.changelog_i18n))}</div>
      ${r.format !== 'zip' ? `<div class="mp-release-3d" data-rel-bp="${esc(r.id)}" hidden></div>` : ''}
      ${r.format !== 'zip' ? `<div class="mp-release-vfx" data-rel-vfx="${esc(r.id)}" hidden></div>` : ''}
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
      <div class="mp-section-head"><h2 class="mp-section-title"><i class="fa-solid fa-book-open"></i> <span id="mp-readme-name">README</span></h2>
        <span id="mp-readme-langs"></span></div>
      <div id="mp-readme" class="mp-markdown"></div>
      <p class="mp-form-hint mp-readme-hint" id="mp-readme-hint" hidden></p>
    </section>`;
  }

  // README for releases-only mode (saved as text - there are no files to hold a
  // README.md). In files mode the repo's README.md is rendered instead (see
  // loadReadme) and this text is ignored.
  function readmeTextHTML(d) {
    if (d.mode !== 'releases') return '';
    if (!Object.keys(textVersions(d.readme_text, d.readme_i18n)).length && !d.is_owner) return '';
    return `<section class="mp-section" id="mp-readme-text-section">${readmeTextInnerHTML(d)}</section>`;
  }

  // Head + body of the releases-only README, rebuilt on every language switch.
  function readmeTextInnerHTML(d) {
    const text = local(d.readme_text, d.readme_i18n);
    const editBtn = d.is_owner
      ? `<button type="button" class="mp-btn mp-btn-sm" id="mp-edit-readme"><i class="fa-solid fa-pen"></i> ${esc(text ? t('Edit README') : t('Add README'))}</button>` : '';
    const body = text ? renderMarkdown(text)
      : `<p class="mp-muted">${esc(t('No README yet.'))}</p>`;
    return `<div class="mp-section-head">
        <h2 class="mp-section-title"><i class="fa-solid fa-book-open"></i> ${esc(t('README'))}</h2>
        ${editBtn}
      </div>
      <div class="mp-markdown">${body}</div>`;
  }

  // Repaint everything the language switch touches. Each piece falls back to
  // English on its own, so a mod translated in patches still reads through.
  function repaintTranslations() {
    const d = state.detail;
    if (!d) return;
    const set = (id, text) => {
      const el = document.getElementById(id);
      if (el) { el.textContent = text; el.hidden = !text; }
    };
    set('mp-title', local(d.title, d.title_i18n));
    set('mp-summary', local(d.summary, d.summary_i18n));
    const sw = document.getElementById('mp-langswitch');
    if (sw) sw.innerHTML = pageLangTabsHTML(d);
    const desc = document.getElementById('mp-description-section');
    if (desc) desc.innerHTML = descriptionInnerHTML(d);
    const sec = document.getElementById('mp-readme-text-section');
    if (sec) {
      sec.innerHTML = readmeTextInnerHTML(d);
      const edit = sec.querySelector('#mp-edit-readme');
      if (edit) edit.addEventListener('click', openReadmeEdit);
    } else {
      paintRepoReadme();
    }
    // Releases: swap the text in place so the rows' own handlers stay wired.
    (d.releases || []).forEach((r) => {
      const title = document.querySelector(`[data-rel-title="${cssEsc(r.id)}"]`);
      if (title) title.textContent = local(r.title, r.title_i18n);
      const log = document.querySelector(`[data-rel-changelog="${cssEsc(r.id)}"]`);
      if (log) {
        const text = local(r.changelog, r.changelog_i18n);
        log.textContent = text;
        log.hidden = !text;
      }
    });
    rerunI18n();
  }

  // Release ids are hex ObjectIds, but never build a selector from data without
  // escaping it.
  const cssEsc = (s) => (window.CSS && CSS.escape ? CSS.escape(String(s)) : String(s));

  // The switch survives section rebuilds ($root itself is never replaced), so it
  // is delegated and wired once.
  $root.addEventListener('click', (e) => {
    const btn = e.target.closest('[data-content-lang]');
    if (!btn) return;
    state.contentLang = btn.getAttribute('data-content-lang');
    repaintTranslations();
  });
  // A reader who hasn't picked a language follows the site's.
  document.addEventListener('btt-lang-changed', () => { if (!state.contentLang) repaintTranslations(); });

  function openReadmeEdit() {
    const d = state.detail;
    const m = openModal(t('Edit README'), `<form class="mp-form" id="mp-readme-form">
      ${ModsI18n.editorHTML('mp-readme')}
      <label class="mp-form-field"><span id="mp-readme-label">${esc(t('README (Markdown)'))}</span><textarea name="readme" rows="14" maxlength="60000"></textarea></label>
      <p class="mp-form-hint">${esc(t('Shown as the main content for releases-only mods. Markdown + safe HTML (badges, alignment, tables) supported.'))}
        ${esc(t('Colour text with [text]{#ff8a3d}, [text]{gold} or [text]{#fff on #1f2733}.'))}
        ${esc(t('Leave a translation empty to remove it.'))}</p>
      <p class="mp-form-error" hidden></p>
      <div class="mp-form-actions">
        <button type="button" class="mp-btn" data-close>${esc(t('Cancel'))}</button>
        <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Save'))}</button>
      </div></form>`, { wide: true });
    const form = m.wrap.querySelector('#mp-readme-form');
    const collect = ModsI18n.wireEditor(form, 'mp-readme', [{
      base: d.readme_text, translations: d.readme_i18n,
      area: form.querySelector('textarea[name="readme"]'),
      labelEl: form.querySelector('#mp-readme-label'), label: t('README (Markdown)'),
    }]);

    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      const [{ base, translations }] = collect();
      const r = await apiJSON('/v1/mods/hub/projects/' + PROJ_PATH,
        { method: 'PATCH', json: { readme_text: base, readme_i18n: translations } });
      if (r.ok) { m.close(); toast(t('Saved.')); await loadDetail(); }
      else showFormError(form, errMsg(r, 'Could not save README.'));
    });
  }

  // Render the branch's README.md (root preferred) under the file browser, plus
  // any README.<lang>.md translations sitting next to it.
  async function loadReadme(entries, commitId) {
    const sec = document.getElementById('mp-readme-section');
    if (!sec) return;
    const list = entries || [];
    const base = list.find((e) => e.path.toLowerCase() === 'readme.md')
      || list.find((e) => e.path.toLowerCase().endsWith('/readme.md'));
    if (!base || !commitId) { sec.hidden = true; return; }
    const dir = base.path.slice(0, base.path.lastIndexOf('/') + 1);   // '' at the root
    const files = { en: base.path };
    list.forEach((e) => {
      if (e.path.slice(0, e.path.lastIndexOf('/') + 1) !== dir) return;
      const m = /^readme\.([a-z-]+)\.md$/i.exec(e.path.slice(dir.length));
      const code = m && LANG_BY_LOWER[m[1].toLowerCase()];
      if (code && code !== 'en') files[code] = e.path;
    });
    state.repoReadme = { files, commitId, cache: {} };
    // The repo's languages only surface once the tree has loaded - fold them
    // into the page switch now that we know them.
    const sw = document.getElementById('mp-langswitch');
    if (sw) sw.innerHTML = pageLangTabsHTML(state.detail);
    await paintRepoReadme();
  }

  // Fetch (once per language) + render the active repo README.
  async function paintRepoReadme() {
    const sec = document.getElementById('mp-readme-section');
    const info = state.repoReadme;
    if (!sec || !info) return;
    const active = pickLang(info.files);
    if (!active) { sec.hidden = true; return; }
    try {
      if (info.cache[active] == null) {
        const url = `/site/mods/projects/${PROJ_PATH}/raw/${info.commitId}/`
          + info.files[active].split('/').map(encodeURIComponent).join('/');
        const r = await siteGET(url);
        if (!r.ok) { sec.hidden = true; return; }
        info.cache[active] = await r.text();
      }
      document.getElementById('mp-readme').innerHTML = renderMarkdown(info.cache[active]);
      const nameEl = document.getElementById('mp-readme-name');
      if (nameEl) nameEl.textContent = info.files[active].split('/').pop();
      // The file-naming convention is invisible otherwise, so tell the owner
      // about it once - only while there's nothing but the English README.
      const hint = document.getElementById('mp-readme-hint');
      if (hint) {
        const solo = Object.keys(info.files).length < 2 && state.detail && state.detail.is_owner;
        hint.textContent = solo ? t('Add README.fr.md (or any language) next to it and readers who use it get that version.') : '';
        hint.hidden = !solo;
      }
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
            <button type="button" class="mp-btn mp-btn-sm" id="mp-clone-copy" aria-label="${esc(t('Copy'))}"><i class="fa-solid fa-copy"></i></button>
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
    // The download is a plain link, so the browser gives no in-page feedback.
    // Confirm it started and repeat the one thing they need to do next.
    document.querySelectorAll('a[href*="/download"]').forEach((a) => {
      a.addEventListener('click', () => toast(t('Downloading - drop the file in your Trove mods folder.')));
    });
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
    if (cnt) {
      const n = Number(r.data.star_count || 0);
      cnt.textContent = n.toLocaleString();
      cnt.hidden = !n;                       // back to a bare star at zero
    }
  }

  async function forkProject() {
    if (!state.viewer) { location.href = '/login'; return; }
    if (!confirm(t('Start your own version of this mod, with the files copied over?'))) return;
    const r = await apiJSON('/v1/mods/hub/projects/' + PROJ_PATH + '/fork', { method: 'POST' });
    if (r.ok && r.data && r.data.slug) { location.href = modUrl(r.data); }
    else toast(errMsg(r, 'Could not fork this mod.'), true);
  }

  // Source-locked mods can't be forked; this starts a NEW project that credits
  // the original as inspiration (no file copy).
  async function createInspired() {
    if (!state.viewer) { location.href = '/login'; return; }
    if (!confirm(t('Start your own mod that credits this one as inspiration?'))) return;
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
    document.querySelectorAll('[data-rel-edit]').forEach((b) =>
      b.addEventListener('click', () => openReleaseEdit(b.getAttribute('data-rel-edit'))));
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
    document.querySelectorAll('[data-rel-inspect]').forEach((b) =>
      b.addEventListener('click', () => openReleaseContents(b.getAttribute('data-rel-inspect'))));
    document.querySelectorAll('[data-rel-cfgset]').forEach((b) =>
      b.addEventListener('click', () => openAttachConfig(b.getAttribute('data-rel-cfgset'))));
    loadReleaseBlueprints();
    loadReleaseVfx();
    loadReleaseCfgs();
  }

  // Owner action: pack a config into a release that's ALREADY out. This rewrites a
  // published build, so the trade-off is spelled out rather than buried - see the
  // service docstring for why players who already have it are left alone.
  function openAttachConfig(id) {
    const m = openModal(t('Attach a config to this release'), `<form class="mp-form" id="mp-cfgset-form">
      <div class="mp-warn mp-warn-fix">
        <div class="mp-warn-head"><i class="fa-solid fa-triangle-exclamation"></i>
          <strong>${esc(t('This rewrites a build that is already published.'))}</strong></div>
        <ul class="mp-warn-list">
          <li>${esc(t('Players who already installed this release keep what they have - they are not prompted to re-download, so the config will not reach them.'))}</li>
          <li>${esc(t('Their copy still shows as installed, and no false update appears.'))}</li>
          <li>${esc(t('To actually deliver the config to everyone, cut a new release instead.'))}</li>
        </ul>
      </div>
      <label class="mp-form-field"><span>${esc(t('Settings file (.cfg)'))}</span>
        <input type="file" name="config" accept=".cfg,text/plain" required>
        <p class="mp-form-hint">${esc(t('Packed as ui/<title>.cfg, replacing the build\'s current config if it has one.'))}</p>
      </label>
      <p class="mp-form-error" hidden></p>
      <div class="mp-form-actions">
        <button type="button" class="mp-btn" data-close>${esc(t('Cancel'))}</button>
        <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Rewrite this build'))}</button>
      </div></form>`, { wide: true });
    const f = m.wrap.querySelector('#mp-cfgset-form');
    f.addEventListener('submit', async (e) => {
      e.preventDefault();
      if (!f.config.files.length) { showFormError(f, t('Choose a .cfg file.')); return; }
      const btn = f.querySelector('button[type=submit]');
      btn.disabled = true;
      const fd = new FormData();
      fd.append('file', f.config.files[0]);
      const r = await apiForm('/v1/mods/hub/releases/' + encodeURIComponent(id) + '/config', fd);
      btn.disabled = false;
      if (!r.ok) { showFormError(f, errMsg(r, 'Could not attach the config.')); return; }
      m.close();
      toast(r.data && r.data.changed === false
        ? t('That build already carries this exact config.') : t('Config packed into the build.'));
      await loadDetail();
    });
  }

  // ─── Build contents (the decoded artifact) ─────────────────────────────
  // What's actually inside a release: the .tmod header the game reads, and every
  // packed file as a folder tree (any file downloadable on its own, bar the
  // preview image). Replaces the old flat per-release file list.

  async function openReleaseContents(id) {
    const m = openModal(t('Build contents'),
      `<div class="mp-insp" id="mp-insp">${esc(t('Loading…'))}</div>`, { wide: true });
    const box = m.wrap.querySelector('#mp-insp');
    let d;
    try {
      const r = await siteGET('/site/mods/releases/' + encodeURIComponent(id) + '/inspect');
      if (!r.ok) throw new Error('http');
      d = await r.json();
    } catch (_) { box.textContent = t('Could not read this build.'); return; }
    if (!d.readable) { box.textContent = t('This build could not be decoded.'); return; }
    box.innerHTML = inspectorHTML(d, id);
    box.querySelectorAll('[data-insp-file]').forEach((b) =>
      b.addEventListener('click', () => downloadReleaseFile(id, b.getAttribute('data-insp-file'))));
  }

  function inspectorHTML(d, id) {
    const chip = (icon, text) => `<span class="mp-insp-chip"><i class="fa-solid ${icon}"></i> ${esc(text)}</span>`;
    const head = `<div class="mp-insp-chips">
      ${chip('fa-file-zipper', (d.format === 'zip' ? '.zip' : '.tmod') + ' · ' + fmtBytes(d.size))}
      ${chip('fa-folder-tree', d.file_count + ' ' + t(d.file_count === 1 ? 'file' : 'files'))}
      ${d.total_size ? chip('fa-box-open', t('Unpacked') + ' ' + fmtBytes(d.total_size)) : ''}
      ${d.version != null ? chip('fa-code-branch', t('Format version') + ' ' + d.version) : ''}
    </div>
    <p class="mp-insp-sha" title="${esc(t('Content hash of this build'))}"><code>${esc(d.sha256 || '')}</code></p>`;

    // Header properties: what the game and mod sites read off the build.
    const props = d.properties || {};
    const keys = Object.keys(props).sort((a, b) => a.toLowerCase() < b.toLowerCase() ? -1 : 1);
    const propRows = keys.map((k) => `<tr><th>${esc(k)}</th><td>${esc(props[k])}</td></tr>`).join('');
    const cats = (d.categories || []).length
      ? `<p class="mp-insp-cats">${(d.categories).map((c) => `<span class="mp-tag">${esc(c)}</span>`).join('')}</p>` : '';
    const propsBlock = keys.length
      ? `<h3 class="mp-insp-h">${esc(t('Header'))}</h3>${cats}
         <table class="mp-insp-props"><tbody>${propRows}</tbody></table>`
      : `<h3 class="mp-insp-h">${esc(t('Header'))}</h3><p class="mp-muted">${esc(t('A .zip build carries no header.'))}</p>`;

    return head + propsBlock
      + `<h3 class="mp-insp-h">${esc(t('Files'))}</h3>`
      + `<div class="mp-insp-tree">${treeHTML(fileTree(d.files || []), d, id)}</div>`;
  }

  // Flat paths -> nested folders. Folders keep their own total size so a collapsed
  // one still says how much is under it.
  function fileTree(files) {
    const root = { dirs: new Map(), files: [], size: 0 };
    files.forEach((f) => {
      const parts = String(f.path || '').split('/').filter(Boolean);
      let node = root;
      node.size += Number(f.size || 0);
      parts.slice(0, -1).forEach((part) => {
        if (!node.dirs.has(part)) node.dirs.set(part, { dirs: new Map(), files: [], size: 0 });
        node = node.dirs.get(part);
        node.size += Number(f.size || 0);
      });
      node.files.push(f);
    });
    return root;
  }

  function treeHTML(node, d, id, depth) {
    depth = depth || 0;
    const dirs = [...node.dirs.entries()].sort((a, b) => a[0].toLowerCase() < b[0].toLowerCase() ? -1 : 1);
    const folders = dirs.map(([name, child]) => {
      const count = countFiles(child);
      return `<details class="mp-insp-dir" ${depth < 1 ? 'open' : ''}>
        <summary><i class="fa-solid fa-folder"></i> <span class="mp-insp-name">${esc(name)}</span>
          <span class="mp-insp-meta">${count} · ${fmtBytes(child.size)}</span></summary>
        <div class="mp-insp-kids">${treeHTML(child, d, id, depth + 1)}</div>
      </details>`;
    }).join('');
    const files = node.files.map((f) => {
      const low = String(f.path).toLowerCase();
      // The preview image isn't served individually (it's the build's thumbnail),
      // so it's listed but not offered as a download.
      const isPreview = d.preview_path && low === d.preview_path;
      const isConfig = d.config_path && low === d.config_path;
      const role = isPreview ? `<span class="mp-insp-role">${esc(t('Preview'))}</span>`
        : isConfig ? `<span class="mp-insp-role">${esc(t('Config'))}</span>` : '';
      const dl = isPreview ? ''
        : `<button type="button" class="mp-btn mp-btn-sm" data-insp-file="${esc(f.path)}" aria-label="${esc(t('Download'))}"><i class="fa-solid fa-download"></i></button>`;
      return `<div class="mp-insp-file">
        <i class="fa-regular fa-file"></i>
        <span class="mp-insp-name" title="${esc(f.path)}">${esc(String(f.path).split('/').pop())}</span>
        ${role}<span class="mp-insp-meta">${fmtBytes(f.size)}</span>${dl}
      </div>`;
    }).join('');
    return folders + files;
  }

  function countFiles(node) {
    let n = node.files.length;
    node.dirs.forEach((child) => { n += countFiles(child); });
    return n;
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

  // A mod's settings live in a .cfg that belongs in the game's ModCfgs folder, so
  // installing the .tmod alone isn't enough. When a release packs one we surface it
  // as its own button next to Download; the file is pulled out of the .tmod on the
  // fly (nothing is stored server-side).
  function loadReleaseCfgs() {
    // The header button and the release row can point at the SAME release - group
    // the slots so that release is only looked inside once.
    const byRelease = new Map();
    document.querySelectorAll('[data-rel-cfg]').forEach((slot) => {
      const relId = slot.getAttribute('data-rel-cfg');
      if (!byRelease.has(relId)) byRelease.set(relId, []);
      byRelease.get(relId).push(slot);
    });
    byRelease.forEach(async (slots, relId) => {
      try {
        const r = await siteGET('/site/mods/releases/' + encodeURIComponent(relId) + '/cfgs');
        if (!r.ok) return;
        const body = (await r.json()) || {};
        const all = body.items || [];
        // The owner's "attach a config" action rides on the same answer this call
        // already gives: only a build with a Flash UI can carry one, so a mod that
        // can't never shows the action at all.
        if (state.detail.is_owner && body.has_flash_ui) {
          document.querySelectorAll('[data-rel-cfgset]').forEach((b) => {
            if (b.getAttribute('data-rel-cfgset') !== relId) return;
            b.innerHTML = `<i class="fa-solid fa-sliders"></i> ${esc(all.length ? t('Replace config') : t('Attach config'))}`;
            b.hidden = false;
          });
        }
        if (!all.length) return;
        // A build says which of its files IS the config (configPath), so a mod that
        // happens to pack several .cfg files still gets ONE button. Only when
        // nothing is declared do we show them all - named, since we can't know
        // which one matters. The rest stay reachable under Files either way.
        const declared = all.find((f) => f.declared);
        const items = declared ? [declared] : all.slice(0, 5);
        const hidden = all.length - items.length;
        slots.forEach((slot) => {
          const sm = slot.closest('.mp-release-actions') ? ' mp-btn-sm' : '';
          slot.innerHTML = items.map((f) => `<button type="button" class="mp-btn${sm} mp-cfg-btn" data-cfg-rel="${esc(relId)}" data-cfg-path="${esc(f.path)}" title="${esc(f.filename)}">
            <i class="fa-solid fa-sliders"></i> ${esc(items.length === 1 ? t('Config') : f.filename)}</button>`).join('')
            + (hidden > 0 ? `<span class="mp-muted mp-cfg-more">${esc(t('More under Files'))}</span>` : '');
          slot.querySelectorAll('.mp-cfg-btn').forEach((b) => b.addEventListener('click', () =>
            downloadReleaseCfg(b.getAttribute('data-cfg-rel'), b.getAttribute('data-cfg-path'))));
        });
      } catch (_) { /* a release with no readable config just shows no button */ }
    });
  }

  async function downloadReleaseCfg(id, path) {
    try {
      const r = await siteGET('/site/mods/releases/' + encodeURIComponent(id) + '/cfg?path=' + encodeURIComponent(path));
      if (!r.ok) { toast(t('Could not download that file.'), true); return; }
      const blob = await r.blob();
      const a = document.createElement('a');
      a.href = URL.createObjectURL(blob);
      // The server names the file (it restores the casing the game expects); fall
      // back to the packed name if the header isn't readable.
      const cd = r.headers.get('Content-Disposition') || '';
      const m = /filename="([^"]+)"/.exec(cd);
      a.download = m ? m[1] : path.split('/').pop();
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
    if (r.ok) { toast(i >= 0 ? t('Edition shown.') : t('Edition hidden.')); await loadDetail(); }
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
    return window.BTTModal.open({ title, body: bodyHTML, wide });
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
      ${ModsI18n.editorHTML('mp-text')}
      <label class="mp-form-field"><span id="mp-title-label">${esc(t('Title'))}</span><input name="title" maxlength="120" required></label>
      <label class="mp-form-field"><span id="mp-summary-label">${esc(t('Short summary'))}</span><input name="summary" maxlength="280"></label>
      <label class="mp-form-field"><span id="mp-desc-label">${esc(t('Description (Markdown)'))}</span><textarea name="description" maxlength="40000"></textarea></label>
      <label class="mp-form-field"><span id="mp-warn-label"><i class="fa-solid fa-triangle-exclamation"></i> ${esc(t('Warnings'))}</span><textarea name="warnings" rows="3" maxlength="4000" placeholder="${esc(t('Highlighted below the description. <br> starts a new warning block.'))}"></textarea></label>
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
    const editForm = m.wrap.querySelector('#mp-edit-form');
    // One tab strip over all four prose fields: pick a language, write the whole
    // mod in it. The title's translation is display only (see the model).
    const collectText = ModsI18n.wireEditor(editForm, 'mp-text', [
      { base: d.title, translations: d.title_i18n, area: editForm.querySelector('input[name="title"]'),
        labelEl: editForm.querySelector('#mp-title-label'), label: t('Title') },
      { base: d.summary, translations: d.summary_i18n, area: editForm.querySelector('input[name="summary"]'),
        labelEl: editForm.querySelector('#mp-summary-label'), label: t('Short summary') },
      { base: d.description, translations: d.description_i18n, area: editForm.querySelector('textarea[name="description"]'),
        labelEl: editForm.querySelector('#mp-desc-label'), label: t('Description (Markdown)') },
      { base: d.warnings, translations: d.warnings_i18n, area: editForm.querySelector('textarea[name="warnings"]'),
        labelEl: editForm.querySelector('#mp-warn-label'), label: t('Warnings') },
    ]);
    editForm.addEventListener('submit', async (e) => {
      e.preventDefault();
      const f = e.target;
      const [title, summary, desc, warn] = collectText();
      const body = {
        title: title.base.trim(), title_i18n: title.translations,
        summary: summary.base.trim(), summary_i18n: summary.translations,
        description: desc.base, description_i18n: desc.translations,
        warnings: warn.base, warnings_i18n: warn.translations,
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
        <div class="mp-drop" id="mp-drop" tabindex="0" role="button" aria-label="${esc(t('Drop files here or click to choose'))}"><i class="fa-solid fa-cloud-arrow-up" aria-hidden="true"></i> ${esc(t('Drop files here or click to choose'))}</div>
        <input type="file" id="mp-files" multiple hidden>
        <div class="mp-droplist" id="mp-droplist"></div>
      </div>
      <label class="mp-form-field"><span>${esc(t('Commit message'))}</span><input name="message" maxlength="500" required></label>
      <p class="mp-form-hint">${esc(t('Folder paths from a drag-drop are kept as the in-mod path.'))}</p>
      <p class="mp-form-hint">${esc(t('Files hold your mod source. A built .tmod goes under New release instead.'))}</p>
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
    // A built .tmod is a release, not source - drop it here and it would sit in
    // Files where nobody can install it. Rejected client-side and server-side.
    const add = (files) => {
      let blocked = false;
      for (const f of files) {
        const path = (f.webkitRelativePath || f.name);
        if (/\.tmod$/i.test(path)) { blocked = true; continue; }
        picked.push({ file: f, path });
      }
      refresh();
      const err = m.wrap.querySelector('.mp-form-error');
      if (!err) return;
      err.textContent = blocked ? t('A built .tmod goes in a release, not in your files - use New release to upload it.') : '';
      err.hidden = !blocked;
    };
    drop.addEventListener('click', () => input.click());
    drop.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' || e.key === ' ' || e.key === 'Spacebar') { e.preventDefault(); input.click(); }
    });
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
  // Read a .tmod's header - its properties + the paths it packs - without pulling
  // in the whole file: the header is uncompressed and its size is the first field.
  // Layout: u64 header size, u16 version, u16 property count, then LEB128-length-
  // prefixed name/value pairs, then the file table (1-byte path length, the path,
  // then four LEB128 fields). Returns null for anything that doesn't parse.
  async function readTmodHeader(file) {
    try {
      const first = new Uint8Array(await file.slice(0, 12).arrayBuffer());
      if (first.length < 12) return null;
      const size = new DataView(first.buffer).getUint32(0, true);
      const high = new DataView(first.buffer).getUint32(4, true);
      if (high !== 0 || size < 12 || size > file.size || size > 16 * 1024 * 1024) return null;
      const b = new Uint8Array(await file.slice(0, size).arrayBuffer());
      const dv = new DataView(b.buffer);
      let pos = 10;                                   // skip size (8) + version (2)
      const propCount = dv.getUint16(pos, true); pos += 2;
      const leb = () => {
        let result = 0, shift = 0, byte;
        do { byte = b[pos++]; result += (byte & 0x7f) * Math.pow(2, shift); shift += 7; }
        while (byte & 0x80);
        return result;
      };
      const dec = new TextDecoder();
      const str = (n) => dec.decode(b.subarray(pos, pos += n));
      const props = {};
      for (let i = 0; i < propCount; i++) {
        const name = str(leb());
        props[name] = str(leb());
      }
      const paths = [];
      while (pos < size) {
        paths.push(str(b[pos++]));
        leb(); leb(); leb(); leb();                   // index / offset / size / checksum
      }
      return { props, paths };
    } catch (_) { return null; }
  }

  function fileToBase64(file) {
    return new Promise((resolve, reject) => {
      const fr = new FileReader();
      fr.onerror = () => reject(fr.error);
      fr.onload = () => resolve(String(fr.result).split(',')[1] || '');
      fr.readAsDataURL(file);
    });
  }

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
    // Settings file for a Flash UI mod. Stays hidden - and unsent - until we can
    // see that the build actually ships a .swf, since nothing else reads one.
    const configField = `
      <div class="mp-form-field" id="mp-cfg-field" hidden><span>${esc(t('Settings file (.cfg)'))}</span>
        <input type="file" name="config" accept=".cfg,text/plain">
        <p class="mp-form-hint"><code id="mp-cfg-name"></code> ${esc(t('Packed into the build under this name, where Trove looks for it. Stored only in the build, not your files.'))}</p>
      </div>`;
    const m = openModal(t('New release'), `<form class="mp-form" id="mp-rel-form">
      ${modeRow}
      <label class="mp-form-field"><span>${esc(t('Version tag'))}</span><input name="tag" maxlength="60" required placeholder="v1.0.0"></label>
      <label class="mp-form-field"><span>${esc(t('Title'))}</span><input name="title" maxlength="160"></label>
      ${compileFields}
      ${authorField}
      ${previewPicker}
      <label class="mp-form-field" data-mode="upload" ${filesMode ? 'hidden' : ''}><span>${esc(t('Build file (.tmod or .zip)'))}</span><input type="file" name="file" accept=".tmod,.zip,application/octet-stream,application/zip"></label>
      <label class="mp-form-field" data-mode="upload" ${filesMode ? 'hidden' : ''}><span>${esc(t('Edition'))}</span><input name="upload_branch" maxlength="80" value="${esc(state.detail.default_branch || 'main')}"></label>
      ${configField}
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

    // ── Settings file (.cfg) ────────────────────────────────────────────────
    // Only a mod with a Flash UI (.swf) can carry one, so the field appears only
    // once we can see a .swf in the build being cut: the branch's committed files
    // when compiling, or the picked .tmod's own file table when uploading. If we
    // can't tell (unreadable file, tree fetch failed), it stays hidden - the
    // server enforces the same rule either way.
    const cfgField = f.querySelector('#mp-cfg-field');
    const cfgInput = f.querySelector('input[name=config]');
    const cfgName = f.querySelector('#mp-cfg-name');
    const swfByRef = {};                       // ref -> does its tree hold a .swf
    const packedCfgName = (title) =>
      'ui/' + (String(title || '').replace(/[<>:"/\\|?*\x00-\x1f]/g, '').trim().replace(/\.+$/, '')
        .slice(0, 120) || 'mod').toLowerCase() + '.cfg';

    async function branchHasSwf(ref) {
      if (ref in swfByRef) return swfByRef[ref];
      let has = false;
      try {
        const r = await siteGET(`/site/mods/projects/${PROJ_PATH}/tree?ref=${encodeURIComponent(ref)}`);
        if (r.ok) {
          const d = await r.json();
          has = (d.entries || []).some((e) => String(e.path || '').toLowerCase().endsWith('.swf'));
        }
      } catch (_) { has = false; }
      swfByRef[ref] = has;
      return has;
    }

    async function refreshConfigField() {
      const mode = f.mode.value;
      const fmt = fmtSel ? fmtSel.value : 'tmod';
      let show = false;
      let title = '';
      if (mode === 'upload') {
        const picked = f.file && f.file.files[0];
        const head = (picked && !/\.zip$/i.test(picked.name)) ? await readTmodHeader(picked) : null;
        show = !!(head && head.paths.some((p) => p.toLowerCase().endsWith('.swf')));
        title = head ? (head.props.title || '') : '';
      } else {
        show = fmt === 'tmod' && await branchHasSwf(f.ref ? f.ref.value : '');
        title = state.detail.title || '';       // what the compiler stamps as the title
      }
      if (!show && cfgInput) cfgInput.value = '';   // never send what we've hidden
      if (cfgName) cfgName.textContent = packedCfgName(title);
      if (cfgField) cfgField.hidden = !show;
    }
    f.querySelectorAll('input[name=mode]').forEach((r) => r.addEventListener('change', refreshConfigField));
    if (fmtSel) fmtSel.addEventListener('change', refreshConfigField);
    if (f.ref) f.ref.addEventListener('change', refreshConfigField);
    if (f.file) f.file.addEventListener('change', refreshConfigField);
    refreshConfigField();

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
      inp.type = 'file'; inp.accept = 'image/png,image/jpeg,image/gif';
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
        // Only sent when the field is showing, i.e. the build has a Flash UI.
        if (!cfgField.hidden && cfgInput.files.length) fd.append('config', cfgInput.files[0]);
        r = await apiForm('/v1/mods/hub/projects/' + PROJ_PATH + '/releases/upload', fd);
      } else {
        const cfg = (!cfgField.hidden && cfgInput.files.length)
          ? await fileToBase64(cfgInput.files[0]) : null;
        r = await apiJSON('/v1/mods/hub/projects/' + PROJ_PATH + '/releases', {
          json: {
            tag: f.tag.value.trim(), title: f.title.value.trim(),
            changelog: f.changelog.value, ref: f.ref.value,
            format: f.format ? f.format.value : 'tmod', status: f.status.value,
            preview_sha: (f.format && f.format.value === 'tmod') ? selectedPreview : null,
            author: f.author ? f.author.value.trim() : null,
            config_base64: (f.format && f.format.value === 'tmod') ? cfg : null,
          },
        });
      }
      btn.disabled = false;
      if (r.ok) { m.close(); toast(t('Release published.')); await loadDetail(); }
      else showFormError(f, errMsg(r, 'Could not create the release.'));
    });
  }

  // Edit a cut release's wording (the tag and the built file are fixed), in as
  // many languages as the modder wants.
  function openReleaseEdit(id) {
    const r = (state.detail.releases || []).find((x) => x.id === id);
    if (!r) return;
    const m = openModal(t('Edit release') + ' ' + r.tag, `<form class="mp-form" id="mp-reledit-form">
      ${ModsI18n.editorHTML('mp-rel')}
      <label class="mp-form-field"><span id="mp-rel-title-label">${esc(t('Release title'))}</span><input name="title" maxlength="160"></label>
      <label class="mp-form-field"><span id="mp-rel-log-label">${esc(t('Changelog'))}</span><textarea name="changelog" rows="8" maxlength="20000"></textarea></label>
      <p class="mp-form-hint">${esc(t('Leave a translation empty to remove it.'))}</p>
      <p class="mp-form-error" hidden></p>
      <div class="mp-form-actions">
        <button type="button" class="mp-btn" data-close>${esc(t('Cancel'))}</button>
        <button type="submit" class="mp-btn mp-btn-primary">${esc(t('Save'))}</button>
      </div></form>`, { wide: true });
    const form = m.wrap.querySelector('#mp-reledit-form');
    const collect = ModsI18n.wireEditor(form, 'mp-rel', [
      { base: r.title, translations: r.title_i18n, area: form.querySelector('input[name="title"]'),
        labelEl: form.querySelector('#mp-rel-title-label'), label: t('Release title') },
      { base: r.changelog, translations: r.changelog_i18n, area: form.querySelector('textarea[name="changelog"]'),
        labelEl: form.querySelector('#mp-rel-log-label'), label: t('Changelog') },
    ]);
    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      const [title, log] = collect();
      const res = await apiJSON('/v1/mods/hub/releases/' + encodeURIComponent(id), {
        method: 'PATCH',
        json: {
          title: title.base.trim(), title_i18n: title.translations,
          changelog: log.base, changelog_i18n: log.translations,
        },
      });
      if (res.ok) { m.close(); toast(t('Saved.')); await loadDetail(); }
      else showFormError(form, errMsg(res, 'Could not update the release.'));
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
      const r = await apiJSON('/v1/moderation/report', { json: { target_type: 'mod', handle: HANDLE, slug: SLUG, reason: f.reason.value.trim() } });
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
