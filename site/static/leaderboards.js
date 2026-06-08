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
  };

  // ─── DOM refs ──────────────────────────────────────────────────────
  const $dayPicker = document.getElementById('lb-day-picker');
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
    // Pull the retention window in parallel with the anchor list so the
    // subtitle settles to the current config value before the user sees
    // the page. Both endpoints are tiny and uncoupled.
    const [stamps, config] = await Promise.all([
      fetchJSON('/site/leaderboards/timestamps'),
      fetchJSON('/site/leaderboards/config').catch(() => null),
    ]);
    state.anchors = stamps.items || [];
    if (config && Number.isFinite(config.hot_retention_days)) {
      state.hotRetentionDays = config.hot_retention_days;
    }
    renderSubtitle();

    buildDays();
    renderDayPicker();

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
            : `<span class="lb-day-meta">${formatTroveTime(d.anchor)}</span>`}
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

  function formatTroveTime(unixAnchor) {
    // "Last pull" time in trove-time. Trove-time = real UTC - 11h.
    const trove = new Date((unixAnchor - TROVE_OFFSET_SECONDS) * 1000);
    const pad = (n) => String(n).padStart(2, '0');
    return `${pad(trove.getUTCHours())}:${pad(trove.getUTCMinutes())}`;
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
  async function loadBoards() {
    $boardList.innerHTML = `<p class="lb-loading">${t('Loading…')}</p>`;
    state.boards = [];
    state.selectedUuid = null;
    resetEntries();
    try {
      const data = await fetchJSON(`/site/leaderboards/boards?created_at=${state.anchor}`);
      state.boards = data.items || [];
    } catch (err) {
      $boardList.innerHTML = errorHTML(err);
      return;
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

    const html = [];
    for (const [cat, boards] of groups) {
      html.push(`<div class="lb-category">${esc(cat)}</div>`);
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
    rerunI18n();
  }

  async function selectBoard(uuid) {
    if (state.selectedUuid === uuid) return;
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

    $entriesBody.innerHTML = `<p class="lb-loading" data-i18n>Loading…</p>`;
    $entriesFoot.hidden = true;
    rerunI18n();
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
      $entriesBody.innerHTML = errorHTML(err);
      $entriesFoot.hidden = true;
    } finally {
      state.loadingEntries = false;
      $loadMore.disabled = false;
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
      return;
    }
    $playerPanel.hidden = false;
    $playerName.textContent = trimmed;
    $playerBody.innerHTML = `<p class="lb-loading" data-i18n>Loading…</p>`;
    rerunI18n();
    $playerPanel.scrollIntoView({ behavior: 'smooth', block: 'nearest' });

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
    });

    $mobileTrigger.addEventListener('click', () => {
      const open = $sidebar.classList.toggle('open');
      $mobileTrigger.setAttribute('aria-expanded', String(open));
    });

    // React to back/forward.
    window.addEventListener('hashchange', async () => {
      const hash = parseHash();
      if (hash.anchor && hash.anchor !== state.anchor) {
        const idx = state.days.findIndex((d) => d.anchor === hash.anchor);
        if (idx >= 0) await selectDay(idx);
      }
      if (hash.board != null && hash.board !== state.selectedUuid) {
        const exists = state.boards.find((b) => b.uuid === hash.board);
        if (exists) selectBoard(hash.board);
      }
    });

    // Re-render the entries pane + day picker + subtitle on language
    // switch so JS-injected chrome (empty hint, table headers, "X
    // entries" count, Today/Yesterday labels, retention number in the
    // subtitle) picks up the new dictionary.
    document.addEventListener('btt-lang-changed', () => {
      renderSubtitle();
      renderDayPicker();
      if (state.selectedUuid) renderEntries();
      else resetEntries();
      renderBoardList();
    });
  }

  // ─── URL hash helpers ──────────────────────────────────────────────
  function parseHash() {
    const out = { anchor: null, board: null };
    const raw = location.hash.replace(/^#/, '');
    if (!raw) return out;
    const params = new URLSearchParams(raw);
    if (params.has('anchor')) out.anchor = Number(params.get('anchor')) || null;
    if (params.has('board')) out.board = Number(params.get('board')) || null;
    return out;
  }

  function updateHash() {
    const parts = [];
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
})();
