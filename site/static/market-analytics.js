/* ═══════════════════════════════════════════════════════════════════════
   /market - Analytics tab (Beta)
   ───────────────────────────────────────────────────────────────────────
   A cross-market view layered onto the /market page: biggest movers,
   underpriced deals (flip finder), and a per-item price + supply timeline
   with merchant-event bands. Self-contained IIFE - owns the Browse/Analytics
   view toggle and lazy-loads on first switch, so it never touches market.js's
   state. Backed by /site/market/analytics/*.
   ═══════════════════════════════════════════════════════════════════════ */

(function () {
  'use strict';

  const { esc, fetchJSON } = window.BTTUtil;

  const state = {
    view: 'browse',
    loaded: false,       // analytics data fetched at least once
    days: 14,
    minDiscount: 0.25,
    allItems: null,      // cached /site/market/items for the timeline picker
    timelineItem: null,
  };

  const $ = (id) => document.getElementById(id);
  const $browse = $('mkt-view-browse');
  const $analytics = $('mkt-view-analytics');
  const $days = $('mkt-an-days');
  const $discount = $('mkt-an-discount');
  const $movers = $('mkt-an-movers');
  const $deals = $('mkt-an-deals');
  const $timeline = $('mkt-an-timeline');
  const $itemInput = $('mkt-an-item');
  const $itemSuggest = $('mkt-an-item-suggest');

  boot();

  function boot() {
    for (const tab of document.querySelectorAll('.mkt-viewtab')) {
      tab.addEventListener('click', () => switchView(tab.dataset.view));
    }
    if ($days) $days.addEventListener('change', () => {
      state.days = Number($days.value) || 14;
      loadMovers(); loadDeals();
      if (state.timelineItem) loadTimeline(state.timelineItem);
    });
    if ($discount) $discount.addEventListener('change', () => {
      state.minDiscount = Number($discount.value) || 0.25;
      loadDeals();
    });
    wireItemPicker();
    window.addEventListener('resize', debounce(() => {
      if (state.view === 'analytics' && state.timelineItem && state._lastTimeline) {
        renderTimeline(state._lastTimeline);
      }
    }, 200));
  }

  function switchView(view) {
    if (view === state.view) return;
    state.view = view;
    const analytics = view === 'analytics';
    $browse.hidden = analytics;
    $analytics.hidden = !analytics;
    for (const tab of document.querySelectorAll('.mkt-viewtab')) {
      const on = tab.dataset.view === view;
      tab.classList.toggle('active', on);
      tab.setAttribute('aria-selected', String(on));
    }
    if (analytics && !state.loaded) {
      state.loaded = true;
      loadMovers();
      loadDeals();
    }
  }

  // ─── Movers ────────────────────────────────────────────────────────
  async function loadMovers() {
    $movers.innerHTML = `<p class="mkt-loading" data-i18n>Loading…</p>`;
    rerunI18n();
    try {
      const data = await fetchJSON(`/site/market/analytics/movers?days=${state.days}`);
      renderMovers(data);
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
        const pct = (r.change * 100);
        const sign = pct >= 0 ? '+' : '';
        return `<button type="button" class="mkt-an-mrow" data-item="${esc(r.name)}">
                  <span class="mkt-an-mrow-name">${esc(r.name)}</span>
                  <span class="mkt-an-mrow-nums">
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

  // ─── Deals ─────────────────────────────────────────────────────────
  async function loadDeals() {
    $deals.innerHTML = `<p class="mkt-loading" data-i18n>Loading…</p>`;
    rerunI18n();
    try {
      const data = await fetchJSON(
        `/site/market/analytics/deals?days=${state.days}&min_discount=${state.minDiscount}`);
      renderDeals(data);
    } catch (err) {
      $deals.innerHTML = errorHTML(err); rerunI18n();
    }
  }

  function renderDeals(data) {
    const rows = data.items || [];
    if (!rows.length) {
      $deals.innerHTML = `<p class="mkt-an-empty" data-i18n>No listings are under the median by that much right now.</p>`;
      rerunI18n();
      return;
    }
    const body = rows.map((d) => {
      const pct = (d.discount * 100).toFixed(0);
      return `<tr>
        <td class="mkt-an-deal-name"><button type="button" class="mkt-an-link" data-item="${esc(d.name)}">${esc(d.name)}</button></td>
        <td class="mkt-an-num">${fmtFlux(d.price_each)}</td>
        <td class="mkt-an-num mkt-an-muted">${fmtFlux(d.median_each)}</td>
        <td class="mkt-an-num"><span class="mkt-an-disc">-${pct}%</span></td>
        <td class="mkt-an-num mkt-an-muted">${fmtNum(d.stack)}</td>
        <td class="mkt-an-num mkt-an-muted">${fmtFlux(d.price)}</td>
      </tr>`;
    }).join('');
    $deals.innerHTML = `
      <table class="mkt-an-table">
        <thead><tr>
          <th data-i18n>Item</th>
          <th class="mkt-an-num" data-i18n>Price/ea</th>
          <th class="mkt-an-num" data-i18n>Median/ea</th>
          <th class="mkt-an-num" data-i18n>Discount</th>
          <th class="mkt-an-num" data-i18n>Stack</th>
          <th class="mkt-an-num" data-i18n>Total</th>
        </tr></thead>
        <tbody>${body}</tbody>
      </table>`;
    wireItemLinks($deals);
    rerunI18n();
  }

  // ─── Timeline picker ───────────────────────────────────────────────
  function wireItemPicker() {
    if (!$itemInput) return;
    $itemInput.addEventListener('input', debounce(async () => {
      const q = $itemInput.value.trim().toLowerCase();
      if (!q) { $itemSuggest.hidden = true; return; }
      const items = await ensureItems();
      const hits = items.filter((n) => n.toLowerCase().includes(q)).slice(0, 12);
      $itemSuggest.innerHTML = hits.length
        ? hits.map((n) => `<button type="button" class="mkt-an-suggest-item" data-item="${esc(n)}">${esc(n)}</button>`).join('')
        : `<p class="mkt-an-suggest-empty" data-i18n>No items match.</p>`;
      for (const el of $itemSuggest.querySelectorAll('[data-item]')) {
        el.addEventListener('click', () => {
          $itemInput.value = el.dataset.item;
          $itemSuggest.hidden = true;
          loadTimeline(el.dataset.item);
        });
      }
      $itemSuggest.hidden = false;
      rerunI18n();
    }, 200));
    document.addEventListener('click', (e) => {
      if (!e.target.closest('.mkt-an-item-wrap')) $itemSuggest.hidden = true;
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
    $timeline.innerHTML = `<p class="mkt-loading" data-i18n>Loading…</p>`;
    rerunI18n();
    try {
      const data = await fetchJSON(
        `/site/market/analytics/timeline?name=${encodeURIComponent(name)}&days=${state.days}`);
      state._lastTimeline = data;
      renderTimeline(data);
    } catch (err) {
      $timeline.innerHTML = errorHTML(err); rerunI18n();
    }
  }

  function renderTimeline(data) {
    const pts = data.points || [];
    if (pts.length < 2) {
      $timeline.innerHTML = `<p class="mkt-an-empty">${esc(t('Not enough history for {name} yet.').replace('{name}', data.name))}</p>`;
      return;
    }
    const W = 820, H = 340, m = { l: 58, r: 14, t: 14, b: 58 };
    const pw = W - m.l - m.r;
    const volH = 46, gap = 12;
    const priceH = H - m.t - m.b - volH - gap;
    const priceTop = m.t, priceBot = m.t + priceH;
    const volTop = priceBot + gap, volBot = volTop + volH;

    const xMin = pts[0].bucket;
    const xMax = Math.max(data.now || pts[pts.length - 1].bucket, pts[pts.length - 1].bucket + 43200);
    const xSpan = Math.max(1, xMax - xMin);
    const xToPx = (t2) => m.l + ((t2 - xMin) / xSpan) * pw;

    let pMin = Infinity, pMax = -Infinity, vMax = 0;
    for (const p of pts) {
      pMin = Math.min(pMin, p.p25);
      pMax = Math.max(pMax, p.p75);
      vMax = Math.max(vMax, p.listings);
    }
    if (pMin === pMax) { pMin *= 0.95; pMax *= 1.05 || 1; }
    const pad = (pMax - pMin) * 0.08 || 1;
    pMin = Math.max(0, pMin - pad); pMax += pad;
    const yToPx = (v) => priceBot - ((v - pMin) / (pMax - pMin)) * priceH;
    const vToPx = (v) => volBot - (vMax ? (v / vMax) * volH : 0);

    // Event bands (clipped to the visible range).
    const bands = (data.events || []).map((ev) => {
      const x0 = xToPx(Math.max(ev.starts_at, xMin));
      const x1 = xToPx(Math.min(ev.ends_at, xMax));
      if (x1 <= x0) return '';
      return `<rect x="${x0.toFixed(1)}" y="${priceTop}" width="${(x1 - x0).toFixed(1)}"
                   height="${(volBot - priceTop).toFixed(1)}" class="mkt-an-band"/>
              <text x="${((x0 + x1) / 2).toFixed(1)}" y="${(priceTop + 11).toFixed(1)}"
                    class="mkt-an-band-lbl" text-anchor="middle">${esc(ev.name)}</text>`;
    }).join('');

    // Quartile band (p75 across, then p25 back).
    const top = pts.map((p) => `${xToPx(p.bucket).toFixed(1)},${yToPx(p.p75).toFixed(1)}`);
    const bot = pts.map((p) => `${xToPx(p.bucket).toFixed(1)},${yToPx(p.p25).toFixed(1)}`).reverse();
    const bandPath = `M${top.join(' L')} L${bot.join(' L')} Z`;

    // Median line + dots.
    const med = pts.map((p) => `${xToPx(p.bucket).toFixed(1)},${yToPx(p.p50).toFixed(1)}`);
    const dots = pts.map((p) =>
      `<circle cx="${xToPx(p.bucket).toFixed(1)}" cy="${yToPx(p.p50).toFixed(1)}" r="2.6" class="mkt-an-dot"/>`).join('');

    // Volume bars.
    const barW = Math.max(2, (pw / pts.length) * 0.6);
    const vbars = pts.map((p) => {
      const x = xToPx(p.bucket) - barW / 2;
      const y = vToPx(p.listings);
      return `<rect x="${x.toFixed(1)}" y="${y.toFixed(1)}" width="${barW.toFixed(1)}"
                   height="${(volBot - y).toFixed(1)}" class="mkt-an-vbar"/>`;
    }).join('');

    // Axes: y price ticks (3) + x date ticks (start / mid / end).
    const yticks = [pMin, (pMin + pMax) / 2, pMax].map((v) => {
      const y = yToPx(v);
      return `<line x1="${m.l}" y1="${y.toFixed(1)}" x2="${W - m.r}" y2="${y.toFixed(1)}" class="mkt-an-grid"/>
              <text x="${m.l - 6}" y="${(y + 3).toFixed(1)}" class="mkt-an-ytick" text-anchor="end">${fmtFlux(v)}</text>`;
    }).join('');
    const xticks = [xMin, (xMin + xMax) / 2, xMax].map((tv, i) => {
      const x = xToPx(tv);
      const anchor = i === 0 ? 'start' : i === 2 ? 'end' : 'middle';
      return `<text x="${x.toFixed(1)}" y="${(volBot + 16).toFixed(1)}" class="mkt-an-xtick" text-anchor="${anchor}">${fmtDate(tv)}</text>`;
    }).join('');

    $timeline.innerHTML = `
      <div class="mkt-an-chart-legend">
        <span><i class="mkt-an-key mkt-an-key-med"></i> ${esc(t('Median/ea'))}</span>
        <span><i class="mkt-an-key mkt-an-key-band"></i> ${esc(t('25-75% range'))}</span>
        <span><i class="mkt-an-key mkt-an-key-vol"></i> ${esc(t('New listings'))}</span>
        <span><i class="mkt-an-key mkt-an-key-evt"></i> ${esc(t('Merchant event'))}</span>
      </div>
      <svg viewBox="0 0 ${W} ${H}" class="mkt-an-svg" preserveAspectRatio="xMidYMid meet" role="img"
           aria-label="${esc(t('Price and volume for {name}').replace('{name}', data.name))}">
        ${bands}
        ${yticks}
        <path d="${bandPath}" class="mkt-an-bandfill"/>
        ${vbars}
        <polyline points="${med.join(' ')}" class="mkt-an-medline" fill="none"/>
        ${dots}
        <line x1="${m.l}" y1="${volTop.toFixed(1)}" x2="${W - m.r}" y2="${volTop.toFixed(1)}" class="mkt-an-grid"/>
        ${xticks}
      </svg>
      <p class="mkt-an-chart-meta">${esc(
        t('{name} · {n} day(s) · {b} buckets')
          .replace('{name}', data.name).replace('{n}', data.days).replace('{b}', pts.length))}</p>`;
  }

  // ─── Helpers ───────────────────────────────────────────────────────
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
  function fmtDate(ts) {
    const d = new Date(ts * 1000);
    return `${d.getUTCMonth() + 1}/${d.getUTCDate()}`;
  }
  function t(s) { return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s; }
  function rerunI18n() { if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh(); }
  function debounce(fn, ms) {
    let h;
    return function (...a) { clearTimeout(h); h = setTimeout(() => fn.apply(this, a), ms); };
  }
})();
