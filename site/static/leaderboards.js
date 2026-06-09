/* ═══════════════════════════════════════════════════════════════════════
   /leaderboards — page logic
   ───────────────────────────────────────────────────────────────────────
   Fetches anchors, boards, and entries from /site/leaderboards/* (which
   bypass the public API surface and read the database directly). Renders
   a sidebar of boards grouped by category and an entries table for the
   selected one.

   URL hash mirrors selection: ?anchor=X&board=Y. Reload-safe and
   bookmarkable. Locale labels come through i18n.js (data-i18n attrs on
   chrome strings); board/player names are game data and stay verbatim.
   ═══════════════════════════════════════════════════════════════════════ */

(function () {
  'use strict';

  const PAGE_SIZE = 100;
  const DAY_SECONDS = 86400;
  const TROVE_OFFSET_SECONDS = 11 * 3600;  // trove-time = real UTC - 11h
  const PICKER_DAYS = 7;

  // Defined ABOVE `state` because state's initial value calls
  // readMinConfidence(), which closes over this key — declaring it
  // later puts it in the temporal dead zone at first call and the
  // try/catch silently returns the default.
  const CHEATERS_MIN_KEY = 'btt_lb_cheaters_min';
  function readMinConfidence() {
    try {
      const raw = localStorage.getItem(CHEATERS_MIN_KEY);
      if (raw == null) return 0.9;
      const v = Number(raw);
      return Number.isFinite(v) ? Math.min(1, Math.max(0, v)) : 0.9;
    } catch (_) { return 0.9; }
  }
  function writeMinConfidence(v) {
    try { localStorage.setItem(CHEATERS_MIN_KEY, String(v)); } catch (_) {}
  }

  // Same TDZ note as readMinConfidence: declared above ``state`` so
  // its initial-value call doesn't hit an unset binding. Stores the
  // set of category names the user has collapsed; everything missing
  // from the set stays expanded.
  const COLLAPSED_CATS_KEY = 'btt_lb_collapsed_cats';
  function readCollapsedCategories() {
    try {
      const raw = localStorage.getItem(COLLAPSED_CATS_KEY);
      if (!raw) return new Set();
      const arr = JSON.parse(raw);
      return new Set(Array.isArray(arr) ? arr.map(String) : []);
    } catch (_) { return new Set(); }
  }
  function writeCollapsedCategories(set) {
    try {
      localStorage.setItem(COLLAPSED_CATS_KEY, JSON.stringify([...set]));
    } catch (_) {}
  }

  const state = {
    anchors: [],            // list of unix-second anchors stored in DB, newest first
    days: [],               // 7 entries: {troveDate, dayStart, dayEnd, anchor|null}
    anchor: null,           // currently selected anchor (null when day has no data)
    selectedDayIdx: null,   // index into `days` of the active chip
    boards: [],
    boardFilter: '',
    selectedUuid: null,
    entries: [],
    entriesTotal: 0,
    loadingEntries: false,
    hotRetentionDays: 3,    // pulled from /site/leaderboards/config; subtitle reflects it
    cheaters: null,         // cached payload from /site/leaderboards/cheaters
    activity: null,         // cached payload from /site/leaderboards/activity
    cheatersMinConfidence: readMinConfidence(),  // slider value, persisted
    activeTab: 'boards',         // 'boards' | 'cheaters'
    cheatersLoaded: false,       // becomes true after the first lazy fetch
    cheatersLoading: false,      // guards against double-firing during fetch
    boardChart: null,            // {uuid, data} — cached so resize/lang re-renders don't refetch
    playerChart: null,           // {name, data} — same idea for the per-player chart
    // Categories the user has manually collapsed. Persisted to
    // localStorage so the preference survives reloads. A category
    // missing from the set is treated as expanded (the friendly
    // default — first-time visitors see all boards).
    collapsedCategories: readCollapsedCategories(),
  };

  // Chart palette — picked for readability on the dark theme. We cycle
  // through these in the order series come from the API (which is already
  // sorted by rank / best-rank, so line #1 is the most prominent player).
  const CHART_COLORS = [
    '#4cc9f0', '#f72585', '#43aa8b', '#f8961e', '#b5179e',
    '#4361ee', '#f9c74f', '#7209b7', '#d62828', '#90be6d',
  ];

  // ─── DOM refs ──────────────────────────────────────────────────────
  const $dayPicker = document.getElementById('lb-day-picker');
  const $cheatersMeta = document.getElementById('lb-cheaters-meta');
  const $cheatersBody = document.getElementById('lb-cheaters-body');
  const $cheatersFilter = document.getElementById('lb-cheaters-min');
  const $cheatersFilterValue = document.getElementById('lb-cheaters-min-value');
  const $cheatersFilterHint = document.getElementById('lb-cheaters-filter-hint');
  // Coverage <details> — populated whenever a cheaters payload renders;
  // tells the user exactly which boards were scanned vs skipped + why,
  // so they can verify the analysis covered what they care about.
  const $cheatersCoverage = document.getElementById('lb-cheaters-coverage');
  const $cheatersCoverageCount = document.getElementById('lb-cheaters-coverage-count');
  const $cheatersCoverageBody = document.getElementById('lb-cheaters-coverage-body');
  const $tabBoardsBtn = document.getElementById('lb-tab-boards');
  const $tabCheatersBtn = document.getElementById('lb-tab-cheaters');
  const $tabCheatersBadge = document.getElementById('lb-tab-cheaters-badge');
  const $paneBoards = document.getElementById('lb-pane-boards');
  const $paneCheaters = document.getElementById('lb-pane-cheaters');
  const $boardSearch = document.getElementById('lb-board-search');
  const $boardList = document.getElementById('lb-board-list');
  const $entriesTitle = document.getElementById('lb-entries-title');
  const $entriesMeta = document.getElementById('lb-entries-meta');
  const $entriesBody = document.getElementById('lb-entries-body');
  const $entriesFoot = document.getElementById('lb-entries-foot');
  const $loadMore = document.getElementById('lb-load-more');
  const $playerSearch = document.getElementById('lb-player-search');
  const $playerPanel = document.getElementById('lb-player-panel');
  const $playerName = document.getElementById('lb-player-name');
  const $playerBody = document.getElementById('lb-player-body');
  const $playerClose = document.getElementById('lb-player-close');
  const $mobileTrigger = document.getElementById('lb-mobile-trigger');
  const $mobileSelected = document.getElementById('lb-mobile-selected');
  const $sidebar = document.getElementById('lb-sidebar');

  // ─── Boot ──────────────────────────────────────────────────────────
  init().catch((err) => {
    console.error('[leaderboards] boot failed', err);
    $boardList.innerHTML = errorHTML(err);
  });

  async function init() {
    // Only the boards-tab data is fetched eagerly so the first paint
    // is fast. The cheaters analysis is lazy-loaded the first time
    // the user activates that tab (see ensureCheatersLoaded). Activity
    // estimate is tiny and goes inline in the hero, so it ships with
    // the eager batch.
    const [stamps, config, activity, activityHistory] = await Promise.all([
      fetchJSON('/site/leaderboards/timestamps'),
      fetchJSON('/site/leaderboards/config').catch(() => null),
      fetchJSON('/site/leaderboards/activity').catch(() => null),
      // History feeds the sparkline next to the current-hour pill.
      // ?days=7 covers a full week of hourly captures (~168 points);
      // missing the chart entirely is fine on a fresh deploy so a
      // failure here is silenced.
      fetchJSON('/site/leaderboards/activity/history?days=7').catch(() => null),
    ]);
    state.anchors = stamps.items || [];
    if (config && Number.isFinite(config.hot_retention_days)) {
      state.hotRetentionDays = config.hot_retention_days;
    }
    state.activity = activity;
    state.activityHistory = activityHistory;
    // Open/closed state is sticky across visits — honour whatever the
    // user chose last time. The trend section starts hidden in the HTML
    // so a false here just leaves it that way.
    state.activityTrendOpen = readActivityTrendOpen();
    renderSubtitle();
    renderActivity();
    if (state.activityTrendOpen) setActivityTrendOpen(true);

    buildDays();
    renderDayPicker();

    // Keep the "Today" chip's relative "Xm ago" label fresh if the page
    // sits open. Re-rendering the whole (tiny) picker every 60s is cheap
    // and reattaches its click handlers harmlessly. Guarded so we only
    // ever install one interval.
    if (!state._dayTicker) {
      state._dayTicker = setInterval(() => {
        if (state.days && state.days.length) renderDayPicker();
      }, 60_000);
    }

    if (!state.days.some((d) => d.anchor != null)) {
      $boardList.innerHTML = `<p class="lb-board-empty" data-i18n>No leaderboard data has been captured yet. Check back later.</p>`;
      rerunI18n();
      wireEvents();
      return;
    }

    // Read initial state from the URL hash (deep-link friendly).
    const hash = parseHash();
    let startIdx = -1;
    if (hash.anchor) {
      startIdx = state.days.findIndex((d) => d.anchor === hash.anchor);
    }
    if (startIdx < 0) {
      // Default: latest day that actually has data (usually [0] = today).
      startIdx = state.days.findIndex((d) => d.anchor != null);
    }

    await selectDay(startIdx);

    const initialBoard = hash.board != null && state.boards.find((b) => b.uuid === hash.board);
    if (initialBoard) selectBoard(initialBoard.uuid);

    wireEvents();

    // Activate the requested tab AFTER boards are wired so the boards
    // view is ready to render (or stay hidden) regardless of which tab
    // the user lands on. If hash says cheaters, this triggers the
    // first fetch.
    if (hash.tab === 'cheaters') {
      switchTab('cheaters');
    }
  }

  // ─── Tabs ──────────────────────────────────────────────────────────
  // Two tabs: 'boards' (default, all the board-browser machinery) and
  // 'cheaters' (lazy-fetched on first activation). URL hash carries
  // ``tab=cheaters`` for deep-link + back-button support.
  function switchTab(name) {
    if (name !== 'boards' && name !== 'cheaters') name = 'boards';
    if (state.activeTab === name) return;
    state.activeTab = name;

    const boardsActive = name === 'boards';
    if ($tabBoardsBtn) {
      $tabBoardsBtn.classList.toggle('active', boardsActive);
      $tabBoardsBtn.setAttribute('aria-selected', String(boardsActive));
    }
    if ($tabCheatersBtn) {
      $tabCheatersBtn.classList.toggle('active', !boardsActive);
      $tabCheatersBtn.setAttribute('aria-selected', String(!boardsActive));
    }
    if ($paneBoards) {
      $paneBoards.classList.toggle('active', boardsActive);
      $paneBoards.hidden = !boardsActive;
    }
    if ($paneCheaters) {
      $paneCheaters.classList.toggle('active', !boardsActive);
      $paneCheaters.hidden = boardsActive;
    }
    updateHash();

    if (name === 'cheaters') ensureCheatersLoaded();
  }

  async function ensureCheatersLoaded() {
    if (state.cheatersLoaded || state.cheatersLoading) {
      // Already loaded (or in flight) — just re-render to reflect any
      // language-change or slider-change since the last render.
      renderCheaters();
      return;
    }
    state.cheatersLoading = true;
    // Show the friendlier "Crunching…" placeholder immediately so the
    // tab isn't empty while the fetch resolves. textContent so a later
    // language switch can re-translate via renderCheaters(). The
    // server-side warmer almost always has this cached — when it
    // doesn't (cold boot / brand new anchor), the wait is several
    // seconds and the user deserves to know SOMETHING is happening.
    if ($cheatersMeta) {
      $cheatersMeta.textContent = t('Crunching the latest capture — first paint can take a moment while we warm the caches.');
    }
    try {
      const payload = await fetchJSON('/site/leaderboards/cheaters');
      state.cheaters = payload;
      state.cheatersLoaded = true;
      renderCheaters(payload);
    } catch (err) {
      state.cheaters = { _error: err };
      renderCheaters(state.cheaters);
    } finally {
      state.cheatersLoading = false;
    }
  }


  // ─── Possible cheaters panel ───────────────────────────────────────
  // Pulls from /site/leaderboards/cheaters, which proxies to the
  // statistical-outlier detection (see app/trove/leaderboards/detection.py).
  // Stored on `state.cheaters` so a language switch can re-render
  // without re-fetching.
  function renderCheaters(payload) {
    if (payload && !payload._error) state.cheaters = payload;
    const data = state.cheaters;

    // Always reflect the slider value, even before data lands.
    syncFilterUI();

    // If we've never fetched, leave the panel's seed "Loading…" /
    // "Checking…" placeholder alone — the user hasn't opened the tab
    // yet and there's nothing meaningful to show.
    if (data == null) return;

    if (data._error) {
      if ($tabCheatersBadge) $tabCheatersBadge.hidden = true;
      $cheatersMeta.textContent = t('Failed to load') + '.';
      $cheatersBody.innerHTML = '';
      return;
    }

    const players = data.players || [];
    const flaggedTotal = players.length;
    const boards = data.boards_analyzed || 0;
    const anchor = data.anchor;

    // Apply confidence filter (and remember which originals exist
    // so the "X of Y" hint can show the gap).
    const min = state.cheatersMinConfidence;
    const visible = players.filter((p) => (p.confidence ?? 0) >= min);

    // Tab-strip badge mirrors the visible count — what the user would
    // see if they switched to this tab right now.
    if ($tabCheatersBadge) {
      if (visible.length > 0) {
        $tabCheatersBadge.hidden = false;
        $tabCheatersBadge.textContent = String(visible.length);
      } else {
        $tabCheatersBadge.hidden = true;
      }
    }

    // Filter hint: "Hiding N below 0.90" when the slider is hiding some.
    const hiddenCount = flaggedTotal - visible.length;
    if (hiddenCount > 0) {
      $cheatersFilterHint.textContent = t('hiding {n} below threshold')
        .replace('{n}', hiddenCount);
    } else {
      $cheatersFilterHint.textContent = '';
    }

    // Meta line: who/when this analysis covers.
    if (anchor) {
      const when = formatAnchor(anchor);
      $cheatersMeta.textContent = visible.length > 0
        ? t('Flagged {n} player(s) across {b} board(s) — based on the capture from {when}.')
          .replace('{n}', visible.length).replace('{b}', boards).replace('{when}', when)
        : flaggedTotal > 0
          ? t('All {f} flagged player(s) are below the current confidence threshold — slide left to see them.')
            .replace('{f}', flaggedTotal)
          : t('Scanned {b} board(s) from the capture at {when} — nothing anomalous.')
            .replace('{b}', boards).replace('{when}', when);
    } else {
      $cheatersMeta.textContent = t('No capture available yet to analyse.');
    }

    // Coverage section — list which boards the analysis touched. Always
    // worth showing when we have an anchor (transparency), even when zero
    // players are flagged so the user can see WHICH boards passed clean.
    renderCheatersCoverage(data);

    if (!visible.length) {
      $cheatersBody.innerHTML = `<p class="lb-cheaters-empty" data-i18n>No suspicious activity flagged.</p>`;
      rerunI18n();
      return;
    }

    // Player rows. Each summary is a button (keyboard-friendly), each
    // expandable section lists the boards + evidence.
    $cheatersBody.innerHTML = visible.map((p, idx) => {
      const totalEvidence = p.leaderboards.reduce(
        (acc, b) => acc + (b.evidence || []).length, 0,
      );
      const boardCountLabel = t('{n} board(s)').replace('{n}', p.leaderboards.length);
      const evidenceCountLabel = t('{n} flag(s)').replace('{n}', totalEvidence);
      const confidence = p.confidence ?? 0;
      const confClass = confidenceClass(confidence);
      return `
        <div class="lb-cheater-row" data-idx="${idx}">
          <button type="button" class="lb-cheater-summary"
                  aria-expanded="false" data-act="toggle">
            <span class="lb-cheater-name">
              <span class="lb-cheater-name-text">${esc(p.player_name)}</span>
              <span class="lb-cheater-name-link" data-act="open-history"
                    data-player="${esc(p.player_name)}"
                    data-i18n>View history</span>
            </span>
            <span class="lb-cheater-stats">
              <span class="lb-confidence ${confClass}" title="${t('Confidence')}">${formatConfidence(confidence)}</span>
              <span class="lb-stat-pill">${esc(boardCountLabel)}</span>
              <span class="lb-stat-pill danger">${esc(evidenceCountLabel)}</span>
            </span>
            <i class="fa-solid fa-chevron-down lb-cheater-caret" aria-hidden="true"></i>
          </button>
          <div class="lb-cheater-detail">
            ${p.leaderboards.map(renderCheaterBoard).join('')}
          </div>
        </div>`;
    }).join('');
    rerunI18n();

    // Wire expansion + history-open + go-to-board clicks. The nested
    // action buttons STOP-PROPAGATE so they don't also toggle the
    // parent row.
    for (const row of $cheatersBody.querySelectorAll('[data-idx]')) {
      const summary = row.querySelector('[data-act="toggle"]');
      summary.addEventListener('click', () => {
        const expanded = row.classList.toggle('expanded');
        summary.setAttribute('aria-expanded', String(expanded));
      });
      const histLink = row.querySelector('[data-act="open-history"]');
      if (histLink) {
        histLink.addEventListener('click', (e) => {
          e.stopPropagation();
          const name = histLink.dataset.player;
          $playerSearch.value = name;
          searchPlayer(name);
        });
      }
      for (const link of row.querySelectorAll('[data-act="goto-board"]')) {
        link.addEventListener('click', (e) => {
          e.stopPropagation();
          gotoBoard(Number(link.dataset.uuid));
        });
      }
    }
  }

  function syncFilterUI() {
    if (!$cheatersFilter) return;
    const v = state.cheatersMinConfidence;
    if (Number(($cheatersFilter.value)) !== v) $cheatersFilter.value = String(v);
    if ($cheatersFilterValue) $cheatersFilterValue.textContent = formatConfidence(v);
  }

  function formatConfidence(c) {
    // Truncate (not round) so 0.998 renders as 0.99 — rounding would
    // misleadingly display "1.00" for sub-1 confidences.
    if (c >= 1) return '1.00';
    return (Math.floor(c * 100) / 100).toFixed(2);
  }

  function confidenceClass(c) {
    if (c >= 0.9) return 'lb-confidence-high';
    if (c >= 0.7) return 'lb-confidence-mid';
    return 'lb-confidence-low';
  }

  // ─── Cheaters coverage (analyzed + skipped boards) ────────────────
  // The collapsible coverage panel under the cheaters tab meta line.
  // Design intent: at a glance, the reader sees (1) WHAT FRACTION of
  // analyzable boards landed in each cadence bucket (the stats strip up
  // top, where the bar widths are proportional to count), and (2) WHICH
  // boards specifically were touched, grouped by category and accented
  // by cadence via a left-edge color stripe. Skipped boards live in a
  // separate section below with the reason rendered as its own bucket
  // strip so the count of operator-excluded vs below-min-size is
  // scannable without reading the per-row labels. Backward compatible:
  // older API payloads (pre-analyzed_boards) just hide the element.
  function renderCheatersCoverage(data) {
    if (!$cheatersCoverage) return;
    const analyzed = (data && data.analyzed_boards) || [];
    const excluded = (data && data.excluded_boards) || [];
    if (analyzed.length === 0 && excluded.length === 0) {
      $cheatersCoverage.hidden = true;
      return;
    }
    $cheatersCoverage.hidden = false;
    if ($cheatersCoverageCount) {
      const a = analyzed.length, s = excluded.length;
      $cheatersCoverageCount.textContent = s > 0
        ? t('{a} analyzed · {s} skipped').replace('{a}', a).replace('{s}', s)
        : t('{a} analyzed').replace('{a}', a);
    }
    if (!$cheatersCoverageBody) return;

    const parts = [];
    if (analyzed.length) {
      parts.push(renderCoverageStats(analyzed));
      parts.push(renderCoverageGroups(analyzed, false));
    }
    if (excluded.length) {
      parts.push(renderCoverageSkipped(excluded));
    }
    $cheatersCoverageBody.innerHTML = parts.join('');
    rerunI18n();
  }

  // Stats strip — three big-number tiles, one per cadence bucket, each
  // with a proportional bar underneath. The bars share a denominator
  // (analyzed total) so visual width comparisons across buckets are
  // honest. Zero-count buckets still render so the reader sees the
  // absence — "0 weekly" is information.
  function renderCoverageStats(analyzed) {
    const by = { daily: 0, weekly: 0, lifetime: 0 };
    for (const b of analyzed) by[cadenceBucket(b)]++;
    const total = analyzed.length || 1;
    const tile = (key, label) => {
      const n = by[key];
      const pct = (n / total) * 100;
      return `
        <div class="lb-cov-stat lb-cov-stat--${key}">
          <div class="lb-cov-stat-head">
            <span class="lb-cov-stat-count">${n}</span>
            <span class="lb-cov-stat-label">${esc(label)}</span>
          </div>
          <div class="lb-cov-stat-bar">
            <div class="lb-cov-stat-bar-fill" style="width:${pct.toFixed(1)}%"></div>
          </div>
        </div>`;
    };
    return `
      <div class="lb-cov-stats">
        ${tile('daily', t('daily'))}
        ${tile('weekly', t('weekly'))}
        ${tile('lifetime', t('lifetime'))}
      </div>`;
  }

  // Grouped board rows, one section per category. Each row carries a
  // cadence-tinted left bar + soft gradient bleed into the surface;
  // entry count is right-aligned in a tabular figure so the columns
  // line up vertically when names truncate.
  function renderCoverageGroups(boards, isSkipped) {
    const groups = new Map();
    for (const b of boards) {
      const cat = b.category || '—';
      if (!groups.has(cat)) groups.set(cat, []);
      groups.get(cat).push(b);
    }
    const out = [];
    for (const [cat, items] of groups) {
      out.push(`
        <div class="lb-cov-group">
          <div class="lb-cov-group-head">
            <span class="lb-cov-group-name">${esc(cat)}</span>
            <span class="lb-cov-group-rule"></span>
            <span class="lb-cov-group-count">${items.length}</span>
          </div>
          <div class="lb-cov-group-body">
            ${items.map((b) => renderCoverageRow(b, isSkipped)).join('')}
          </div>
        </div>`);
    }
    return out.join('');
  }

  function renderCoverageRow(b, isSkipped) {
    const bucket = cadenceBucket(b);
    // Classname carries the bucket so the left-edge accent + gradient
    // tint can vary per row without inline style. ``rsn-*`` adds a
    // second class when skipped so the muted state is independent.
    const classes = ['lb-cov-row', `kind-${bucket}`];
    if (isSkipped) classes.push('skipped', `rsn-${b.reason || 'unknown'}`);
    // Right column shows entries (analyzed) or reason (skipped). For
    // skipped rows the reason text is short and acts as the visual
    // sorting cue.
    let right;
    if (isSkipped) {
      const reasonText = b.reason === 'admin_excluded'
        ? t('opted out')
        : b.reason === 'below_min_size'
          ? (typeof b.entries === 'number'
              ? t('only {n}').replace('{n}', b.entries)
              : t('too few'))
          : (b.reason || '');
      right = `<span class="lb-cov-row-meta lb-cov-row-reason">${esc(reasonText)}</span>`;
    } else {
      const entries = typeof b.entries === 'number' ? b.entries : null;
      right = entries == null
        ? ''
        : `<span class="lb-cov-row-meta lb-cov-row-entries">${entries.toLocaleString()}</span>`;
    }
    // Tooltip: always full name + entries when relevant, so truncation
    // never loses information.
    const tipParts = [b.name];
    if (typeof b.entries === 'number') {
      tipParts.push(t('{n} entries').replace('{n}', b.entries));
    }
    const tip = tipParts.join(' · ');
    return `
      <div class="${classes.join(' ')}" title="${esc(tip)}">
        <span class="lb-cov-row-dot" aria-hidden="true"></span>
        <span class="lb-cov-row-name">${esc(b.name)}</span>
        ${right}
      </div>`;
  }

  // Skipped section: own heading + reason-breakdown tiles, then the
  // grouped rows (reuses renderCoverageGroups with skipped=true).
  function renderCoverageSkipped(excluded) {
    const byReason = { admin_excluded: 0, below_min_size: 0 };
    for (const b of excluded) {
      if (b.reason === 'admin_excluded' || b.reason === 'below_min_size') {
        byReason[b.reason]++;
      }
    }
    const total = excluded.length || 1;
    const reasonTile = (key, label) => {
      const n = byReason[key];
      if (n === 0) return '';
      const pct = (n / total) * 100;
      return `
        <div class="lb-cov-stat lb-cov-stat--${key}">
          <div class="lb-cov-stat-head">
            <span class="lb-cov-stat-count">${n}</span>
            <span class="lb-cov-stat-label">${esc(label)}</span>
          </div>
          <div class="lb-cov-stat-bar">
            <div class="lb-cov-stat-bar-fill" style="width:${pct.toFixed(1)}%"></div>
          </div>
        </div>`;
    };
    return `
      <div class="lb-cov-skipped">
        <div class="lb-cov-skipped-head">
          <span class="lb-cov-skipped-label" data-i18n>Skipped boards</span>
          <span class="lb-cov-skipped-rule"></span>
          <span class="lb-cov-skipped-count">${excluded.length}</span>
        </div>
        <div class="lb-cov-stats lb-cov-stats--skipped">
          ${reasonTile('admin_excluded', t('opted out by operator'))}
          ${reasonTile('below_min_size', t('too few entries'))}
        </div>
        ${renderCoverageGroups(excluded, true)}
      </div>`;
  }

  // "daily" / "weekly" / "lifetime" — the bucket the row gets tinted by.
  function cadenceBucket(b) {
    const rk = b.reset_kind || 'default';
    if (rk === 'default' || rk === 'none') return 'lifetime';
    if (rk === 'weekly') return 'weekly';
    return 'daily';
  }

  function renderCheaterBoard(b) {
    const meta = [
      t('Rank') + ' #' + b.rank,
      t('Score') + ' ' + formatScore(b.score),
      b.contest_type ? esc(b.contest_type) : null,
    ].filter(Boolean).map((s) => `<span>${esc(s)}</span>`).join('');
    const evidence = (b.evidence || []).map(renderEvidence).join('');
    // Per-board confidence badge (falls back gracefully if older API
    // payloads don't include it).
    const conf = (typeof b.confidence === 'number') ? b.confidence : 0;
    const confBadge = conf > 0
      ? `<span class="lb-confidence ${confidenceClass(conf)}" title="${t('Confidence')}">${formatConfidence(conf)}</span>`
      : '';
    // Board name is a link that jumps to the Leaderboards tab and
    // pre-selects this board at the analysis-snapshot anchor — so the
    // user can verify the flag in context with one click.
    const tooltip = t('Open this board in the Leaderboards view');
    return `
      <div class="lb-cheater-board">
        <div class="lb-cheater-board-head">
          <button type="button" class="lb-cheater-board-name lb-cheater-board-link"
                  data-act="goto-board" data-uuid="${b.uuid}" title="${esc(tooltip)}">
            ${esc(b.name)}
            <i class="fa-solid fa-arrow-right-long" aria-hidden="true"></i>
          </button>
          ${confBadge}
          <span class="lb-cheater-board-meta">${meta}</span>
        </div>
        <div class="lb-evidence">${evidence}</div>
      </div>`;
  }

  async function gotoBoard(uuid) {
    // Bring the user to the Leaderboards tab with the snapshot's
    // anchor and the target board selected. The cheaters analysis
    // anchor (state.cheaters.anchor) is the most recent capture, which
    // is usually "Today" in the day picker — but we look it up to
    // handle edge cases where the picker is offset by a missed cycle.
    switchTab('boards');
    const targetAnchor = state.cheaters && state.cheaters.anchor;
    if (targetAnchor) {
      const idx = state.days.findIndex((d) => d.anchor === targetAnchor);
      if (idx >= 0 && state.selectedDayIdx !== idx) {
        await selectDay(idx);
      }
    }
    // The board list may have just (re-)loaded — if our target board
    // isn't in this capture's board list, just bail. Otherwise select
    // it; selectBoard handles "already selected" early-out internally.
    const board = state.boards.find((b) => b.uuid === uuid);
    if (board) selectBoard(uuid);
    // Smooth-scroll to the entries pane so mobile / narrow viewports
    // see the result without an extra scroll.
    const entriesEl = document.getElementById('lb-entries');
    if (entriesEl) entriesEl.scrollIntoView({ behavior: 'smooth', block: 'start' });
  }

  function renderEvidence(ev) {
    const cls = {
      score_outlier: 'lb-evidence-type-score',
      rank_gap: 'lb-evidence-type-rank',
      velocity_outlier: 'lb-evidence-type-velocity',
    }[ev.type] || '';
    const label = {
      score_outlier: t('Score outlier'),
      rank_gap: t('Rank gap'),
      velocity_outlier: t('Velocity'),
    }[ev.type] || ev.type;
    // Summaries come from the API verbatim — they include dynamic
    // numbers in English. Localising them would require structured
    // measurements + a templating pass; deferred for now.
    return `
      <div class="lb-evidence-row">
        <span class="lb-evidence-type ${cls}">${esc(label)}</span>
        <span class="lb-evidence-summary">${esc(ev.summary || '')}</span>
      </div>`;
  }

  // ─── Activity estimate (active player count) ──────────────────────
  // Live count of distinct top-leaderboard players who scored on at
  // least one lifetime-accumulating board between the two most recent
  // captures. Renders as a small pill under the subtitle. Stays hidden
  // when the API can't compute one yet (e.g. only one anchor in the DB,
  // typical right after the bot starts sending unique timestamps).
  // localStorage key so the user's open/closed preference for the
  // activity trend chart persists across visits.
  const ACTIVITY_TREND_OPEN_KEY = 'btt_lb_activity_trend_open';
  function readActivityTrendOpen() {
    try { return localStorage.getItem(ACTIVITY_TREND_OPEN_KEY) === '1'; }
    catch (_) { return false; }
  }
  function writeActivityTrendOpen(open) {
    try { localStorage.setItem(ACTIVITY_TREND_OPEN_KEY, open ? '1' : '0'); }
    catch (_) { /* private mode / blocked storage — fail soft */ }
  }

  function renderActivity() {
    const el = document.getElementById('lb-activity');
    if (!el) return;
    const data = state.activity;
    if (!data || data.estimate == null || !data.window_end) {
      el.hidden = true;
      return;
    }
    const hours = data.duration_hours || 0;
    const tmpl = hours >= 0.95 && hours <= 1.05
      ? t('~{n} active players in the last hour')
      : t('~{n} active players in the last {h}h');
    const text = tmpl
      .replace('{n}', `<span class="lb-activity-count">${Number(data.estimate).toLocaleString()}</span>`)
      .replace('{h}', hours.toFixed(1));
    // Caret icon on the right tells the user this is a toggle. Rotated
    // via CSS when the parent carries .open. Always shown — the
    // expandable section renders an empty-state hint if no points are
    // available yet (e.g. fresh deploy), so clicking is never wasted.
    el.innerHTML =
      `<span class="lb-activity-dot" aria-hidden="true"></span>` +
      `<span class="lb-activity-text">${text}</span>` +
      `<i class="fa-solid fa-chevron-down lb-activity-caret" aria-hidden="true"></i>`;
    el.title = data.methodology || '';
    el.hidden = false;

    // Wire the click toggle ONCE — guard against re-binding on
    // language changes (renderActivity re-runs on i18n refresh).
    if (!el.dataset.toggleWired) {
      el.addEventListener('click', () => {
        const open = !state.activityTrendOpen;
        setActivityTrendOpen(open);
      });
      el.dataset.toggleWired = '1';
    }
  }

  // Open/close the activity trend chart. Renders the SVG on first
  // open (chart can be heavy) and remembers the choice in localStorage.
  function setActivityTrendOpen(open) {
    const pill = document.getElementById('lb-activity');
    const wrap = document.getElementById('lb-activity-trend');
    state.activityTrendOpen = !!open;
    writeActivityTrendOpen(state.activityTrendOpen);
    if (pill) {
      pill.classList.toggle('open', state.activityTrendOpen);
      pill.setAttribute('aria-expanded', String(state.activityTrendOpen));
    }
    if (wrap) wrap.hidden = !state.activityTrendOpen;
    if (!state.activityTrendOpen) return;

    // If we don't have history yet (initial fetch failed or returned
    // empty), retry once now so re-opening later actually loads data
    // that may have been ingested since first paint.
    const cached = state.activityHistory;
    const haveData = cached && cached.points && cached.points.length >= 2;
    if (!haveData) {
      renderActivityTrend();   // paint the empty-state placeholder
      fetchJSON('/site/leaderboards/activity/history?days=7')
        .then((d) => { state.activityHistory = d; renderActivityTrend(); })
        .catch(() => { /* leave empty-state in place */ });
      return;
    }
    renderActivityTrend();
  }

  // ─── Activity history sparkline ───────────────────────────────────
  // Tiny inline-SVG line chart of activity ESTIMATES over the last 7
  // days. Plots ``estimate_per_hour`` rather than the raw estimate —
  // that flattens spikes caused by missed captures (a 2h window would
  // otherwise show roughly 2× the count of a healthy 1h window). The
  // chart is decorative — no axes, no labels — but a hover indicator
  // surfaces (when, raw count, duration). Hidden when fewer than 2
  // points are available (fresh deploy with no history yet).
  function renderActivityTrend() {
    const wrap = document.getElementById('lb-activity-trend');
    const host = document.getElementById('lb-activity-trend-chart');
    const range = document.getElementById('lb-activity-trend-range');
    const tip = document.getElementById('lb-activity-trend-tip');
    if (!wrap || !host) return;
    // Visibility is owned by setActivityTrendOpen; this function only
    // paints the contents.

    const data = state.activityHistory;
    const points = (data && data.points) || [];
    if (points.length < 2) {
      // Empty state — explain why the chart isn't here yet without
      // wasting the click. The pill stays clickable; if the user
      // re-opens later when data has accumulated, it'll paint.
      if (range) range.textContent = '';
      host.innerHTML =
        `<div class="lb-activity-trend-empty" data-i18n>` +
          t('Captures every hour — chart appears once 2+ are stored.') +
        `</div>`;
      if (tip) tip.hidden = true;
      rerunI18n();
      return;
    }

    // Range label (e.g. "last 7d · 42 captures") — gives the user a
    // sense of how much of the window is filled in.
    if (range) {
      const days = data.days || 7;
      const captures = points.length;
      range.textContent = t('last {d}d · {n} captures')
        .replace('{d}', days).replace('{n}', captures);
    }

    // SVG geometry — fixed pixel size so the line shape is consistent
    // across viewports. Width fills the hero column; height is fixed
    // so the hero doesn't shift on chart load.
    const W = 320, H = 56;
    const padX = 4, padT = 4, padB = 8;
    const plotW = W - padX * 2, plotH = H - padT - padB;

    const xs = points.map((p) => p.window_end);
    const ys = points.map((p) => p.estimate_per_hour || 0);
    const xMin = xs[0], xMax = xs[xs.length - 1];
    const xRange = Math.max(1, xMax - xMin);
    let yMax = Math.max(...ys, 1);
    const yMin = 0;             // anchor at zero so the line shape is honest
    const yRange = Math.max(1, yMax - yMin);
    const xToPx = (t) => padX + ((t - xMin) / xRange) * plotW;
    const yToPx = (v) => padT + (1 - (v - yMin) / yRange) * plotH;

    // Build path + an area fill underneath for visual weight.
    const linePts = points.map(
      (p, i) => `${i === 0 ? 'M' : 'L'}${xToPx(p.window_end).toFixed(1)},${yToPx(p.estimate_per_hour).toFixed(1)}`,
    ).join(' ');
    const areaPts = linePts +
      ` L${xToPx(xMax).toFixed(1)},${(padT + plotH).toFixed(1)}` +
      ` L${xToPx(xMin).toFixed(1)},${(padT + plotH).toFixed(1)} Z`;

    const svgNS = 'http://www.w3.org/2000/svg';
    const svg = document.createElementNS(svgNS, 'svg');
    svg.setAttribute('viewBox', `0 0 ${W} ${H}`);
    svg.setAttribute('preserveAspectRatio', 'none');
    svg.setAttribute('class', 'lb-activity-trend-svg');
    svg.setAttribute('role', 'img');
    // Area fill
    const area = document.createElementNS(svgNS, 'path');
    area.setAttribute('class', 'lb-activity-trend-area');
    area.setAttribute('d', areaPts);
    svg.appendChild(area);
    // Line on top
    const line = document.createElementNS(svgNS, 'path');
    line.setAttribute('class', 'lb-activity-trend-line');
    line.setAttribute('d', linePts);
    line.setAttribute('fill', 'none');
    svg.appendChild(line);
    // Hover dot (initially hidden)
    const dot = document.createElementNS(svgNS, 'circle');
    dot.setAttribute('class', 'lb-activity-trend-dot');
    dot.setAttribute('r', '3');
    dot.style.opacity = '0';
    svg.appendChild(dot);
    // Mouse overlay
    const overlay = document.createElementNS(svgNS, 'rect');
    overlay.setAttribute('x', padX);
    overlay.setAttribute('y', padT);
    overlay.setAttribute('width', plotW);
    overlay.setAttribute('height', plotH);
    overlay.setAttribute('fill', 'transparent');
    svg.appendChild(overlay);

    host.innerHTML = '';
    host.appendChild(svg);

    // Hover: find the point nearest the cursor's x, populate the tip.
    function onMove(evt) {
      const r = svg.getBoundingClientRect();
      const sx = ((evt.clientX - r.left) / r.width) * W;
      const ratio = Math.max(0, Math.min(1, (sx - padX) / plotW));
      const targetT = xMin + ratio * xRange;
      let best = 0, bestDist = Infinity;
      for (let i = 0; i < points.length; i++) {
        const d = Math.abs(points[i].window_end - targetT);
        if (d < bestDist) { bestDist = d; best = i; }
      }
      const p = points[best];
      dot.setAttribute('cx', xToPx(p.window_end).toFixed(1));
      dot.setAttribute('cy', yToPx(p.estimate_per_hour).toFixed(1));
      dot.style.opacity = '1';
      if (!tip) return;
      const when = new Date(p.window_end * 1000);
      const windowLabel = p.duration_hours && p.duration_hours > 1.1
        ? t('over {h}h window (gap)').replace('{h}', p.duration_hours.toFixed(1))
        : t('1h window');
      tip.innerHTML = `
        <strong>${Number(p.estimate).toLocaleString()}</strong>
        <span class="lb-activity-trend-tip-sub">${esc(windowLabel)}</span>
        <span class="lb-activity-trend-tip-when">${esc(when.toLocaleString(undefined, {
          month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit',
        }))}</span>`;
      tip.hidden = false;
    }
    function onLeave() {
      dot.style.opacity = '0';
      if (tip) tip.hidden = true;
    }
    overlay.addEventListener('mousemove', onMove);
    overlay.addEventListener('mouseleave', onLeave);
    overlay.addEventListener('touchstart', (e) => {
      if (e.touches[0]) onMove(e.touches[0]);
    }, {passive: true});
    overlay.addEventListener('touchend', onLeave);

    // a11y: SVG title summarising the latest data point
    const lastP = points[points.length - 1];
    svg.setAttribute('aria-label', t('Activity trend, latest ~{n} active players per hour')
      .replace('{n}', Math.round(lastP.estimate_per_hour || 0)));
  }


  // ─── Subtitle (dynamic retention window) ───────────────────────────
  // The retention number in the subtitle tracks the runtime config
  // tunable ``leaderboards_hot_retention_days``. JS owns this node
  // entirely (no [data-i18n] on it) — we translate the template via
  // BTTi18n.t(), substitute the actual number, and write it directly.
  // Re-runs on language change via the listener in wireEvents().
  function renderSubtitle() {
    const el = document.getElementById('lb-subtitle');
    if (!el) return;
    const template = t('Top players per board, ranked. Hourly captures, {days}-day live retention, full archive beyond.');
    el.textContent = template.replace('{days}', String(state.hotRetentionDays));
  }

  // ─── Day picker (trove-day = real UTC - 11h) ───────────────────────
  // A trove-day [N] starts at real UTC date N at 11:00 (trove 00:00) and
  // ends at real UTC date N+1 at 11:00. To group anchors by trove-day we
  // subtract the 11h offset and floor to the day in UTC; the resulting
  // calendar date is the trove-day label.
  function troveDayKeyFor(unix) {
    return Math.floor((unix - TROVE_OFFSET_SECONDS) / DAY_SECONDS);
  }

  function buildDays() {
    // Current trove-day key from real-time "now". This is the chip we
    // mark as "Today".
    const now = Math.floor(Date.now() / 1000);
    const todayKey = troveDayKeyFor(now);

    // Index every stored anchor by its trove-day. We keep the MAX (the
    // last capture of that day) — the only one we care about, per spec.
    const byDay = new Map();
    for (const ts of state.anchors) {
      const key = troveDayKeyFor(ts);
      const prev = byDay.get(key);
      if (prev == null || ts > prev) byDay.set(key, ts);
    }

    state.days = [];
    for (let i = 0; i < PICKER_DAYS; i++) {
      const key = todayKey - i;
      const dayStart = key * DAY_SECONDS + TROVE_OFFSET_SECONDS;       // real UTC start (11:00)
      const dayEnd = dayStart + DAY_SECONDS;                            // real UTC end (next 11:00)
      const troveDate = new Date(dayStart * 1000 - TROVE_OFFSET_SECONDS * 1000); // trove-time midnight
      state.days.push({
        key,
        troveDate,
        dayStart,
        dayEnd,
        anchor: byDay.get(key) || null,
        relative: i,  // 0=today, 1=yesterday, ...
      });
    }
  }

  function renderDayPicker() {
    if (!state.days.length) {
      $dayPicker.innerHTML = `<span class="lb-day-loading" data-i18n>No captures yet</span>`;
      rerunI18n();
      return;
    }
    $dayPicker.innerHTML = state.days.map((d, idx) => {
      const cls = ['lb-day'];
      if (d.anchor == null) cls.push('lb-day-empty');
      if (idx === state.selectedDayIdx) cls.push('active');
      const dis = d.anchor == null ? 'disabled aria-disabled="true"' : 'aria-selected="' + (idx === state.selectedDayIdx) + '"';
      const role = d.anchor == null ? '' : 'role="tab"';
      return `
        <button type="button" class="${cls.join(' ')}" data-day-idx="${idx}" ${dis} ${role}>
          <span class="lb-day-rel" data-i18n>${dayRelativeLabel(d.relative)}</span>
          <span class="lb-day-date">${dayCalendarLabel(d.troveDate)}</span>
          ${d.anchor == null
            ? `<span class="lb-day-meta" data-i18n>No data</span>`
            : `<span class="lb-day-meta" title="${esc(captureTitle(d.anchor))}">${esc(formatCaptureTime(d.anchor, d.relative))}</span>`}
        </button>`;
    }).join('');
    rerunI18n();

    for (const btn of $dayPicker.querySelectorAll('[data-day-idx]:not([disabled])')) {
      btn.addEventListener('click', () => selectDay(Number(btn.dataset.dayIdx)));
    }
  }

  function dayRelativeLabel(rel) {
    if (rel === 0) return 'Today';
    if (rel === 1) return 'Yesterday';
    return 'Day';  // generic label, the date below carries the specifics
  }

  function dayCalendarLabel(troveDate) {
    // Trove-date as "Mon Jun 8" — the JS Date passed in is already at
    // trove-time midnight so getMonth/getDate are the trove-day fields.
    const months = ['Jan','Feb','Mar','Apr','May','Jun','Jul','Aug','Sep','Oct','Nov','Dec'];
    const days = ['Sun','Mon','Tue','Wed','Thu','Fri','Sat'];
    return `${days[troveDate.getUTCDay()]} ${months[troveDate.getUTCMonth()]} ${troveDate.getUTCDate()}`;
  }

  function formatCaptureTime(unixAnchor, relativeDay) {
    // "Last capture" meta on each day chip. UTC anchors are confusing to
    // read directly, so:
    //   • Today (relativeDay 0): a relative "Xm ago" / "Xh ago" — the
    //     most intuitive read for "how fresh is this?".
    //   • Older days: the capture clock time converted to the USER'S
    //     LOCAL timezone (not UTC, not trove-time) so it matches the
    //     clock on their wall.
    if (relativeDay === 0) {
      const now = Math.floor(Date.now() / 1000);
      const diff = Math.max(0, now - unixAnchor);
      if (diff < 60) return t('just now');
      if (diff < 3600) return t('{n}m ago').replace('{n}', Math.round(diff / 60));
      return t('{n}h ago').replace('{n}', Math.round(diff / 3600));
    }
    // Local wall-clock time. ``new Date(unix*1000)`` is already in the
    // browser's timezone; toLocaleTimeString formats it per the user's
    // locale (24h vs 12h handled for us).
    return new Date(unixAnchor * 1000).toLocaleTimeString(undefined, {
      hour: '2-digit', minute: '2-digit',
    });
  }

  function captureTitle(unixAnchor) {
    // Hover detail: full local date + time so the short label above is
    // never ambiguous regardless of which branch produced it.
    return new Date(unixAnchor * 1000).toLocaleString(undefined, {
      weekday: 'short', month: 'short', day: 'numeric',
      hour: '2-digit', minute: '2-digit',
    });
  }

  async function selectDay(idx) {
    if (idx < 0 || idx >= state.days.length) return;
    const day = state.days[idx];
    if (day.anchor == null) return;  // empty day, do nothing
    if (state.selectedDayIdx === idx && state.anchor === day.anchor) return;

    state.selectedDayIdx = idx;
    state.anchor = day.anchor;
    renderDayPicker();  // refresh active state
    updateHash();
    await loadBoards();
  }

  // ─── Fetch helpers ─────────────────────────────────────────────────
  async function fetchJSON(path) {
    const res = await fetch(path, { headers: { Accept: 'application/json' } });
    if (!res.ok) {
      let msg = `HTTP ${res.status}`;
      try {
        const body = await res.json();
        if (body && body.error && body.error.message) msg = body.error.message;
      } catch (_) { /* no body */ }
      throw new Error(msg);
    }
    return res.json();
  }

  function formatAnchor(ts) {
    const d = new Date(ts * 1000);
    // Show "YYYY-MM-DD HH:mm UTC" — readable, sortable, no surprise
    // local-time gotchas (the data is anchored in UTC).
    const pad = (n) => String(n).padStart(2, '0');
    return `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())} ${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())} UTC`;
  }

  // ─── Boards ────────────────────────────────────────────────────────
  // Loading model:
  //   • If we ALREADY have boards in state (a previous day's list, or
  //     this same day from cache), leave them on screen and just paint
  //     a subtle "refreshing" overlay class on the sidebar. The user
  //     keeps a usable UI while the new anchor's boards load.
  //   • If state.boards is empty (first paint, or a load error wiped
  //     them), show the "crunching latest data" placeholder so the
  //     user knows the bot is processing — much friendlier than a
  //     bare "Loading…".
  async function loadBoards() {
    const previousBoards = state.boards;
    const hadBoards = previousBoards.length > 0;
    if (!hadBoards) {
      $boardList.innerHTML = `
        <p class="lb-loading lb-loading-crunch" data-i18n>Crunching the latest capture — first paint can take a moment while we warm the caches.</p>`;
      rerunI18n();
      resetEntries();
    } else {
      // Dim the sidebar so it's visually clear something's in flight.
      $boardList.classList.add('lb-refreshing');
    }
    state.selectedUuid = null;
    try {
      const data = await fetchJSON(`/site/leaderboards/boards?created_at=${state.anchor}`);
      const fresh = data.items || [];
      state.boards = fresh;
      if (hadBoards) resetEntries();   // clear the now-stale entries pane
    } catch (err) {
      // Keep the previous list visible if we had one — failure to refresh
      // shouldn't make the page worse than it was.
      $boardList.classList.remove('lb-refreshing');
      if (!hadBoards) $boardList.innerHTML = errorHTML(err);
      return;
    } finally {
      $boardList.classList.remove('lb-refreshing');
    }
    renderBoardList();
  }

  function renderBoardList() {
    if (!state.boards.length) {
      $boardList.innerHTML = `<p class="lb-board-empty" data-i18n>No boards for this capture.</p>`;
      rerunI18n();
      return;
    }
    const filter = state.boardFilter.toLowerCase();
    const visible = filter
      ? state.boards.filter((b) =>
          b.name.toLowerCase().includes(filter) ||
          (b.category || '').toLowerCase().includes(filter))
      : state.boards;

    if (!visible.length) {
      $boardList.innerHTML = `<p class="lb-board-empty" data-i18n>No boards match your filter.</p>`;
      rerunI18n();
      return;
    }

    // Group by category, preserve server-side ordering within each.
    const groups = new Map();
    for (const b of visible) {
      const cat = b.category || 'Other';
      if (!groups.has(cat)) groups.set(cat, []);
      groups.get(cat).push(b);
    }

    // When the user is filtering by text, override their collapsed
    // preferences and show every matching group expanded — otherwise a
    // remembered-collapsed category could hide the match the user is
    // explicitly searching for. The persisted set is left untouched so
    // clearing the filter restores the user's original layout.
    const filterActive = filter.length > 0;
    const selectedCat = state.boards
      .find((b) => b.uuid === state.selectedUuid)?.category || null;

    const html = [];
    for (const [cat, boards] of groups) {
      // The category that holds the currently-selected board is always
      // expanded so the active item never hides behind a collapsed
      // header (which would look like the selection got lost).
      const collapsed = !filterActive
        && cat !== selectedCat
        && state.collapsedCategories.has(cat);
      const catKey = String(cat);
      html.push(`
        <div class="lb-category-group" data-category="${esc(catKey)}"
             data-collapsed="${collapsed ? 'true' : 'false'}">
          <button type="button" class="lb-category"
                  aria-expanded="${collapsed ? 'false' : 'true'}"
                  data-cat="${esc(catKey)}">
            <i class="fa-solid fa-chevron-down lb-category-caret" aria-hidden="true"></i>
            <span class="lb-category-name">${esc(cat)}</span>
            <span class="lb-category-count">${boards.length}</span>
          </button>
          <div class="lb-category-body">`);
      for (const b of boards) {
        const isActive = b.uuid === state.selectedUuid;
        const tag = b.contest_type
          ? `<span class="lb-contest-tag">${esc(b.contest_type)}</span>`
          : '';
        html.push(`
          <button type="button" class="lb-board${isActive ? ' active' : ''}"
                  data-uuid="${b.uuid}" title="${esc(b.name)}">
            <span class="lb-board-name">${esc(b.name)}</span>
            ${tag}
          </button>`);
      }
      html.push(`</div></div>`);
    }
    $boardList.innerHTML = html.join('');

    for (const btn of $boardList.querySelectorAll('[data-uuid]')) {
      btn.addEventListener('click', () => {
        selectBoard(Number(btn.dataset.uuid));
        // On mobile, close the drawer after a selection.
        $sidebar.classList.remove('open');
        $mobileTrigger.setAttribute('aria-expanded', 'false');
      });
    }

    for (const head of $boardList.querySelectorAll('.lb-category[data-cat]')) {
      head.addEventListener('click', () => toggleCategory(head.dataset.cat));
    }
  }

  function toggleCategory(cat) {
    // Local state lives on the Set, which is also written to
    // localStorage so the preference survives a reload. We never
    // collapse the category that holds the active board (renderBoardList
    // enforces this on the next pass).
    if (state.collapsedCategories.has(cat)) {
      state.collapsedCategories.delete(cat);
    } else {
      state.collapsedCategories.add(cat);
    }
    writeCollapsedCategories(state.collapsedCategories);
    renderBoardList();
  }

  // ─── Entries ───────────────────────────────────────────────────────
  function resetEntries() {
    state.entries = [];
    state.entriesTotal = 0;
    // Use data-i18n on the title + hint so the i18n sweep translates
    // them on every language switch — the textContent path used to lose
    // the original English source after the first JS-set, and t() at
    // resolve time misses the async-loaded dict on first render.
    $entriesTitle.setAttribute('data-i18n', '');
    $entriesTitle.textContent = 'Pick a board to see entries';
    $entriesMeta.textContent = '';
    $entriesBody.innerHTML = `<p class="lb-hint" data-i18n>Choose a board on the left to load its ranked entries.</p>`;
    $entriesFoot.hidden = true;
    hideBoardChart();
    rerunI18n();
  }

  async function selectBoard(uuid) {
    if (state.selectedUuid === uuid) return;
    const hadPreviousBoard = state.selectedUuid != null;
    state.selectedUuid = uuid;
    state.entries = [];
    state.entriesTotal = 0;

    const board = state.boards.find((b) => b.uuid === uuid);
    if (board) {
      // Drop data-i18n + untrack the node — otherwise i18n.refresh()'s
      // restoreAll restores the cached English source ("Pick a board…")
      // back over the board name we're about to write.
      $entriesTitle.removeAttribute('data-i18n');
      if (window.BTTi18n && window.BTTi18n.untrack) window.BTTi18n.untrack($entriesTitle);
      $entriesTitle.textContent = board.name;
      $mobileSelected.removeAttribute('data-i18n');
      if (window.BTTi18n && window.BTTi18n.untrack) window.BTTi18n.untrack($mobileSelected);
      $mobileSelected.textContent = board.name;
    }
    updateHash();

    // Refresh active state in sidebar.
    for (const btn of $boardList.querySelectorAll('[data-uuid]')) {
      btn.classList.toggle('active', Number(btn.dataset.uuid) === uuid);
    }

    // Don't blank the entries pane between boards — show a subtle
    // dimming overlay instead so the user keeps their place visually
    // while the next page fetches. The empty-state copy ("Loading…")
    // only shows on the very first paint.
    if (hadPreviousBoard) {
      $entriesBody.classList.add('lb-refreshing');
    } else {
      $entriesBody.innerHTML = `
        <p class="lb-loading lb-loading-crunch" data-i18n>Crunching the latest capture — first paint can take a moment while we warm the caches.</p>`;
    }
    $entriesFoot.hidden = true;
    rerunI18n();
    // Kick off the chart fetch in parallel with the entries — independent
    // requests, no need to serialise. The chart's wrap stays hidden if
    // the payload doesn't yield a chartable window (<2 anchors).
    loadBoardChart(uuid).catch((err) => {
      console.warn('[leaderboards] board chart failed', err);
      hideBoardChart();
    });
    await loadMoreEntries(/* reset = */ true);
  }

  async function loadMoreEntries(reset = false) {
    if (state.loadingEntries || state.selectedUuid == null) return;
    state.loadingEntries = true;
    if (!reset) $loadMore.disabled = true;
    const offset = reset ? 0 : state.entries.length;
    try {
      const data = await fetchJSON(
        `/site/leaderboards/${state.selectedUuid}/entries`
        + `?created_at=${state.anchor}&limit=${PAGE_SIZE}&offset=${offset}`,
      );
      state.entries = reset ? (data.items || []) : state.entries.concat(data.items || []);
      state.entriesTotal = data.total || 0;
      renderEntries();
    } catch (err) {
      // Only blow away the body with an error message if we have
      // nothing better on screen — a refresh failure on top of an
      // existing list should NOT erase the list.
      if (!state.entries.length) {
        $entriesBody.innerHTML = errorHTML(err);
        $entriesFoot.hidden = true;
      } else {
        console.warn('[leaderboards] entries refresh failed; keeping previous data', err);
      }
    } finally {
      state.loadingEntries = false;
      $loadMore.disabled = false;
      // Whether the fetch succeeded or failed, lift the dimming overlay
      // so the UI isn't stuck looking pending.
      $entriesBody.classList.remove('lb-refreshing');
    }
  }

  function renderEntries() {
    if (!state.entries.length) {
      $entriesBody.innerHTML = `<p class="lb-hint" data-i18n>No entries for this board.</p>`;
      $entriesMeta.textContent = '';
      $entriesFoot.hidden = true;
      rerunI18n();
      return;
    }

    const rows = state.entries.map((e) => `
      <div class="lb-td lb-rank ${rankClass(e.rank)}">${e.rank}</div>
      <div class="lb-td"><span class="lb-player" data-player="${esc(e.player_name)}"><span class="lb-player-icon"></span>${esc(e.player_name)}</span></div>
      <div class="lb-td lb-score">${esc(formatScore(e.score))}</div>
    `).join('');

    $entriesBody.innerHTML = `
      <div class="lb-entries-table">
        <div class="lb-th lb-rank" data-i18n>Rank</div>
        <div class="lb-th" data-i18n>Player</div>
        <div class="lb-th lb-score" data-i18n>Score</div>
        ${rows}
      </div>`;

    // Re-translate the freshly-injected data-i18n nodes BEFORE setting
    // the count meta — t('entries') reads the active dict and we want
    // it stable across this render.
    rerunI18n();

    const shown = state.entries.length;
    $entriesMeta.textContent = state.entriesTotal > shown
      ? `${shown.toLocaleString()} / ${state.entriesTotal.toLocaleString()} ${t('entries')}`
      : `${shown.toLocaleString()} ${t('entries')}`;

    $entriesFoot.hidden = shown >= state.entriesTotal;

    // Wire click → player history.
    for (const el of $entriesBody.querySelectorAll('[data-player]')) {
      el.addEventListener('click', () => {
        const name = el.dataset.player;
        $playerSearch.value = name;
        searchPlayer(name);
      });
    }
  }

  function rankClass(rank) {
    if (rank === 1) return 'lb-rank-top';
    if (rank === 2) return 'lb-rank-top-2';
    if (rank === 3) return 'lb-rank-top-3';
    return '';
  }

  function formatScore(score) {
    // Integers stay integers; fractional values keep up to 2 decimals.
    return Number.isInteger(score)
      ? score.toLocaleString()
      : Number(score).toLocaleString(undefined, { maximumFractionDigits: 2 });
  }

  // ─── Player history ────────────────────────────────────────────────
  async function searchPlayer(name) {
    const trimmed = (name || '').trim();
    if (!trimmed) {
      $playerPanel.hidden = true;
      hidePlayerChart();
      return;
    }
    $playerPanel.hidden = false;
    $playerName.textContent = trimmed;
    $playerBody.innerHTML = `<p class="lb-loading" data-i18n>Loading…</p>`;
    rerunI18n();
    $playerPanel.scrollIntoView({ behavior: 'smooth', block: 'nearest' });

    // Per-board chart fetch fires in parallel with the flat history — the
    // chart wraps stays hidden if the payload has too few anchors to plot.
    loadPlayerChart(trimmed).catch((err) => {
      console.warn('[leaderboards] player chart failed', err);
      hidePlayerChart();
    });

    try {
      const data = await fetchJSON(
        `/site/leaderboards/players/${encodeURIComponent(trimmed)}/history?limit=50`,
      );
      // Prefer the canonical name from the first matching row — the
      // server returns whatever Trove stored, which may differ in case
      // from what the user typed. Falls back to the typed input when
      // there are no matches.
      const canonical = (data.items && data.items.length && data.items[0].player_name) || trimmed;
      $playerName.textContent = canonical;
      renderPlayerHistory(data.items || []);
    } catch (err) {
      $playerBody.innerHTML = errorHTML(err);
    }
  }

  function renderPlayerHistory(items) {
    if (!items.length) {
      $playerBody.innerHTML = `<p class="lb-hint" data-i18n>No recent appearances found for this player.</p>`;
      rerunI18n();
      return;
    }
    // Resolve uuid→board name from the current anchor's list when we can.
    // (Falls through to "Board #UUID" if it's a board the player only appears
    // on in a different anchor than the one we've loaded boards for.)
    const nameByUuid = new Map(state.boards.map((b) => [b.uuid, b.name]));
    $playerBody.innerHTML = items.map((it) => {
      const boardName = nameByUuid.get(it.leaderboard) || `Board #${it.leaderboard}`;
      return `
        <div class="lb-player-row">
          <div class="lb-ph-board">${esc(boardName)}</div>
          <div class="lb-ph-rank">#${it.rank}</div>
          <div class="lb-ph-score">${esc(formatScore(it.score))}</div>
          <div class="lb-ph-when">${esc(formatAnchor(it.created_at))}</div>
        </div>`;
    }).join('');
    rerunI18n();
  }

  // ─── Event wiring ──────────────────────────────────────────────────
  function wireEvents() {
    $boardSearch.addEventListener('input', () => {
      state.boardFilter = $boardSearch.value || '';
      renderBoardList();
    });

    $loadMore.addEventListener('click', () => loadMoreEntries(false));

    let searchTimer = null;
    $playerSearch.addEventListener('input', () => {
      clearTimeout(searchTimer);
      const val = $playerSearch.value.trim();
      // Hide the panel immediately if cleared; otherwise debounce 350ms.
      if (!val) { $playerPanel.hidden = true; return; }
      searchTimer = setTimeout(() => searchPlayer(val), 350);
    });
    $playerSearch.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') {
        e.preventDefault();
        clearTimeout(searchTimer);
        searchPlayer($playerSearch.value.trim());
      }
    });
    $playerClose.addEventListener('click', () => {
      $playerPanel.hidden = true;
      $playerSearch.value = '';
      hidePlayerChart();
    });

    $mobileTrigger.addEventListener('click', () => {
      const open = $sidebar.classList.toggle('open');
      $mobileTrigger.setAttribute('aria-expanded', String(open));
    });

    // Tab strip — switch + lazy-load cheaters on first activation.
    if ($tabBoardsBtn) {
      $tabBoardsBtn.addEventListener('click', () => switchTab('boards'));
    }
    if ($tabCheatersBtn) {
      $tabCheatersBtn.addEventListener('click', () => switchTab('cheaters'));
    }

    // Confidence-filter slider. Live re-render as the slider drags —
    // dataset is small (handful of players) so this is cheap. Persists
    // to localStorage so the chosen strictness sticks across reloads.
    if ($cheatersFilter) {
      $cheatersFilter.addEventListener('input', () => {
        const v = Math.round(Number($cheatersFilter.value) * 100) / 100;
        state.cheatersMinConfidence = v;
        writeMinConfidence(v);
        renderCheaters();
      });
    }

    // React to back/forward.
    window.addEventListener('hashchange', async () => {
      const hash = parseHash();
      const desiredTab = hash.tab === 'cheaters' ? 'cheaters' : 'boards';
      if (desiredTab !== state.activeTab) switchTab(desiredTab);
      if (hash.anchor && hash.anchor !== state.anchor) {
        const idx = state.days.findIndex((d) => d.anchor === hash.anchor);
        if (idx >= 0) await selectDay(idx);
      }
      if (hash.board != null && hash.board !== state.selectedUuid) {
        const exists = state.boards.find((b) => b.uuid === hash.board);
        if (exists) selectBoard(hash.board);
      }
    });

    // Re-render the entries pane + day picker + subtitle + cheaters
    // panel on language switch so JS-injected chrome picks up the new
    // dictionary. (The cheaters panel's evidence summaries come from
    // the API in English and don't re-localise.)
    document.addEventListener('btt-lang-changed', () => {
      renderSubtitle();
      renderActivity();
      renderDayPicker();
      renderCheaters();
      if (state.selectedUuid) renderEntries();
      else resetEntries();
      renderBoardList();
    });
  }

  // ─── URL hash helpers ──────────────────────────────────────────────
  function parseHash() {
    const out = { anchor: null, board: null, tab: null };
    const raw = location.hash.replace(/^#/, '');
    if (!raw) return out;
    const params = new URLSearchParams(raw);
    if (params.has('anchor')) out.anchor = Number(params.get('anchor')) || null;
    if (params.has('board')) out.board = Number(params.get('board')) || null;
    if (params.has('tab')) out.tab = params.get('tab');
    return out;
  }

  function updateHash() {
    const parts = [];
    // Tab first so the URL reads naturally: #tab=cheaters or
    // #anchor=…&board=… on the boards view.
    if (state.activeTab && state.activeTab !== 'boards') {
      parts.push(`tab=${state.activeTab}`);
    }
    if (state.anchor) parts.push(`anchor=${state.anchor}`);
    if (state.selectedUuid != null) parts.push(`board=${state.selectedUuid}`);
    const next = parts.length ? '#' + parts.join('&') : location.pathname;
    history.replaceState(null, '', next);
  }

  // ─── i18n ──────────────────────────────────────────────────────────
  // i18n.js exposes BTTi18n.t(s) for JS-built strings and BTTi18n.refresh()
  // to re-translate freshly-injected [data-i18n] nodes (e.g. the entries
  // table headers, which get rebuilt every render).
  function t(s) {
    return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s;
  }
  function rerunI18n() {
    if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh();
  }

  // ─── Util ──────────────────────────────────────────────────────────
  function esc(s) {
    return String(s ?? '').replace(/[&<>"']/g, (c) =>
      ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  }

  function errorHTML(err) {
    const msg = (err && err.message) || String(err);
    return `<p class="lb-error">${t('Failed to load')}: ${esc(msg)}</p>`;
  }

  // ─── Charts ────────────────────────────────────────────────────────
  // Inline-SVG line charts. One renderer (drawLineChart) drives both the
  // per-board chart (top-N players' score over 7d) and the per-player
  // chart (one line per board the player appears on, same window).
  //
  // No external dependency. The chart is interactive: hover snaps to the
  // nearest anchor and pops a tooltip with each series' value at that
  // moment, plus a legend underneath that highlights on hover.
  //
  // State on `state.boardChart` / `state.playerChart` persists payloads
  // so window-resize and language-switch re-render without re-fetching.

  // ─── Per-board chart ───────────────────────────────────────────────
  async function loadBoardChart(uuid) {
    const $wrap = document.getElementById('lb-board-chart-wrap');
    const $meta = document.getElementById('lb-board-chart-meta');
    const $chart = document.getElementById('lb-board-chart');
    const $legend = document.getElementById('lb-board-chart-legend');
    if (!$wrap || !$chart) return;
    // Optimistic show — give the user a "something is happening" cue
    // while the fetch runs. drawBoardChart hides again on empty.
    $wrap.hidden = false;
    $chart.innerHTML = `<p class="lb-chart-empty" data-i18n>Loading…</p>`;
    rerunI18n();
    const data = await fetchJSON(`/site/leaderboards/${uuid}/history?days=7&top=5`);
    // The fetch could have completed after the user moved on to another
    // board — bail if so.
    if (state.selectedUuid !== uuid) return;
    state.boardChart = { uuid, data };
    drawBoardChart();
  }

  function drawBoardChart() {
    const $wrap = document.getElementById('lb-board-chart-wrap');
    const $meta = document.getElementById('lb-board-chart-meta');
    const $chart = document.getElementById('lb-board-chart');
    const $legend = document.getElementById('lb-board-chart-legend');
    if (!$wrap || !$chart || !state.boardChart) return;
    const { data } = state.boardChart;
    // Need ≥ 2 distinct anchors AND a series with ≥ 2 points to plot a
    // line. Anything less is just a single dot — we'd rather hide the
    // figure than render a chart that says nothing.
    const usableSeries = (data.series || []).filter((s) => (s.points || []).length >= 2);
    if (!data.anchors || data.anchors.length < 2 || !usableSeries.length) {
      $wrap.hidden = true;
      return;
    }
    $wrap.hidden = false;

    if ($meta) {
      $meta.textContent = t('Top {n} · {h} captures')
        .replace('{n}', usableSeries.length)
        .replace('{h}', data.anchors.length);
    }

    const series = usableSeries.map((s, i) => ({
      key: `p:${s.player_name}`,
      label: s.player_name,
      color: CHART_COLORS[i % CHART_COLORS.length],
      // Pass ``synthetic`` through to the renderer. Server-injected
      // reset-zero markers carry synthetic=true; the chart draws them
      // into the line path (so the cliff is visible) but skips the
      // hover-dot + tooltip on them (so they don't pretend to be data).
      points: s.points.map((p) => ({
        x: p.created_at, y: p.score, rank: p.rank, synthetic: !!p.synthetic,
      })),
    }));
    drawLineChart($chart, $legend, {
      anchors: data.anchors,
      series,
      valueLabel: t('Score'),
      tooltipNameSuffix: (s, p) => (p.rank ? ` · #${p.rank}` : ''),
    });
  }

  function hideBoardChart() {
    const $wrap = document.getElementById('lb-board-chart-wrap');
    if ($wrap) $wrap.hidden = true;
    state.boardChart = null;
  }

  // ─── Per-player chart ──────────────────────────────────────────────
  async function loadPlayerChart(name) {
    const $wrap = document.getElementById('lb-player-chart-wrap');
    const $chart = document.getElementById('lb-player-chart');
    if (!$wrap || !$chart) return;
    $wrap.hidden = false;
    $chart.innerHTML = `<p class="lb-chart-empty" data-i18n>Loading…</p>`;
    rerunI18n();
    const data = await fetchJSON(
      `/site/leaderboards/players/${encodeURIComponent(name)}/series?days=7`,
    );
    // Bail if the user has cleared the search or typed a different name
    // while this fetch was in flight.
    const current = ($playerSearch.value || '').trim().toLowerCase();
    if (current !== name.trim().toLowerCase()) return;
    state.playerChart = { name, data };
    drawPlayerChart();
  }

  function drawPlayerChart() {
    const $wrap = document.getElementById('lb-player-chart-wrap');
    const $meta = document.getElementById('lb-player-chart-meta');
    const $chart = document.getElementById('lb-player-chart');
    const $legend = document.getElementById('lb-player-chart-legend');
    if (!$wrap || !$chart || !state.playerChart) return;
    const { data } = state.playerChart;
    const usableSeries = (data.series || []).filter((s) => (s.points || []).length >= 2);
    if (!data.anchors || data.anchors.length < 2 || !usableSeries.length) {
      $wrap.hidden = true;
      return;
    }
    $wrap.hidden = false;

    if ($meta) {
      $meta.textContent = t('{n} board(s) · {h} captures')
        .replace('{n}', usableSeries.length)
        .replace('{h}', data.anchors.length);
    }

    // Per-board chart for a player can have many lines — cap legend
    // chips at the first 8 (sorted by best-rank from the server). The
    // tooltip still surfaces the full set on hover.
    const limit = Math.min(usableSeries.length, 8);
    const series = usableSeries.slice(0, limit).map((s, i) => ({
      key: `b:${s.uuid}`,
      label: s.name,
      color: CHART_COLORS[i % CHART_COLORS.length],
      // See drawBoardChart for why ``synthetic`` is forwarded.
      points: s.points.map((p) => ({
        x: p.created_at, y: p.score, rank: p.rank, synthetic: !!p.synthetic,
      })),
    }));
    drawLineChart($chart, $legend, {
      anchors: data.anchors,
      series,
      valueLabel: t('Score'),
      // Each line is one board, so the tooltip already shows board name.
      // Surface the player's rank-at-that-moment as a secondary detail.
      tooltipNameSuffix: (s, p) => (p.rank ? ` · #${p.rank}` : ''),
    });
  }

  function hidePlayerChart() {
    const $wrap = document.getElementById('lb-player-chart-wrap');
    if ($wrap) $wrap.hidden = true;
    state.playerChart = null;
  }

  // ─── Generic SVG line-chart renderer ───────────────────────────────
  // Inputs:
  //   container   — <div> that will host the <svg>
  //   legendNode  — <div> for the legend chips
  //   opts.anchors            — all unix timestamps in window (defines x range)
  //   opts.series             — [{key, label, color, points:[{x, y, ...}]}]
  //   opts.valueLabel         — y-axis label (currently embedded in tooltip)
  //   opts.tooltipNameSuffix  — fn(series, point) → extra text after label
  //
  // The chart is a fully-laid-out SVG with grid, axes, polylines, and a
  // single mouse-tracking overlay that resolves to the nearest anchor on
  // hover. Legend chips highlight/dim series on hover.
  function drawLineChart(container, legendNode, opts) {
    container.innerHTML = '';
    const { anchors, series } = opts;
    if (!anchors.length || !series.length) return;

    // Layout. Width follows the container; height comes from CSS (220 / 180).
    const rect = container.getBoundingClientRect();
    const W = Math.max(280, Math.round(rect.width || 600));
    const H = Math.max(120, Math.round(rect.height || 220));
    const padL = 44, padR = 12, padT = 10, padB = 24;
    const plotW = W - padL - padR;
    const plotH = H - padT - padB;

    // x scale: linear time (unix seconds). Use the anchors' min/max
    // rather than series' points so multiple charts (board + player)
    // for the same window line up visually.
    const xMin = anchors[0];
    const xMax = anchors[anchors.length - 1];
    const xRange = Math.max(1, xMax - xMin);
    const xToPx = (t) => padL + ((t - xMin) / xRange) * plotW;

    // y scale: union of all series' point.y. 5% padding so series
    // don't kiss the top/bottom of the chart.
    let yMin = Infinity, yMax = -Infinity;
    for (const s of series) {
      for (const p of s.points) {
        if (p.y < yMin) yMin = p.y;
        if (p.y > yMax) yMax = p.y;
      }
    }
    if (!isFinite(yMin) || !isFinite(yMax)) return;
    if (yMin === yMax) { yMin -= 1; yMax += 1; }  // flat line → fake spread
    const pad = (yMax - yMin) * 0.08;
    yMin -= pad;
    yMax += pad;
    const yRange = yMax - yMin;
    const yToPx = (v) => padT + (1 - (v - yMin) / yRange) * plotH;

    const svgNS = 'http://www.w3.org/2000/svg';
    const svg = document.createElementNS(svgNS, 'svg');
    svg.setAttribute('viewBox', `0 0 ${W} ${H}`);
    svg.setAttribute('preserveAspectRatio', 'none');

    // y-grid + labels — 4 horizontal lines.
    const yTicks = 4;
    for (let i = 0; i <= yTicks; i++) {
      const v = yMin + (yRange * i) / yTicks;
      const y = yToPx(v);
      const line = document.createElementNS(svgNS, 'line');
      line.setAttribute('class', 'lb-chart-grid');
      line.setAttribute('x1', padL); line.setAttribute('x2', W - padR);
      line.setAttribute('y1', y); line.setAttribute('y2', y);
      svg.appendChild(line);
      const label = document.createElementNS(svgNS, 'text');
      label.setAttribute('class', 'lb-chart-axis-label');
      label.setAttribute('x', padL - 6);
      label.setAttribute('y', y + 3);
      label.setAttribute('text-anchor', 'end');
      label.textContent = abbrevScore(v);
      svg.appendChild(label);
    }

    // x-axis labels — 3 evenly-spaced anchors.
    const xLabelCount = 3;
    for (let i = 0; i < xLabelCount; i++) {
      const ratio = i / (xLabelCount - 1);
      const t_ = xMin + xRange * ratio;
      const x = padL + ratio * plotW;
      const txt = document.createElementNS(svgNS, 'text');
      txt.setAttribute('class', 'lb-chart-axis-label');
      txt.setAttribute('x', x);
      txt.setAttribute('y', H - 6);
      txt.setAttribute('text-anchor', i === 0 ? 'start' : i === xLabelCount - 1 ? 'end' : 'middle');
      txt.textContent = formatShortDate(t_);
      svg.appendChild(txt);
    }

    // Series polylines + points. Each line gets a stable data-key so
    // legend hover can find it. Points are drawn AFTER lines so circles
    // sit on top of the stroke.
    for (const s of series) {
      const points = s.points;
      const pts = points.map((p) => `${xToPx(p.x).toFixed(1)},${yToPx(p.y).toFixed(1)}`).join(' ');
      const poly = document.createElementNS(svgNS, 'polyline');
      poly.setAttribute('class', 'lb-series-line');
      poly.setAttribute('points', pts);
      poly.setAttribute('stroke', s.color);
      poly.dataset.key = s.key;
      svg.appendChild(poly);
    }
    for (const s of series) {
      for (const p of s.points) {
        // Synthetic reset-zero markers live IN the polyline (so the cliff
        // is visible) but get no dot — they're not data, and a dot at
        // (R, 0) would imply we have a capture at the exact reset moment.
        // Tooltip skips them naturally because their x isn't in
        // ``data.anchors``.
        if (p.synthetic) continue;
        const c = document.createElementNS(svgNS, 'circle');
        c.setAttribute('class', 'lb-series-point');
        c.setAttribute('cx', xToPx(p.x).toFixed(1));
        c.setAttribute('cy', yToPx(p.y).toFixed(1));
        c.setAttribute('r', 2.5);
        c.setAttribute('fill', s.color);
        c.dataset.key = s.key;
        c.dataset.anchor = String(p.x);
        svg.appendChild(c);
      }
    }

    // Hover guide line (initially hidden via 0 opacity).
    const guide = document.createElementNS(svgNS, 'line');
    guide.setAttribute('class', 'lb-chart-guide');
    guide.setAttribute('y1', padT);
    guide.setAttribute('y2', padT + plotH);
    guide.style.opacity = '0';
    svg.appendChild(guide);

    // Mouse-tracking overlay — transparent rect covers the plot area
    // and resolves cursor x → nearest anchor on mousemove.
    const overlay = document.createElementNS(svgNS, 'rect');
    overlay.setAttribute('x', padL);
    overlay.setAttribute('y', padT);
    overlay.setAttribute('width', plotW);
    overlay.setAttribute('height', plotH);
    overlay.setAttribute('fill', 'transparent');
    overlay.style.cursor = 'crosshair';
    svg.appendChild(overlay);

    container.appendChild(svg);

    // Tooltip — DOM, not SVG, so we can use CSS box-shadow etc.
    const tooltip = document.createElement('div');
    tooltip.className = 'lb-chart-tooltip';
    container.appendChild(tooltip);

    function findNearestAnchor(svgX) {
      // SVG X is in viewBox space (0..W); the chart plot spans padL..W-padR.
      // Map back to a time, then snap to the closest anchor.
      const ratio = Math.max(0, Math.min(1, (svgX - padL) / plotW));
      const target = xMin + ratio * xRange;
      let best = anchors[0], bestDist = Infinity;
      for (const a of anchors) {
        const d = Math.abs(a - target);
        if (d < bestDist) { bestDist = d; best = a; }
      }
      return best;
    }

    function valueAt(s, anchor) {
      // O(n) — n is at most ~168 per series, called once per hover.
      for (const p of s.points) if (p.x === anchor) return p;
      return null;
    }

    function clearHighlight() {
      container.classList.remove('lb-chart-hover');
      for (const el of svg.querySelectorAll('.is-active')) el.classList.remove('is-active');
      tooltip.classList.remove('is-visible');
      guide.style.opacity = '0';
    }

    function onMove(evt) {
      // Convert client coords to viewBox coords.
      const r = svg.getBoundingClientRect();
      const sx = ((evt.clientX - r.left) / r.width) * W;
      const anchor = findNearestAnchor(sx);
      const x = xToPx(anchor);
      guide.setAttribute('x1', x);
      guide.setAttribute('x2', x);
      guide.style.opacity = '1';

      // Find which series have a point at this anchor (any may be missing).
      const rows = [];
      for (const s of series) {
        const p = valueAt(s, anchor);
        if (p == null) continue;
        rows.push({ s, p });
      }
      if (!rows.length) {
        tooltip.classList.remove('is-visible');
        return;
      }

      // Highlight the closest series (the one whose y is nearest the cursor)
      // and dim the others. Keeps a busy chart readable.
      const cursorY = ((evt.clientY - r.top) / r.height) * H;
      let bestKey = rows[0].s.key, bestDy = Infinity;
      for (const { s, p } of rows) {
        const dy = Math.abs(yToPx(p.y) - cursorY);
        if (dy < bestDy) { bestDy = dy; bestKey = s.key; }
      }
      container.classList.add('lb-chart-hover');
      for (const el of svg.querySelectorAll('[data-key]')) {
        el.classList.toggle('is-active', el.dataset.key === bestKey);
      }
      // Legend chip mirror.
      if (legendNode) {
        for (const chip of legendNode.querySelectorAll('[data-key]')) {
          chip.classList.toggle('is-muted', chip.dataset.key !== bestKey);
        }
      }

      // Tooltip body — anchor time on top, then each series with its value.
      // Sort rows so the active line is on top (matches the highlight).
      rows.sort((a, b) => (a.s.key === bestKey ? -1 : b.s.key === bestKey ? 1 : 0));
      const lines = rows.map(({ s, p }) => {
        const suffix = opts.tooltipNameSuffix ? opts.tooltipNameSuffix(s, p) : '';
        return `
          <div class="lb-chart-tooltip-row">
            <span class="lb-chart-tooltip-swatch" style="background:${s.color}"></span>
            <span class="lb-chart-tooltip-name">${esc(s.label)}${esc(suffix)}</span>
            <span class="lb-chart-tooltip-value">${esc(formatScore(p.y))}</span>
          </div>`;
      }).join('');
      tooltip.innerHTML = `
        <p class="lb-chart-tooltip-when">${esc(formatAnchor(anchor))}</p>
        ${lines}`;
      // Position in container coordinates (it's the offset parent).
      // Use the rect of the container, not the SVG, since the tooltip is
      // a sibling DOM child of the container.
      const containerRect = container.getBoundingClientRect();
      const px = evt.clientX - containerRect.left;
      tooltip.style.left = `${px}px`;
      tooltip.style.top = `${Math.max(0, yToPx(rows[0].p.y) - 6)}px`;
      tooltip.classList.add('is-visible');
    }

    overlay.addEventListener('mousemove', onMove);
    overlay.addEventListener('mouseleave', () => {
      // Don't fully clear if the cursor moved to a legend chip — that
      // chip's hover handler will take over the active-series state.
      clearHighlight();
      if (legendNode) {
        for (const chip of legendNode.querySelectorAll('[data-key]')) {
          chip.classList.remove('is-muted');
        }
      }
    });

    // Build legend chips beneath the chart. Each chip mirrors the
    // series color and toggles the highlight on hover.
    if (legendNode) {
      legendNode.innerHTML = series.map((s) => `
        <span class="lb-chart-legend-item" data-key="${esc(s.key)}">
          <span class="lb-chart-legend-swatch" style="background:${s.color}"></span>
          ${esc(s.label)}
        </span>
      `).join('');
      for (const chip of legendNode.querySelectorAll('[data-key]')) {
        chip.addEventListener('mouseenter', () => {
          container.classList.add('lb-chart-hover');
          for (const el of svg.querySelectorAll('[data-key]')) {
            el.classList.toggle('is-active', el.dataset.key === chip.dataset.key);
          }
          for (const other of legendNode.querySelectorAll('[data-key]')) {
            other.classList.toggle('is-muted', other.dataset.key !== chip.dataset.key);
          }
        });
        chip.addEventListener('mouseleave', () => {
          clearHighlight();
          for (const other of legendNode.querySelectorAll('[data-key]')) {
            other.classList.remove('is-muted');
          }
        });
      }
    }
  }

  // ─── Chart-only formatting helpers ─────────────────────────────────
  function abbrevScore(v) {
    // Compact axis labels: 12,345 → 12.3k, 1,234,567 → 1.2M.
    const abs = Math.abs(v);
    if (abs >= 1e9) return (v / 1e9).toFixed(abs >= 1e10 ? 0 : 1) + 'B';
    if (abs >= 1e6) return (v / 1e6).toFixed(abs >= 1e7 ? 0 : 1) + 'M';
    if (abs >= 1e3) return (v / 1e3).toFixed(abs >= 1e4 ? 0 : 1) + 'k';
    if (Number.isInteger(v)) return String(v);
    return v.toFixed(1);
  }

  function formatShortDate(unix) {
    // x-axis labels: short form, e.g. "Jun 1 11:00" — same date math
    // as the day picker (trove-day, real UTC - 11h).
    const d = new Date(unix * 1000);
    const months = ['Jan','Feb','Mar','Apr','May','Jun','Jul','Aug','Sep','Oct','Nov','Dec'];
    const pad = (n) => String(n).padStart(2, '0');
    return `${months[d.getUTCMonth()]} ${d.getUTCDate()} ${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}`;
  }

  // ─── Window resize + language re-render ────────────────────────────
  // Debounced resize handler so a drag-resize doesn't recompute the
  // viewBox 60×/s. Both charts re-render with cached payloads.
  let _resizeTimer = null;
  window.addEventListener('resize', () => {
    clearTimeout(_resizeTimer);
    _resizeTimer = setTimeout(() => {
      if (state.boardChart) drawBoardChart();
      if (state.playerChart) drawPlayerChart();
    }, 120);
  });
  // Language switch re-render hooks into the existing dispatch — see
  // wireEvents()'s 'btt-lang-changed' listener (extended below).
  document.addEventListener('btt-lang-changed', () => {
    if (state.boardChart) drawBoardChart();
    if (state.playerChart) drawPlayerChart();
  });
})();
