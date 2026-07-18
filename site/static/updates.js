/* ═══════════════════════════════════════════════════════════════════════
   /updates - page logic
   ───────────────────────────────────────────────────────────────────────
   Fetches branches, versions, tree, file history, and inline diffs from
   /site/updates/* (same-origin proxies that skip the public API's
   token/scope/rate-limit pipeline). Renders three tabs sharing one
   state object: explorer (tree + per-file history), changes (per-
   version change browser), compare (two-version diff).

   URL hash mirrors selection:
     branch=…, tab=…, path=…, version=… (ordinal)
   Reload-safe and bookmarkable.

   Locale labels go through i18n.js (data-i18n attrs); game-data labels
   (paths, version tags, sha hashes) render verbatim.
   ═══════════════════════════════════════════════════════════════════════ */

(function () {
  'use strict';

  const { esc, fetchJSON } = window.BTTUtil;

  const PAGE_SIZE_CHANGES = 200;
  const VERSIONS_VISIBLE = 12;   // recent version chips in the timeline strip
  const TREE_PAGE = 300;         // sidebar rows rendered per "load more" (some Trove
                                 // folders hold 50k+ files - never render them all)

  const state = {
    branches: [],            // [{branch, current_version, current_ordinal, ...}]
    branch: null,            // active branch name
    versions: [],            // recent VersionInfo for the active branch
    versionsTotal: 0,
    selectedVersion: null,   // ordinal - drives the "Changes" tab + tree badges
    activeTab: 'explorer',   // 'explorer' | 'changes' | 'compare'
    viewMode: readViewMode(),// 'list' | 'grid' - explorer sidebar as rows vs a thumbnail gallery
    treeSort: readTreeSort(),// 'name' | 'modified' - order of the explorer listing

    // Explorer tab
    treeCache: new Map(),    // prefix → entries (avoid re-fetching as you walk)
    treePrefix: '',          // current directory prefix
    treeVisible: TREE_PAGE,  // how many rows of the current dir are rendered (paged)
    treeFilter: '',          // sidebar search text
    // Full-tree search: when treeFilter is set the sidebar shows matches from
    // ANYWHERE in the branch (server-side), not just the loaded directory.
    searchResults: null,     // null = not searching; [] = searched, no hits
    searchTotal: 0,          // true match count (may exceed searchResults.length)
    searchLoading: false,
    searchToken: 0,          // guards against out-of-order async responses
    selectedPath: null,      // null = directory view, string = file detail view

    // Per-file detail
    fileHistory: null,       // {path, items}
    historyPicks: { a: null, b: null },  // ordinals chosen for inline compare-jump

    // Per-version "touched paths" - used to badge tree rows that
    // changed in the selected version. Just the modified/added/removed
    // path set for fast lookups.
    versionTouched: null,    // { ordinal, byPath: Map<path, 'added'|'modified'|'removed'> }

    // Changes tab
    changes: { entries: [], total: 0, ordinal: null, version_tag: null,
               counts: {added: 0, modified: 0, removed: 0}, filter: 'all',
               offset: 0, loading: false },
    // Which change-tree folders are collapsed, remembered PER VERSION (keyed by
    // ordinal) so it survives re-fetching the change-list, switching versions,
    // and the browser Back button. Lives outside `changes` (which gets rebuilt).
    changesCollapsed: new Map(),   // ordinal → Set<dirPath>

    // Compare tab
    compare: { from: null, to: null, path: '', payload: null, loading: false },
  };

  // ─── DOM refs ──────────────────────────────────────────────────────
  const $ = (id) => document.getElementById(id);
  const $branches = $('up-branches');
  const $meta = $('up-meta');

  const $versions = $('up-versions');
  const $versionsHint = $('up-versions-hint');

  const $tabExplorer = $('up-tab-explorer');
  const $tabChanges = $('up-tab-changes');
  const $tabCompare = $('up-tab-compare');
  const $tabChangesBadge = $('up-tab-changes-badge');
  const $paneExplorer = $('up-pane-explorer');
  const $paneChanges = $('up-pane-changes');
  const $paneCompare = $('up-pane-compare');

  const $viewToggle = $('up-view-toggle');
  const $sortSelect = $('up-sort-select');

  const $treeSearch = $('up-tree-search');
  const $breadcrumbs = $('up-breadcrumbs');
  const $tree = $('up-tree');
  const $sidebar = $('up-sidebar');
  const $mobileTrigger = $('up-mobile-trigger');
  const $mobileSelected = $('up-mobile-selected');

  const $detailEmpty = $('up-detail-empty');
  const $detailFile = $('up-detail-file');
  const $detailBack = $('up-detail-back');
  const $detailTitle = $('up-detail-title');
  const $detailMeta = $('up-detail-meta');
  const $detailDownload = $('up-detail-download');
  const $history = $('up-history');
  const $preview = $('up-preview');
  const $previewPre = $('up-preview-pre');
  const $previewImage = $('up-preview-image');
  const $previewImg = $('up-preview-img');
  const $previewImgCap = $('up-preview-imgcap');
  const $previewDds = $('up-preview-dds');
  const $previewCanvas = $('up-preview-canvas');
  const $previewDdsCap = $('up-preview-ddscap');
  const $previewPng = $('up-preview-png');
  const $previewModel = $('up-preview-model');
  const $preview3d = $('up-preview-3d');
  const $previewHex = $('up-preview-hex');
  const $previewNote = $('up-preview-note');

  // Hard cap on the bytes a hex viewer will render client-side. Matches the
  // server's VIEW_MAX_BYTES so anything it flags as "binary" fits in one dump.
  const HEX_MAX_BYTES = 1024 * 1024;
  let _previewToken = 0;   // guards against out-of-order image/hex fetches

  const $changesMeta = $('up-changes-meta');
  const $changesBody = $('up-changes-body');
  const $changesFoot = $('up-changes-foot');
  const $changesMore = $('up-changes-more');
  const $countAdd = $('up-changes-count-added');
  const $countMod = $('up-changes-count-modified');
  const $countRem = $('up-changes-count-removed');

  const $comparePath = $('up-compare-path');
  const $compareFrom = $('up-compare-from');
  const $compareTo = $('up-compare-to');
  const $compareRun = $('up-compare-run');
  const $compareMeta = $('up-compare-meta');
  const $compareBody = $('up-compare-body');

  // ─── Boot ──────────────────────────────────────────────────────────
  init().catch((err) => {
    console.error('[updates] boot failed', err);
    $tree.innerHTML = errorHTML(err);
  });

  async function init() {
    const data = await fetchJSON('/site/updates/branches');
    state.branches = data.items || [];
    if (!state.branches.length) {
      $branches.innerHTML = `<span class="up-branches-loading" data-i18n>No branches tracked yet.</span>`;
      rerunI18n();
      return;
    }
    renderBranchTabs();
    applyViewMode();   // reflect the remembered list/grid choice before first render
    if ($sortSelect) $sortSelect.value = state.treeSort;

    // URL hash priority - pick branch + tab + path + version from the
    // hash if present, else default to the first branch / latest version.
    // Applied with history writes suppressed, then canonicalised as ONE
    // replace so the initial load doesn't leave spurious back-stack entries.
    const hash = parseHash();
    const startBranch =
      (hash.branch && state.branches.find((b) => b.branch === hash.branch)?.branch)
      || state.branches[0].branch;
    _suppressHash = true;
    try {
      await selectBranch(startBranch);
      if (hash.version != null) {
        const v = state.versions.find((x) => x.ordinal === hash.version);
        if (v) await selectVersion(v.ordinal);
      }
      if (hash.path) await openTreePath(hash.path);
      if (hash.tab) switchTab(hash.tab);
    } finally {
      _suppressHash = false;
    }
    writeHash(false);

    wireEvents();
  }

  // ─── Branch tabs ───────────────────────────────────────────────────
  function renderBranchTabs() {
    $branches.innerHTML = state.branches.map((b) => `
      <button type="button" class="up-branch${b.branch === state.branch ? ' active' : ''}"
              data-branch="${esc(b.branch)}" role="tab"
              aria-selected="${b.branch === state.branch}">
        <span class="up-branch-dot"></span>
        ${esc(branchLabel(b.branch))}
      </button>`).join('');
    for (const btn of $branches.querySelectorAll('[data-branch]')) {
      btn.addEventListener('click', () => navigateBranch(btn.dataset.branch));
    }
  }

  // A branch switch loads versions + tree over the network, so suppress the
  // per-step hash writes and push a single entry once it settles.
  async function navigateBranch(branch) {
    _suppressHash = true;
    try { await selectBranch(branch); }
    finally { _suppressHash = false; }
    scheduleHash(true);
  }

  function branchLabel(b) {
    // Canonical short labels; the server's id is the technical key.
    if (b === 'live-us') return 'Live US';
    if (b === 'pts')     return 'PTS';
    return b;
  }

  async function selectBranch(branch) {
    if (state.branch === branch) return;
    state.branch = branch;
    // Reset every per-branch piece of state - they're not cross-branch.
    state.treeCache.clear();
    state.treePrefix = '';
    state.treeVisible = TREE_PAGE;
    state.selectedPath = null;
    state.fileHistory = null;
    state.versions = [];
    state.versionTouched = null;
    state.changes = {
      entries: [], total: 0, ordinal: null, version_tag: null,
      counts: {added: 0, modified: 0, removed: 0}, filter: 'all',
      offset: 0, loading: false,
    };
    state.changesCollapsed.clear();   // ordinals are per-branch
    state.compare = { from: null, to: null, path: '', payload: null, loading: false };

    renderBranchTabs();

    // Three fetches in parallel - versions drive the strip + change-tab
    // default selection; tree drives the explorer.
    const [versionsRes, _t] = await Promise.all([
      fetchJSON(`/site/updates/${branch}/versions?limit=${VERSIONS_VISIBLE}`),
      loadTree(''),
    ]);
    state.versions = versionsRes.items || [];
    state.versionsTotal = versionsRes.total || 0;

    // Default selected version = the latest one. Drives the tree-badge
    // overlay AND the changes-tab list.
    if (state.versions.length) {
      await selectVersion(state.versions[0].ordinal);
    }

    renderBranchMeta();
    renderVersions();
    renderCompareSelectOptions();
    renderDetail();
    renderTree();
    renderTab();
  }

  function renderBranchMeta() {
    const meta = state.branches.find((b) => b.branch === state.branch);
    if (!meta) { $meta.textContent = ''; return; }
    const last = meta.last_probe_at ? new Date(meta.last_probe_at) : null;
    const ago = last ? formatRelativeWhen(last) : t('never probed');
    $meta.textContent = t('{n} files · current version {tag} · last probe {when}')
      .replace('{n}', formatInt(meta.file_count))
      .replace('{tag}', meta.current_version || '-')
      .replace('{when}', ago);
  }

  // ─── Version timeline ──────────────────────────────────────────────
  function renderVersions() {
    if (!state.versions.length) {
      $versions.innerHTML = `<p class="up-hint" data-i18n>No captured versions yet.</p>`;
      $versionsHint.textContent = '';
      rerunI18n();
      return;
    }
    $versionsHint.textContent = state.versionsTotal > state.versions.length
      ? t('Showing the most recent {n} of {t}')
        .replace('{n}', state.versions.length).replace('{t}', state.versionsTotal)
      : t('{n} version(s)').replace('{n}', state.versions.length);

    $versions.innerHTML = state.versions.map((v, idx) => {
      const when = v.captured_at ? new Date(v.captured_at) : null;
      const isLatest = idx === 0;
      const isActive = v.ordinal === state.selectedVersion;
      const cls = ['up-version'];
      if (isLatest) cls.push('up-version-latest');
      if (isActive) cls.push('active');
      const latestBadge = isLatest
        ? `<span class="up-version-badge"><i class="fa-solid fa-bolt" aria-hidden="true"></i> ${t('Latest')}</span>`
        : '';
      return `
        <button type="button" class="${cls.join(' ')}"
                data-ordinal="${v.ordinal}"${isActive ? ' aria-current="true"' : ''}
                title="${esc(v.version_tag)}">
          ${latestBadge}
          <span class="up-version-tag">${esc(v.version_tag)}</span>
          <span class="up-version-when">${when ? esc(formatWhen(when)) : ''}</span>
          <span class="up-version-counts">
            <span class="up-count-pill up-count-pill-add">+${formatInt(v.files_added)}</span>
            <span class="up-count-pill up-count-pill-mod">~${formatInt(v.files_modified)}</span>
            <span class="up-count-pill up-count-pill-rem">−${formatInt(v.files_removed)}</span>
          </span>
        </button>`;
    }).join('');
    for (const btn of $versions.querySelectorAll('[data-ordinal]')) {
      btn.addEventListener('click', () => selectVersion(Number(btn.dataset.ordinal)));
    }
  }

  async function selectVersion(ordinal) {
    if (state.selectedVersion === ordinal) {
      renderVersions();
      return;
    }
    state.selectedVersion = ordinal;
    renderVersions();
    scheduleHash(true);

    const v = state.versions.find((x) => x.ordinal === ordinal);
    // Pull this version's change list - drives the changes tab + the
    // touched-path overlay in the tree. Hard-cap at 5000 paths for
    // overlay purposes: very large versions still render correctly,
    // they just stop badging beyond that count.
    const versionChanges = await fetchJSON(
      `/site/updates/${state.branch}/changes?ordinal=${ordinal}&limit=2000`,
    );
    state.changes = {
      entries: versionChanges.entries || [],
      total: versionChanges.total || 0,
      ordinal,
      version_tag: versionChanges.version_tag,
      counts: {
        added: versionChanges.files_added || 0,
        modified: versionChanges.files_modified || 0,
        removed: versionChanges.files_removed || 0,
      },
      filter: state.changes.filter || 'all',
      offset: versionChanges.entries.length,
      loading: false,
    };
    // Build the touched-overlay index. Same data we just fetched; we
    // index it once and the tree-render checks O(1) per row.
    const byPath = new Map();
    for (const e of versionChanges.entries || []) byPath.set(e.path, e.type);
    state.versionTouched = { ordinal, byPath };

    renderChanges();
    renderTree();
    if ($tabChangesBadge) {
      const total = state.changes.total || 0;
      if (total > 0) {
        $tabChangesBadge.hidden = false;
        $tabChangesBadge.textContent = formatInt(total);
      } else {
        $tabChangesBadge.hidden = true;
      }
    }
  }

  // ─── Tab strip ─────────────────────────────────────────────────────
  function switchTab(name) {
    if (name !== 'explorer' && name !== 'changes' && name !== 'compare') name = 'explorer';
    if (state.activeTab === name) return;
    state.activeTab = name;
    renderTab();
    scheduleHash(true);
  }
  function renderTab() {
    const map = {
      explorer: { btn: $tabExplorer, pane: $paneExplorer },
      changes:  { btn: $tabChanges,  pane: $paneChanges },
      compare:  { btn: $tabCompare,  pane: $paneCompare },
    };
    for (const [name, { btn, pane }] of Object.entries(map)) {
      const isActive = state.activeTab === name;
      btn.classList.toggle('active', isActive);
      btn.setAttribute('aria-selected', String(isActive));
      btn.tabIndex = isActive ? 0 : -1;   // roving tabindex for the tablist
      pane.classList.toggle('active', isActive);
      pane.hidden = !isActive;
    }
  }

  // ─── Explorer - tree + breadcrumbs ─────────────────────────────────
  async function loadTree(prefix) {
    if (state.treeCache.has(prefix)) return state.treeCache.get(prefix);
    const data = await fetchJSON(
      `/site/updates/${state.branch}/tree?prefix=${encodeURIComponent(prefix)}`,
    );
    state.treeCache.set(prefix, data.entries || []);
    return data.entries || [];
  }

  function renderBreadcrumbs() {
    const segments = state.treePrefix
      ? state.treePrefix.replace(/\/$/, '').split('/').filter(Boolean)
      : [];
    const parts = [
      `<button type="button" class="up-crumb${segments.length === 0 ? ' up-crumb-last' : ''}"
               data-prefix="">
         <i class="fa-solid fa-house" aria-hidden="true"></i> ${t('root')}
       </button>`,
    ];
    let acc = '';
    for (let i = 0; i < segments.length; i++) {
      acc += segments[i] + '/';
      const isLast = i === segments.length - 1;
      parts.push(`<span class="up-crumb-sep">/</span>`);
      parts.push(`
        <button type="button" class="up-crumb${isLast ? ' up-crumb-last' : ''}"
                data-prefix="${esc(acc)}">${esc(segments[i])}</button>`);
    }
    $breadcrumbs.innerHTML = parts.join('');
    for (const btn of $breadcrumbs.querySelectorAll('[data-prefix]')) {
      btn.addEventListener('click', () => navigateTree(btn.dataset.prefix));
    }
  }

  async function navigateTree(prefix) {
    state.treePrefix = prefix;
    state.treeVisible = TREE_PAGE;   // fresh directory - reset paging
    state.selectedPath = null;  // leaving file-detail view
    state.fileHistory = null;
    renderTree();
    renderDetail();
    scheduleHash(true);
    await loadTree(prefix);
    renderTree();
  }

  async function renderTree() {
    // When a search is active, the sidebar shows full-tree matches instead of
    // the current directory listing (so files inside collapsed folders show up).
    if (state.treeFilter) { renderSearchResults(); return; }

    renderBreadcrumbs();
    const entries = state.treeCache.get(state.treePrefix);
    if (!entries) {
      $tree.innerHTML = `<p class="up-loading" data-i18n>Loading…</p>`;
      rerunI18n();
      return;
    }
    if (!entries.length) {
      $tree.innerHTML = `<p class="up-tree-empty" data-i18n>Nothing here.</p>`;
      rerunI18n();
      return;
    }
    // Render at most treeVisible rows - a "load more" reveals the next page.
    // Trove folders like blueprints/ can hold tens of thousands of files, so
    // dumping the whole listing into the DOM at once would lock up the page.
    const ordered = sortEntries(entries);
    const shown = Math.min(state.treeVisible, ordered.length);
    const visible = ordered.slice(0, shown);

    const touched = state.versionTouched && state.versionTouched.byPath;

    // "load more" footer, shared by both layouts.
    const more = entries.length > shown
      ? `<button type="button" class="up-tree-more" data-tree-more>
           <i class="fa-solid fa-chevron-down" aria-hidden="true"></i>
           ${esc(t('Load more'))}
           <span class="up-tree-more-count">${esc(
             t('{n} of {total}')
               .replace('{n}', formatInt(shown))
               .replace('{total}', formatInt(entries.length)))}</span>
         </button>`
      : '';

    // Grid/gallery layout: a tile per entry (image thumbnail when the file is a
    // renderable image, a file-type tile otherwise).
    if (state.viewMode === 'grid') {
      const tiles = visible.map((e) => tileHTML(e, touched)).join('');
      $tree.innerHTML = `<div class="up-gallery">${tiles}</div>${more}`;
      observeDdsTiles();
      return;
    }

    const rows = visible.map((e) => {
      const icon = e.is_dir
        ? 'fa-folder'
        : iconForName(e.name);
      const sizeOrCount = entryMeta(e);

      // Touched indicator: directory shows a dot if ANY descendant
      // changed; file shows the change type.
      let touchHTML = '';
      if (touched) {
        if (!e.is_dir) {
          const kind = touched.get(e.path);
          if (kind) {
            const cls = kind === 'modified' ? 'up-touch-mod'
                      : kind === 'removed'  ? 'up-touch-rem'
                      : '';
            touchHTML = `<span class="up-row-touch ${cls}" title="${esc(t(kind))}"></span>`;
          }
        } else {
          // For dirs, check if any descendant path starts with this prefix.
          for (const p of touched.keys()) {
            if (p.startsWith(e.path)) { touchHTML = `<span class="up-row-touch" title="${esc(t('contains changes'))}"></span>`; break; }
          }
        }
      }
      return `
        <button type="button" class="up-row${state.selectedPath === e.path ? ' active' : ''}"
                data-path="${esc(e.path)}" data-is-dir="${e.is_dir ? '1' : ''}"
                title="${esc(e.path)}">
          <span class="up-row-icon"><i class="fa-solid ${icon}" aria-hidden="true"></i></span>
          <span class="up-row-name">${esc(e.name)}${touchHTML}</span>
          <span class="up-row-meta">${esc(sizeOrCount)}</span>
        </button>`;
    }).join('');
    $tree.innerHTML = rows + more;
  }

  // ─── Explorer - full-tree search ───────────────────────────────────
  // The sidebar box searches the WHOLE branch, not just the folder that
  // happens to be open. Debounced so we don't fire a request per keystroke,
  // and token-guarded so a slow response for an old query can't overwrite a
  // newer one.
  let _searchTimer = null;
  function scheduleSearch() {
    if (_searchTimer) clearTimeout(_searchTimer);
    const needle = state.treeFilter.trim();
    if (!needle) {
      state.searchResults = null;
      state.searchTotal = 0;
      state.searchLoading = false;
      renderTree();
      return;
    }
    state.searchLoading = true;
    renderSearchResults();  // paint the "Searching…" state immediately
    _searchTimer = setTimeout(() => runSearch(needle), 200);
  }

  async function runSearch(needle) {
    const token = ++state.searchToken;
    try {
      const data = await fetchJSON(
        `/site/updates/${state.branch}/search?q=${encodeURIComponent(needle)}`,
      );
      if (token !== state.searchToken) return;  // stale - a newer query won
      state.searchResults = data.entries || [];
      state.searchTotal = data.total || 0;
    } catch (err) {
      if (token !== state.searchToken) return;
      state.searchResults = [];
      state.searchTotal = 0;
      $tree.innerHTML = errorHTML(err);
      return;
    } finally {
      if (token === state.searchToken) state.searchLoading = false;
    }
    renderSearchResults();
  }

  function renderSearchResults() {
    const needle = state.treeFilter.trim();
    $breadcrumbs.innerHTML = `
      <span class="up-crumb up-crumb-last">
        <i class="fa-solid fa-magnifying-glass" aria-hidden="true"></i>
        ${esc(t('Search'))}
      </span>`;

    if (state.searchLoading && state.searchResults === null) {
      $tree.innerHTML = `<p class="up-loading" data-i18n>Loading…</p>`;
      rerunI18n();
      return;
    }
    const results = sortEntries(state.searchResults || []);
    if (!results.length) {
      $tree.innerHTML = `<p class="up-tree-empty" data-i18n>Nothing here.</p>`;
      rerunI18n();
      return;
    }

    const touched = state.versionTouched && state.versionTouched.byPath;
    const capped = state.searchTotal > results.length
      ? `<p class="up-tree-hint">${esc(
          t('Showing {n} of {total}')
            .replace('{n}', formatInt(results.length))
            .replace('{total}', formatInt(state.searchTotal)))}</p>`
      : '';

    if (state.viewMode === 'grid') {
      const tiles = results.map((e) => tileHTML(e, touched, { search: true })).join('');
      $tree.innerHTML = capped + `<div class="up-gallery">${tiles}</div>`;
      observeDdsTiles();
      return;
    }

    const rows = results.map((e) => {
      const icon = iconForName(e.name);
      const dir = e.path.slice(0, e.path.length - e.name.length);
      let touchHTML = '';
      if (touched) {
        const kind = touched.get(e.path);
        if (kind) {
          const cls = kind === 'modified' ? 'up-touch-mod'
                    : kind === 'removed'  ? 'up-touch-rem' : '';
          touchHTML = `<span class="up-row-touch ${cls}" title="${esc(t(kind))}"></span>`;
        }
      }
      return `
        <button type="button" class="up-row up-row-search${state.selectedPath === e.path ? ' active' : ''}"
                data-path="${esc(e.path)}" title="${esc(e.path)}">
          <span class="up-row-icon"><i class="fa-solid ${icon}" aria-hidden="true"></i></span>
          <span class="up-row-name">
            <span class="up-row-file">${esc(e.name)}${touchHTML}</span>
            ${dir ? `<span class="up-row-dir">${esc(dir)}</span>` : ''}
          </span>
          <span class="up-row-meta">${esc(entryMeta(e))}</span>
        </button>`;
    }).join('');
    $tree.innerHTML = capped + rows;
  }

  // openTreePath drills into deep paths (e.g. coming from the URL hash
  // or compare-tab path field). If the path ends with `/` it lands in
  // the directory; otherwise it opens the file detail view.
  async function openTreePath(path) {
    if (!path) return;
    if (path.endsWith('/')) {
      await navigateTree(path);
      // Pre-warm one level up too so back-navigation is instant.
      const up = path.replace(/[^/]+\/$/, '');
      if (up !== path) loadTree(up).catch(() => {});
      return;
    }
    // File - navigate to its parent directory AND open it.
    const slash = path.lastIndexOf('/');
    const parent = slash >= 0 ? path.slice(0, slash + 1) : '';
    if (state.treePrefix !== parent) {
      state.treePrefix = parent;
      state.treeVisible = TREE_PAGE;
      await loadTree(parent);
    }
    await openFile(path);
  }

  // Jump from the Changes tab to a file in the Explorer. Suppresses the
  // intermediate hash writes so the whole tab-switch + file-open lands as ONE
  // history entry - a single Back press then returns to the changes list.
  async function openChangePath(path) {
    _suppressHash = true;
    try {
      switchTab('explorer');
      await openTreePath(path);
    } finally {
      _suppressHash = false;
    }
    scheduleHash(true);
  }

  // ─── Explorer - file detail (history) ──────────────────────────────
  async function openFile(path) {
    state.selectedPath = path;
    if ($mobileSelected) {
      $mobileSelected.removeAttribute('data-i18n');
      $mobileSelected.textContent = path;
      if (window.BTTi18n && window.BTTi18n.untrack) window.BTTi18n.untrack($mobileSelected);
    }
    renderTree();
    renderDetail();
    scheduleHash(true);
    if ($sidebar) $sidebar.classList.remove('open');

    try {
      const data = await fetchJSON(
        `/site/updates/${state.branch}/file/history?path=${encodeURIComponent(path)}`,
      );
      // Bail if user clicked away while the fetch was running.
      if (state.selectedPath !== path) return;
      state.fileHistory = data;
      // Default pick: A = oldest, B = newest. Lets a single click on
      // "Compare" diff the full history.
      const items = data.items || [];
      if (items.length >= 2) {
        state.historyPicks.a = items[items.length - 1].ordinal;
        state.historyPicks.b = items[0].ordinal;
      } else {
        state.historyPicks.a = null;
        state.historyPicks.b = null;
      }
      renderDetail();
    } catch (err) {
      if (state.selectedPath !== path) return;
      $history.innerHTML = errorHTML(err);
    }
    loadPreview(path);
  }

  // ─── Explorer - in-browser preview ─────────────────────────────────
  // Fetches the view endpoint, which classifies the file by `kind`:
  //   text   → render the UTF-8 content in a <pre>
  //   image  → render an <img> straight from the raw download endpoint
  //   binary → fetch the raw bytes (≤1 MB) and render a hex dump
  //   too_large / missing / removed → just a note; the download link covers it
  async function loadPreview(path) {
    if (!$preview) return;
    const token = ++_previewToken;
    resetPreview();
    let data;
    try {
      data = await fetchJSON(
        `/site/updates/${state.branch}/file/view?path=${encodeURIComponent(path)}`,
      );
    } catch (err) {
      return;   // 404 (removed/missing) etc → no preview, no error noise
    }
    if (token !== _previewToken || state.selectedPath !== path) return;  // clicked away
    $preview.hidden = false;

    // `kind` is authoritative; fall back to `reason`/`viewable` for older payloads.
    const kind = data.kind
      || (data.viewable ? 'text'
          : data.reason === 'image' ? 'image'
          : data.reason === 'too_large' ? 'too_large'
          : data.reason === 'binary' ? 'binary' : 'missing');

    if (kind === 'text' && typeof data.text === 'string') {
      $previewPre.textContent = data.text;
      $previewPre.hidden = false;
      return;
    }
    if (kind === 'image') {
      $previewImg.alt = path;
      $previewImg.onerror = () => {
        if (token !== _previewToken) return;
        $previewImage.hidden = true;
        $previewNote.textContent = t('Preview unavailable.');
        $previewNote.hidden = false;
      };
      $previewImg.src =
        `/v1/updates/${state.branch}/file?path=${encodeURIComponent(path)}`;
      $previewImgCap.textContent = formatBytes(data.size);
      $previewImage.hidden = false;
      return;
    }
    if (kind === 'dds') {
      renderDdsPreview(path, data.size, token);
      return;
    }
    if (kind === 'blueprint') {
      const url = `/site/updates/${state.branch}/file/blueprint?path=${encodeURIComponent(path)}`;
      $preview3d.onclick = () => {
        if (!window.BlueprintViewer) { $previewNote.textContent = t('3D viewer is unavailable.'); $previewNote.hidden = false; return; }
        window.BlueprintViewer.open({ url, title: path });
      };
      $previewModel.hidden = false;
      return;
    }
    if (kind === 'binary') {
      renderHexPreview(path, data.size, token);
      return;
    }
    const note = kind === 'too_large'
      ? t('Too large to preview ({size}) — download it to inspect.')
          .replace('{size}', formatBytes(data.size))
      : t('Preview unavailable.');
    $previewNote.textContent = note;
    $previewNote.hidden = false;
  }

  function resetPreview() {
    $preview.hidden = true;
    $previewPre.hidden = true;
    $previewPre.textContent = '';
    $previewImage.hidden = true;
    $previewImg.onerror = null;
    $previewImg.removeAttribute('src');
    $previewDds.hidden = true;
    $previewPng.onclick = null;
    $previewModel.hidden = true;
    $preview3d.onclick = null;
    $previewHex.hidden = true;
    $previewHex.textContent = '';
    $previewNote.hidden = true;
  }

  // Fetch the raw bytes of a small binary file and render a classic hex dump
  // (offset | 16 hex bytes | ASCII gutter). The server only flags files ≤1 MB as
  // "binary", so a single dump is always bounded; we guard the size anyway.
  async function renderHexPreview(path, size, token) {
    $previewHex.hidden = false;
    $previewHex.textContent = t('Loading…');
    if (size > HEX_MAX_BYTES) {
      $previewHex.hidden = true;
      $previewNote.textContent = t('Too large to preview ({size}) — download it to inspect.')
        .replace('{size}', formatBytes(size));
      $previewNote.hidden = false;
      return;
    }
    let buf;
    try {
      const res = await fetch(
        `/v1/updates/${state.branch}/file?path=${encodeURIComponent(path)}`,
        { credentials: 'omit' },
      );
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      buf = new Uint8Array(await res.arrayBuffer());
    } catch (err) {
      if (token !== _previewToken || state.selectedPath !== path) return;
      $previewHex.hidden = true;
      $previewNote.textContent = t('Binary file — download it to inspect.');
      $previewNote.hidden = false;
      return;
    }
    if (token !== _previewToken || state.selectedPath !== path) return;  // clicked away
    $previewHex.textContent = hexDump(buf);
  }

  // Build a hex dump string. One line per 16 bytes:
  //   00000000  89 50 4e 47 0d 0a 1a 0a  00 00 00 0d 49 48 44 52  |.PNG........IHDR|
  function hexDump(bytes) {
    const HEX = [];
    for (let i = 0; i < 256; i++) HEX.push(i.toString(16).padStart(2, '0'));
    const lines = [];
    for (let off = 0; off < bytes.length; off += 16) {
      const slice = bytes.subarray(off, off + 16);
      let hex = '';
      let ascii = '';
      for (let i = 0; i < 16; i++) {
        if (i === 8) hex += ' ';
        if (i < slice.length) {
          const b = slice[i];
          hex += HEX[b] + ' ';
          ascii += (b >= 0x20 && b < 0x7f) ? String.fromCharCode(b) : '.';
        } else {
          hex += '   ';
        }
      }
      lines.push(off.toString(16).padStart(8, '0') + '  ' + hex + '  ' + ascii);
    }
    return lines.join('\n');
  }

  // ─── Explorer - DDS texture preview (decoded client-side) ──────────────
  // The DDS decoder is an ES module exposed on window.decodeDDS by an inline
  // module in the page. It may not be ready yet when the first file is clicked.
  function ensureDDS() {
    if (window.decodeDDS) return Promise.resolve(window.decodeDDS);
    return new Promise((resolve) => {
      document.addEventListener('btt-dds-ready', () => resolve(window.decodeDDS || null), { once: true });
      setTimeout(() => resolve(window.decodeDDS || null), 4000);   // don't hang forever
    });
  }

  async function renderDdsPreview(path, size, token) {
    const decodeDDS = await ensureDDS();
    if (token !== _previewToken || state.selectedPath !== path) return;
    if (!decodeDDS) { renderHexPreview(path, size, token); return; }
    let buf;
    try {
      const res = await fetch(
        `/v1/updates/${state.branch}/file?path=${encodeURIComponent(path)}`,
        { credentials: 'omit' },
      );
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      buf = await res.arrayBuffer();
    } catch (err) {
      if (token !== _previewToken || state.selectedPath !== path) return;
      renderHexPreview(path, size, token);
      return;
    }
    if (token !== _previewToken || state.selectedPath !== path) return;
    let img;
    try {
      img = decodeDDS(buf);
    } catch (err) {
      renderHexPreview(path, size, token);   // unsupported DDS format → hex fallback
      return;
    }
    const canvas = $previewCanvas;
    canvas.width = img.width;
    canvas.height = img.height;
    canvas.getContext('2d').putImageData(new ImageData(img.rgba, img.width, img.height), 0, 0);
    $previewDdsCap.textContent = `${img.width}×${img.height} · ${formatBytes(size)}`;
    $previewPng.onclick = () => downloadCanvasPng(canvas, path);
    $previewDds.hidden = false;
  }

  function downloadCanvasPng(canvas, path) {
    const base = path.slice(path.lastIndexOf('/') + 1).replace(/\.dds$/i, '') || 'texture';
    canvas.toBlob((blob) => {
      if (!blob) return;
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = `${base}.png`;
      document.body.appendChild(a);
      a.click();
      a.remove();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    }, 'image/png');
  }

  function renderDetail() {
    // The in-app Back button appears once there's in-app history to step back
    // through (e.g. after jumping here from the changes list); it just drives
    // the browser's own history, so it stays in sync with the Back button.
    if ($detailBack) $detailBack.hidden = _navDepth <= 0;

    // In grid mode the detail pane only appears once a file is picked (the
    // gallery itself is the browser); this class gates that in CSS.
    if ($paneExplorer) $paneExplorer.classList.toggle('up-file-open', !!state.selectedPath);

    if (!state.selectedPath) {
      $detailEmpty.hidden = false;
      $detailFile.hidden = true;
      return;
    }
    $detailEmpty.hidden = true;
    $detailFile.hidden = false;
    $detailTitle.textContent = state.selectedPath;

    // Latest entry shapes the meta header.
    const items = (state.fileHistory && state.fileHistory.items) || [];
    if (!items.length) {
      $detailMeta.textContent = '';
      $detailDownload.hidden = true;
      $history.innerHTML = `<p class="up-loading" data-i18n>Loading…</p>`;
      rerunI18n();
      return;
    }
    const latest = items[0];
    const latestTouched = latest.type === 'removed'
      ? t('removed in latest capture')
      : `${formatBytes(latest.size)} · ${shortSha(latest.content_sha256)}`;
    $detailMeta.textContent = `${items.length} ${t('captures')} · ${latestTouched}`;

    // Download link only when the file is currently present.
    if (latest.type !== 'removed') {
      $detailDownload.hidden = false;
      $detailDownload.href =
        `/v1/updates/${state.branch}/file?path=${encodeURIComponent(state.selectedPath)}`;
    } else {
      $detailDownload.hidden = true;
    }

    // Render rows. Shift-click → set as comparison pick A; ordinary
    // click → set as B. A "Compare A↔B" row at the top fires the diff.
    $history.innerHTML = `
      <div class="up-history-row" id="up-history-cta" style="cursor: default; background: rgba(76,201,240,.08); border-color: rgba(76,201,240,.30);">
        <span class="up-history-type" style="background: transparent; color: var(--accent-blue, #4cc9f0);">A↔B</span>
        <span class="up-history-tag" id="up-history-cta-label"></span>
        <button type="button" class="up-detail-download" id="up-history-go" style="padding: 6px 12px; font-size: .76rem;">
          <i class="fa-solid fa-code-compare" aria-hidden="true"></i> ${esc(t('Compare'))}
        </button>
        <span></span>
      </div>
      ${items.map((it) => `
        <div class="up-history-row${state.historyPicks.a === it.ordinal ? ' is-pick-a' : ''}${state.historyPicks.b === it.ordinal ? ' is-pick-b' : ''}"
             data-ordinal="${it.ordinal}">
          <span class="up-history-type up-history-type-${esc(it.type)}">${esc(t(it.type))}</span>
          <span class="up-history-tag" title="${esc(it.version_tag)}">${esc(it.version_tag)}</span>
          <span class="up-history-size">${it.type === 'removed' ? '-' : esc(formatBytes(it.size))}</span>
          <span class="up-history-when">${esc(formatWhen(new Date(it.captured_at)))}</span>
        </div>
      `).join('')}
    `;
    rerunI18n();
    renderHistoryCTA();

    for (const row of $history.querySelectorAll('[data-ordinal]')) {
      row.addEventListener('click', (e) => {
        const ord = Number(row.dataset.ordinal);
        if (e.shiftKey) state.historyPicks.a = ord;
        else state.historyPicks.b = ord;
        // If only one is set, mirror so the user always sees a pair.
        if (state.historyPicks.a == null) state.historyPicks.a = ord;
        if (state.historyPicks.b == null) state.historyPicks.b = ord;
        renderDetail();
      });
    }
    const go = document.getElementById('up-history-go');
    if (go) go.addEventListener('click', () => jumpToCompare());
  }

  function renderHistoryCTA() {
    const label = document.getElementById('up-history-cta-label');
    if (!label) return;
    const a = state.historyPicks.a, b = state.historyPicks.b;
    if (a && b && a !== b) {
      const aV = (state.fileHistory.items || []).find((x) => x.ordinal === a);
      const bV = (state.fileHistory.items || []).find((x) => x.ordinal === b);
      label.textContent = `${aV ? aV.version_tag : a} → ${bV ? bV.version_tag : b}`;
    } else {
      label.textContent = t('Click a row to set B; shift-click to set A');
    }
  }

  function jumpToCompare() {
    const a = state.historyPicks.a, b = state.historyPicks.b;
    if (!a || !b || a === b) return;
    state.compare.from = a;
    state.compare.to = b;
    state.compare.path = state.selectedPath;
    $comparePath.value = state.selectedPath;
    $compareFrom.value = String(a);
    $compareTo.value = String(b);
    switchTab('compare');
    runCompare();
  }

  // ─── Changes tab ───────────────────────────────────────────────────
  function renderChanges() {
    const { entries, total, ordinal, version_tag, counts, filter } = state.changes;
    $countAdd.textContent = formatInt(counts.added);
    $countMod.textContent = formatInt(counts.modified);
    $countRem.textContent = formatInt(counts.removed);

    if (ordinal == null) {
      $changesMeta.setAttribute('data-i18n', '');
      $changesMeta.textContent = 'Pick a version above to see its change-list.';
      $changesBody.innerHTML = `<p class="up-hint" data-i18n>Select a version chip above to populate this list.</p>`;
      $changesFoot.hidden = true;
      rerunI18n();
      return;
    }
    $changesMeta.removeAttribute('data-i18n');
    const totalRender = filter === 'all'
      ? total
      : counts[filter] || 0;
    $changesMeta.textContent = t('Version {tag} · {n} change(s)')
      .replace('{tag}', version_tag || '-').replace('{n}', formatInt(totalRender));

    const filtered = filter === 'all'
      ? entries
      : entries.filter((e) => e.type === filter);

    if (!filtered.length) {
      $changesBody.innerHTML = `<p class="up-hint" data-i18n>No changes match this filter.</p>`;
      $changesFoot.hidden = true;
      rerunI18n();
      return;
    }

    // Render the change-list as a collapsible folder tree instead of a flat
    // list, so a version touching hundreds of files is navigable. Clicks are
    // handled by one delegated listener wired in wireEvents().
    const root = buildChangeTree(filtered);
    $changesBody.innerHTML = `<div class="up-ctree">${renderChangeNodes(root, 0)}</div>`;
    rerunI18n();

    // Pagination: backend gave us up to 2000 in one shot, and we
    // filter client-side. If total > what we have, show load-more
    // that fetches the next page.
    $changesFoot.hidden = entries.length >= total;
  }

  // Build a directory tree from flat change entries. Each dir node carries the
  // rolled-up added/modified/removed counts of everything beneath it.
  function buildChangeTree(entries) {
    const mk = (name, path) => ({
      name, path, dirs: new Map(), files: [],
      counts: { added: 0, modified: 0, removed: 0 },
    });
    const root = mk('', '');
    for (const e of entries) {
      const parts = e.path.split('/');
      let node = root;
      if (node.counts[e.type] != null) node.counts[e.type]++;
      for (let i = 0; i < parts.length - 1; i++) {
        let child = node.dirs.get(parts[i]);
        if (!child) { child = mk(parts[i], node.path + parts[i] + '/'); node.dirs.set(parts[i], child); }
        node = child;
        if (node.counts[e.type] != null) node.counts[e.type]++;
      }
      node.files.push({ ...e, name: parts[parts.length - 1] });
    }
    return root;
  }

  function changeCountPills(c) {
    const parts = [];
    if (c.added)    parts.push(`<span class="up-cpill up-cpill-add">+${formatInt(c.added)}</span>`);
    if (c.modified) parts.push(`<span class="up-cpill up-cpill-mod">~${formatInt(c.modified)}</span>`);
    if (c.removed)  parts.push(`<span class="up-cpill up-cpill-rem">−${formatInt(c.removed)}</span>`);
    return `<span class="up-ctree-counts">${parts.join('')}</span>`;
  }

  // The collapsed-folder set for the version currently shown in the Changes tab.
  // Kept per-ordinal so each version remembers its own layout across re-renders,
  // version switches, and Back/Forward.
  function changesCollapsedSet() {
    const ord = state.changes.ordinal;
    let s = state.changesCollapsed.get(ord);
    if (!s) { s = new Set(); state.changesCollapsed.set(ord, s); }
    return s;
  }

  // Recursive HTML for a tree level. Single-child directory chains are collapsed
  // into one "a/b/c" row (git-style) so deep Trove paths stay readable.
  function renderChangeNodes(node, depth) {
    const collapsed = changesCollapsedSet();
    let html = '';
    for (const dn of [...node.dirs.keys()].sort()) {
      let d = node.dirs.get(dn);
      let name = d.name;
      while (d.dirs.size === 1 && d.files.length === 0) {
        const only = d.dirs.values().next().value;
        name += '/' + only.name;
        d = only;
      }
      const isOpen = !collapsed.has(d.path);
      html += `
        <div class="up-ctree-dir" data-cdir="${esc(d.path)}" style="--depth:${depth}">
          <i class="fa-solid fa-chevron-${isOpen ? 'down' : 'right'} up-ctree-caret" aria-hidden="true"></i>
          <i class="fa-solid fa-folder up-ctree-icon" aria-hidden="true"></i>
          <span class="up-ctree-name">${esc(name)}</span>
          ${changeCountPills(d.counts)}
        </div>`;
      if (isOpen) {
        html += `<div class="up-ctree-children">${renderChangeNodes(d, depth + 1)}</div>`;
      }
    }
    for (const e of node.files.slice().sort((a, b) => (a.name < b.name ? -1 : 1))) {
      html += `
        <div class="up-ctree-file" data-path="${esc(e.path)}" data-type="${esc(e.type)}"
             style="--depth:${depth}" title="${esc(e.path)}">
          <span class="up-change-type up-change-type-${esc(e.type)}">${esc(t(e.type))}</span>
          <i class="fa-solid ${iconForName(e.name)} up-ctree-ficon" aria-hidden="true"></i>
          <span class="up-ctree-fname">${esc(e.name)}</span>
          <span class="up-change-size">${e.type === 'removed' ? '-' : esc(formatBytes(e.size))}</span>
        </div>`;
    }
    return html;
  }

  async function loadMoreChanges() {
    const { ordinal, offset, total } = state.changes;
    if (state.changes.loading || ordinal == null || offset >= total) return;
    state.changes.loading = true;
    $changesMore.disabled = true;
    try {
      const data = await fetchJSON(
        `/site/updates/${state.branch}/changes?ordinal=${ordinal}`
        + `&limit=${PAGE_SIZE_CHANGES * 5}&offset=${offset}`,
      );
      state.changes.entries = state.changes.entries.concat(data.entries || []);
      state.changes.offset = state.changes.entries.length;
      renderChanges();
    } finally {
      state.changes.loading = false;
      $changesMore.disabled = false;
    }
  }

  // ─── Compare tab ───────────────────────────────────────────────────
  function renderCompareSelectOptions() {
    const opts = state.versions.map((v) => `
      <option value="${v.ordinal}">#${v.ordinal} · ${esc(v.version_tag)}</option>
    `).join('');
    if ($compareFrom) $compareFrom.innerHTML = opts;
    if ($compareTo)   $compareTo.innerHTML = opts;
    // Default: from = oldest of the shown strip, to = latest.
    if (state.versions.length >= 2) {
      $compareFrom.value = String(state.versions[state.versions.length - 1].ordinal);
      $compareTo.value   = String(state.versions[0].ordinal);
    }
  }

  async function runCompare() {
    const path = ($comparePath.value || '').trim();
    const from = Number($compareFrom.value);
    const to = Number($compareTo.value);
    if (!path) {
      $compareMeta.textContent = t('Enter a path first.');
      return;
    }
    if (!from || !to) {
      $compareMeta.textContent = t('Pick two versions.');
      return;
    }
    state.compare = { from, to, path, payload: null, loading: true };
    $compareMeta.textContent = t('Comparing…');
    $compareBody.innerHTML = `<p class="up-loading" data-i18n>Loading…</p>`;
    rerunI18n();
    scheduleHash(true);
    try {
      const payload = await fetchJSON(
        `/site/updates/${state.branch}/file/compare?path=${encodeURIComponent(path)}`
        + `&from=${from}&to=${to}`,
      );
      state.compare.payload = payload;
      state.compare.loading = false;
      renderCompare();
    } catch (err) {
      state.compare.loading = false;
      $compareBody.innerHTML = errorHTML(err);
    }
  }

  function renderCompare() {
    const p = state.compare.payload;
    if (!p) return;
    const summary = `
      <div class="up-diff-summary">
        <div class="up-diff-side">
          <span class="up-diff-side-label">${esc(t('From'))} · #${p.from.ordinal}</span>
          <span class="up-diff-side-tag">${esc(p.from.version_tag)}</span>
          <span class="up-diff-side-sub">${p.from.content_sha256 ? esc(shortSha(p.from.content_sha256)) + ' · ' + esc(formatBytes(p.from.size)) : esc(t('absent'))}</span>
          <span class="up-diff-side-sub">${p.from.captured_at ? esc(formatWhen(new Date(p.from.captured_at))) : ''}</span>
        </div>
        <div class="up-diff-side">
          <span class="up-diff-side-label">${esc(t('To'))} · #${p.to.ordinal}</span>
          <span class="up-diff-side-tag">${esc(p.to.version_tag)}</span>
          <span class="up-diff-side-sub">${p.to.content_sha256 ? esc(shortSha(p.to.content_sha256)) + ' · ' + esc(formatBytes(p.to.size)) : esc(t('absent'))}</span>
          <span class="up-diff-side-sub">${p.to.captured_at ? esc(formatWhen(new Date(p.to.captured_at))) : ''}</span>
        </div>
      </div>`;
    $compareMeta.textContent = `${p.path}`;

    if (p.identical) {
      $compareBody.innerHTML = `${summary}
        <div class="up-diff-banner up-diff-banner-ok">
          <i class="fa-solid fa-check" aria-hidden="true"></i>
          ${esc(t('No changes between these two versions.'))}
        </div>`;
      return;
    }

    if (!p.is_text) {
      $compareBody.innerHTML = `${summary}
        <div class="up-diff-banner">
          <i class="fa-solid fa-triangle-exclamation" aria-hidden="true"></i>
          ${esc(t('Binary file - inline diff skipped.'))} ${p.reason ? `(${esc(p.reason)})` : ''}
        </div>`;
      return;
    }

    if (!p.hunks || !p.hunks.length) {
      $compareBody.innerHTML = `${summary}
        <div class="up-diff-banner">
          ${esc(t('Files differ but no hunks were produced (empty result).'))}
        </div>`;
      return;
    }

    const hunks = p.hunks.map((h) => {
      const lines = h.lines.map((ln) => {
        const cls = ln.kind === 'add' ? 'up-line-add'
                  : ln.kind === 'remove' ? 'up-line-remove'
                  : 'up-line-equal';
        const leftN = ln.left  != null ? String(ln.left)  : '';
        const rightN = ln.right != null ? String(ln.right) : '';
        return `<div class="up-line ${cls}">
          <span class="up-line-num">${esc(leftN)}</span>
          <span class="up-line-num">${esc(rightN)}</span>
          <span class="up-line-text">${esc(ln.text)}</span>
        </div>`;
      }).join('');
      return `
        <section class="up-hunk">
          <div class="up-hunk-head">@@ -${h.left_start} +${h.right_start} @@</div>
          <div class="up-hunk-lines">${lines}</div>
        </section>`;
    }).join('');

    $compareBody.innerHTML = `${summary}${hunks}`;
  }

  // ─── Event wiring ──────────────────────────────────────────────────
  function wireEvents() {
    // Explorer/Changes/Compare tablist: click, plus arrow/Home/End roving per
    // the WAI-ARIA tabs pattern (panels carry role=tabpanel in the template).
    const tabBtns = [$tabExplorer, $tabChanges, $tabCompare];
    for (const btn of tabBtns) {
      btn.addEventListener('click', () => switchTab(btn.dataset.tab));
      btn.addEventListener('keydown', (e) => {
        const i = tabBtns.indexOf(btn);
        let j = -1;
        if (e.key === 'ArrowRight' || e.key === 'ArrowDown') j = (i + 1) % tabBtns.length;
        else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') j = (i - 1 + tabBtns.length) % tabBtns.length;
        else if (e.key === 'Home') j = 0;
        else if (e.key === 'End') j = tabBtns.length - 1;
        if (j < 0) return;
        e.preventDefault();
        switchTab(tabBtns[j].dataset.tab);
        tabBtns[j].focus();
      });
    }

    $treeSearch.addEventListener('input', () => {
      state.treeFilter = $treeSearch.value || '';
      scheduleSearch();
    });

    // View toggle (list ↔ grid gallery).
    if ($viewToggle) {
      $viewToggle.addEventListener('click', (e) => {
        const btn = e.target.closest('[data-view]');
        if (btn) setViewMode(btn.dataset.view);
      });
    }

    // Sort order (name ↔ last modified).
    if ($sortSelect) {
      $sortSelect.addEventListener('change', () => setTreeSort($sortSelect.value));
    }

    // Gallery image load failures → swap the <img> for its file-type icon.
    // Capture phase because `error` on <img> doesn't bubble. Guarded by the
    // data-fallback marker so it only touches gallery thumbnails.
    $tree.addEventListener('error', (e) => {
      const img = e.target;
      if (!img || img.tagName !== 'IMG' || !img.dataset.fallback) return;
      const span = document.createElement('span');
      span.className = 'up-tile-icon';
      span.innerHTML = `<i class="fa-solid ${img.dataset.fallback}" aria-hidden="true"></i>`;
      img.replaceWith(span);
    }, true);

    // One delegated handler for the whole sidebar list (directory rows, search
    // rows, and the "load more" footer). Delegation keeps the listener count
    // constant no matter how many rows a huge folder eventually reveals.
    $tree.addEventListener('click', (e) => {
      const more = e.target.closest('[data-tree-more]');
      if (more) { state.treeVisible += TREE_PAGE; renderTree(); return; }
      const row = e.target.closest('[data-path]');
      if (!row || !$tree.contains(row)) return;
      if (row.dataset.isDir) navigateTree(row.dataset.path);
      else openFile(row.dataset.path);
    });

    // Change-type filter chips.
    for (const chip of document.querySelectorAll('.up-changes-filter [data-filter]')) {
      chip.addEventListener('click', () => {
        for (const c of document.querySelectorAll('.up-changes-filter [data-filter]')) {
          c.classList.toggle('active', c === chip);
        }
        state.changes.filter = chip.dataset.filter;
        renderChanges();
      });
    }
    if ($changesMore) $changesMore.addEventListener('click', () => loadMoreChanges());

    // Changes tree: one delegated handler - toggle folders, or jump a file into
    // the explorer. Delegation survives the tree re-rendering on every toggle.
    $changesBody.addEventListener('click', (e) => {
      const dir = e.target.closest('[data-cdir]');
      if (dir && $changesBody.contains(dir)) {
        const set = changesCollapsedSet();
        const p = dir.dataset.cdir;
        if (set.has(p)) set.delete(p); else set.add(p);
        renderChanges();
        return;
      }
      const file = e.target.closest('[data-path]');
      if (file && $changesBody.contains(file)) {
        openChangePath(file.dataset.path);
      }
    });

    // Compare tab triggers.
    $compareRun.addEventListener('click', () => runCompare());
    $comparePath.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') { e.preventDefault(); runCompare(); }
    });

    if ($mobileTrigger) {
      $mobileTrigger.addEventListener('click', () => {
        const open = $sidebar.classList.toggle('open');
        $mobileTrigger.setAttribute('aria-expanded', String(open));
      });
    }

    // In-app Back: defer to the browser's history so it behaves identically to
    // the Back button (returns to the changes list, the parent folder, etc.).
    if ($detailBack) $detailBack.addEventListener('click', () => history.back());

    // Browser Back/Forward (and manual hash edits) → re-apply the hash to state.
    window.addEventListener('hashchange', () => { reconcileFromHash(); });

    document.addEventListener('btt-lang-changed', () => {
      renderBranchTabs();
      renderBranchMeta();
      renderVersions();
      renderTree();
      renderDetail();
      renderChanges();
      if (state.compare.payload) renderCompare();
    });
  }

  // ─── URL hash helpers ──────────────────────────────────────────────
  function parseHash() {
    const out = { branch: null, version: null, path: null, tab: null };
    const raw = location.hash.replace(/^#/, '');
    if (!raw) return out;
    const params = new URLSearchParams(raw);
    if (params.has('branch')) out.branch = params.get('branch');
    if (params.has('version')) {
      const n = Number(params.get('version'));
      if (Number.isFinite(n)) out.version = n;
    }
    if (params.has('path')) out.path = params.get('path');
    if (params.has('tab')) out.tab = params.get('tab');
    return out;
  }
  // The whole view lives in the hash (branch/version/tab/path) so it's
  // bookmarkable AND traversable with the browser Back/Forward buttons. Each
  // user navigation PUSHES one history entry; incidental syncs REPLACE. Writes
  // are coalesced through a 0ms timer so a compound action (e.g. a changes-row
  // click that switches tab AND opens a file) still records a single entry.
  let _suppressHash = false;   // true while applying state FROM the hash (reconcile/init)
  let _hashTimer = null;
  let _hashPush = false;
  let _navDepth = 0;           // our position in the pushed-history stack (for the Back button)

  function hashString() {
    const parts = [];
    if (state.branch) parts.push(`branch=${encodeURIComponent(state.branch)}`);
    if (state.selectedVersion) parts.push(`version=${state.selectedVersion}`);
    if (state.activeTab && state.activeTab !== 'explorer') {
      parts.push(`tab=${state.activeTab}`);
    }
    if (state.selectedPath) {
      parts.push(`path=${encodeURIComponent(state.selectedPath)}`);
    } else if (state.treePrefix) {
      parts.push(`path=${encodeURIComponent(state.treePrefix)}`);
    }
    return parts.length ? '#' + parts.join('&') : location.pathname;
  }

  function writeHash(push) {
    const next = hashString();
    const nextFrag = next.startsWith('#') ? next : '';
    if (nextFrag === location.hash) return;   // unchanged - no dup entry
    if (push) {
      _navDepth += 1;
      history.pushState({ d: _navDepth }, '', next);
      renderDetail();   // reveal the in-app Back button now that history has depth
    } else {
      history.replaceState({ d: _navDepth }, '', next);
    }
  }

  // Request a hash write. Coalesces a synchronous burst of calls into one entry.
  function scheduleHash(push) {
    if (_suppressHash) return;
    if (push) _hashPush = true;
    if (_hashTimer) return;
    _hashTimer = setTimeout(() => {
      _hashTimer = null;
      const p = _hashPush; _hashPush = false;
      writeHash(p);
    }, 0);
  }

  // Apply the hash to state without writing history back (used by Back/Forward
  // and the initial load). Fully syncs every axis so going back also *clears*
  // things the target entry doesn't have (e.g. an open file).
  async function applyHash(h) {
    if (h.branch && h.branch !== state.branch) await selectBranch(h.branch);
    if (h.version != null && h.version !== state.selectedVersion) {
      const v = state.versions.find((x) => x.ordinal === h.version);
      if (v) await selectVersion(v.ordinal);
    }
    const targetPath = h.path || '';
    const curPath = state.selectedPath || state.treePrefix || '';
    if (targetPath !== curPath) {
      if (h.path) await openTreePath(h.path);
      else await navigateTree('');   // back to root, clears the open file
    }
    const targetTab = h.tab || 'explorer';
    if (targetTab !== state.activeTab) switchTab(targetTab);
  }

  async function reconcileFromHash() {
    _suppressHash = true;
    try {
      _navDepth = (history.state && history.state.d) || 0;
      await applyHash(parseHash());
    } finally {
      _suppressHash = false;
    }
    renderDetail();   // refresh the Back button for the new depth
  }

  // ─── View mode (list ↔ grid gallery) ───────────────────────────────
  // The explorer sidebar renders either as a compact list of rows or as a
  // thumbnail gallery. The choice is remembered across visits in localStorage.
  const VIEW_KEY = 'bttUpdatesView';

  function readViewMode() {
    try {
      return localStorage.getItem(VIEW_KEY) === 'grid' ? 'grid' : 'list';
    } catch (_) { return 'list'; }
  }
  function saveViewMode(mode) {
    try { localStorage.setItem(VIEW_KEY, mode); } catch (_) {}
  }

  // Reflect state.viewMode onto the DOM: the pane class drives the CSS layout
  // switch, and the toggle buttons show which mode is active.
  function applyViewMode() {
    const grid = state.viewMode === 'grid';
    if ($paneExplorer) $paneExplorer.classList.toggle('up-view-grid', grid);
    if ($viewToggle) {
      for (const btn of $viewToggle.querySelectorAll('[data-view]')) {
        const on = btn.dataset.view === state.viewMode;
        btn.classList.toggle('active', on);
        btn.setAttribute('aria-pressed', String(on));
      }
    }
  }

  function setViewMode(mode) {
    mode = mode === 'grid' ? 'grid' : 'list';
    if (state.viewMode === mode) return;
    state.viewMode = mode;
    saveViewMode(mode);
    applyViewMode();
    renderTree();
    renderDetail();
  }

  // ─── Sort order (name ↔ last modified) ─────────────────────────────
  const SORT_KEY = 'bttUpdatesSort';

  function readTreeSort() {
    try {
      return localStorage.getItem(SORT_KEY) === 'modified' ? 'modified' : 'name';
    } catch (_) { return 'name'; }
  }
  function saveTreeSort(mode) {
    try { localStorage.setItem(SORT_KEY, mode); } catch (_) {}
  }
  function setTreeSort(mode) {
    mode = mode === 'modified' ? 'modified' : 'name';
    if (state.treeSort === mode) return;
    state.treeSort = mode;
    saveTreeSort(mode);
    if ($sortSelect) $sortSelect.value = mode;
    renderTree();
  }

  // Return a copy of `entries` in the active order. 'name' is the server order
  // (directories first, alphabetical). 'modified' sorts by the newest version that
  // touched each entry (its rolled-up last_ordinal), newest first, dirs winning ties.
  function sortEntries(entries) {
    if (state.treeSort !== 'modified') return entries;
    return entries.slice().sort((a, b) => {
      const d = (b.last_ordinal || 0) - (a.last_ordinal || 0);
      if (d) return d;
      if (!!a.is_dir !== !!b.is_dir) return a.is_dir ? -1 : 1;
      return a.name < b.name ? -1 : a.name > b.name ? 1 : 0;
    });
  }

  // Compact "modified N ago" label for a listing entry, or '' when unknown.
  function entryModifiedLabel(e) {
    if (!e.last_modified_at) return '';
    const d = new Date(e.last_modified_at);
    if (isNaN(d.getTime())) return '';
    return formatRelativeWhen(d);
  }

  // The meta text shown at the trailing edge of a row/tile: the modified time when
  // sorting by it (falling back to size/count if unknown), else size/count.
  function entryMeta(e) {
    if (state.treeSort === 'modified') {
      const when = entryModifiedLabel(e);
      if (when) return when;
    }
    return e.is_dir ? `${formatInt(e.file_count)} ${t('files')}` : formatBytes(e.size);
  }

  // Classify a file by extension for the gallery: 'img' renders straight from the
  // raw-file endpoint; 'dds' is decoded to a canvas client-side; null → a tile.
  function imageKindForName(name) {
    const ext = name.slice(name.lastIndexOf('.') + 1).toLowerCase();
    if (ext === 'png' || ext === 'jpg' || ext === 'jpeg' || ext === 'webp' || ext === 'gif') return 'img';
    if (ext === 'dds') return 'dds';
    return null;
  }

  // One gallery tile. Folders navigate; image files show a thumbnail; everything
  // else shows a file-type icon. Carries data-path/data-is-dir so the existing
  // delegated $tree click handler drives it with no extra wiring.
  function tileHTML(e, touched, opts) {
    opts = opts || {};
    const isDir = !!e.is_dir;
    const imgKind = isDir ? null : imageKindForName(e.name);

    let thumb;
    if (isDir) {
      thumb = `<span class="up-tile-icon"><i class="fa-solid fa-folder" aria-hidden="true"></i></span>`;
    } else if (imgKind === 'img') {
      const src = `/v1/updates/${encodeURIComponent(state.branch)}/file?path=${encodeURIComponent(e.path)}`;
      // data-fallback lets the capture-phase error handler swap in an icon if the
      // image fails to load (inline onerror is disallowed by the page CSP).
      thumb = `<img class="up-tile-img" loading="lazy" decoding="async" alt=""
                    src="${src}" data-fallback="${esc(iconForName(e.name))}">`;
    } else if (imgKind === 'dds') {
      thumb = `<canvas class="up-tile-img up-tile-dds" data-dds-path="${esc(e.path)}"
                       width="0" height="0"></canvas>`;
    } else {
      thumb = `<span class="up-tile-icon"><i class="fa-solid ${iconForName(e.name)}" aria-hidden="true"></i></span>`;
    }

    // Touched badge (changed in the selected version): a dot on the tile corner.
    let touchHTML = '';
    if (touched && !isDir) {
      const kind = touched.get(e.path);
      if (kind) {
        const cls = kind === 'modified' ? 'up-touch-mod'
                  : kind === 'removed'  ? 'up-touch-rem' : '';
        touchHTML = `<span class="up-tile-touch ${cls}" title="${esc(t(kind))}"></span>`;
      }
    } else if (touched && isDir) {
      for (const p of touched.keys()) {
        if (p.startsWith(e.path)) { touchHTML = `<span class="up-tile-touch" title="${esc(t('contains changes'))}"></span>`; break; }
      }
    }

    // Meta line: dir path for search hits (or the modified time when that's the
    // active sort), else size / child count (or modified time).
    let meta;
    if (opts.search) {
      const dir = e.path.slice(0, e.path.length - e.name.length);
      meta = (state.treeSort === 'modified' && entryModifiedLabel(e)) || dir || entryMeta(e);
    } else {
      meta = entryMeta(e);
    }

    return `
      <button type="button" class="up-tile${state.selectedPath === e.path ? ' active' : ''}"
              data-path="${esc(e.path)}" data-is-dir="${isDir ? '1' : ''}"
              title="${esc(e.path)}">
        <span class="up-tile-thumb">${thumb}${touchHTML}</span>
        <span class="up-tile-body">
          <span class="up-tile-name">${esc(e.name)}</span>
          <span class="up-tile-meta">${esc(meta)}</span>
        </span>
      </button>`;
  }

  // Decode DDS thumbnails lazily: only tiles scrolled near the viewport are
  // fetched + decoded, so a folder of hundreds of textures doesn't stall.
  let _ddsObserver = null;
  function observeDdsTiles() {
    if (typeof IntersectionObserver === 'undefined') return;
    const canvases = $tree.querySelectorAll('canvas.up-tile-dds[data-dds-path]');
    if (!canvases.length) return;
    if (!_ddsObserver) {
      _ddsObserver = new IntersectionObserver((entries, obs) => {
        for (const en of entries) {
          if (en.isIntersecting) { obs.unobserve(en.target); decodeDdsTile(en.target); }
        }
      }, { rootMargin: '250px' });
    }
    for (const c of canvases) _ddsObserver.observe(c);
  }

  async function decodeDdsTile(canvas) {
    const path = canvas.dataset.ddsPath;
    const decodeDDS = await ensureDDS();
    if (!decodeDDS) { ddsTileFallback(canvas); return; }
    let buf;
    try {
      const res = await fetch(
        `/v1/updates/${state.branch}/file?path=${encodeURIComponent(path)}`,
        { credentials: 'omit' },
      );
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      buf = await res.arrayBuffer();
    } catch (_) { ddsTileFallback(canvas); return; }
    if (!canvas.isConnected) return;   // tile was re-rendered away
    let img;
    try { img = decodeDDS(buf); } catch (_) { ddsTileFallback(canvas); return; }
    canvas.width = img.width;
    canvas.height = img.height;
    canvas.getContext('2d').putImageData(new ImageData(img.rgba, img.width, img.height), 0, 0);
    canvas.classList.add('is-loaded');
  }

  function ddsTileFallback(canvas) {
    if (!canvas.isConnected) return;
    const span = document.createElement('span');
    span.className = 'up-tile-icon';
    span.innerHTML = '<i class="fa-solid fa-file-image" aria-hidden="true"></i>';
    canvas.replaceWith(span);
  }

  // ─── i18n ──────────────────────────────────────────────────────────
  function t(s) {
    return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s;
  }
  function rerunI18n() {
    if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh();
  }

  // ─── Fetch + util ──────────────────────────────────────────────────
  function errorHTML(err) {
    const msg = (err && err.message) || String(err);
    return `<p class="up-hint">${esc(t('Failed to load'))}: ${esc(msg)}</p>`;
  }

  function formatInt(n) {
    return Number(n || 0).toLocaleString();
  }

  function formatBytes(n) {
    n = Number(n || 0);
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(2)} MB`;
    return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
  }

  function shortSha(sha) {
    return sha ? sha.slice(0, 10) : '';
  }

  function formatWhen(d) {
    if (!d || isNaN(d.getTime())) return '';
    const pad = (n) => String(n).padStart(2, '0');
    return `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())} ${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())} UTC`;
  }

  function formatRelativeWhen(d) {
    const diff = (Date.now() - d.getTime()) / 1000;
    if (diff < 60) return t('just now');
    if (diff < 3600) return t('{n}m ago').replace('{n}', Math.floor(diff / 60));
    if (diff < 86400) return t('{n}h ago').replace('{n}', Math.floor(diff / 3600));
    return t('{n}d ago').replace('{n}', Math.floor(diff / 86400));
  }

  // Pick a FontAwesome icon class for a file based on its extension.
  // Trove-specific buckets (.binfab, .tfa) get a generic file-binary;
  // everything else uses the obvious type-icon when available.
  function iconForName(name) {
    const ext = name.slice(name.lastIndexOf('.') + 1).toLowerCase();
    switch (ext) {
      case 'lua': case 'js': case 'py':            return 'fa-file-code';
      case 'json': case 'yaml': case 'yml': case 'xml': case 'config': case 'cfg':
                                                   return 'fa-file-code';
      case 'txt': case 'md': case 'log':           return 'fa-file-lines';
      case 'png': case 'jpg': case 'jpeg': case 'webp': case 'gif': case 'dds':
                                                   return 'fa-file-image';
      case 'wav': case 'ogg': case 'mp3':          return 'fa-file-audio';
      case 'binfab': case 'blueprint': case 'tfa':
      case 'tfi': case 'umat': case 'tex':         return 'fa-cube';
      default:                                     return 'fa-file';
    }
  }
})();
