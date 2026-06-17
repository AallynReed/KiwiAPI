/* Public player-profile page (/player/<name>).
   Fetches /site/leaderboards/players/<name>/profile (tokenless same-origin) and
   renders the header badge, summary chips, and a recent-appearances table.
   Self-contained; degrades to a friendly empty state on no data / error. */
(function () {
  'use strict';

  const root = document.body;
  const name = (root.dataset.player || '').trim();
  const tr = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);

  function esc(s) {
    return String(s == null ? '' : s).replace(/[&<>"']/g, (c) =>
      ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  }
  function num(n) {
    return (typeof n === 'number' && isFinite(n)) ? n.toLocaleString() : '—';
  }
  function score(n) {
    return (typeof n === 'number' && isFinite(n)) ? Math.round(n).toLocaleString() : '—';
  }
  function when(unix) {
    if (!unix) return '—';
    try { return new Date(unix * 1000).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' }); }
    catch (_) { return '—'; }
  }
  function deltaCell(d) {
    if (d == null || d === 0) return `<span class="pl-delta-flat">·</span>`;
    // rank_delta: positive = climbed (good).
    return d > 0
      ? `<span class="pl-delta-up">▲ ${num(Math.abs(d))}</span>`
      : `<span class="pl-delta-down">▼ ${num(Math.abs(d))}</span>`;
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
      chip(tr('Boards'), num(s.boards_appeared), tr('appeared on')),
      chip(tr('Appearances'), num(s.appearances), tr('recent captures')),
      chip(tr('Last seen'), when(s.latest_anchor), ''),
    ].join('');

    const recent = data.recent || [];
    if (!recent.length) {
      recentEl.innerHTML = `<p class="pl-empty">${esc(tr("This name hasn't appeared on any tracked leaderboard yet."))}</p>`;
      return;
    }
    recentEl.innerHTML = `
      <table class="pl-table">
        <thead>
          <tr>
            <th>${esc(tr('Board'))}</th>
            <th class="pl-num">${esc(tr('Rank'))}</th>
            <th class="pl-num">${esc(tr('Score'))}</th>
            <th class="pl-num">${esc(tr('Δ Rank'))}</th>
            <th class="pl-num">${esc(tr('When'))}</th>
          </tr>
        </thead>
        <tbody>
          ${recent.map((e) => `
            <tr>
              <td>${esc(e.board_name || ('#' + e.leaderboard))}</td>
              <td class="pl-num pl-rank">#${num(e.rank)}</td>
              <td class="pl-num">${score(e.score)}</td>
              <td class="pl-num">${deltaCell(e.rank_delta)}</td>
              <td class="pl-num">${esc(when(e.created_at))}</td>
            </tr>`).join('')}
        </tbody>
      </table>`;
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
