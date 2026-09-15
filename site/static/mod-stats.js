/* Mod statistics page (/zakros-ui-stats).
 *
 * Fetches /site/mod-stats (the latest daily snapshot plus per-day totals) and
 * renders the totals strip, a downloads-over-time line and a sortable table.
 */
(() => {
  'use strict';

  const { esc } = window.BTTUtil;
  const tr = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);
  const num = (n) => (n == null ? '-' : Number(n).toLocaleString());

  const total = (m) => m.hub.top_release + (m.steam ? m.steam.lifetime_subscriptions : 0);

  const COLUMNS = [
    { key: 'title', label: 'Mod', value: (m) => m.title.toLowerCase() },
    { key: 'hub_top', group: 'hub', label: 'Top release', value: (m) => m.hub.top_release },
    { key: 'hub_7d', group: 'hub', label: '7 days', value: (m) => m.hub.downloads_7d },
    { key: 'hub_stars', group: 'hub', label: 'Stars', value: (m) => m.hub.stars },
    { key: 'ts_downloads', group: 'trovesaurus', label: 'Downloads', value: (m) => m.trovesaurus && m.trovesaurus.downloads },
    { key: 'ts_likes', group: 'trovesaurus', label: 'Likes', value: (m) => m.trovesaurus && m.trovesaurus.likes },
    { key: 'st_subs', group: 'steam', label: 'Subscribers', value: (m) => m.steam && m.steam.subscriptions },
    { key: 'st_lifetime', group: 'steam', label: 'Lifetime', value: (m) => m.steam && m.steam.lifetime_subscriptions },
    { key: 'st_favorites', group: 'steam', label: 'Favorites', value: (m) => m.steam && m.steam.favorites },
    { key: 'st_views', group: 'steam', label: 'Views', value: (m) => m.steam && m.steam.views },
    { key: 'total', label: 'Total', value: total },
  ];
  const GROUPS = { hub: 'Mods Hub', trovesaurus: 'Trovesaurus', steam: 'Steam Workshop' };

  let mods = [];
  let sortKey = 'total';
  let sortDesc = true;

  function renderTotals(t) {
    const all = t.hub_top_release + t.steam_lifetime_subscriptions;
    const cells = [
      ['Total', all, true],
      ['Mods Hub top releases', t.hub_top_release],
      ['Trovesaurus downloads', t.trovesaurus_downloads],
      ['Steam subscribers', t.steam_subscriptions],
      ['Steam lifetime subscriptions', t.steam_lifetime_subscriptions],
    ];
    const el = document.getElementById('ms-totals');
    el.innerHTML = cells.map(([label, value, isAll]) => `
      <div class="ms-total">
        <p class="ms-total-label">${esc(tr(label))}</p>
        <p class="ms-total-value${isAll ? ' is-all' : ''}">${num(value)}</p>
      </div>`).join('');
    el.hidden = false;
  }

  function renderHistory(history) {
    if (history.length < 2) return;
    const W = 800, H = 220, L = 64, R = 12, T = 12, B = 28;
    const values = history.map((d) => d.hub_top_release + d.steam_lifetime_subscriptions);
    const lo = Math.min(...values), hi = Math.max(...values);
    const span = hi - lo || 1;
    const x = (i) => L + (i * (W - L - R)) / (history.length - 1);
    const y = (v) => T + (H - T - B) * (1 - (v - lo) / span);
    const points = values.map((v, i) => `${x(i).toFixed(1)},${y(v).toFixed(1)}`).join(' ');
    const last = history.length - 1;
    const svg = `
      <svg viewBox="0 0 ${W} ${H}" role="img" aria-label="${esc(tr('Total per day'))}">
        <line class="ms-axis" x1="${L}" y1="${H - B}" x2="${W - R}" y2="${H - B}"></line>
        <text class="ms-tick" x="${L - 8}" y="${y(hi) + 4}" text-anchor="end">${num(hi)}</text>
        <text class="ms-tick" x="${L - 8}" y="${y(lo) + 4}" text-anchor="end">${num(lo)}</text>
        <text class="ms-tick" x="${L}" y="${H - 8}">${esc(history[0].day)}</text>
        <text class="ms-tick" x="${W - R}" y="${H - 8}" text-anchor="end">${esc(history[last].day)}</text>
        <polyline class="ms-line" points="${points}"></polyline>
        <circle class="ms-dot" cx="${x(last)}" cy="${y(values[last])}" r="3.5"></circle>
      </svg>`;
    document.getElementById('ms-chart').innerHTML = svg;
    document.getElementById('ms-history').hidden = false;
  }

  function cell(col, m) {
    if (col.key === 'title') {
      return `<td><a href="/mods/${encodeURIComponent(m.handle)}/${encodeURIComponent(m.slug)}">${esc(m.title)}</a></td>`;
    }
    const v = col.value(m);
    if (v == null) return `<td class="is-none">-</td>`;
    const content = col.key === 'ts_downloads'
      ? `<a href="https://trovesaurus.com/mod=${m.trovesaurus.id}" rel="noopener">${num(v)}</a>`
      : col.key === 'st_subs'
        ? `<a href="https://steamcommunity.com/sharedfiles/filedetails/?id=${m.steam.id}" rel="noopener">${num(v)}</a>`
        : num(v);
    return `<td${col.key === 'total' ? ' class="is-total"' : ''}>${content}</td>`;
  }

  function renderTable() {
    const col = COLUMNS.find((c) => c.key === sortKey);
    const sorted = mods.slice().sort((a, b) => {
      const va = col.value(a), vb = col.value(b);
      if (va === vb) return a.title.localeCompare(b.title);
      if (va == null) return 1;
      if (vb == null) return -1;
      return (va < vb ? -1 : 1) * (sortDesc ? -1 : 1);
    });
    const groupRow = ['<th scope="col"></th>']
      .concat(Object.entries(GROUPS).map(([g, label]) => {
        const span = COLUMNS.filter((c) => c.group === g).length;
        return `<th scope="colgroup" colspan="${span}" class="ms-group">${esc(tr(label))}</th>`;
      }))
      .concat('<th scope="col"></th>').join('');
    const headRow = COLUMNS.map((c) => {
      const active = c.key === sortKey;
      const icon = active ? (sortDesc ? 'fa-arrow-down' : 'fa-arrow-up') : '';
      const ariaSort = active ? ` aria-sort="${sortDesc ? 'descending' : 'ascending'}"` : '';
      return `<th scope="col"${ariaSort}><button type="button" class="ms-sort" data-key="${c.key}" aria-pressed="${active}">${esc(tr(c.label))}${icon ? `<i class="fa-solid ${icon}" aria-hidden="true"></i>` : ''}</button></th>`;
    }).join('');
    const body = sorted.map((m) => `<tr>${COLUMNS.map((c) => cell(c, m)).join('')}</tr>`).join('');
    document.getElementById('ms-table').innerHTML = `
      <table class="ms-table">
        <thead><tr>${groupRow}</tr><tr>${headRow}</tr></thead>
        <tbody>${body}</tbody>
      </table>`;
  }

  document.getElementById('ms-table').addEventListener('click', (e) => {
    const btn = e.target.closest('.ms-sort');
    if (!btn) return;
    const key = btn.dataset.key;
    if (key === sortKey) sortDesc = !sortDesc;
    else { sortKey = key; sortDesc = key !== 'title'; }
    renderTable();
    const again = document.querySelector(`.ms-sort[data-key="${key}"]`);
    if (again) again.focus();
  });

  fetch('/site/mod-stats')
    .then((r) => (r.ok ? r.json() : Promise.reject(r.status)))
    .then((data) => {
      if (!data.mods.length) {
        document.getElementById('ms-table').innerHTML = `<div class="ms-loading">${esc(tr('The first snapshot has not been taken yet.'))}</div>`;
        return;
      }
      mods = data.mods;
      document.getElementById('ms-updated').textContent =
        `${tr('Updated')} ${new Date(data.updated_at).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })}`;
      renderTotals(data.totals);
      renderHistory(data.history);
      renderTable();
    })
    .catch(() => {
      document.getElementById('ms-table').innerHTML = `<div class="ms-loading">${esc(tr('Statistics could not be loaded.'))}</div>`;
    });
})();
