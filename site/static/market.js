/* ═══════════════════════════════════════════════════════════════════════
   /market - page logic
   ───────────────────────────────────────────────────────────────────────
   Sidebar of items (filterable client-side), main pane with summary
   stats + paginated listings table for the selected item. Backed by
   the same-origin /site/market/* JSON proxies; no token required.

   URL hash: #item=<name> for deep-linking / refresh-stability.
   ═══════════════════════════════════════════════════════════════════════ */

(function () {
  'use strict';

  const { esc, fetchJSON, apiUrl } = window.BTTUtil;

  const PAGE_SIZE = 100;

  // Sidebar group collapse-state persistence. MUST be declared before
  // ``state`` below - ``loadCollapsed()`` runs in its initializer, and a
  // later ``const`` would still be in the temporal dead zone there (the
  // ReferenceError gets swallowed by the try/catch, silently resetting
  // the saved state on every page load). Keys are prefixed ('c:' + name,
  // or a sentinel) so an admin category can never collide with the
  // system groups.
  const COLLAPSE_KEY = 'mkt-collapsed-groups';
  const OTHER_KEY = '__other__';
  const UNTRACKED_KEY = '__untracked__';

  function loadCollapsed() {
    try {
      return new Set(JSON.parse(localStorage.getItem(COLLAPSE_KEY) || '[]'));
    } catch (_) { return new Set(); }
  }

  const state = {
    items: [],            // [{name}, ...] from /site/market/items
    categories: [],       // [{name, items:[names]}, ...] admin-defined, ordered
    untracked: new Set(), // names with listings but off the scan allow-list
    collapsed: loadCollapsed(),  // Set of collapsed group keys (persisted)
    images: {},           // item name -> blueprint path (thumbnail source)
    imageBranch: 'live-us',
    itemFilter: '',
    selected: null,       // currently selected item name
    summary: null,        // {min/max/avg/median/count/...} for selected
    listings: [],         // accumulated paginated listings
    listingsTotal: 0,
    loadingListings: false,
    // Price-evolution chart state. Cached so the include-expired toggle
    // can swap modes without losing scroll position, and so a language
    // change re-renders without re-fetching.
    chart: {
      name: null,         // item the cached payload is for
      includeExpired: true,
      keepOutliers: false, // default: filter outliers (log-space MAD)
      payload: null,      // {points, window_start, window_end, ...}
      loading: false,
    },
  };

  // ─── DOM refs ──────────────────────────────────────────────────────
  const $ = (id) => document.getElementById(id);
  const $itemSearch = $('mkt-item-search');
  const $items = $('mkt-items');
  const $detailEmpty = $('mkt-detail-empty');
  const $detailBody = $('mkt-detail-body');
  const $detailTitle = $('mkt-detail-title');
  const $detailMeta = $('mkt-detail-meta');
  const $untrackedWarn = $('mkt-untracked-warn');
  const $detailThumb = $('mkt-detail-thumb');
  const $sumCount = $('mkt-sum-count');
  const $sumMedian = $('mkt-sum-median');
  const $sumMin = $('mkt-sum-min');
  const $sumMax = $('mkt-sum-max');
  const $listingsBody = $('mkt-listings-body');
  const $listingsMeta = $('mkt-listings-meta');
  const $listingsFoot = $('mkt-listings-foot');
  const $loadMore = $('mkt-load-more');
  const $sidebar = $('mkt-sidebar');
  const $mobileTrigger = $('mkt-mobile-trigger');
  const $mobileSelected = $('mkt-mobile-selected');
  // Price-evolution chart
  const $chartWrap = $('mkt-chart-wrap');
  const $chart = $('mkt-chart');
  const $chartMeta = $('mkt-chart-meta');
  const $chartToggle = $('mkt-chart-include-expired');
  const $chartOutlierToggle = $('mkt-chart-keep-outliers');

  // ─── Boot ──────────────────────────────────────────────────────────
  init().catch((err) => {
    console.error('[market] boot failed', err);
    $items.innerHTML = errorHTML(err);
  });

  async function init() {
    // Items and their thumbnail map load in parallel; the image map is
    // cosmetic, so a failure there degrades to a plain (image-less) list.
    const [data, imgData] = await Promise.all([
      fetchJSON('/site/market/items'),
      fetchJSON('/site/market/item-images').catch(() => ({ images: {} })),
    ]);
    state.items = (data.items || []).map((name) => ({ name }));
    state.categories = data.categories || [];
    state.untracked = new Set(data.untracked || []);
    state.images = imgData.images || {};
    if (imgData.branch) state.imageBranch = imgData.branch;
    renderItems();

    const hash = parseHash();
    const startName = hash.item && state.items.some((i) => i.name === hash.item)
      ? hash.item
      : null;
    if (startName) await selectItem(startName);

    wireEvents();
  }

  // Thumbnail for an item, reusing the codex blueprint→PNG renderer. Only items
  // we could pin to a codex model have an entry in state.images; the rest render
  // without a thumb. A stale/unrenderable blueprint 404s and the <img> removes
  // its own wrapper via onerror, so the layout never keeps an empty box.
  function thumbHTML(name, size, cls) {
    const bp = state.images[name];
    if (!bp) return '';
    const src = apiUrl('/site/codexes/render?blueprint=' + encodeURIComponent(bp)
      + '&branch=' + encodeURIComponent(state.imageBranch) + '&dim=' + size);
    return `<span class="${cls}"><img loading="lazy" decoding="async" alt=""
      src="${esc(src)}" onerror="this.closest('.${cls}').remove()"></span>`;
  }

  // ─── Items sidebar ─────────────────────────────────────────────────
  function saveCollapsed() {
    try {
      localStorage.setItem(COLLAPSE_KEY, JSON.stringify([...state.collapsed]));
    } catch (_) { /* private mode etc. - collapse just won't persist */ }
  }

  // The display group key an item belongs to. Untracked items (listings
  // stored, but off the scan allow-list) always render under the system
  // "Untracked" group - their category membership is retained server-side,
  // so re-adding them to the allow-list restores their group. Otherwise the
  // first category (in admin order) that lists the name wins; everything
  // else falls into "Other".
  function groupKeyFor(name) {
    if (state.untracked.has(name)) return UNTRACKED_KEY;
    for (const c of state.categories) {
      if (c.items.includes(name)) return 'c:' + c.name;
    }
    return OTHER_KEY;
  }

  function itemButtonHTML(it) {
    const untracked = state.untracked.has(it.name);
    const title = untracked
      ? it.name + ' - ' + t('This item is no longer tracked')
      : it.name;
    return `
      <button type="button" class="mkt-item${it.name === state.selected ? ' active' : ''}${untracked ? ' mkt-item-untracked' : ''}"
              data-name="${esc(it.name)}" title="${esc(title)}">
        ${thumbHTML(it.name, 48, 'mkt-item-thumb')}
        <span class="mkt-item-name">${esc(it.name)}</span>
      </button>`;
  }

  function renderItems() {
    if (!state.items.length) {
      $items.innerHTML = `<p class="mkt-items-empty" data-i18n>No market data captured yet - check back after the next hourly sweep.</p>`;
      rerunI18n();
      return;
    }
    const filter = state.itemFilter.toLowerCase();
    const visible = filter
      ? state.items.filter((it) => it.name.toLowerCase().includes(filter))
      : state.items.slice();

    if (!visible.length) {
      $items.innerHTML = `<p class="mkt-items-empty" data-i18n>No items match that filter.</p>`;
      rerunI18n();
      return;
    }

    // Sort once on the client - the API returns these alphabetised
    // already, but the filter-rendered subset stays stable that way too.
    visible.sort((a, b) => a.name.localeCompare(b.name));

    // Untracked items (off the scan allow-list) always split into a
    // trailing system "Untracked" group, regardless of category.
    const tracked = visible.filter((it) => !state.untracked.has(it.name));
    const untracked = visible.filter((it) => state.untracked.has(it.name));

    // While searching every group renders expanded (a match hidden
    // behind a collapsed header would look like "no result") and the
    // headers become inert; the stored collapse state comes back as
    // soon as the filter clears.
    const groupHTML = (g) => {
      const open = filter ? true : !state.collapsed.has(g.key);
      const label = g.label != null
        ? esc(g.label)
        : (g.key === UNTRACKED_KEY
          ? `<span data-i18n>Untracked</span>`
          : `<span data-i18n>Other</span>`);
      const warnIcon = g.key === UNTRACKED_KEY
        ? `<i class="fa-solid fa-triangle-exclamation mkt-group-warn" aria-hidden="true"></i> `
        : '';
      return `
        <section class="mkt-group${open ? '' : ' collapsed'}">
          <button type="button" class="mkt-group-head" data-group="${esc(g.key)}"
                  aria-expanded="${open}">
            <i class="fa-solid fa-chevron-down mkt-group-chev" aria-hidden="true"></i>
            <span class="mkt-group-name">${warnIcon}${label}</span>
            <span class="mkt-group-count">${g.items.length}</span>
          </button>
          <div class="mkt-group-items"${open ? '' : ' hidden'}>
            ${g.items.map(itemButtonHTML).join('')}
          </div>
        </section>`;
    };

    let html;
    if (!state.categories.length) {
      // No categories defined - plain flat list, exactly the old layout
      // (plus the system Untracked section below when applicable).
      html = tracked.map(itemButtonHTML).join('');
    } else {
      // Split the tracked items into the admin-ordered category groups +
      // a trailing "Other" group. Categories may reference names that are
      // not currently trading - the intersection here just skips them. An
      // item claimed by two categories renders in the first one only.
      const byName = new Map(tracked.map((it) => [it.name, it]));
      const used = new Set();
      const groups = [];
      for (const c of state.categories) {
        const members = c.items.filter((n) => byName.has(n) && !used.has(n));
        if (!members.length) continue;
        for (const n of members) used.add(n);
        groups.push({
          key: 'c:' + c.name,
          label: c.name,
          items: members.map((n) => byName.get(n)),
        });
      }
      const rest = tracked.filter((it) => !used.has(it.name));
      if (rest.length) groups.push({ key: OTHER_KEY, label: null, items: rest });
      html = groups.map(groupHTML).join('');
    }
    if (untracked.length) {
      html += groupHTML({ key: UNTRACKED_KEY, label: null, items: untracked });
    }
    $items.innerHTML = html;

    for (const head of $items.querySelectorAll('[data-group]')) {
      head.addEventListener('click', () => {
        if (state.itemFilter) return;   // inert while searching
        const key = head.dataset.group;
        if (state.collapsed.has(key)) state.collapsed.delete(key);
        else state.collapsed.add(key);
        saveCollapsed();
        renderItems();
      });
    }
    rerunI18n();

    for (const btn of $items.querySelectorAll('[data-name]')) {
      btn.addEventListener('click', () => {
        selectItem(btn.dataset.name);
        $sidebar.classList.remove('open');
        $mobileTrigger.setAttribute('aria-expanded', 'false');
      });
    }
  }

  // ─── Selected item: summary + listings ─────────────────────────────
  async function selectItem(name) {
    if (state.selected === name) return;
    state.selected = name;
    state.summary = null;
    state.listings = [];
    state.listingsTotal = 0;

    // Deep-links (#item=...) can land on an item inside a collapsed
    // group - expand it so the active highlight is actually visible.
    // (In the flat no-categories layout the key never matches a stored
    // collapse entry, so this is a no-op there.)
    const gkey = groupKeyFor(name);
    if (state.collapsed.has(gkey)) {
      state.collapsed.delete(gkey);
      saveCollapsed();
      renderItems();
    }

    // Warn when the item's data has stopped updating (off the allow-list).
    if ($untrackedWarn) $untrackedWarn.hidden = !state.untracked.has(name);

    // Active state in sidebar; mobile-trigger label.
    for (const btn of $items.querySelectorAll('[data-name]')) {
      btn.classList.toggle('active', btn.dataset.name === name);
    }
    if ($mobileSelected) {
      $mobileSelected.removeAttribute('data-i18n');
      $mobileSelected.textContent = name;
      if (window.BTTi18n && window.BTTi18n.untrack) {
        window.BTTi18n.untrack($mobileSelected);
      }
    }

    // Swap empty-state → detail body.
    $detailEmpty.hidden = true;
    $detailBody.hidden = false;
    if ($detailThumb) {
      const bp = state.images[name];
      if (bp) {
        const src = apiUrl('/site/codexes/render?blueprint=' + encodeURIComponent(bp)
          + '&branch=' + encodeURIComponent(state.imageBranch) + '&dim=96');
        $detailThumb.innerHTML = `<img loading="lazy" decoding="async" alt=""
          src="${esc(src)}" onerror="this.closest('#mkt-detail-thumb').hidden = true">`;
        $detailThumb.hidden = false;
      } else {
        $detailThumb.innerHTML = '';
        $detailThumb.hidden = true;
      }
    }
    $detailTitle.textContent = name;
    $detailMeta.textContent = '';
    resetSummary();
    $listingsBody.innerHTML = `<p class="mkt-loading" data-i18n>${t('Loading the latest prices - this can take a moment.')}</p>`;
    $listingsFoot.hidden = true;

    updateHash();

    // Reset chart cache + UI to "loading" so a left-over chart from
    // the previous item doesn't flash before the new one lands.
    state.chart.name = null;
    state.chart.payload = null;
    if ($chartWrap) {
      $chartWrap.hidden = false;
      if ($chart) $chart.innerHTML = `<p class="mkt-chart-loading" data-i18n>${t('Loading…')}</p>`;
      if ($chartMeta) $chartMeta.textContent = '';
    }

    // Three requests in parallel - summary is small, listings is
    // paginated, chart-history is the heaviest (up to 5000 rows for
    // an active item). All independent so we kick them together.
    await Promise.all([
      loadSummary(name),
      loadListings(name, true),
      loadChart(name),
    ]);
  }

  async function loadSummary(name) {
    try {
      const data = await fetchJSON(`/site/market/items/${encodeURIComponent(name)}/summary`);
      state.summary = data;
      renderSummary();
    } catch (err) {
      $sumCount.textContent = $sumMedian.textContent = $sumMin.textContent = $sumMax.textContent = '-';
      // Surface the error inline only if the listings query also fails -
      // a 404 from summary (no active listings) is just "-" four times.
      console.warn('[market] summary fetch failed', err);
    }
  }

  function resetSummary() {
    $sumCount.textContent = '-';
    $sumMedian.textContent = '-';
    $sumMin.textContent = '-';
    $sumMax.textContent = '-';
  }

  function renderSummary() {
    const s = state.summary;
    if (!s) return;
    // Service field names are ``min_each`` / ``max_each`` / ``median_each``
    // (NOT ``*_price_each``). Trips up the eye because the underlying row
    // field IS ``price_each`` - the aggregation just doesn't carry the
    // prefix forward. See app/trove/market/service.py::item_summary.
    if (s.count != null) $sumCount.textContent = formatInt(s.count);
    if (s.median_each != null) $sumMedian.textContent = formatPrice(s.median_each);
    if (s.min_each != null)    $sumMin.textContent    = formatPrice(s.min_each);
    if (s.max_each != null)    $sumMax.textContent    = formatPrice(s.max_each);
  }

  async function loadListings(name, reset) {
    if (state.loadingListings) return;
    state.loadingListings = true;
    if (!reset) $loadMore.disabled = true;
    const offset = reset ? 0 : state.listings.length;
    try {
      const data = await fetchJSON(
        `/site/market/listings?name=${encodeURIComponent(name)}`
        + `&hide_expired=true&sort=%2Bprice_each&limit=${PAGE_SIZE}&offset=${offset}`,
      );
      // Bail if user moved on while the fetch was running.
      if (state.selected !== name) return;
      state.listings = reset
        ? (data.items || [])
        : state.listings.concat(data.items || []);
      state.listingsTotal = data.total || 0;
      renderListings();
    } catch (err) {
      if (!state.listings.length) $listingsBody.innerHTML = errorHTML(err);
    } finally {
      state.loadingListings = false;
      $loadMore.disabled = false;
    }
  }

  function renderListings() {
    if (!state.listings.length) {
      $listingsBody.innerHTML = `<p class="mkt-listings-empty" data-i18n>No active listings for this item right now.</p>`;
      $listingsMeta.textContent = '';
      $listingsFoot.hidden = true;
      rerunI18n();
      return;
    }
    const rows = state.listings.map((l) => `
      <div class="mkt-row">
        <span class="mkt-cell mkt-cell-price">${esc(formatPrice(l.price_each))}</span>
        <span class="mkt-cell mkt-cell-stack">×${esc(formatInt(l.stack))}</span>
        <span class="mkt-cell mkt-cell-total">${esc(formatPrice(l.price))}</span>
        <span class="mkt-cell mkt-cell-when">${esc(formatRelative(l.last_seen))}</span>
      </div>
    `).join('');
    $listingsBody.innerHTML = `
      <div class="mkt-table">
        <div class="mkt-th mkt-cell-price"   data-i18n>Each</div>
        <div class="mkt-th mkt-cell-stack"   data-i18n>Stack</div>
        <div class="mkt-th mkt-cell-total"   data-i18n>Total</div>
        <div class="mkt-th mkt-cell-when"    data-i18n>Last seen</div>
        ${rows}
      </div>`;
    rerunI18n();

    const shown = state.listings.length;
    $listingsMeta.textContent = state.listingsTotal > shown
      ? `${formatInt(shown)} / ${formatInt(state.listingsTotal)} ${t('listings')}`
      : `${formatInt(shown)} ${t('listings')}`;
    $listingsFoot.hidden = shown >= state.listingsTotal;
  }

  // ─── Price-evolution chart ─────────────────────────────────────────
  async function loadChart(name) {
    if (!$chartWrap) return;
    state.chart.name = name;
    state.chart.loading = true;
    const includeExpired = state.chart.includeExpired;
    const keepOutliers   = state.chart.keepOutliers;
    try {
      const data = await fetchJSON(
        `/site/market/items/${encodeURIComponent(name)}/history`
        + `?days=7`
        + `&include_expired=${includeExpired ? 'true' : 'false'}`
        + `&keep_outliers=${keepOutliers ? 'true' : 'false'}`,
      );
      // Bail if the user moved on while the fetch was running.
      if (state.selected !== name) return;
      state.chart.payload = data;
      renderChart();
    } catch (err) {
      console.warn('[market] history fetch failed', err);
      if (state.selected !== name) return;
      if ($chart) $chart.innerHTML = errorHTML(err);
      if ($chartMeta) $chartMeta.textContent = '';
    } finally {
      state.chart.loading = false;
    }
  }

  function renderChart() {
    if (!$chartWrap || !$chart) return;
    const p = state.chart.payload;
    if (!p || !p.points || !p.points.length) {
      // No chartable data. Hide the wrap entirely rather than showing
      // an empty pane - the listings table below still gives the user
      // something useful.
      $chartWrap.hidden = true;
      return;
    }
    $chartWrap.hidden = false;

    drawScatterChart($chart, p.points);

    // Meta line - count + window + truncation hint, plus an outlier
    // sub-message when the filter dropped anything. The price-range
    // info ("between 41M and 50M") gives users a concrete handle on
    // what they're hiding before they decide to toggle outliers back
    // on.
    const from = formatRelative(p.window_start);
    const tmpl = p.truncated
      ? t('{n} listings (capped) · window {from} → now')
      : t('{n} listings · window {from} → now');
    let meta = tmpl
      .replace('{n}', formatInt(p.count))
      .replace('{from}', from);

    if ((p.outliers_excluded || 0) > 0) {
      // Server keeps `outliers_min_price` and `outliers_max_price`
      // around so we can describe the excluded band concretely. If
      // the range collapses (single outlier) we drop the "between"
      // wording and just show the one price.
      const lo = p.outliers_min_price;
      const hi = p.outliers_max_price;
      const range = (lo != null && hi != null)
        ? (lo === hi ? formatPrice(lo) : `${formatPrice(lo)}–${formatPrice(hi)}`)
        : '';
      meta += ' · ' + t('{n} outlier(s) excluded ({range})')
        .replace('{n}', formatInt(p.outliers_excluded))
        .replace('{range}', range || '-');
    }
    $chartMeta.textContent = meta;
  }

  // SVG scatter of (created_at, price_each) with a smoothed median
  // trend line overlaid. No tooltip library - bare DOM + a hover
  // overlay that finds the nearest point and shows a small card.
  function drawScatterChart(container, points) {
    container.innerHTML = '';
    if (!points.length) return;

    const W = Math.max(280, Math.round(container.clientWidth || 600));
    const H = Math.max(160, Math.round(container.clientHeight || 220));
    const padL = 48, padR = 12, padT = 10, padB = 26;
    const plotW = W - padL - padR;
    const plotH = H - padT - padB;

    // X = listing creation time, Y = price_each (linear). For the
    // y-scale we trim extreme outliers via the 1st/99th percentile so
    // a single 9-figure flux post doesn't squash the rest of the cloud
    // into a single horizontal line at y=0.
    const xMin = points[0].created_at;
    const xMax = points[points.length - 1].created_at;
    const xRange = Math.max(1, xMax - xMin);
    const xToPx = (t_) => padL + ((t_ - xMin) / xRange) * plotW;

    const prices = points.map((p) => p.price_each).sort((a, b) => a - b);
    const p01 = prices[Math.floor(prices.length * 0.01)];
    const p99 = prices[Math.floor(prices.length * 0.99)];
    let yMin = Math.max(0, p01);
    let yMax = Math.max(p99, yMin + 1);
    if (yMin === yMax) { yMin = 0; yMax = yMax * 1.2 + 1; }
    const yRange = yMax - yMin;
    const yToPx = (v) => padT + (1 - (Math.min(yMax, Math.max(yMin, v)) - yMin) / yRange) * plotH;

    const svgNS = 'http://www.w3.org/2000/svg';
    const svg = document.createElementNS(svgNS, 'svg');
    svg.setAttribute('viewBox', `0 0 ${W} ${H}`);
    // The viewBox is sized to the container's own pixel dimensions, so a
    // uniform-scale fit fills it exactly. Using the default "xMidYMid meet"
    // (rather than "none") means a transient size change before the debounced
    // re-render letterboxes instead of horizontally stretching the axis text.
    svg.setAttribute('preserveAspectRatio', 'xMidYMid meet');

    // Y grid + labels.
    const yTicks = 4;
    for (let i = 0; i <= yTicks; i++) {
      const v = yMin + (yRange * i) / yTicks;
      const y = yToPx(v);
      const line = document.createElementNS(svgNS, 'line');
      line.setAttribute('class', 'mkt-chart-grid');
      line.setAttribute('x1', padL); line.setAttribute('x2', W - padR);
      line.setAttribute('y1', y); line.setAttribute('y2', y);
      svg.appendChild(line);
      const label = document.createElementNS(svgNS, 'text');
      label.setAttribute('class', 'mkt-chart-axis-label');
      label.setAttribute('x', padL - 6);
      label.setAttribute('y', y + 3);
      label.setAttribute('text-anchor', 'end');
      label.textContent = abbrevPrice(v);
      svg.appendChild(label);
    }
    // X labels - 4 across the window.
    const xLabelCount = 4;
    for (let i = 0; i < xLabelCount; i++) {
      const ratio = i / (xLabelCount - 1);
      const t_ = xMin + xRange * ratio;
      const x = padL + ratio * plotW;
      const txt = document.createElementNS(svgNS, 'text');
      txt.setAttribute('class', 'mkt-chart-axis-label');
      txt.setAttribute('x', x);
      txt.setAttribute('y', H - 6);
      txt.setAttribute('text-anchor',
        i === 0 ? 'start' : (i === xLabelCount - 1 ? 'end' : 'middle'));
      txt.textContent = formatShortDate(t_);
      svg.appendChild(txt);
    }

    // Bucketed median trend line - split the window into N buckets,
    // compute the median price_each per bucket, draw a polyline through
    // the bucket centers. Buckets with no points are skipped.
    const BUCKETS = 24;
    const bucketSize = xRange / BUCKETS;
    const trendPts = [];
    for (let b = 0; b < BUCKETS; b++) {
      const lo = xMin + b * bucketSize;
      const hi = lo + bucketSize;
      const bucket = points.filter((p) => p.created_at >= lo && p.created_at < hi);
      if (!bucket.length) continue;
      const sorted = bucket.map((p) => p.price_each).sort((a, b_) => a - b_);
      const med = sorted[Math.floor(sorted.length / 2)];
      const cx = xToPx(lo + bucketSize / 2);
      const cy = yToPx(med);
      trendPts.push(`${cx.toFixed(1)},${cy.toFixed(1)}`);
    }
    if (trendPts.length >= 2) {
      const trend = document.createElementNS(svgNS, 'polyline');
      trend.setAttribute('class', 'mkt-chart-trend');
      trend.setAttribute('points', trendPts.join(' '));
      svg.appendChild(trend);
    }

    // Scatter - small circle per listing. Recent points are brighter
    // (alpha curve from window_start..xMax) so the eye reads activity
    // direction without a separate legend.
    for (const p of points) {
      const dot = document.createElementNS(svgNS, 'circle');
      const recency = (p.created_at - xMin) / xRange;        // 0..1
      const alpha = 0.30 + recency * 0.70;
      dot.setAttribute('class', 'mkt-chart-dot');
      dot.setAttribute('cx', xToPx(p.created_at).toFixed(1));
      dot.setAttribute('cy', yToPx(p.price_each).toFixed(1));
      dot.setAttribute('r', '2.5');
      dot.setAttribute('opacity', alpha.toFixed(2));
      svg.appendChild(dot);
    }

    container.appendChild(svg);

    // Hover guide + tooltip - same pattern as the leaderboards chart.
    // Mouse-tracking rect resolves cursor → nearest point by squared
    // distance in viewBox space.
    const tooltip = document.createElement('div');
    tooltip.className = 'mkt-chart-tooltip';
    container.appendChild(tooltip);
    const overlay = document.createElementNS(svgNS, 'rect');
    overlay.setAttribute('x', padL); overlay.setAttribute('y', padT);
    overlay.setAttribute('width', plotW); overlay.setAttribute('height', plotH);
    overlay.setAttribute('fill', 'transparent');
    overlay.style.cursor = 'crosshair';
    svg.appendChild(overlay);

    function nearest(svgX, svgY) {
      let best = null, bestD = Infinity;
      for (const p of points) {
        const dx = xToPx(p.created_at) - svgX;
        const dy = yToPx(p.price_each) - svgY;
        const d = dx * dx + dy * dy;
        if (d < bestD) { bestD = d; best = p; }
      }
      return best;
    }
    // ``nearest()`` is an O(n) scan over up to 5000 points, so running it on
    // every raw mousemove would flood the main thread. Coalesce moves into one
    // rAF tick - we only ever resolve the cursor once per frame off the most
    // recent pointer position.
    let rafId = 0;
    let lastX = 0, lastY = 0;
    const paint = () => {
      rafId = 0;
      const r = svg.getBoundingClientRect();
      const sx = ((lastX - r.left) / r.width) * W;
      const sy = ((lastY - r.top) / r.height) * H;
      const hit = nearest(sx, sy);
      if (!hit) return;
      const total = (hit.price != null && hit.stack != null)
        ? `${formatPrice(hit.price_each)} × ${formatInt(hit.stack)} = ${formatPrice(hit.price)}`
        : formatPrice(hit.price_each);
      tooltip.innerHTML = `
        <p class="mkt-chart-tooltip-when">${esc(formatAbsolute(hit.created_at))}</p>
        <p class="mkt-chart-tooltip-row">${esc(total)}</p>`;
      const containerRect = container.getBoundingClientRect();
      tooltip.style.left = `${lastX - containerRect.left}px`;
      tooltip.style.top  = `${yToPx(hit.price_each) - 6}px`;
      tooltip.classList.add('is-visible');
    };
    overlay.addEventListener('mousemove', (evt) => {
      lastX = evt.clientX;
      lastY = evt.clientY;
      if (!rafId) rafId = requestAnimationFrame(paint);
    });
    overlay.addEventListener('mouseleave', () => {
      if (rafId) { cancelAnimationFrame(rafId); rafId = 0; }
      tooltip.classList.remove('is-visible');
    });
  }

  function abbrevPrice(v) {
    if (v >= 1_000_000) return (v / 1_000_000).toFixed(1) + 'M';
    if (v >= 1_000)     return (v / 1_000).toFixed(1) + 'k';
    if (Number.isInteger(v)) return String(v);
    return v.toFixed(1);
  }

  function formatShortDate(unix) {
    const d = new Date(unix * 1000);
    const months = ['Jan','Feb','Mar','Apr','May','Jun','Jul','Aug','Sep','Oct','Nov','Dec'];
    const pad = (n) => String(n).padStart(2, '0');
    return `${months[d.getMonth()]} ${d.getDate()} ${pad(d.getHours())}:00`;
  }

  function formatAbsolute(unix) {
    const d = new Date(unix * 1000);
    const pad = (n) => String(n).padStart(2, '0');
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())} ${tzAbbrev(d)}`;
  }

  function tzAbbrev(d) {
    try {
      const parts = new Intl.DateTimeFormat(undefined, { timeZoneName: 'short' }).formatToParts(d);
      const tz = parts.find((p) => p.type === 'timeZoneName');
      return (tz && tz.value) || 'local';
    } catch (_) { return 'local'; }
  }


  // ─── Events ────────────────────────────────────────────────────────
  function wireEvents() {
    $itemSearch.addEventListener('input', () => {
      state.itemFilter = $itemSearch.value || '';
      renderItems();
    });

    $loadMore.addEventListener('click', () => {
      if (state.selected) loadListings(state.selected, false);
    });

    if ($mobileTrigger) {
      $mobileTrigger.addEventListener('click', () => {
        const open = $sidebar.classList.toggle('open');
        $mobileTrigger.setAttribute('aria-expanded', String(open));
      });
    }

    if ($chartToggle) {
      $chartToggle.addEventListener('change', () => {
        state.chart.includeExpired = $chartToggle.checked;
        if (state.selected) loadChart(state.selected);
      });
    }
    if ($chartOutlierToggle) {
      $chartOutlierToggle.addEventListener('change', () => {
        // "Show outliers" inverts ``keep_outliers``: checked = include
        // the extremes (turn the filter OFF), unchecked = clean cloud
        // (filter ON, the default).
        state.chart.keepOutliers = $chartOutlierToggle.checked;
        if (state.selected) loadChart(state.selected);
      });
    }

    // Debounced resize so the SVG viewBox refits without thrashing.
    let _chartResizeTimer = null;
    window.addEventListener('resize', () => {
      clearTimeout(_chartResizeTimer);
      _chartResizeTimer = setTimeout(() => {
        if (state.chart.payload) renderChart();
      }, 120);
    });

    window.addEventListener('hashchange', async () => {
      const h = parseHash();
      if (h.item && h.item !== state.selected) {
        const exists = state.items.some((it) => it.name === h.item);
        if (exists) await selectItem(h.item);
      }
    });

    document.addEventListener('btt-lang-changed', () => {
      renderItems();
      if (state.summary) renderSummary();
      if (state.listings.length) renderListings();
      if (state.chart.payload) renderChart();
    });
  }

  // ─── URL hash ──────────────────────────────────────────────────────
  function parseHash() {
    const out = { item: null };
    const raw = location.hash.replace(/^#/, '');
    if (!raw) return out;
    const params = new URLSearchParams(raw);
    if (params.has('item')) out.item = params.get('item');
    return out;
  }
  function updateHash() {
    if (state.selected) {
      const next = '#item=' + encodeURIComponent(state.selected);
      history.replaceState(null, '', next);
    }
  }

  // ─── Fetch + util ──────────────────────────────────────────────────
  function errorHTML(err) {
    const msg = (err && err.message) || String(err);
    return `<p class="mkt-error">${esc(t('Failed to load'))}: ${esc(msg)}</p>`;
  }

  function t(s) {
    return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s;
  }
  function rerunI18n() {
    if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh();
  }

  function formatInt(n) {
    return Number(n || 0).toLocaleString();
  }

  function formatPrice(n) {
    n = Number(n || 0);
    // Flux runs into the millions - abbreviate above 1k for table density,
    // but keep the underlying value as title for hover.
    if (n >= 1_000_000) return (n / 1_000_000).toFixed(2) + 'M';
    if (n >= 1_000)     return (n / 1_000).toFixed(1) + 'k';
    if (Number.isInteger(n)) return n.toString();
    return n.toFixed(2);
  }

  function formatRelative(unix) {
    const now = Math.floor(Date.now() / 1000);
    const diff = Math.max(0, now - unix);
    if (diff < 60)         return t('just now');
    if (diff < 3600)       return t('{n}m ago').replace('{n}', Math.floor(diff / 60));
    if (diff < 86400)      return t('{n}h ago').replace('{n}', Math.floor(diff / 3600));
    return t('{n}d ago').replace('{n}', Math.floor(diff / 86400));
  }
})();
