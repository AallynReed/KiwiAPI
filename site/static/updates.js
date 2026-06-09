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

  const PAGE_SIZE_CHANGES = 200;
  // How many recent version chips to show in the timeline strip.
  const VERSIONS_VISIBLE = 12;

  const state = {
    branches: [],            // [{branch, current_version, current_ordinal, ...}]
    branch: null,            // active branch name
    versions: [],            // recent VersionInfo for the active branch
    versionsTotal: 0,
    selectedVersion: null,   // ordinal - drives the "Changes" tab + tree badges
    activeTab: 'explorer',   // 'explorer' | 'changes' | 'compare'

    // Explorer tab
    treeCache: new Map(),    // prefix → entries (avoid re-fetching as you walk)
    treePrefix: '',          // current directory prefix
    treeFilter: '',          // sidebar search text
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

  const $treeSearch = $('up-tree-search');
  const $breadcrumbs = $('up-breadcrumbs');
  const $tree = $('up-tree');
  const $sidebar = $('up-sidebar');
  const $mobileTrigger = $('up-mobile-trigger');
  const $mobileSelected = $('up-mobile-selected');

  const $detailEmpty = $('up-detail-empty');
  const $detailFile = $('up-detail-file');
  const $detailTitle = $('up-detail-title');
  const $detailMeta = $('up-detail-meta');
  const $detailDownload = $('up-detail-download');
  const $history = $('up-history');

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

    // URL hash priority - pick branch + tab + path + version from the
    // hash if present, else default to the first branch / latest version.
    const hash = parseHash();
    const startBranch =
      (hash.branch && state.branches.find((b) => b.branch === hash.branch)?.branch)
      || state.branches[0].branch;
    await selectBranch(startBranch, /* skipURL = */ true);

    if (hash.version != null) {
      const v = state.versions.find((x) => x.ordinal === hash.version);
      if (v) await selectVersion(v.ordinal);
    }
    if (hash.path) {
      await openTreePath(hash.path);
    }
    if (hash.tab) switchTab(hash.tab);

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
      btn.addEventListener('click', () => selectBranch(btn.dataset.branch));
    }
  }

  function branchLabel(b) {
    // Canonical short labels; the server's id is the technical key.
    if (b === 'live-us') return 'Live US';
    if (b === 'pts')     return 'PTS';
    return b;
  }

  async function selectBranch(branch, skipURL) {
    if (state.branch === branch) return;
    state.branch = branch;
    // Reset every per-branch piece of state - they're not cross-branch.
    state.treeCache.clear();
    state.treePrefix = '';
    state.selectedPath = null;
    state.fileHistory = null;
    state.versions = [];
    state.versionTouched = null;
    state.changes = {
      entries: [], total: 0, ordinal: null, version_tag: null,
      counts: {added: 0, modified: 0, removed: 0}, filter: 'all',
      offset: 0, loading: false,
    };
    state.compare = { from: null, to: null, path: '', payload: null, loading: false };

    renderBranchTabs();
    if (!skipURL) updateHash();

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
                data-ordinal="${v.ordinal}" role="tab"
                aria-selected="${isActive}"
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
    updateHash();

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
    updateHash();
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
    state.selectedPath = null;  // leaving file-detail view
    state.fileHistory = null;
    renderTree();
    renderDetail();
    updateHash();
    await loadTree(prefix);
    renderTree();
  }

  async function renderTree() {
    renderBreadcrumbs();
    const entries = state.treeCache.get(state.treePrefix);
    if (!entries) {
      $tree.innerHTML = `<p class="up-loading" data-i18n>Loading…</p>`;
      rerunI18n();
      return;
    }
    let visible = entries;
    if (state.treeFilter) {
      const needle = state.treeFilter.toLowerCase();
      visible = entries.filter((e) => e.name.toLowerCase().includes(needle));
    }
    if (!visible.length) {
      $tree.innerHTML = `<p class="up-tree-empty" data-i18n>Nothing here.</p>`;
      rerunI18n();
      return;
    }

    const touched = state.versionTouched && state.versionTouched.byPath;
    const rows = visible.map((e) => {
      const icon = e.is_dir
        ? 'fa-folder'
        : iconForName(e.name);
      const sizeOrCount = e.is_dir
        ? `${formatInt(e.file_count)} ${t('files')}`
        : formatBytes(e.size);

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
    $tree.innerHTML = rows;

    for (const btn of $tree.querySelectorAll('[data-path]')) {
      btn.addEventListener('click', () => {
        const path = btn.dataset.path;
        if (btn.dataset.isDir) {
          navigateTree(path);
        } else {
          openFile(path);
        }
      });
    }
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
      await loadTree(parent);
    }
    await openFile(path);
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
    updateHash();
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
  }

  function renderDetail() {
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

    $changesBody.innerHTML = filtered.map((e) => `
      <div class="up-change-row" data-path="${esc(e.path)}" data-type="${esc(e.type)}">
        <span class="up-change-type up-change-type-${esc(e.type)}">${esc(t(e.type))}</span>
        <span class="up-change-path" title="${esc(e.path)}">${esc(e.path)}</span>
        <span class="up-change-size">${e.type === 'removed' ? '-' : esc(formatBytes(e.size))}</span>
      </div>
    `).join('');
    rerunI18n();

    for (const row of $changesBody.querySelectorAll('[data-path]')) {
      row.addEventListener('click', () => {
        switchTab('explorer');
        openTreePath(row.dataset.path);
      });
    }

    // Pagination: backend gave us up to 2000 in one shot, and we
    // filter client-side. If total > what we have, show load-more
    // that fetches the next page.
    $changesFoot.hidden = entries.length >= total;
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
    updateHash();
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
    $tabExplorer.addEventListener('click', () => switchTab('explorer'));
    $tabChanges.addEventListener('click', () => switchTab('changes'));
    $tabCompare.addEventListener('click', () => switchTab('compare'));

    $treeSearch.addEventListener('input', () => {
      state.treeFilter = $treeSearch.value || '';
      renderTree();
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

    window.addEventListener('hashchange', async () => {
      const h = parseHash();
      if (h.branch && h.branch !== state.branch) await selectBranch(h.branch);
      if (h.version != null && h.version !== state.selectedVersion) {
        const v = state.versions.find((x) => x.ordinal === h.version);
        if (v) await selectVersion(v.ordinal);
      }
      if (h.path && h.path !== state.selectedPath) await openTreePath(h.path);
      if (h.tab && h.tab !== state.activeTab) switchTab(h.tab);
    });

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
  function updateHash() {
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
    const next = parts.length ? '#' + parts.join('&') : location.pathname;
    history.replaceState(null, '', next);
  }

  // ─── i18n ──────────────────────────────────────────────────────────
  function t(s) {
    return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s;
  }
  function rerunI18n() {
    if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh();
  }

  // ─── Fetch + util ──────────────────────────────────────────────────
  async function fetchJSON(path) {
    const res = await fetch(path, { headers: { Accept: 'application/json' } });
    if (!res.ok) {
      let msg = `HTTP ${res.status}`;
      try {
        const body = await res.json();
        if (body && body.detail) msg = body.detail;
        else if (body && body.error && body.error.message) msg = body.error.message;
      } catch (_) { /* no body */ }
      throw new Error(msg);
    }
    return res.json();
  }

  function esc(s) {
    return String(s ?? '').replace(/[&<>"']/g, (c) =>
      ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  }

  function errorHTML(err) {
    const msg = (err && err.message) || String(err);
    return `<p class="up-hint">${esc(t('Failed to load'))}: ${esc(msg)}</p>`;
  }

  // Right-pad zero-style numbers, locale-aware grouping.
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
