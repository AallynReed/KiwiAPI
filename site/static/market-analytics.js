/* ═══════════════════════════════════════════════════════════════════════
   /market - Analytics tab
   ───────────────────────────────────────────────────────────────────────
   A cross-market dashboard layered onto the /market page:
     • Market pulse   - live KPI strip (listings / items / value / top mover / top traded)
     • Biggest movers - median-price risers & fallers, with sample confidence
     • Liquidity      - sell-through % + time-to-sell, from listing lifespans
     • Most traded    - new-listing supply leaders
     • Underpriced    - flip finder with estimated profit (sortable)
     • Price & volume - per-item timeline w/ quartile band, moving average,
                        merchant-event bands, hover tooltips, and a compare overlay
   Self-contained IIFE - owns the Browse/Analytics view toggle and lazy-loads on
   first switch, so it never touches market.js's state. Backed by
   /site/market/analytics/*.
   ═══════════════════════════════════════════════════════════════════════ */

(function () {
  'use strict';

  const { esc, fetchJSON, segmentGaps, debounce } = window.BTTUtil;

  const state = {
    view: 'browse',
    loaded: false,       // analytics data fetched at least once
    days: 14,
    minDiscount: 0.25,
    allItems: null,      // cached /site/market/items for the pickers
    timelineItem: null,
    compareItem: null,
    _lastTimeline: null,
    _lastCompare: null,
    // Cached rows + sort state for the sortable tables.
    deals: { rows: [], sort: { key: 'discount', dir: 'desc' } },
    liquidity: { rows: [], sort: { key: 'sell_through', dir: 'desc' } },
    volume: { rows: [], sort: { key: 'listings', dir: 'desc' } },
  };

  const $ = (id) => document.getElementById(id);
  const $browse = $('mkt-view-browse');
  const $analytics = $('mkt-view-analytics');
  // This file owns the tablist for the whole page, including panels it
  // doesn't render. Keyed by the tab's data-view; a missing element is
  // skipped, so a panel can be dropped from the template without
  // breaking the switcher.
  const PANELS = {
    browse: $browse,
    analytics: $analytics,
    fees: $('mkt-view-fees'),
  };
  const $days = $('mkt-an-days');
  const $discount = $('mkt-an-discount');
  const $pulse = $('mkt-an-pulse');
  const $movers = $('mkt-an-movers');
  const $liquidity = $('mkt-an-liquidity');
  const $volume = $('mkt-an-volume');
  const $deals = $('mkt-an-deals');
  const $timeline = $('mkt-an-timeline');
  const $itemInput = $('mkt-an-item');
  const $itemSuggest = $('mkt-an-item-suggest');
  const $cmpInput = $('mkt-an-item-cmp');
  const $cmpSuggest = $('mkt-an-item-cmp-suggest');

  boot();

  function boot() {
    // Browse/Analytics tablist: click to switch, plus arrow/Home/End keys to
    // roam between tabs (WAI-ARIA tabs pattern). The panels carry
    // role=tabpanel + aria-labelledby in the template.
    const tabs = Array.from(document.querySelectorAll('.mkt-viewtab'));
    for (const tab of tabs) {
      tab.addEventListener('click', () => switchView(tab.dataset.view));
      tab.addEventListener('keydown', (e) => {
        const i = tabs.indexOf(tab);
        let j = -1;
        if (e.key === 'ArrowRight' || e.key === 'ArrowDown') j = (i + 1) % tabs.length;
        else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') j = (i - 1 + tabs.length) % tabs.length;
        else if (e.key === 'Home') j = 0;
        else if (e.key === 'End') j = tabs.length - 1;
        if (j < 0) return;
        e.preventDefault();
        switchView(tabs[j].dataset.view);
        tabs[j].focus();
      });
    }
    if ($days) $days.addEventListener('change', () => {
      state.days = Number($days.value) || 14;
      loadAll();
      if (state.timelineItem) loadTimeline(state.timelineItem);
      if (state.compareItem) loadCompare(state.compareItem);
    });
    if ($discount) $discount.addEventListener('change', () => {
      state.minDiscount = Number($discount.value) || 0.25;
      loadDeals();
    });
    wireItemPicker($itemInput, $itemSuggest, (name) => { loadTimeline(name); });
    wireItemPicker($cmpInput, $cmpSuggest, (name) => { loadCompare(name); });
    window.addEventListener('resize', debounce(() => {
      if (state.view === 'analytics' && state.timelineItem && state._lastTimeline) {
        renderTimeline();
      }
    }, 200));
  }

  function switchView(view) {
    if (view === state.view || !PANELS[view]) return;
    state.view = view;
    for (const key of Object.keys(PANELS)) {
      if (PANELS[key]) PANELS[key].hidden = key !== view;
    }
    for (const tab of document.querySelectorAll('.mkt-viewtab')) {
      const on = tab.dataset.view === view;
      tab.classList.toggle('active', on);
      tab.setAttribute('aria-selected', String(on));
      tab.tabIndex = on ? 0 : -1;
    }
    if (view === 'analytics' && !state.loaded) {
      state.loaded = true;
      loadAll();
    }
    // Panels owned by other files (the Fees tab) can only size their SVGs
    // once they're actually visible, so announce every switch.
    document.dispatchEvent(new CustomEvent('btt-mkt-view', { detail: view }));
  }

  function loadAll() {
    loadOverview();
    loadMovers();
    loadLiquidity();
    loadVolume();
    loadDeals();
  }

  // ─── Market pulse (KPI strip) ──────────────────────────────────────
  async function loadOverview() {
    $pulse.innerHTML = `<p class="mkt-loading" data-i18n>Loading…</p>`;
    rerunI18n();
    try {
      renderPulse(await fetchJSON(`/site/market/analytics/overview?days=${state.days}`));
    } catch (err) {
      $pulse.innerHTML = errorHTML(err); rerunI18n();
    }
  }

  function renderPulse(d) {
    const kpi = (label, icon, value, sub, cls) => `
      <div class="mkt-an-kpi">
        <span class="mkt-an-kpi-label"><i class="fa-solid ${icon}" aria-hidden="true"></i> ${esc(t(label))}</span>
        <span class="mkt-an-kpi-value ${cls || ''}">${value}</span>
        ${sub ? `<span class="mkt-an-kpi-sub">${sub}</span>` : ''}
      </div>`;

    const mover = d.top_mover;
    const moverPct = mover ? mover.change * 100 : 0;
    const moverCard = mover
      ? `<button type="button" class="mkt-an-kpi mkt-an-kpi-btn" data-item="${esc(mover.name)}">
           <span class="mkt-an-kpi-label"><i class="fa-solid fa-arrow-trend-up" aria-hidden="true"></i> ${esc(t('Top mover'))}</span>
           <span class="mkt-an-kpi-value ${moverPct >= 0 ? 'is-up' : 'is-down'}">${moverPct >= 0 ? '+' : ''}${moverPct.toFixed(1)}%</span>
           <span class="mkt-an-kpi-sub">${esc(mover.name)}</span>
         </button>`
      : kpi('Top mover', 'fa-arrow-trend-up', '—', '');

    const traded = d.top_traded;
    const tradedCard = traded
      ? `<button type="button" class="mkt-an-kpi mkt-an-kpi-btn" data-item="${esc(traded.name)}">
           <span class="mkt-an-kpi-label"><i class="fa-solid fa-fire" aria-hidden="true"></i> ${esc(t('Most traded'))}</span>
           <span class="mkt-an-kpi-value">${esc(traded.name)}</span>
           <span class="mkt-an-kpi-sub">${fmtNum(traded.listings)} ${esc(t('new listings'))}</span>
         </button>`
      : kpi('Most traded', 'fa-fire', '—', '');

    $pulse.innerHTML =
      kpi('Active listings', 'fa-tags', fmtNum(d.active_listings), '') +
      kpi('Items on market', 'fa-box', fmtNum(d.active_items), '') +
      kpi('Value posted', 'fa-coins', fmtFlux(d.total_value), t('flux across active listings')) +
      moverCard + tradedCard;
    wireItemLinks($pulse);
    rerunI18n();
  }

  // ─── Movers ────────────────────────────────────────────────────────
  async function loadMovers() {
    $movers.innerHTML = `<p class="mkt-loading" data-i18n>Loading…</p>`;
    rerunI18n();
    try {
      renderMovers(await fetchJSON(`/site/market/analytics/movers?days=${state.days}`));
    } catch (err) {
      $movers.innerHTML = errorHTML(err); rerunI18n();
    }
  }

  function renderMovers(data) {
    const col = (title, rows, up) => {
      if (!rows.length) {
        return `<div class="mkt-an-mcol">
                  <h3 class="mkt-an-mcol-title ${up ? 'is-up' : 'is-down'}">${esc(title)}</h3>
                  <p class="mkt-an-empty" data-i18n>Nothing notable this window.</p>
                </div>`;
      }
      const items = rows.map((r) => {
        const pct = r.change * 100;
        const sign = pct >= 0 ? '+' : '';
        return `<button type="button" class="mkt-an-mrow" data-item="${esc(r.name)}"
                        title="${esc(t('Based on {n} recent listings').replace('{n}', r.recent_n))}">
                  <span class="mkt-an-mrow-name">${esc(r.name)}</span>
                  <span class="mkt-an-mrow-nums">
                    ${confBadge(r.recent_n)}
                    <span class="mkt-an-mrow-med">${fmtFlux(r.recent_med)}</span>
                    <span class="mkt-an-chg ${up ? 'is-up' : 'is-down'}">${sign}${pct.toFixed(1)}%</span>
                  </span>
                </button>`;
      }).join('');
      return `<div class="mkt-an-mcol">
                <h3 class="mkt-an-mcol-title ${up ? 'is-up' : 'is-down'}">
                  <i class="fa-solid ${up ? 'fa-arrow-trend-up' : 'fa-arrow-trend-down'}" aria-hidden="true"></i>
                  ${esc(title)}
                </h3>${items}</div>`;
    };
    $movers.innerHTML =
      col(t('Risers'), data.risers || [], true) +
      col(t('Fallers'), data.fallers || [], false);
    wireItemLinks($movers);
    rerunI18n();
  }

  // ─── Sortable table helper ─────────────────────────────────────────
  // cfg: { columns, rows, sort:{key,dir} }. Each column:
  //   { key, label, num?, sortable?(=true), defDir?('desc'|'asc'),
  //     render(row)->cellHTML, tdClass?(row)->str, sortVal?(row)->number|string }
  function renderTable(el, cfg) {
    const { columns, rows, sort } = cfg;
    const sortCol = columns.find((c) => c.key === sort.key) || columns[0];
    const dir = sort.dir === 'asc' ? 1 : -1;
    const sorted = rows.slice().sort((a, b) => {
      const va = sortCol.sortVal ? sortCol.sortVal(a) : a[sortCol.key];
      const vb = sortCol.sortVal ? sortCol.sortVal(b) : b[sortCol.key];
      if (va == null && vb == null) return 0;
      if (va == null) return 1;                 // nulls always last
      if (vb == null) return -1;
      if (typeof va === 'string') return va.localeCompare(vb) * dir;
      return (va - vb) * dir;
    });

    const thead = columns.map((c) => {
      const active = c.key === sort.key;
      const canSort = c.sortable !== false;
      const caret = active ? (sort.dir === 'asc' ? '▲' : '▼') : '▾';
      const cls = [c.num ? 'mkt-an-num' : '', canSort ? 'mkt-an-sort' : '', active ? 'is-active' : '']
        .filter(Boolean).join(' ');
      // Expose the sort state to assistive tech, and make the header a real
      // <button> so it's keyboard-operable (Enter/Space) rather than a
      // click-only <th>.
      const ariaSort = canSort
        ? ` aria-sort="${active ? (sort.dir === 'asc' ? 'ascending' : 'descending') : 'none'}"`
        : '';
      const inner = canSort
        ? `<button type="button" class="mkt-an-sortbtn" data-sort="${c.key}">${esc(t(c.label))}` +
          `<span class="mkt-an-caret" aria-hidden="true">${caret}</span></button>`
        : esc(t(c.label));
      return `<th class="${cls}"${ariaSort}>${inner}</th>`;
    }).join('');

    const body = sorted.map((r) => '<tr>' + columns.map((c) => {
      const cls = c.tdClass ? c.tdClass(r) : (c.num ? 'mkt-an-num' : '');
      return `<td class="${cls}">${c.render(r)}</td>`;
    }).join('') + '</tr>').join('');

    el.innerHTML = `<table class="mkt-an-table"><thead><tr>${thead}</tr></thead><tbody>${body}</tbody></table>`;
    for (const btn of el.querySelectorAll('.mkt-an-sortbtn[data-sort]')) {
      btn.addEventListener('click', () => {
        const key = btn.dataset.sort;
        const col = columns.find((c) => c.key === key);
        if (sort.key === key) sort.dir = sort.dir === 'asc' ? 'desc' : 'asc';
        else { sort.key = key; sort.dir = col && col.defDir === 'asc' ? 'asc' : 'desc'; }
        renderTable(el, cfg);
      });
    }
    wireItemLinks(el);
    rerunI18n();
  }

  const nameCol = {
    key: 'name', label: 'Item', defDir: 'asc',
    render: (r) => `<button type="button" class="mkt-an-link" data-item="${esc(r.name)}">${esc(r.name)}</button>`,
    tdClass: () => 'mkt-an-deal-name',
  };

  // ─── Liquidity & demand ────────────────────────────────────────────
  async function loadLiquidity() {
    $liquidity.innerHTML = `<p class="mkt-loading" data-i18n>Loading…</p>`;
    rerunI18n();
    try {
      const data = await fetchJSON(`/site/market/analytics/liquidity?days=${state.days}`);
      state.liquidity.rows = data.items || [];
      renderLiquidity();
    } catch (err) {
      $liquidity.innerHTML = errorHTML(err); rerunI18n();
    }
  }

  function renderLiquidity() {
    if (!state.liquidity.rows.length) {
      $liquidity.innerHTML = `<p class="mkt-an-empty" data-i18n>Not enough completed listings yet to estimate sell-through.</p>`;
      rerunI18n();
      return;
    }
    renderTable($liquidity, {
      rows: state.liquidity.rows,
      sort: state.liquidity.sort,
      columns: [
        nameCol,
        { key: 'sell_through', label: 'Sell-through', num: true,
          render: (r) => meter(r.sell_through) },
        { key: 'median_time_to_sell', label: 'Time to sell', num: true,
          render: (r) => `<span class="mkt-an-muted">${fmtDur(r.median_time_to_sell)}</span>` },
        { key: 'sold', label: 'Sold', num: true,
          render: (r) => `<span class="is-up">${fmtNum(r.sold)}</span>` },
        { key: 'expired', label: 'Expired', num: true,
          render: (r) => `<span class="mkt-an-muted">${fmtNum(r.expired)}</span>` },
        { key: 'concluded', label: 'Sample', num: true, sortable: true,
          render: (r) => confBadge(r.concluded) },
      ],
    });
  }

  function meter(frac) {
    const pct = Math.round((frac || 0) * 100);
    return `<span class="mkt-an-meter">
              <span class="mkt-an-meter-track"><span class="mkt-an-meter-fill" style="width:${pct}%"></span></span>
              <span class="mkt-an-meter-pct">${pct}%</span>
            </span>`;
  }

  // ─── Most traded (volume) ──────────────────────────────────────────
  async function loadVolume() {
    $volume.innerHTML = `<p class="mkt-loading" data-i18n>Loading…</p>`;
    rerunI18n();
    try {
      const data = await fetchJSON(`/site/market/analytics/volume?days=${state.days}`);
      state.volume.rows = data.items || [];
      renderVolume();
    } catch (err) {
      $volume.innerHTML = errorHTML(err); rerunI18n();
    }
  }

  function renderVolume() {
    const rows = state.volume.rows;
    if (!rows.length) {
      $volume.innerHTML = `<p class="mkt-an-empty" data-i18n>No listings posted in this window yet.</p>`;
      rerunI18n();
      return;
    }
    const maxListings = Math.max(1, ...rows.map((r) => r.listings));
    renderTable($volume, {
      rows,
      sort: state.volume.sort,
      columns: [
        nameCol,
        { key: 'listings', label: 'New listings', num: true,
          tdClass: () => 'mkt-an-num mkt-an-volcell',
          render: (r) => `<span class="mkt-an-volbar" style="width:${(r.listings / maxListings * 100).toFixed(1)}%"></span><span>${fmtNum(r.listings)}</span>` },
        { key: 'units', label: 'Units', num: true, render: (r) => `<span class="mkt-an-muted">${fmtNum(r.units)}</span>` },
        { key: 'median_each', label: 'Median/ea', num: true, render: (r) => fmtFlux(r.median_each) },
        { key: 'total_value', label: 'Value posted', num: true, render: (r) => `<span class="mkt-an-muted">${fmtFlux(r.total_value)}</span>` },
      ],
    });
  }

  // ─── Deals ─────────────────────────────────────────────────────────
  async function loadDeals() {
    $deals.innerHTML = `<p class="mkt-loading" data-i18n>Loading…</p>`;
    rerunI18n();
    try {
      const data = await fetchJSON(
        `/site/market/analytics/deals?days=${state.days}&min_discount=${state.minDiscount}`);
      state.deals.rows = (data.items || []).map((d) => ({
        ...d, profit: Math.max(0, (d.median_each - d.price_each)) * d.stack,
      }));
      renderDeals();
    } catch (err) {
      $deals.innerHTML = errorHTML(err); rerunI18n();
    }
  }

  function renderDeals() {
    if (!state.deals.rows.length) {
      $deals.innerHTML = `<p class="mkt-an-empty" data-i18n>No listings are under the median by that much right now.</p>`;
      rerunI18n();
      return;
    }
    renderTable($deals, {
      rows: state.deals.rows,
      sort: state.deals.sort,
      columns: [
        nameCol,
        { key: 'price_each', label: 'Price/ea', num: true, render: (r) => fmtFlux(r.price_each) },
        { key: 'median_each', label: 'Median/ea', num: true, render: (r) => `<span class="mkt-an-muted">${fmtFlux(r.median_each)}</span>` },
        { key: 'discount', label: 'Discount', num: true, render: (r) => `<span class="mkt-an-disc">-${(r.discount * 100).toFixed(0)}%</span>` },
        { key: 'profit', label: 'Est. profit', num: true, render: (r) => `<span class="mkt-an-profit" title="${esc(t('If resold at the median'))}">${fmtFlux(r.profit)}</span>` },
        { key: 'stack', label: 'Stack', num: true, render: (r) => `<span class="mkt-an-muted">${fmtNum(r.stack)}</span>` },
        { key: 'price', label: 'Total', num: true, render: (r) => `<span class="mkt-an-muted">${fmtFlux(r.price)}</span>` },
        { key: 'sample_size', label: 'Conf.', num: true, render: (r) => confBadge(r.sample_size) },
      ],
    });
  }

  // ─── Item pickers (timeline + compare) ─────────────────────────────
  function wireItemPicker(input, suggest, onPick) {
    if (!input || !suggest) return;
    // Keep the combobox's aria-expanded in lockstep with the suggestion
    // popup's visibility so screen readers announce the open/closed state.
    const setOpen = (open) => {
      suggest.hidden = !open;
      input.setAttribute('aria-expanded', String(open));
    };
    input.addEventListener('input', debounce(async () => {
      const q = input.value.trim().toLowerCase();
      if (!q) { setOpen(false); return; }
      const items = await ensureItems();
      const hits = items.filter((n) => n.toLowerCase().includes(q)).slice(0, 12);
      suggest.innerHTML = hits.length
        ? hits.map((n) => `<button type="button" class="mkt-an-suggest-item" data-item="${esc(n)}">${esc(n)}</button>`).join('')
        : `<p class="mkt-an-suggest-empty" data-i18n>No items match.</p>`;
      for (const el of suggest.querySelectorAll('[data-item]')) {
        el.addEventListener('click', () => {
          input.value = el.dataset.item;
          setOpen(false);
          onPick(el.dataset.item);
        });
      }
      setOpen(true);
      rerunI18n();
    }, 200));
    input.addEventListener('search', () => {          // native clear (x) button
      if (!input.value.trim() && input === $cmpInput) clearCompare();
    });
    document.addEventListener('click', (e) => {
      if (!e.target.closest('.mkt-an-item-wrap')) setOpen(false);
    });
  }

  async function ensureItems() {
    if (state.allItems) return state.allItems;
    try {
      const data = await fetchJSON('/site/market/items');
      state.allItems = data.items || [];
    } catch (_) { state.allItems = []; }
    return state.allItems;
  }

  function wireItemLinks(root) {
    for (const el of root.querySelectorAll('[data-item]')) {
      el.addEventListener('click', () => {
        const name = el.dataset.item;
        if ($itemInput) $itemInput.value = name;
        loadTimeline(name);
        $timeline.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
      });
    }
  }

  // ─── Timeline chart ────────────────────────────────────────────────
  async function loadTimeline(name) {
    state.timelineItem = name;
    if ($itemInput) $itemInput.value = name;
    $timeline.innerHTML = `<p class="mkt-loading" data-i18n>Loading…</p>`;
    rerunI18n();
    try {
      state._lastTimeline = await fetchJSON(
        `/site/market/analytics/timeline?name=${encodeURIComponent(name)}&days=${state.days}`);
      renderTimeline();
    } catch (err) {
      $timeline.innerHTML = errorHTML(err); rerunI18n();
    }
  }

  async function loadCompare(name) {
    if (!state.timelineItem) {          // no primary yet - promote to primary
      return loadTimeline(name);
    }
    if (name === state.timelineItem) return;
    state.compareItem = name;
    try {
      state._lastCompare = await fetchJSON(
        `/site/market/analytics/timeline?name=${encodeURIComponent(name)}&days=${state.days}`);
      renderTimeline();
    } catch (_) { clearCompare(); }
  }

  function clearCompare() {
    state.compareItem = null;
    state._lastCompare = null;
    if ($cmpInput) $cmpInput.value = '';
    if (state.timelineItem) renderTimeline();
  }

  function renderTimeline() {
    const data = state._lastTimeline;
    if (!data) return;
    const pts = data.points || [];
    if (pts.length < 2) {
      $timeline.innerHTML = `<p class="mkt-an-empty">${esc(t('Not enough history for {name} yet.').replace('{name}', data.name))}</p>`;
      return;
    }
    const cmp = state._lastCompare;
    const compareMode = !!(cmp && (cmp.points || []).length >= 2);
    compareMode ? drawCompare(data, cmp) : drawSingle(data);
  }

  // Chart geometry shared by both modes.
  const GEO = { W: 820, H: 340, m: { l: 58, r: 14, t: 14, b: 58 }, volH: 46, gap: 12 };

  function xScale(data, cmp) {
    const pts = data.points;
    const allBuckets = pts.map((p) => p.bucket).concat(cmp ? cmp.points.map((p) => p.bucket) : []);
    const xMin = Math.min(...allBuckets);
    const last = Math.max(...allBuckets);
    const xMax = Math.max(data.now || last, last + 43200);
    const span = Math.max(1, xMax - xMin);
    const pw = GEO.W - GEO.m.l - GEO.m.r;
    return { xMin, xMax, xToPx: (t2) => GEO.m.l + ((t2 - xMin) / span) * pw, pw };
  }

  function eventBands(data, x, top, bottom) {
    return (data.events || []).map((ev) => {
      const x0 = x.xToPx(Math.max(ev.starts_at, x.xMin));
      const x1 = x.xToPx(Math.min(ev.ends_at, x.xMax));
      if (x1 <= x0) return '';
      return `<rect x="${x0.toFixed(1)}" y="${top}" width="${(x1 - x0).toFixed(1)}"
                   height="${(bottom - top).toFixed(1)}" class="mkt-an-band"/>
              <text x="${((x0 + x1) / 2).toFixed(1)}" y="${(top + 11).toFixed(1)}"
                    class="mkt-an-band-lbl" text-anchor="middle">${esc(ev.name)}</text>`;
    }).join('');
  }

  function xTicks(x, y) {
    return [x.xMin, (x.xMin + x.xMax) / 2, x.xMax].map((tv, i) => {
      const anchor = i === 0 ? 'start' : i === 2 ? 'end' : 'middle';
      return `<text x="${x.xToPx(tv).toFixed(1)}" y="${y.toFixed(1)}" class="mkt-an-xtick" text-anchor="${anchor}">${fmtDate(tv)}</text>`;
    }).join('');
  }

  function movingAvg(pts, key, win) {
    return pts.map((p, i) => {
      const lo = Math.max(0, i - win + 1);
      let s = 0, n = 0;
      for (let j = lo; j <= i; j++) { s += pts[j][key]; n++; }
      return { bucket: p.bucket, v: s / n };
    });
  }

  // --- Single-item view: quartile band + median + MA + volume + events ---
  function drawSingle(data) {
    const pts = data.points;
    const { W, H, m, volH, gap } = GEO;
    const priceH = H - m.t - m.b - volH - gap;
    const priceTop = m.t, priceBot = m.t + priceH;
    const volTop = priceBot + gap, volBot = volTop + volH;
    const x = xScale(data, null);

    let pMin = Infinity, pMax = -Infinity, vMax = 0;
    for (const p of pts) { pMin = Math.min(pMin, p.p25); pMax = Math.max(pMax, p.p75); vMax = Math.max(vMax, p.listings); }
    if (pMin === pMax) { pMin *= 0.95; pMax = pMax * 1.05 || 1; }
    const pad = (pMax - pMin) * 0.08 || 1;
    pMin = Math.max(0, pMin - pad); pMax += pad;
    const yToPx = (v) => priceBot - ((v - pMin) / (pMax - pMin)) * priceH;
    const vToPx = (v) => volBot - (vMax ? (v / vMax) * volH : 0);

    const bands = eventBands(data, x, priceTop, volBot);
    const yticks = [pMin, (pMin + pMax) / 2, pMax].map((v) => {
      const yy = yToPx(v);
      return `<line x1="${m.l}" y1="${yy.toFixed(1)}" x2="${W - m.r}" y2="${yy.toFixed(1)}" class="mkt-an-grid"/>
              <text x="${m.l - 6}" y="${(yy + 3).toFixed(1)}" class="mkt-an-ytick" text-anchor="end">${fmtFlux(v)}</text>`;
    }).join('');

    // A bucket nobody listed in produces no point at all, so band, median and
    // average are all split on the missing buckets: solid inside a run of
    // consecutive buckets, a dashed + dimmed bridge across a hole. Drawing one
    // unbroken line would present an unmeasured stretch as an observed trend.
    const seg = segmentGaps(pts, { x: (p) => p.bucket, step: (data.bucket_hours || 24) * 3600 });
    const runs = seg.runs.filter((r) => r.length >= 2);
    const px = (p, v) => `${x.xToPx(p.bucket).toFixed(1)},${yToPx(v).toFixed(1)}`;

    const bandPath = runs.map((run) => {
      const top = run.map((p) => px(p, p.p75));
      const bot = run.map((p) => px(p, p.p25)).reverse();
      return `M${top.join(' L')} L${bot.join(' L')} Z`;
    }).join(' ');

    const medLines = runs.map((run) =>
      `<polyline points="${run.map((p) => px(p, p.p50)).join(' ')}" class="mkt-an-medline" fill="none"/>`).join('');
    // Point pairs go through join(' '), never a `${x} ${y}` template - the
    // minifier's template lexer can eat that space (see minify_static.py).
    const gapLines = seg.bridges.map(([a, b]) =>
      `<polyline points="${[px(a, a.p50), px(b, b.p50)].join(' ')}" class="mkt-an-medline mkt-an-gapline" fill="none"
        ><title>${esc(t('No listings in this stretch'))}</title></polyline>`).join('');
    const dots = pts.map((p) =>
      `<circle cx="${x.xToPx(p.bucket).toFixed(1)}" cy="${yToPx(p.p50).toFixed(1)}" r="2.6" class="mkt-an-dot"/>`).join('');

    // Moving-average trend (3-bucket trailing) once there's enough to smooth.
    // Averaged per run, never across a hole - a trailing mean that reaches over
    // missing buckets is an average of things that were never measured.
    let maLine = '';
    if (pts.length >= 4) {
      maLine = runs.filter((run) => run.length >= 4).map((run) =>
        `<polyline points="${movingAvg(run, 'p50', 3)
          .map((a) => `${x.xToPx(a.bucket).toFixed(1)},${yToPx(a.v).toFixed(1)}`)
          .join(' ')}" class="mkt-an-maline"/>`).join('');
    }

    const barW = Math.max(2, (x.pw / pts.length) * 0.6);
    const vbars = pts.map((p) => {
      const bx = x.xToPx(p.bucket) - barW / 2, by = vToPx(p.listings);
      return `<rect x="${bx.toFixed(1)}" y="${by.toFixed(1)}" width="${barW.toFixed(1)}" height="${(volBot - by).toFixed(1)}" class="mkt-an-vbar"/>`;
    }).join('');

    const svg = `
      <svg viewBox="0 0 ${W} ${H}" class="mkt-an-svg" preserveAspectRatio="xMidYMid meet" role="img"
           aria-label="${esc(t('Price and volume for {name}').replace('{name}', data.name))}">
        ${bands}${yticks}
        <path d="${bandPath}" class="mkt-an-bandfill"/>
        ${vbars}
        ${medLines}${gapLines}
        ${maLine}${dots}
        <line x1="${m.l}" y1="${volTop.toFixed(1)}" x2="${W - m.r}" y2="${volTop.toFixed(1)}" class="mkt-an-grid"/>
        <line class="mkt-an-hoverline" x1="0" y1="${priceTop}" x2="0" y2="${volBot}" style="opacity:0"/>
        ${xTicks(x, volBot + 16)}
        <rect class="mkt-an-hover-rect" x="${m.l}" y="${priceTop}" width="${x.pw}" height="${(volBot - priceTop).toFixed(1)}" fill="transparent"/>
      </svg>`;

    $timeline.innerHTML = legend(seg.bridges.length > 0) + svg +
      `<div class="mkt-an-tip"></div>` +
      `<p class="mkt-an-chart-meta">${esc(t('{name} · {n} day(s) · {b} buckets')
        .replace('{name}', data.name).replace('{n}', data.days).replace('{b}', pts.length))}</p>`;

    wireHover(pts, x, (p) => tipSingle(p));
    rerunI18n();
  }

  // --- Compare view: two series indexed to 100 at window start ---
  function drawCompare(data, cmp) {
    const { W, H, m } = GEO;
    const priceTop = m.t, priceBot = H - m.b;
    const priceH = priceBot - priceTop;
    const x = xScale(data, cmp);

    const norm = (pts) => {
      const base = pts[0].p50 || 1;
      return pts.map((p) => ({ bucket: p.bucket, idx: (p.p50 / base) * 100, p50: p.p50 }));
    };
    const a = norm(data.points), b = norm(cmp.points);
    let lo = Infinity, hi = -Infinity;
    for (const s of [a, b]) for (const p of s) { lo = Math.min(lo, p.idx); hi = Math.max(hi, p.idx); }
    if (lo === hi) { lo -= 5; hi += 5; }
    const pad = (hi - lo) * 0.1 || 5;
    lo -= pad; hi += pad;
    const yToPx = (v) => priceBot - ((v - lo) / (hi - lo)) * priceH;

    const bands = eventBands(data, x, priceTop, priceBot);
    const yticks = [lo, 100, hi].filter((v, i, arr) => arr.indexOf(v) === i).map((v) => {
      const yy = yToPx(v);
      return `<line x1="${m.l}" y1="${yy.toFixed(1)}" x2="${W - m.r}" y2="${yy.toFixed(1)}" class="mkt-an-grid"/>
              <text x="${m.l - 6}" y="${(yy + 3).toFixed(1)}" class="mkt-an-ytick" text-anchor="end">${Math.round(v)}</text>`;
    }).join('');

    // Both series are drawn gap-aware (see drawSingle): solid over consecutive
    // buckets, dashed + dimmed where an item had no listings to price.
    const step = (data.bucket_hours || 24) * 3600;
    const xy = (p) => `${x.xToPx(p.bucket).toFixed(1)},${yToPx(p.idx).toFixed(1)}`;
    let hasGap = false;
    const line = (s, cls) => {
      const seg = segmentGaps(s, { x: (p) => p.bucket, step });
      if (seg.bridges.length) hasGap = true;
      return seg.runs.filter((r) => r.length >= 2)
          .map((run) => `<polyline points="${run.map(xy).join(' ')}" class="${cls}" fill="none"/>`).join('')
        + seg.bridges.map((pair) =>
          `<polyline points="${pair.map(xy).join(' ')}" class="${cls} mkt-an-gapline" fill="none"
            ><title>${esc(t('No listings in this stretch'))}</title></polyline>`).join('');
    };
    const dots = (s, cls) => s.map((p) => `<circle cx="${x.xToPx(p.bucket).toFixed(1)}" cy="${yToPx(p.idx).toFixed(1)}" r="2.4" class="${cls}"/>`).join('');

    const svg = `
      <svg viewBox="0 0 ${W} ${H}" class="mkt-an-svg" preserveAspectRatio="xMidYMid meet" role="img"
           aria-label="${esc(t('Price comparison'))}">
        ${bands}${yticks}
        ${line(a, 'mkt-an-medline')}${line(b, 'mkt-an-cmpline')}
        ${dots(a, 'mkt-an-dot')}${dots(b, 'mkt-an-cmpdot')}
        <line class="mkt-an-hoverline" x1="0" y1="${priceTop}" x2="0" y2="${priceBot}" style="opacity:0"/>
        ${xTicks(x, priceBot + 16)}
        <rect class="mkt-an-hover-rect" x="${m.l}" y="${priceTop}" width="${x.pw}" height="${priceH.toFixed(1)}" fill="transparent"/>
      </svg>`;

    // `line()` above sets hasGap while the svg literal is built, so this reads
    // the final value.
    $timeline.innerHTML = legendCompare(data.name, cmp.name, hasGap) + svg +
      `<div class="mkt-an-tip"></div>` +
      `<p class="mkt-an-chart-meta">${esc(t('Indexed to 100 at window start · higher = bigger gain'))}</p>`;

    // Hover keyed on primary buckets; tooltip pulls the nearest compare bucket too.
    wireHover(a, x, (p) => tipCompare(p, a, b, data.name, cmp.name));
    rerunI18n();
  }

  // ``hasGap`` adds the dashed "No data" key - only shown when the chart
  // actually bridges a hole, so the legend never explains an absent treatment.
  function legend(hasGap) {
    return `<div class="mkt-an-chart-legend">
        <span><i class="mkt-an-key mkt-an-key-med"></i> ${esc(t('Median/ea'))}</span>
        <span><i class="mkt-an-key mkt-an-key-band"></i> ${esc(t('25-75% range'))}</span>
        <span><i class="mkt-an-key mkt-an-key-ma"></i> ${esc(t('3-day average'))}</span>
        <span><i class="mkt-an-key mkt-an-key-vol"></i> ${esc(t('New listings'))}</span>
        <span><i class="mkt-an-key mkt-an-key-evt"></i> ${esc(t('Merchant event'))}</span>
        ${hasGap ? `<span><i class="mkt-an-key mkt-an-key-gap"></i> ${esc(t('No data'))}</span>` : ''}
      </div>`;
  }

  function legendCompare(nameA, nameB, hasGap) {
    return `<div class="mkt-an-chart-legend">
        <span><i class="mkt-an-key mkt-an-key-med"></i> ${esc(nameA)}</span>
        <span><i class="mkt-an-key mkt-an-key-cmp"></i> ${esc(nameB)}</span>
        <span><i class="mkt-an-key mkt-an-key-evt"></i> ${esc(t('Merchant event'))}</span>
        ${hasGap ? `<span><i class="mkt-an-key mkt-an-key-gap"></i> ${esc(t('No data'))}</span>` : ''}
      </div>`;
  }

  // Nearest-bucket hover: guide line + tooltip. `series` is the list the cursor
  // snaps to (objects with .bucket); `tipHTML(point)` builds the card body.
  function wireHover(series, x, tipHTML) {
    const svg = $timeline.querySelector('.mkt-an-svg');
    const tip = $timeline.querySelector('.mkt-an-tip');
    const rect = svg && svg.querySelector('.mkt-an-hover-rect');
    const guide = svg && svg.querySelector('.mkt-an-hoverline');
    if (!svg || !tip || !rect) return;

    const move = (evt) => {
      const r = svg.getBoundingClientRect();
      const sx = ((evt.clientX - r.left) / r.width) * GEO.W;   // viewBox space
      let best = null, bestD = Infinity;
      for (const p of series) {
        const d = Math.abs(x.xToPx(p.bucket) - sx);
        if (d < bestD) { bestD = d; best = p; }
      }
      if (!best) return;
      const px = x.xToPx(best.bucket);
      if (guide) { guide.setAttribute('x1', px); guide.setAttribute('x2', px); guide.style.opacity = '1'; }
      tip.innerHTML = tipHTML(best);
      // Position the tip in container pixels (viewBox px → client px via width ratio).
      const wrap = $timeline.getBoundingClientRect();
      const scale = r.width / GEO.W;
      tip.style.left = `${(r.left - wrap.left) + px * scale}px`;
      tip.style.top = `${(r.top - wrap.top) + GEO.m.t * scale}px`;
      tip.classList.add('is-visible');
    };
    rect.addEventListener('mousemove', move);
    rect.addEventListener('mouseleave', () => {
      tip.classList.remove('is-visible');
      if (guide) guide.style.opacity = '0';
    });
  }

  function tipSingle(p) {
    return `<p class="mkt-an-tip-when">${esc(fmtFullDate(p.bucket))}</p>
      <div class="mkt-an-tip-row"><span>${esc(t('Median'))}</span><b>${fmtFlux(p.p50)}</b></div>
      <div class="mkt-an-tip-row"><span>${esc(t('Range'))}</span><b>${fmtFlux(p.p25)}–${fmtFlux(p.p75)}</b></div>
      <div class="mkt-an-tip-row"><span>${esc(t('New listings'))}</span><b>${fmtNum(p.listings)}</b></div>`;
  }

  function tipCompare(pa, a, b, nameA, nameB) {
    const nearest = (list, bucket) => list.reduce((best, p) =>
      (best === null || Math.abs(p.bucket - bucket) < Math.abs(best.bucket - bucket)) ? p : best, null);
    const pb = nearest(b, pa.bucket);
    const row = (name, color, p) => `<div class="mkt-an-tip-row">
        <span><i class="mkt-an-tip-dot" style="background:${color}"></i>${esc(name)}</span>
        <b>${p ? Math.round(p.idx) + ' · ' + fmtFlux(p.p50) : '—'}</b></div>`;
    return `<p class="mkt-an-tip-when">${esc(fmtFullDate(pa.bucket))}</p>
      ${row(nameA, 'var(--an-med)', pa)}${row(nameB, 'var(--an-cmp)', pb)}`;
  }

  // ─── Helpers ───────────────────────────────────────────────────────
  function confBadge(n) {
    n = Number(n || 0);
    const cls = n < 5 ? 'is-low' : n >= 20 ? 'is-high' : '';
    return `<span class="mkt-an-conf ${cls}" title="${esc(t('Sample size'))}">n=${fmtNum(n)}</span>`;
  }
  function errorHTML(err) {
    return `<p class="mkt-an-empty">${esc(t('Failed to load'))}: ${esc((err && err.message) || err)}</p>`;
  }
  function fmtFlux(n) {
    n = Number(n || 0);
    if (n >= 1_000_000) return (n / 1_000_000).toFixed(2).replace(/\.?0+$/, '') + 'M';
    if (n >= 1_000) return (n / 1_000).toFixed(1).replace(/\.0$/, '') + 'k';
    return Math.round(n).toLocaleString();
  }
  function fmtNum(n) { return Number(n || 0).toLocaleString(); }
  function fmtDur(s) {
    if (s == null) return '—';
    const h = s / 3600;
    if (h < 1) return `${Math.max(1, Math.round(s / 60))}m`;
    if (h < 48) return `${h < 10 ? h.toFixed(1).replace(/\.0$/, '') : Math.round(h)}h`;
    return `${(h / 24).toFixed(1).replace(/\.0$/, '')}d`;
  }
  function fmtDate(ts) {
    const d = new Date(ts * 1000);
    return `${d.getMonth() + 1}/${d.getDate()}`;
  }
  function fmtFullDate(ts) {
    const d = new Date(ts * 1000);
    return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
  }
  function t(s) { return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s; }
  function rerunI18n() { if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh(); }
})();
