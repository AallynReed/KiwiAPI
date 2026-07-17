/* Trove Store History (/store). Vanilla + inline SVG, CSP-clean.
 * Data comes from the same-origin /site/store/* proxies; in-game art is
 * fetched from /site/store/texture and .dds is decoded via window.decodeDDS
 * (the same module the /updates file viewer uses). */
(function () {
  'use strict';
  const { esc, fetchJSON } = window.BTTUtil;
  const $ = (id) => document.getElementById(id);
  const DAY = 86400;

  const state = {
    tab: 'gallery',
    anchor: null,
    products: [],
    categories: [],
    category: null,      // selected category index (null = all)
    timeline: null,      // lazily loaded
    filters: { q: '', active: true, onSale: false, kind: '' },
    tlKind: '',
  };

  // ── formatting ──────────────────────────────────────────────────────────
  const fmtInt = (n) => (n == null ? '' : Number(n).toLocaleString());
  function fmtDate(ts) {
    if (!ts) return '—';
    return new Date(ts * 1000).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
  }
  function daysAgo(ts) {
    if (!ts) return '';
    const d = Math.floor((Date.now() / 1000 - ts) / DAY);
    if (d <= 0) return 'today';
    if (d === 1) return 'yesterday';
    if (d < 30) return `${d}d ago`;
    if (d < 365) return `${Math.round(d / 30)}mo ago`;
    return `${(d / 365).toFixed(1)}y ago`;
  }
  // In-game currencies show their game icon; real-money currencies show a symbol
  // and are stored in MINOR units (cents), so divide by 100 for display.
  const CURRENCY = {
    TWC: { cls: 'twc', label: 'Credits', icon: 'ui/store/icon_store_credit.png' },
    TWP: { cls: 'twp', label: 'Cubits', icon: 'ui/store/icon_store_cubit.png' },
  };
  const CASH_SYM = { EUR: '€', USD: '$', GBP: '£', CAD: 'CA$', AUD: 'A$',
    BRL: 'R$', RUB: '₽', JPY: '¥', KRW: '₩', CNY: '¥' };
  function fmtCash(currency, cost) {
    const sym = CASH_SYM[currency];
    const amount = (cost / 100).toFixed(2);         // real money is in cents
    return sym ? `${sym}${amount}` : `${amount} ${currency}`;
  }
  function pill(pr) {
    const c = CURRENCY[pr.currency];
    if (c) {
      const amount = pr.monthly ? `${fmtInt(pr.monthly)}/mo` : fmtInt(pr.cost);
      return `<span class="st-pill ${c.cls}" title="${esc(c.label)}"><img class="st-coin" src="${texUrl(c.icon)}" alt="${esc(c.label)}" width="14" height="14" loading="lazy">${esc(amount)}</span>`;
    }
    return `<span class="st-pill cash">${esc(fmtCash(pr.currency, pr.cost))}</span>`;
  }
  function pricePills(p) {
    // Real-money SKU with a pre-formatted string from the game wins (already
    // localised + correctly scaled, e.g. "€1.49").
    if (p.price_string) return `<span class="st-pill cash">${esc(p.price_string)}</span>`;
    if (!p.prices || !p.prices.length) return '<span class="st-pill" style="opacity:.5">—</span>';
    const seen = new Set();
    return p.prices.map((pr) => {
      const key = pr.currency + ':' + pr.cost;
      if (seen.has(key)) return '';
      seen.add(key);
      return pill(pr);
    }).join('');
  }

  // ── in-game texture loading (dds decode / img) ──────────────────────────
  let _ddsReady = null;
  function ensureDDS() {
    if (window.decodeDDS) return Promise.resolve(window.decodeDDS);
    if (_ddsReady) return _ddsReady;
    _ddsReady = new Promise((res) => {
      document.addEventListener('btt-dds-ready', () => res(window.decodeDDS || null), { once: true });
      setTimeout(() => res(window.decodeDDS || null), 4000);
    });
    return _ddsReady;
  }
  function texUrl(path) { return `/site/store/texture?path=${encodeURIComponent(path)}`; }

  // Paint a product's art into `host` (a .st-thumb / .st-d-art element).
  async function paintArt(host, path) {
    host.innerHTML = '<i class="fa-solid fa-store st-thumb-ph"></i>';
    if (!path) return;
    const isDds = /\.dds$/i.test(path);
    if (!isDds) {
      const img = new Image();
      img.loading = 'lazy';
      img.alt = '';
      img.onload = () => { host.innerHTML = ''; host.appendChild(img); };
      img.onerror = () => {};   // keep placeholder
      img.src = texUrl(path);
      return;
    }
    const decodeDDS = await ensureDDS();
    if (!decodeDDS) return;
    try {
      const res = await fetch(texUrl(path), { credentials: 'omit' });
      if (!res.ok) return;
      const buf = await res.arrayBuffer();
      const img = decodeDDS(buf);
      const canvas = document.createElement('canvas');
      canvas.width = img.width; canvas.height = img.height;
      canvas.getContext('2d').putImageData(new ImageData(img.rgba, img.width, img.height), 0, 0);
      host.innerHTML = ''; host.appendChild(canvas);
    } catch (_e) { /* keep placeholder */ }
  }

  // Concurrency-limited art painter so a 300-card grid doesn't fire 300 fetches.
  function makeArtQueue(limit) {
    let active = 0; const q = [];
    const pump = () => {
      while (active < limit && q.length) {
        const job = q.shift(); active++;
        job().finally(() => { active--; pump(); });
      }
    };
    return (host, path) => { q.push(() => paintArt(host, path)); pump(); };
  }
  const enqueueArt = makeArtQueue(6);

  // ── badges ──────────────────────────────────────────────────────────────
  function badges(p) {
    const out = [];
    if (p.active) out.push('<span class="st-badge live" data-i18n>Live</span>');
    else out.push(`<span class="st-badge gone" title="last seen ${esc(fmtDate(p.last_seen))}">${esc(daysAgo(p.last_seen))}</span>`);
    const onSale = (p.prices || []).some((pr) => pr.sale) || p.price_string_sale;
    if (onSale) out.push('<span class="st-badge sale" data-i18n>Sale</span>');
    if (p.deal_expires_at) out.push('<span class="st-badge deal" data-i18n>Deal</span>');
    return out.join('');
  }

  function productImagePath(p) {
    if (p.image) return p.image;
    if (p.textures && p.textures.length) return p.textures[0].texture;
    return null;
  }

  // ── gallery ─────────────────────────────────────────────────────────────
  function cardHTML(p) {
    return `<button class="st-card" data-code="${esc(p.code)}">
      <div class="st-thumb">
        <div class="st-badges">${badges(p)}</div>
        <div class="st-art" data-art="${esc(productImagePath(p) || '')}"><i class="fa-solid fa-store st-thumb-ph"></i></div>
      </div>
      <div class="st-card-meta">
        <div class="st-card-name">${esc(p.name || p.code)}</div>
        <div class="st-price">${pricePills(p)}</div>
      </div>
    </button>`;
  }

  function renderGallery() {
    const grid = $('st-grid'); const empty = $('st-empty');
    let items = state.products;
    if (state.category != null) items = items.filter((p) => (p.categories || []).includes(state.category));
    grid.innerHTML = items.map(cardHTML).join('');
    empty.hidden = items.length > 0;
    // Paint art (concurrency-limited) + wire clicks.
    grid.querySelectorAll('.st-art[data-art]').forEach((el) => {
      const path = el.getAttribute('data-art');
      if (path) enqueueArt(el, path);
    });
    grid.querySelectorAll('.st-card').forEach((el) => {
      el.addEventListener('click', () => openDetail(el.getAttribute('data-code')));
    });
    if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh();
  }

  function renderCategories() {
    const host = $('st-cats');
    const total = state.products.length;
    const rows = [`<button class="st-cat ${state.category == null ? 'active' : ''}" data-cat="">
        <span data-i18n>All packs</span><span class="st-cat-n">${fmtInt(total)}</span></button>`];
    for (const c of state.categories) {
      // Count only within the currently-loaded product set.
      const n = state.products.filter((p) => (p.categories || []).includes(c.index)).length;
      if (!n) continue;
      const label = (c.label || '').replace(/^\$?StoreCategory_/, '').replace(/^\$/, '');
      rows.push(`<button class="st-cat ${state.category === c.index ? 'active' : ''}" data-cat="${c.index}">
        <span>${esc(label)}</span><span class="st-cat-n">${fmtInt(n)}</span></button>`);
    }
    host.innerHTML = rows.join('');
    host.querySelectorAll('.st-cat').forEach((el) => {
      el.addEventListener('click', () => {
        const v = el.getAttribute('data-cat');
        state.category = v === '' ? null : Number(v);
        renderCategories(); renderGallery();
      });
    });
    if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh();
  }

  async function loadGallery() {
    const grid = $('st-grid');
    grid.innerHTML = Array.from({ length: 12 }, () =>
      '<div class="st-card st-skeleton"><div class="st-thumb"></div><div class="st-card-meta"><div class="st-card-name">&nbsp;</div></div></div>').join('');
    const f = state.filters;
    const qs = new URLSearchParams({ active: String(f.active), on_sale: String(f.onSale), limit: '1000' });
    if (f.q) qs.set('q', f.q);
    if (f.kind) qs.set('kind', f.kind);
    try {
      const data = await fetchJSON(`/site/store/products?${qs}`);
      state.products = data.items || [];
      state.anchor = data.anchor;
      renderStats(data.total);
      renderCategories();
      renderGallery();
    } catch (e) {
      grid.innerHTML = `<div class="st-empty"><i class="fa-solid fa-triangle-exclamation"></i><p>Couldn't load the store.</p></div>`;
    }
  }

  function renderStats(total) {
    const host = $('st-stats');
    const live = state.products.filter((p) => p.active).length;
    host.innerHTML = `
      <div class="st-stat"><b>${fmtInt(total != null ? total : state.products.length)}</b><span data-i18n>Packs tracked</span></div>
      <div class="st-stat"><b>${fmtInt(live)}</b><span data-i18n>Live now</span></div>
      <div class="st-stat"><b>${esc(state.anchor ? fmtDate(state.anchor) : '—')}</b><span data-i18n>Last snapshot</span></div>`;
    if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh();
  }

  // ── history timeline (Gantt) ────────────────────────────────────────────
  const TL_LIMIT = 200;
  function renderTimeline() {
    const host = $('st-timeline');
    const tl = state.timeline;
    if (!tl) { host.innerHTML = ''; return; }
    let items = tl.items;
    if (state.tlKind) items = items.filter((it) => it.kind === state.tlKind);
    const shown = items.slice(0, TL_LIMIT);
    const start = tl.span.start, end = tl.span.end;
    const range = Math.max(1, end - start);
    const pct = (t) => `${((t - start) / range) * 100}%`;

    // Year/quarter ticks for the axis.
    const axisTicks = [];
    const sd = new Date(start * 1000), ed = new Date(end * 1000);
    for (let y = sd.getFullYear(); y <= ed.getFullYear(); y++) {
      const t = Math.floor(new Date(y, 0, 1).getTime() / 1000);
      if (t >= start && t <= end) axisTicks.push(`<span>${y}</span>`);
    }
    const axis = `<div class="st-tl-axis"><span>${esc(fmtDate(start))}</span>${axisTicks.join('')}<span>${esc(fmtDate(end))}</span></div>`;

    const rows = shown.map((it) => {
      const bars = (it.availability || []).map((iv) => {
        const left = pct(iv[0]);
        const w = `${Math.max(0.4, ((iv[1] - iv[0]) / range) * 100)}%`;
        const live = it.active && iv === it.availability[it.availability.length - 1];
        return `<div class="st-tl-bar ${live ? 'live' : ''}" style="left:${left};width:${w}"></div>`;
      }).join('');
      const label = esc(it.name || it.code);
      return `<div class="st-tl-row" data-code="${esc(it.code)}" title="${label}">
        <div class="st-tl-label"><span class="st-tl-dot ${it.active ? 'live' : ''}"></span>${label}</div>
        <div class="st-tl-track">${bars}</div>
      </div>`;
    }).join('');

    const more = items.length > TL_LIMIT
      ? `<div class="st-tl-more">Showing the ${TL_LIMIT} most-recent of ${fmtInt(items.length)}. Use the kind filter to narrow.</div>` : '';
    host.innerHTML = axis + rows + more;
    host.querySelectorAll('.st-tl-row').forEach((el) => {
      el.addEventListener('click', () => openDetail(el.getAttribute('data-code')));
    });
  }

  async function loadTimeline() {
    const host = $('st-timeline');
    host.innerHTML = '<div class="st-tl-more">Loading availability history&hellip;</div>';
    try {
      const qs = new URLSearchParams({ limit: '2000' });
      if (state.tlKind) qs.set('kind', state.tlKind);
      state.timeline = await fetchJSON(`/site/store/timeline?${qs}`);
      renderTimeline();
    } catch (e) {
      host.innerHTML = '<div class="st-tl-more">Couldn\'t load the timeline.</div>';
    }
  }

  // ── detail modal ────────────────────────────────────────────────────────
  const modal = () => $('st-modal');
  function closeDetail() { modal().hidden = true; document.body.style.overflow = ''; }
  function openDetail(code) {
    const m = modal(); const body = $('st-modal-body');
    m.hidden = false; document.body.style.overflow = 'hidden';
    body.innerHTML = '<div class="st-tl-more">Loading&hellip;</div>';
    fetchJSON(`/site/store/products/${encodeURIComponent(code)}`).then((p) => {
      body.innerHTML = detailHTML(p);
      const art = body.querySelector('.st-d-art');
      if (art) paintArt(art, productImagePath(p));
      if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh();
    }).catch(() => { body.innerHTML = '<div class="st-tl-more">Couldn\'t load this pack.</div>'; });
  }

  function recordsHTML(r) {
    if (!r) return '';
    const cells = [];
    cells.push(recCell(fmtDate(r.first_seen), 'First seen'));
    cells.push(recCell(r.currently_active ? 'Live now' : daysAgo(r.last_seen), r.currently_active ? 'Status' : 'Last seen'));
    cells.push(recCell(fmtInt(r.total_days_seen), 'Days in store'));
    cells.push(recCell(fmtInt(r.times_available), r.times_available === 1 ? 'Run' : 'Separate runs'));
    if (r.returns) cells.push(recCell(fmtInt(r.returns), 'Times it returned'));
    cells.push(recCell(fmtInt(r.longest_run_days), 'Longest run (days)'));
    if (r.gap_days != null) cells.push(recCell(fmtInt(r.gap_days), 'Days gone'));
    // Cheapest ever (in-game currencies).
    const low = r.price_low || {};
    for (const cur of ['TWC', 'TWP']) {
      if (low[cur] != null && r.price_high && r.price_high[cur] != null && r.price_high[cur] !== low[cur]) {
        cells.push(recCell(`${fmtInt(low[cur])}–${fmtInt(r.price_high[cur])}`, `${(CURRENCY[cur] || {}).label || cur} range`));
      }
    }
    return `<div class="st-records">${cells.join('')}</div>`;
  }
  const recCell = (v, l) => `<div class="st-rec"><b>${esc(v)}</b><span data-i18n>${esc(l)}</span></div>`;

  function detailHTML(p) {
    const desc = p.info ? `<p class="st-d-desc">${esc(p.info)}</p>` : '';
    return `<div class="st-d-head">
        <div class="st-d-art"><i class="fa-solid fa-store st-thumb-ph"></i></div>
        <div class="st-d-title">
          <h2 id="st-modal-name">${esc(p.name || p.code)}</h2>
          <div class="st-price">${pricePills(p)} ${p.active ? '<span class="st-badge live" data-i18n>Live</span>' : `<span class="st-badge gone">${esc(daysAgo(p.last_seen))}</span>`}</div>
          <div class="st-d-code">${esc(p.code)}</div>
          ${desc}
        </div>
      </div>
      ${recordsHTML(p.records)}
      <p class="st-section-h" data-i18n>Availability</p>
      ${availabilitySvg(p.availability, p.active)}
      <p class="st-section-h" data-i18n>Price history</p>
      ${priceSvg(p.price_history)}`;
  }

  // Single-row availability strip for the detail view.
  function availabilitySvg(av, active) {
    av = (av || []).filter((iv) => iv && iv.length === 2);
    if (!av.length) return '<p class="st-hint" data-i18n>No availability recorded yet.</p>';
    const now = Math.floor(Date.now() / 1000);
    const start = av[0][0];
    const end = Math.max(now, av[av.length - 1][1]);
    const range = Math.max(1, end - start);
    const W = 680, H = 46, pad = 4;
    const x = (t) => pad + ((t - start) / range) * (W - 2 * pad);
    const bands = av.map((iv, i) => {
      const live = active && i === av.length - 1;
      const bx = x(iv[0]); const bw = Math.max(2, x(iv[1]) - bx);
      return `<rect class="band ${live ? 'live' : ''}" x="${bx.toFixed(1)}" y="14" width="${bw.toFixed(1)}" height="18" rx="6"></rect>`;
    }).join('');
    const todayX = x(now).toFixed(1);
    return `<svg class="st-chart" viewBox="0 0 ${W} ${H}" preserveAspectRatio="none" role="img">
        ${bands}
        <line class="today" x1="${todayX}" y1="6" x2="${todayX}" y2="40"></line>
        <text class="lbl" x="${pad}" y="44">${esc(fmtDate(start))}</text>
        <text class="lbl" x="${W - pad}" y="44" text-anchor="end">${esc(fmtDate(end))}</text>
      </svg>
      <div class="st-legend">
        <span><i style="background:var(--accent)"></i>In store</span>
        <span><i style="background:var(--accent-green)"></i>Live now</span>
        <span><i style="background:var(--accent-red);height:2px;border-radius:0"></i>Today</span>
      </div>`;
  }

  // Step line of the primary-currency price over time.
  function priceSvg(history) {
    // Flatten history into (ts, cost) points for the dominant in-game currency.
    const pts = [];
    let cur = null;
    for (const pt of (history || [])) {
      const twc = (pt.prices || []).find((x) => x.currency === 'TWC')
        || (pt.prices || []).find((x) => x.currency === 'TWP')
        || (pt.prices || [])[0];
      if (!twc) continue;
      cur = cur || twc.currency;
      if (twc.currency !== cur) continue;
      pts.push([pt.ts, twc.cost]);
    }
    if (pts.length < 2) return `<p class="st-hint" data-i18n>Not enough price points yet — the graph fills in as prices change over time.</p>`;
    const W = 680, H = 160, pad = 30;
    const t0 = pts[0][0], t1 = pts[pts.length - 1][0];
    const tr = Math.max(1, t1 - t0);
    const costs = pts.map((p) => p[1]);
    const lo = Math.min(...costs), hi = Math.max(...costs);
    const vr = Math.max(1, hi - lo);
    const X = (t) => pad + ((t - t0) / tr) * (W - 1.4 * pad);
    const Y = (v) => (H - pad) - ((v - lo) / vr) * (H - 2 * pad);
    // Step path (price holds until the next change).
    let d = `M ${X(pts[0][0]).toFixed(1)} ${Y(pts[0][1]).toFixed(1)}`;
    for (let i = 1; i < pts.length; i++) {
      d += ` L ${X(pts[i][0]).toFixed(1)} ${Y(pts[i - 1][1]).toFixed(1)}`;
      d += ` L ${X(pts[i][0]).toFixed(1)} ${Y(pts[i][1]).toFixed(1)}`;
    }
    const dots = pts.map((p) => `<circle class="dot" cx="${X(p[0]).toFixed(1)}" cy="${Y(p[1]).toFixed(1)}" r="2.5"></circle>`).join('');
    const curLabel = (CURRENCY[cur] || { label: cur }).label;
    return `<svg class="st-chart" viewBox="0 0 ${W} ${H}" role="img">
        <line class="grid" x1="${pad}" y1="${H - pad}" x2="${W - 4}" y2="${H - pad}"></line>
        <line class="grid" x1="${pad}" y1="${pad}" x2="${pad}" y2="${H - pad}"></line>
        <text class="lbl" x="4" y="${pad + 4}">${fmtInt(hi)}</text>
        <text class="lbl" x="4" y="${H - pad}">${fmtInt(lo)}</text>
        <text class="lbl" x="${pad}" y="${H - 6}">${esc(fmtDate(t0))}</text>
        <text class="lbl" x="${W - 4}" y="${H - 6}" text-anchor="end">${esc(fmtDate(t1))}</text>
        <path class="line" d="${d}"></path>${dots}
        <text class="lbl" x="${W - 4}" y="${pad + 4}" text-anchor="end">${esc(curLabel)}</text>
      </svg>`;
  }

  // ── tabs + wiring ───────────────────────────────────────────────────────
  function switchTab(tab) {
    state.tab = tab;
    document.querySelectorAll('.st-tab').forEach((t) => {
      const on = t.getAttribute('data-tab') === tab;
      t.classList.toggle('active', on);
      t.setAttribute('aria-selected', String(on));
    });
    $('st-panel-gallery').hidden = tab !== 'gallery';
    $('st-panel-history').hidden = tab !== 'history';
    if (tab === 'history' && !state.timeline) loadTimeline();
    if (history.replaceState) history.replaceState(null, '', tab === 'history' ? '#history' : '#gallery');
  }

  let _searchT;
  function init() {
    document.querySelectorAll('.st-tab').forEach((t) =>
      t.addEventListener('click', () => switchTab(t.getAttribute('data-tab'))));

    $('st-search').addEventListener('input', (e) => {
      clearTimeout(_searchT);
      state.filters.q = e.target.value.trim();
      _searchT = setTimeout(loadGallery, 250);
    });
    $('st-active-only').addEventListener('change', (e) => { state.filters.active = e.target.checked; loadGallery(); });
    $('st-on-sale').addEventListener('change', (e) => { state.filters.onSale = e.target.checked; loadGallery(); });
    $('st-kind').addEventListener('change', (e) => { state.filters.kind = e.target.value; loadGallery(); });
    $('st-tl-kind').addEventListener('change', (e) => { state.tlKind = e.target.value; loadTimeline(); });

    modal().querySelectorAll('[data-close]').forEach((el) => el.addEventListener('click', closeDetail));
    document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && !modal().hidden) closeDetail(); });

    // categories drive the gallery sidebar; fetch once.
    fetchJSON('/site/store/categories').then((d) => { state.categories = d.items || []; renderCategories(); }).catch(() => {});
    loadGallery();
    if (location.hash === '#history') switchTab('history');
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init);
  else init();
})();
