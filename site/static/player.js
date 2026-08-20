/* Public player-profile page (/player/<name>).
   Fetches /site/leaderboards/players/<name>/profile (tokenless same-origin) and
   renders the header badge, summary chips, and a recent-appearances table.
   Self-contained; degrades to a friendly empty state on no data / error. */
(function () {
  'use strict';

  const { esc } = window.BTTUtil;

  const root = document.body;
  // data-player is the authoritative source (set server-side). Fall back to the
  // /player/<name> path segment if it's ever absent, so the page still resolves.
  const fromPath = decodeURIComponent((location.pathname.match(/\/player\/(.+)$/) || [])[1] || '');
  const name = (root.dataset.player || fromPath).trim();
  const tr = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);

  function num(n) {
    return (typeof n === 'number' && isFinite(n)) ? n.toLocaleString() : '—';
  }
  function score(n) {
    return (typeof n === 'number' && isFinite(n)) ? Math.round(n).toLocaleString() : '—';
  }
  // A delve board's score is not a number to round - it packs a depth and a run
  // time (see BTTUtil.delveKind), and rounding it would report depth 236 for a
  // player who reached 235. Returns {value, note}: the headline and the muted
  // second line, the latter empty for every ordinary board.
  function boardScore(b) {
    const n = b.latest_score;
    if (typeof n !== 'number' || !isFinite(n)) return { value: '—', note: '' };
    const kind = window.BTTUtil.delveKind(b.leaderboard, b.board_name_id);
    if (kind === 'depth_time') {
      const r = window.BTTUtil.delveReading(n);
      return { value: r.depth.toLocaleString(), note: r.clock };
    }
    if (kind === 'minutes') return { value: window.BTTUtil.delveClock(n), note: '' };
    return { value: score(n), note: '' };
  }
  function when(unix) {
    if (!unix) return '—';
    try { return new Date(unix * 1000).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' }); }
    catch (_) { return '—'; }
  }
  // Initial-letter avatar.
  const avatar = document.getElementById('pl-avatar');
  if (avatar && name) avatar.textContent = name.slice(0, 1).toUpperCase();

  async function load() {
    const statsEl = document.getElementById('pl-stats');
    const recentEl = document.getElementById('pl-recent');
    let data;
    try {
      const r = await fetch(`/site/leaderboards/players/${encodeURIComponent(name)}/profile`, { cache: 'default' });
      if (!r.ok) throw new Error('HTTP ' + r.status);
      data = await r.json();
    } catch (err) {
      recentEl.innerHTML = `<p class="pl-empty">${esc(tr('Could not load this profile right now.'))}</p>`;
      return;
    }

    // Verified badge.
    if (data.verified) {
      const badge = document.getElementById('pl-verified');
      if (badge) badge.hidden = false;
    }

    const s = data.summary || {};
    const bestSub = s.best_rank_board_name
      ? `${tr('on')} ${esc(s.best_rank_board_name)}`
      : '';
    statsEl.innerHTML = [
      chip(tr('Best rank'), s.best_rank != null ? '#' + num(s.best_rank) : '—', bestSub),
      chip(tr('Leaderboards'), num(s.boards_appeared), tr('appeared on')),
      chip(tr('Top 10s'), num(s.top10_count), tr('boards')),
      // "Last played" = last capture their score rose (real activity); distinct
      // from "Last seen" = latest appearance (which is ~always now for anyone on
      // a lifetime board). Shows "—" when no movement in the tracked window.
      chip(tr('Last played'), when(s.last_played), s.last_played ? tr('last score gain') : tr('no recent activity')),
      chip(tr('Last seen'), when(s.latest_anchor), tr('latest appearance')),
    ].join('');

    // Username history + alt clusters (both hide themselves when empty).
    renderAliases(data.renames);
    renderClusters(data.alt_clusters, data.player_name || name);

    // One tile PER LEADERBOARD (best rank ever + current standing), not one per
    // capture - so a player on a board across thousands of captures shows a
    // single card. Grouped into collapsible category sections, ordered by
    // leaderboard id (boards within a group by id; groups by their smallest id).
    // The single "Last seen" summary chip above stands in for per-tile dates.
    const boards = data.boards || [];
    if (!boards.length) {
      recentEl.innerHTML = `<p class="pl-empty">${esc(tr("This name hasn't appeared on any tracked leaderboard yet."))}</p>`;
      return;
    }

    const groups = new Map();
    for (const b of boards) {
      const cat = b.category || tr('Other');
      if (!groups.has(cat)) groups.set(cat, []);
      groups.get(cat).push(b);
    }
    for (const arr of groups.values()) arr.sort((a, b) => a.leaderboard - b.leaderboard);
    const orderedCats = [...groups.keys()].sort(
      (a, b) => groups.get(a)[0].leaderboard - groups.get(b)[0].leaderboard,
    );

    recentEl.innerHTML = orderedCats.map((cat) => {
      const items = groups.get(cat);
      return `
        <details class="pl-group" open>
          <summary class="pl-group-head">
            <i class="fa-solid fa-chevron-down pl-group-caret" aria-hidden="true"></i>
            <span class="pl-group-name">${esc(cat)}</span>
            <span class="pl-group-count">${items.length}</span>
          </summary>
          <div class="pl-grid">${items.map(tile).join('')}</div>
        </details>`;
    }).join('');
  }

  // One board card: icon + name, current rank + score headline, best-rank and
  // appearance count as muted context. No per-tile date (see "Last seen" chip).
  function tile(b) {
    const boardName = b.board_name || ('#' + b.leaderboard);
    const cur = b.latest_rank != null ? '#' + num(b.latest_rank) : '—';
    // Crown reflects the current standing on this board (gold/silver/bronze = #1/2/3).
    const crown = window.BTTUtil.crownHtml(b.latest_rank);
    const sc = boardScore(b);
    return `
      <div class="pl-tile">
        <div class="pl-tile-head">
          ${window.BTTUtil.boardIconImg(b.leaderboard, 'pl-tile-icon')}
          <div class="pl-tile-board">${esc(boardName)}</div>
        </div>
        <div class="pl-tile-stats">
          <span class="pl-tile-rank">${crown}${cur}</span>
          <span class="pl-tile-score">${esc(sc.value)}${
            sc.note ? `<span class="pl-tile-score-note" title="${esc(tr('Run time'))}">${esc(sc.note)}</span>` : ''}</span>
        </div>
        <div class="pl-tile-meta">
          <span>${esc(tr('Best'))} #${num(b.best_rank)}</span>
          <span>${num(b.appearances)}&times; ${esc(tr('seen'))}</span>
        </div>
      </div>`;
  }

  // Username history: the rename chain (A → B → current), each name linking to
  // its own profile. Rendered only when the detector found a rename.
  function renderAliases(r) {
    const el = document.getElementById('pl-aliases');
    if (!el || !r || !r.count) { if (el) el.hidden = true; return; }
    // Build the ordered name chain from the edges (each carries from→to). Falls
    // back to the alias set if edges are somehow absent.
    const chain = [];
    for (const e of (r.edges || [])) {
      if (!chain.length) chain.push(e.from_name);
      if (chain[chain.length - 1] !== e.to_name) chain.push(e.to_name);
    }
    const names = chain.length ? chain : (r.aliases || []);
    const current = (r.current_name || names[names.length - 1] || '').toLowerCase();
    const links = names.map((nm) => {
      const isCur = nm.toLowerCase() === current;
      return `<a class="pl-alias${isCur ? ' pl-alias-current' : ''}" href="/player/${encodeURIComponent(nm)}">${esc(nm)}</a>`;
    }).join('<i class="fa-solid fa-arrow-right pl-alias-arrow" aria-hidden="true"></i>');
    el.innerHTML = `
      <div class="pl-aliases-card">
        <i class="fa-solid fa-clock-rotate-left pl-aliases-icon" aria-hidden="true"></i>
        <div class="pl-aliases-body">
          <p class="pl-aliases-title">${esc(tr('Username history'))}
            <span class="pl-aliases-count">${num(r.count)} ${esc(tr(r.count === 1 ? 'rename' : 'renames'))}</span>
          </p>
          <p class="pl-aliases-chain">${links}</p>
        </div>
      </div>`;
    el.hidden = false;
  }

  // Alt clusters this identity was grouped into by the detector. Compact card per
  // cluster: label + method + confidence, member chips (this player highlighted),
  // and the board count. Only shown when membership was found.
  function renderClusters(clusters, playerName) {
    const el = document.getElementById('pl-clusters');
    if (!el || !clusters || !clusters.length) { if (el) el.hidden = true; return; }
    const me = (playerName || '').toLowerCase();
    const methodLabel = (m) => ({
      co_movement: tr('Lockstep'), name_stem: tr('Name match'),
      schedule: tr('Schedule'), both: tr('Multi-signal'),
    }[m] || m);
    const conf = (c) => (c >= 1 ? '1.00' : (Math.floor(c * 100) / 100).toFixed(2));
    const cards = clusters.map((c) => {
      const members = c.members || [];
      const chips = members.map((nm) => {
        const mine = nm.toLowerCase() === me;
        return `<a class="pl-cl-chip${mine ? ' pl-cl-chip-me' : ''}" href="/player/${encodeURIComponent(nm)}">${esc(nm)}</a>`;
      }).join('');
      const more = c.members_truncated > 0
        ? `<span class="pl-cl-chip pl-cl-more">+${num(c.members_truncated)}</span>` : '';
      const mCount = c.member_count != null ? c.member_count : members.length;
      const bCount = c.board_count != null ? c.board_count : (c.boards || []).length;
      return `
        <div class="pl-cl-card">
          <div class="pl-cl-head">
            <i class="fa-solid fa-people-group pl-cl-icon" aria-hidden="true"></i>
            <span class="pl-cl-label">${esc(c.label || c.stem || tr('Cluster'))}</span>
            <span class="pl-cl-method">${esc(methodLabel(c.method))}</span>
            <span class="pl-cl-conf" title="${esc(tr('Confidence'))}">${conf(c.confidence ?? 0)}</span>
          </div>
          ${c.summary ? `<p class="pl-cl-summary">${esc(c.summary)}</p>` : ''}
          <div class="pl-cl-meta">${num(mCount)} ${esc(tr('accounts'))} · ${num(bCount)} ${esc(tr('boards'))}</div>
          <div class="pl-cl-members">${chips}${more}</div>
        </div>`;
    }).join('');
    el.innerHTML = `
      <h2 class="pl-subhead">${esc(tr('Possible alt accounts'))}</h2>
      <p class="pl-cl-intro">${esc(tr('This name was grouped with these accounts by the alt-cluster detector. Grouping is heuristic - not proof of wrongdoing.'))}</p>
      ${cards}`;
    el.hidden = false;
  }

  function chip(label, value, sub) {
    return `<div class="pl-stat">
      <p class="pl-stat-label">${esc(label)}</p>
      <p class="pl-stat-value">${esc(value)}</p>
      ${sub ? `<p class="pl-stat-sub">${esc(sub)}</p>` : ''}
    </div>`;
  }

  if (name) load();
})();
