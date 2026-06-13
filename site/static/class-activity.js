/* ===========================================================================
   Class Activity page (/class-activity)

   Two same-origin JSON proxies (both derived from the leaderboard captures):
     • /site/leaderboards/class-activity/series?period=… - per-class bucketed
       activity-level series for the multi-line chart (shared x `buckets` +
       per-class `values` aligned to it).
     • /site/leaderboards/class-activity/current - latest window's per-class
       active counts + share, for the donut.

   Charts are hand-rolled SVG (no library), matching the /activity page style.
   Each class gets a fixed color from PALETTE so the line + legend + donut agree.
   =========================================================================== */
(function () {
  'use strict';

  const PERIODS = ['1d', '7d', '1m', '3m', '6m', '1y', 'all'];

  // Per-class line colors, indexed by class_index (classes.json source order,
  // alphabetical: 0=Bard … 17=Vanguardian). Hand-picked for separation on the
  // dark chart; the short code is the class's in-game abbreviation.
  // NOTE: Bard (BD) and Solarion (SL) are both yellow by request - their lines
  // are intentionally the same colour.
  const CLASS_COLORS = [
    '#ffd166', // 0  BD Bard          - yellow
    '#9a6a3c', // 1  BR Boomeranger   - brown
    '#e879f9', // 2  CB Candy Barbarian - magenta (filled)
    '#86efac', // 3  CM Chloromancer  - light green
    '#2f9e44', // 4  DT Dino Tamer    - dark green
    '#ff8a3d', // 5  DL Dracolyte     - orange
    '#5eead4', // 6  FT Fae Trickster - mint
    '#58a6ff', // 7  GS Gunslinger    - blue
    '#22d3ee', // 8  IS Ice Sage      - cyan (filled)
    '#cbd5e1', // 9  KT Knight        - light gray
    '#6b7280', // 10 LL Lunar Lancer  - dark gray
    '#f472b6', // 11 NN Neon Ninja    - pink
    '#a3e635', // 12 PC Pirate Captain - lime (filled)
    '#f04438', // 13 RV Revenant      - red
    '#818cf8', // 14 SH Shadow Hunter - indigo (filled)
    '#ffd166', // 15 SL Solarion      - yellow (same as Bard, by request)
    '#9333ea', // 16 TR Tomb Raiser   - purple
    '#f8fafc', // 17 VG Vanguardian   - white
  ];
  // Generic fallback hues for any class beyond CLASS_COLORS (a future addition),
  // so color(i) is always defined and index-stable.
  const PALETTE = [
    '#5dd078', '#58a6ff', '#ff8a3d', '#ffd166', '#a371f7', '#f85149',
    '#2dd4bf', '#f472b6', '#facc15', '#38bdf8', '#fb923c', '#c084fc',
    '#4ade80', '#f87171', '#60a5fa', '#fbbf24', '#34d399', '#e879f9',
  ];
  const color = (i) =>
    CLASS_COLORS[i] || PALETTE[((i % PALETTE.length) + PALETTE.length) % PALETTE.length];

  const state = {
    period: '7d',
    view: 'clean',        // 'clean' (Power-Rank-filtered, default) | 'raw' (everyone)
    series: {},           // period -> payload cache
    current: null,        // series payload currently drawn (for resize redraw)
    donut: null,          // /current payload (for view-toggle redraw, no refetch)
    hidden: new Set(),    // class_index values toggled off in the legend
  };

  // Per-class line values for the active view: clean uses the Power-Rank-filtered
  // array (falls back to raw if a payload predates the clean field).
  function viewValues(cls) {
    if (state.view === 'clean') return cls.values_clean || cls.values || [];
    return cls.values || [];
  }
  // Per-class current count + share for the active view.
  function viewCount(c) {
    return state.view === 'clean' ? c.active_players_clean : c.active_players;
  }
  function viewShare(c) {
    return state.view === 'clean' ? c.share_clean : c.share;
  }

  // ─── i18n + fetch + util ───────────────────────────────────────────
  function t(s) { return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s; }
  function rerunI18n() { if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh(); }
  function esc(s) {
    return String(s ?? '').replace(/[&<>"']/g, (c) =>
      ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  }
  async function fetchJSON(path) {
    const res = await fetch(path, { headers: { Accept: 'application/json' } });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    return res.json();
  }
  function intl(n) { return Number(Math.round(Number(n) || 0)).toLocaleString(); }

  // ─── Trove server time (UTC−11, no DST) - resets sit on day boundaries ──
  const TROVE_OFFSET_SEC = -11 * 3600;
  function troveDate(unix) { return new Date((unix + TROVE_OFFSET_SEC) * 1000); }
  function fmtAxis(unix, period) {
    const dte = troveDate(unix);
    if (period === '1d') return dte.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', timeZone: 'UTC', hourCycle: 'h23' });
    if (period === '7d') return dte.toLocaleDateString(undefined, { weekday: 'short', timeZone: 'UTC' });
    if (period === '1m' || period === '3m') return dte.toLocaleDateString(undefined, { month: 'short', day: 'numeric', timeZone: 'UTC' });
    return dte.toLocaleDateString(undefined, { month: 'short', year: '2-digit', timeZone: 'UTC' });
  }
  function fmtFull(unix, period) {
    const dte = troveDate(unix);
    if (period === '1d' || period === '7d')
      return dte.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit', timeZone: 'UTC', hourCycle: 'h23' }) + ' server';
    return dte.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric', timeZone: 'UTC' });
  }

  // ─── SVG helpers ───────────────────────────────────────────────────
  const SVGNS = 'http://www.w3.org/2000/svg';
  function svgEl(name, attrs) {
    const el = document.createElementNS(SVGNS, name);
    for (const k in attrs) el.setAttribute(k, attrs[k]);
    return el;
  }
  // Weekly (Monday 11:00 UTC = Trove Monday 00:00) reset instants in [from,to].
  function weeklyResetLines(fromTs, toTs) {
    const out = [];
    const d = new Date(fromTs * 1000);
    let tt = Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate(), 11, 0, 0) / 1000;
    if (tt < fromTs) tt += 86400;
    for (; tt <= toTs; tt += 86400) if (new Date(tt * 1000).getUTCDay() === 1) out.push(tt);
    return out;
  }

  // ─── Multi-line chart ──────────────────────────────────────────────
  function renderChart(p) {
    state.current = p;
    const host = document.getElementById('cact-chart');
    const range = document.getElementById('cact-chart-range');
    const tip = document.getElementById('cact-chart-tip');
    if (!host) return;
    const buckets = (p && p.buckets) || [];
    const classes = (p && p.classes) || [];

    if (range) {
      range.textContent = buckets.length
        ? t('{n} points · {from} → {to}')
            .replace('{n}', buckets.length)
            .replace('{from}', fmtAxis(p.window_start, p.period))
            .replace('{to}', fmtAxis(p.window_end, p.period)) + ' · ' + t('Trove server time (UTC−11)')
        : '';
    }
    if (buckets.length < 2) {
      host.innerHTML = `<div class="cact-empty" data-i18n>${
        t('Not enough history stored for this range yet - it fills in as hourly captures accumulate.')
      }</div>`;
      if (tip) tip.hidden = true;
      rerunI18n();
      return;
    }

    const W = Math.max(320, host.clientWidth || 640);
    const H = 320;
    const padL = 46, padR = 14, padT = 14, padB = 30;
    const plotW = W - padL - padR, plotH = H - padT - padB;

    const xMin = Math.min(p.window_start, buckets[0]);
    const xMax = Math.max(p.window_end, buckets[buckets.length - 1]);
    const xRange = Math.max(1, xMax - xMin);

    // The chart plots each class's SHARE of total activity per bucket (lines at an
    // x sum to ~100%); the raw per-hour player count rides along in the tooltip.
    // Denominator = every class measurable in that bucket (NOT just the visible
    // ones), so toggling a class in the legend never rescales the others - matching
    // the donut's share semantics.
    const bucketTotals = buckets.map((_, i) => {
      let s = 0;
      for (const cls of classes) {
        const v = viewValues(cls)[i];
        if (v != null) s += v;
      }
      return s;
    });
    const shareAt = (cls, i) => {              // fraction 0..1, or null at a gap
      const v = viewValues(cls)[i];
      if (v == null) return null;
      const tot = bucketTotals[i];
      return tot > 0 ? v / tot : 0;
    };
    const fmtPct = (frac, dec) => (frac * 100).toFixed(dec == null ? 1 : dec) + '%';

    let yMaxRaw = 0;
    for (const cls of classes) {
      if (state.hidden.has(cls.class_index)) continue;
      for (let i = 0; i < buckets.length; i++) {
        const s = shareAt(cls, i);
        if (s != null && s > yMaxRaw) yMaxRaw = s;
      }
    }
    if (!(yMaxRaw > 0)) yMaxRaw = 0.1;         // sane axis when there's no data
    const yMax = yMaxRaw * 1.12;
    const pctDecimals = (yMaxRaw * 100 < 10) ? 1 : 0;
    const xToPx = (v) => padL + ((v - xMin) / xRange) * plotW;
    const yToPx = (v) => padT + (1 - v / yMax) * plotH;

    const svg = svgEl('svg', {
      viewBox: `0 0 ${W} ${H}`, class: 'cact-chart-svg',
      width: '100%', height: H, role: 'img', preserveAspectRatio: 'none',
    });

    // gridlines + Y labels
    const gridG = svgEl('g', {});
    for (let i = 0; i <= 4; i++) {
      const v = (yMaxRaw / 4) * i;           // fraction 0..yMaxRaw
      const y = yToPx(v);
      gridG.appendChild(svgEl('line', { x1: padL, y1: y.toFixed(1), x2: W - padR, y2: y.toFixed(1), class: 'cact-grid-line' }));
      const lbl = svgEl('text', { x: padL - 8, y: (y + 3.5).toFixed(1), class: 'cact-axis-y' });
      lbl.textContent = fmtPct(v, pctDecimals);
      gridG.appendChild(lbl);
    }
    svg.appendChild(gridG);

    // X labels - 6 ticks across the period window
    const xG = svgEl('g', { class: 'cact-axis-x' });
    const ticks = 6;
    for (let i = 0; i < ticks; i++) {
      const tt = xMin + (xMax - xMin) * (i / (ticks - 1));
      const x = xToPx(tt);
      const txt = svgEl('text', { x: x.toFixed(1), y: (H - 10).toFixed(1), 'text-anchor': i === 0 ? 'start' : i === ticks - 1 ? 'end' : 'middle' });
      txt.textContent = fmtAxis(tt, p.period);
      xG.appendChild(txt);
    }
    svg.appendChild(xG);

    // weekly reset markers (the lines break here anyway) on shorter ranges
    if (p.period === '1d' || p.period === '7d' || p.period === '1m') {
      const rg = svgEl('g', {});
      for (const rt of weeklyResetLines(xMin, xMax)) {
        const x = xToPx(rt);
        if (x < padL - 0.5 || x > W - padR + 0.5) continue;
        rg.appendChild(svgEl('line', { x1: x.toFixed(1), y1: padT, x2: x.toFixed(1), y2: (padT + plotH).toFixed(1), class: 'cact-reset-weekly' }));
      }
      svg.appendChild(rg);
    }

    // one path per visible class; break the path at null buckets (gaps).
    for (const cls of classes) {
      if (state.hidden.has(cls.class_index)) continue;
      let d = '';
      let pen = false;
      for (let i = 0; i < buckets.length; i++) {
        const s = shareAt(cls, i);
        if (s == null) { pen = false; continue; }
        d += `${pen ? 'L' : 'M'}${xToPx(buckets[i]).toFixed(1)},${yToPx(s).toFixed(1)} `;
        pen = true;
      }
      if (d) svg.appendChild(svgEl('path', { d: d.trim(), class: 'cact-line', stroke: color(cls.class_index) }));
    }

    // hover guide + overlay
    const guide = svgEl('line', { class: 'cact-guide', y1: padT, y2: padT + plotH, x1: 0, x2: 0 });
    guide.style.opacity = '0';
    svg.appendChild(guide);
    const overlay = svgEl('rect', { x: padL, y: padT, width: plotW, height: plotH, fill: 'transparent' });
    svg.appendChild(overlay);

    host.innerHTML = '';
    host.appendChild(svg);

    function onMove(evt) {
      const r = svg.getBoundingClientRect();
      const sx = ((evt.clientX - r.left) / r.width) * W;
      const ratio = Math.max(0, Math.min(1, (sx - padL) / plotW));
      const targetT = xMin + ratio * xRange;
      let best = 0, bd = Infinity;
      for (let i = 0; i < buckets.length; i++) {
        const dd = Math.abs(buckets[i] - targetT);
        if (dd < bd) { bd = dd; best = i; }
      }
      const bx = xToPx(buckets[best]);
      guide.setAttribute('x1', bx.toFixed(1)); guide.setAttribute('x2', bx.toFixed(1));
      guide.style.opacity = '1';
      if (!tip) return;
      const rows = [];
      const tot = bucketTotals[best];
      for (const cls of classes) {
        if (state.hidden.has(cls.class_index)) continue;
        const v = viewValues(cls)[best];
        if (v == null) continue;
        rows.push({ name: cls.name, v, frac: tot > 0 ? v / tot : 0, i: cls.class_index });
      }
      rows.sort((a, b) => b.frac - a.frac);
      if (!rows.length) { tip.hidden = true; return; }
      // value column: share %, with the raw active-player count in parentheses.
      tip.innerHTML = `<span class="cact-tip-when">${esc(fmtFull(buckets[best], p.period))}</span>` +
        rows.slice(0, 10).map((rw) =>
          `<span class="cact-tip-row"><span class="cact-tip-sw" style="background:${color(rw.i)}"></span>` +
          `<span class="cact-tip-name">${esc(rw.name)}</span>` +
          `<span class="cact-tip-val">${fmtPct(rw.frac)} <span class="cact-tip-count">(${intl(rw.v)})</span></span></span>`).join('');
      tip.hidden = false;
      const cardW = host.clientWidth || W;
      const leftPx = (bx / W) * cardW;
      tip.style.left = Math.max(8, Math.min(cardW - 8, leftPx)) + 'px';
    }
    function onLeave() { guide.style.opacity = '0'; if (tip) tip.hidden = true; }
    overlay.addEventListener('mousemove', onMove);
    overlay.addEventListener('mouseleave', onLeave);
    overlay.addEventListener('touchstart', (e) => { if (e.touches[0]) onMove(e.touches[0]); }, { passive: true });
    overlay.addEventListener('touchmove', (e) => { if (e.touches[0]) onMove(e.touches[0]); }, { passive: true });
    overlay.addEventListener('touchend', onLeave);
  }

  // ─── Legend (toggle classes) ───────────────────────────────────────
  function renderLegend(p) {
    const host = document.getElementById('cact-legend');
    if (!host) return;
    const classes = (p && p.classes) || [];
    host.innerHTML = '';
    for (const cls of classes) {
      const off = state.hidden.has(cls.class_index);
      const chip = document.createElement('button');
      chip.type = 'button';
      chip.className = 'cact-legend-chip' + (off ? ' off' : '');
      chip.setAttribute('aria-pressed', String(!off));
      const badge = cls.icon
        ? `<img class="cact-legend-icon" src="${esc(cls.icon)}" alt="" loading="lazy" style="border-color:${color(cls.class_index)}" onerror="this.remove()">`
        : `<span class="cact-legend-sw" style="background:${color(cls.class_index)}"></span>`;
      chip.innerHTML = badge + esc(cls.name);
      chip.addEventListener('click', () => {
        if (state.hidden.has(cls.class_index)) state.hidden.delete(cls.class_index);
        else state.hidden.add(cls.class_index);
        if (state.current) renderChart(state.current);
        renderLegend(state.current);
      });
      host.appendChild(chip);
    }
  }

  // ─── Share donut ───────────────────────────────────────────────────
  function renderDonut(cur) {
    const host = document.getElementById('cact-donut');
    const legend = document.getElementById('cact-share-legend');
    const sub = document.getElementById('cact-share-sub');
    if (!host) return;
    // Pick raw vs clean counts/shares for the active view, drop classes with no
    // (or unmeasurable) activity, and re-sort by the chosen view's count desc.
    const classes = ((cur && cur.classes) || [])
      .filter((c) => viewCount(c) != null && viewCount(c) > 0)
      .sort((a, b) => viewCount(b) - viewCount(a));
    const total = cur && (state.view === 'clean' ? cur.total_active_clean : cur.total_active);
    if (!classes.length || !total) {
      host.innerHTML = `<div class="cact-donut-empty">${esc(t('No data yet - it fills in as captures accumulate.'))}</div>`;
      if (legend) legend.innerHTML = '';
      return;
    }
    if (sub && cur.window_end) {
      sub.textContent = t('Share of players on each class, latest snapshot ({when}).').replace('{when}', fmtFull(cur.window_end, '7d'));
    }

    const size = 240, cx = size / 2, cy = size / 2, r = 92, sw = 30;
    const C = 2 * Math.PI * r;
    const svg = svgEl('svg', { viewBox: `0 0 ${size} ${size}`, role: 'img' });
    // Background ring. Literal rgba (presentation attributes don't resolve
    // var()/color-mix) - a faint muted gray that reads on the dark card.
    svg.appendChild(svgEl('circle', {
      cx, cy, r, fill: 'none', 'stroke-width': sw, stroke: 'rgba(154,164,178,0.16)',
    }));
    const g = svgEl('g', { transform: `rotate(-90 ${cx} ${cy})` });
    let acc = 0;
    for (const cls of classes) {     // sorted by the active view's count desc
      const seg = (viewShare(cls) || 0) * C;
      if (seg <= 0) continue;
      g.appendChild(svgEl('circle', {
        cx, cy, r, fill: 'none', stroke: color(cls.class_index), 'stroke-width': sw,
        'stroke-dasharray': `${seg.toFixed(2)} ${(C - seg).toFixed(2)}`,
        'stroke-dashoffset': `${(-acc).toFixed(2)}`,
      }));
      acc += seg;
    }
    svg.appendChild(g);
    const top = classes[0];
    const num = svgEl('text', { x: cx, y: cy - 2, 'text-anchor': 'middle', class: 'cact-donut-center-num', 'font-size': '30' });
    num.textContent = Math.round((viewShare(top) || 0) * 100) + '%';
    svg.appendChild(num);
    const lbl = svgEl('text', { x: cx, y: cy + 20, 'text-anchor': 'middle', class: 'cact-donut-center-label' });
    lbl.textContent = top.name;
    svg.appendChild(lbl);
    host.innerHTML = '';
    host.appendChild(svg);

    if (legend) {
      legend.innerHTML = classes.map((cls) => {
        const badge = cls.icon
          ? `<img class="cact-share-icon" src="${esc(cls.icon)}" alt="" loading="lazy" style="border-color:${color(cls.class_index)}" onerror="this.remove()">`
          : `<span class="cact-share-sw" style="background:${color(cls.class_index)}"></span>`;
        return `<li class="cact-share-row">${badge}` +
          `<span class="cact-share-name">${esc(cls.name)}</span>` +
          `<span class="cact-share-pct">${((viewShare(cls) || 0) * 100).toFixed(1)}% ` +
          `<span class="cact-share-count">(${intl(viewCount(cls))})</span></span></li>`;
      }).join('');
    }
  }

  // ─── View toggle (Established / All) ───────────────────────────────
  // The "established" floors come from whichever payload is loaded (series first,
  // donut fallback). A floor of 0 means that gate is off, so it's dropped here.
  function currentThresholds() {
    const src = (state.current && state.current.power_rank_threshold != null)
      ? state.current : (state.donut || {});
    return {
      pr: src.power_rank_threshold || 0,
      effort: src.effort_threshold || 0,
    };
  }
  function updateViewHint() {
    const hint = document.getElementById('cact-view-hint');
    if (!hint) return;
    if (state.view === 'clean') {
      const { pr, effort } = currentThresholds();
      const parts = [];
      if (pr) parts.push(t('Power Rank ≥ {n}').replace('{n}', intl(pr)));
      if (effort) parts.push(t('Effort ≥ {n}').replace('{n}', intl(effort)));
      hint.textContent = parts.length
        ? t('Established players — {criteria}.').replace('{criteria}', parts.join(', '))
        : t('Established players only.');
    } else {
      hint.textContent = t('Everyone on the class’s Effort board.');
    }
  }
  function setView(view, reflect) {
    if (view !== 'clean' && view !== 'raw') return;
    state.view = view;
    document.querySelectorAll('#cact-views button').forEach((b) => {
      const on = b.dataset.view === view;
      b.classList.toggle('active', on);
      b.setAttribute('aria-selected', String(on));
    });
    updateViewHint();
    if (state.current) renderChart(state.current);
    if (state.donut) renderDonut(state.donut);
    if (reflect) reflectUrl();
  }

  // ─── URL state (period + table view) ───────────────────────────────
  // The URL carries both the time range and the table mode so a link restores
  // exactly what the sharer saw. Internal views map to friendly URL tokens:
  // clean -> "established", raw -> "all".
  const VIEW_TO_URL = { clean: 'established', raw: 'all' };
  const URL_TO_VIEW = { established: 'clean', all: 'raw' };

  function periodFromUrl() {
    try {
      const q = new URLSearchParams(location.search).get('period');
      const cand = (q || (location.hash || '').replace(/^#/, '') || '').toLowerCase();
      return PERIODS.includes(cand) ? cand : null;
    } catch (_) { return null; }
  }
  function viewFromUrl() {
    try {
      const q = (new URLSearchParams(location.search).get('view') || '').toLowerCase();
      return URL_TO_VIEW[q] || null;
    } catch (_) { return null; }
  }
  function reflectUrl() {
    try {
      const qs = '?period=' + encodeURIComponent(state.period) +
                 '&view=' + (VIEW_TO_URL[state.view] || 'established');
      history.replaceState(null, '', location.pathname + qs);
    } catch (_) { /* non-fatal */ }
  }

  // ─── Period loading ────────────────────────────────────────────────
  async function loadPeriod(period, reflect) {
    state.period = period;
    if (reflect) reflectUrl();
    document.querySelectorAll('#cact-periods button').forEach((b) => {
      const on = b.dataset.period === period;
      b.classList.toggle('active', on);
      b.setAttribute('aria-selected', String(on));
    });
    if (state.series[period]) {
      renderChart(state.series[period]);
      renderLegend(state.series[period]);
      updateViewHint();
      return;
    }
    const host = document.getElementById('cact-chart');
    if (host) host.innerHTML = `<div class="cact-loading">${esc(t('Loading…'))}</div>`;
    try {
      const data = await fetchJSON('/site/leaderboards/class-activity/series?period=' + encodeURIComponent(period));
      state.series[period] = data;
      if (state.period === period) { renderChart(data); renderLegend(data); updateViewHint(); }
    } catch (_) {
      if (host) host.innerHTML = `<div class="cact-empty">${esc(t('Could not load this range. Try again shortly.'))}</div>`;
    }
  }

  // ─── Boot ──────────────────────────────────────────────────────────
  function wire() {
    const tabs = document.getElementById('cact-periods');
    if (tabs) {
      tabs.addEventListener('click', (e) => {
        const btn = e.target.closest('button[data-period]');
        if (btn) loadPeriod(btn.dataset.period, true);
      });
    }
    const views = document.getElementById('cact-views');
    if (views) {
      views.addEventListener('click', (e) => {
        const btn = e.target.closest('button[data-view]');
        if (btn) setView(btn.dataset.view, true);
      });
    }
    window.addEventListener('hashchange', () => {
      const p = periodFromUrl();
      if (p && p !== state.period) loadPeriod(p);
    });
    let rz;
    window.addEventListener('resize', () => {
      clearTimeout(rz);
      rz = setTimeout(() => { if (state.current) renderChart(state.current); }, 150);
    });
    document.addEventListener('btt-lang-changed', () => {
      if (state.current) { renderChart(state.current); renderLegend(state.current); }
      if (state.donut) renderDonut(state.donut);
      updateViewHint();
    });
  }

  async function init() {
    wire();
    state.period = periodFromUrl() || state.period;
    state.view = viewFromUrl() || state.view;
    setView(state.view);   // paint the toggle from URL/default (no reflect on load)
    fetchJSON('/site/leaderboards/class-activity/current')
      .then((d) => { state.donut = d; renderDonut(d); updateViewHint(); })
      .catch(() => { /* donut stays empty */ });
    await loadPeriod(state.period);
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init);
  else init();
})();
