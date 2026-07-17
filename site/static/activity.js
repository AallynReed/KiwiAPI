/* Player Activity page (/activity). Two same-origin JSON proxies:
   /site/leaderboards/activity (live estimate + 24h/7d rollups) and
   .../activity/series?period=… (bucketed time-series). The estimate derives
   from the leaderboard captures, hence the shared /site/leaderboards/* path. */
(function () {
  'use strict';

  const { esc, fetchJSON } = window.BTTUtil;

  const PERIODS = ['1d', '7d', '1m'];   // longer ranges (3m/6m/1y/all) removed
  const state = {
    period: '7d',
    series: {},          // period -> payload cache
    live: null,          // /activity payload
    current: null,       // payload currently drawn (for resize redraw)
  };

  // ─── i18n + fetch + util ───────────────────────────────────────────
  function t(s) {
    return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s;
  }
  function rerunI18n() {
    if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh();
  }
  // Compact axis/stat numbers: 4231 -> "4.2k", 1.2M -> "1.2M".
  function abbrev(n) {
    n = Number(n) || 0;
    if (n >= 1e6) return (n / 1e6).toFixed(1).replace(/\.0$/, '') + 'M';
    if (n >= 1e3) return (n / 1e3).toFixed(1).replace(/\.0$/, '') + 'k';
    return String(Math.round(n));
  }
  function intl(n) { return Number(Math.round(Number(n) || 0)).toLocaleString(); }

  // ─── Live hero ─────────────────────────────────────────────────────
  function renderLive() {
    const wrap = document.getElementById('act-live');
    const numEl = document.getElementById('act-now-num');
    const roll = document.getElementById('act-rollups');
    const d = state.live;
    if (!wrap || !numEl) return;
    if (!d || d.estimate == null) { wrap.hidden = true; return; }
    wrap.hidden = false;
    numEl.textContent = '~' + Number(d.estimate).toLocaleString();

    if (roll) {
      const chip = (label, n, title) =>
        `<span class="act-roll" title="${esc(title)}">` +
        `<span class="act-roll-num">~${Number(n).toLocaleString()}</span>` +
        `<span class="act-roll-label">${esc(label)}</span></span>`;
      const chips = [];
      if (d.estimate_24h != null)
        chips.push(chip(t('in the last 24h'), d.estimate_24h, t('Active players in the last 24 hours')));
      if (d.estimate_7d != null)
        chips.push(chip(t('in the last 7 days'), d.estimate_7d, t('Active players in the last 7 days')));
      if (chips.length) { roll.innerHTML = chips.join(''); roll.hidden = false; }
      else roll.hidden = true;
    }
  }

  // ─── Date formatting (per period) ──────────────────────────────────
  // Shown in TROVE SERVER TIME - a fixed UTC−11 (no DST). The daily reset
  // (11:00 UTC) is MIDNIGHT in Trove time, so this puts every reset right on a
  // day boundary - far clearer than UTC's "11:00". We shift the instant by
  // −11h and read it as UTC to get the Trove wall clock; the reset markers use
  // the same clock, so the daily lines sit on 00:00 and the weekly on Monday.
  const TROVE_OFFSET_SEC = -11 * 3600;
  function troveDate(unix) { return new Date((unix + TROVE_OFFSET_SEC) * 1000); }
  function fmtAxis(unix, period) {
    const dte = troveDate(unix);
    if (period === '1d') return dte.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', timeZone: 'UTC', hourCycle: 'h23' });
    if (period === '7d') return dte.toLocaleDateString(undefined, { weekday: 'short', timeZone: 'UTC' });
    if (period === '1m' || period === '3m') return dte.toLocaleDateString(undefined, { month: 'short', day: 'numeric', timeZone: 'UTC' });
    return dte.toLocaleDateString(undefined, { month: 'short', year: '2-digit', timeZone: 'UTC' }); // 6m/1y/all
  }
  function fmtFull(unix, period) {
    const dte = troveDate(unix);
    if (period === '1d' || period === '7d')
      return dte.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit', timeZone: 'UTC', hourCycle: 'h23' }) + ' server';
    return dte.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric', timeZone: 'UTC' });
  }

  // ─── Stat cards ────────────────────────────────────────────────────
  function renderStats(p) {
    const peakEl = document.getElementById('act-peak');
    const peakWhen = document.getElementById('act-peak-when');
    const avgEl = document.getElementById('act-avg');
    const lowEl = document.getElementById('act-low');
    const lowWhen = document.getElementById('act-low-when');
    const pts = (p && p.points) || [];

    if (peakEl) peakEl.textContent = p && p.peak ? intl(p.peak.active) : '—';
    if (peakWhen) peakWhen.textContent = p && p.peak ? fmtFull(p.peak.t, p.period) : t('active players / hour');
    if (avgEl) avgEl.textContent = p && p.average != null ? intl(p.average) : '—';

    // Quietest = the true minimum captured hour (server-provided, timestamped at
    // the actual trough). Fall back to scanning the plotted points for older
    // cached payloads that predate the `low` field.
    let low = (p && p.low) || null;
    if (!low) for (const pt of pts) if (low === null || pt.active < low.active) low = pt;
    if (lowEl) lowEl.textContent = low ? intl(low.active) : '—';
    if (lowWhen) lowWhen.textContent = low ? fmtFull(low.t, p.period) : t('active players / hour');
  }

  // ─── Chart ─────────────────────────────────────────────────────────
  const SVGNS = 'http://www.w3.org/2000/svg';
  function svgEl(name, attrs) {
    const el = document.createElementNS(SVGNS, name);
    for (const k in attrs) el.setAttribute(k, attrs[k]);
    return el;
  }

  // Trove resets: DAILY at 11:00 UTC = MIDNIGHT Trove server time (UTC−11),
  // WEEKLY on Monday. Returns the reset instants (unix seconds) inside
  // [fromTs, toTs], flagging Mondays as weekly. The axis is in Trove time too
  // (see fmtAxis), so these land exactly on the 00:00 day boundaries.
  function resetLines(fromTs, toTs) {
    const out = [];
    const d = new Date(fromTs * 1000);
    let t = Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate(), 11, 0, 0) / 1000;
    if (t < fromTs) t += 86400;
    for (; t <= toTs; t += 86400) {
      out.push({ t, weekly: new Date(t * 1000).getUTCDay() === 1 });
    }
    return out;
  }

  function renderChart(p) {
    state.current = p;
    const host = document.getElementById('act-chart');
    const range = document.getElementById('act-chart-range');
    const tip = document.getElementById('act-chart-tip');
    if (!host) return;
    const points = (p && p.points) || [];

    if (range) {
      range.textContent = points.length
        ? t('{n} points · {from} → {to}')
            .replace('{n}', points.length)
            .replace('{from}', fmtAxis(p.window_start, p.period))
            .replace('{to}', fmtAxis(p.window_end, p.period)) + ' · ' + t('Trove server time (UTC−11)')
        : '';
    }
    if (points.length < 2) {
      host.innerHTML = `<div class="act-empty" data-i18n>${
        t('Not enough history stored for this range yet - it fills in as hourly captures accumulate.')
      }</div>`;
      if (tip) tip.hidden = true;
      rerunI18n();
      return;
    }

    // Pixel geometry - measured from the card so axis labels render crisp.
    const W = Math.max(320, host.clientWidth || 640);
    const H = 300;
    const padL = 46, padR = 14, padT = 14, padB = 30;
    const plotW = W - padL - padR, plotH = H - padT - padB;

    const xs = points.map((q) => q.t);
    const ys = points.map((q) => q.active || 0);
    // Anchor the X axis to the SELECTED PERIOD window (relative: last
    // 1d/7d/1m/…), not just the data's extent - so a sparse range still shows
    // the full timeline with points where they actually fall, instead of
    // collapsing to "today" when only a couple of captures exist.
    const xMin = Math.min(p.window_start, xs[0]);
    const xMax = Math.max(p.window_end, xs[xs.length - 1]);
    const xRange = Math.max(1, xMax - xMin);
    const yMaxRaw = Math.max(...ys, 1);
    const yMax = yMaxRaw * 1.12;        // headroom so the peak isn't on the edge
    const xToPx = (v) => padL + ((v - xMin) / xRange) * plotW;
    const yToPx = (v) => padT + (1 - v / yMax) * plotH;

    const svg = svgEl('svg', {
      viewBox: `0 0 ${W} ${H}`, class: 'act-chart-svg',
      width: '100%', height: H, role: 'img', preserveAspectRatio: 'none',
    });
    const defs = svgEl('defs', {});
    const grad = svgEl('linearGradient', { id: 'act-grad', x1: '0', y1: '0', x2: '0', y2: '1' });
    grad.appendChild(svgEl('stop', { offset: '0%', 'stop-color': '#3fb950', 'stop-opacity': '0.32' }));
    grad.appendChild(svgEl('stop', { offset: '100%', 'stop-color': '#3fb950', 'stop-opacity': '0' }));
    defs.appendChild(grad);
    svg.appendChild(defs);

    // Gridlines + Y labels at 0, ¼, ½, ¾, max.
    const gridG = svgEl('g', { class: 'act-grid' });
    for (let i = 0; i <= 4; i++) {
      const v = (yMaxRaw / 4) * i;
      const y = yToPx(v);
      gridG.appendChild(svgEl('line', {
        x1: padL, y1: y.toFixed(1), x2: W - padR, y2: y.toFixed(1), class: 'act-grid-line',
      }));
      const lbl = svgEl('text', { x: padL - 8, y: (y + 3.5).toFixed(1), class: 'act-axis-y' });
      lbl.textContent = abbrev(v);
      gridG.appendChild(lbl);
    }
    svg.appendChild(gridG);

    // X labels - 6 ticks evenly spaced across the PERIOD window (relative
    // time), independent of where the data points actually fall.
    const xG = svgEl('g', { class: 'act-axis-x' });
    const ticks = 6;
    for (let i = 0; i < ticks; i++) {
      const tt = xMin + (xMax - xMin) * (i / (ticks - 1));
      const x = xToPx(tt);
      const txt = svgEl('text', {
        x: x.toFixed(1), y: (H - 10).toFixed(1),
        'text-anchor': i === 0 ? 'start' : i === ticks - 1 ? 'end' : 'middle',
      });
      txt.textContent = fmtAxis(tt, p.period);
      xG.appendChild(txt);
    }
    svg.appendChild(xG);

    // Area + line. Close the fill under the actual DATA extent (first/last
    // point), not the axis bounds - otherwise a sparse range smears the fill
    // across the empty part of the window.
    const line = points.map((q, i) =>
      `${i === 0 ? 'M' : 'L'}${xToPx(q.t).toFixed(1)},${yToPx(q.active).toFixed(1)}`).join(' ');
    const area = line +
      ` L${xToPx(xs[xs.length - 1]).toFixed(1)},${(padT + plotH).toFixed(1)}` +
      ` L${xToPx(xs[0]).toFixed(1)},${(padT + plotH).toFixed(1)} Z`;
    svg.appendChild(svgEl('path', { d: area, class: 'act-area', fill: 'url(#act-grad)' }));
    svg.appendChild(svgEl('path', { d: line, class: 'act-line', fill: 'none' }));

    // Reset markers - vertical lines at each daily 11:00 UTC reset (Monday's
    // is the weekly reset, drawn distinct). Only on the short ranges where an
    // individual reset is meaningful; on 1m+ they'd be a forest of lines.
    if (p.period === '1d' || p.period === '7d') {
      const rg = svgEl('g', { class: 'act-resets' });
      for (const r of resetLines(xMin, xMax)) {
        const x = xToPx(r.t);
        if (x < padL - 0.5 || x > W - padR + 0.5) continue;   // clip to plot
        rg.appendChild(svgEl('line', {
          x1: x.toFixed(1), y1: padT, x2: x.toFixed(1), y2: (padT + plotH).toFixed(1),
          class: r.weekly ? 'act-reset-line act-reset-weekly' : 'act-reset-line',
        }));
      }
      svg.appendChild(rg);
    }

    const guide = svgEl('line', { class: 'act-guide', y1: padT, y2: padT + plotH, x1: 0, x2: 0 });
    guide.style.opacity = '0';
    svg.appendChild(guide);
    const dot = svgEl('circle', { class: 'act-dot', r: '4', cx: '0', cy: '0' });
    dot.style.opacity = '0';
    svg.appendChild(dot);
    const overlay = svgEl('rect', {
      x: padL, y: padT, width: plotW, height: plotH, fill: 'transparent',
    });
    svg.appendChild(overlay);

    host.innerHTML = '';
    host.appendChild(svg);

    function onMove(evt) {
      const r = svg.getBoundingClientRect();
      const sx = ((evt.clientX - r.left) / r.width) * W;
      const ratio = Math.max(0, Math.min(1, (sx - padL) / plotW));
      const targetT = xMin + ratio * xRange;
      let best = 0, bd = Infinity;
      for (let i = 0; i < points.length; i++) {
        const dd = Math.abs(points[i].t - targetT);
        if (dd < bd) { bd = dd; best = i; }
      }
      const q = points[best];
      const px = xToPx(q.t), py = yToPx(q.active);
      guide.setAttribute('x1', px.toFixed(1)); guide.setAttribute('x2', px.toFixed(1));
      guide.style.opacity = '1';
      dot.setAttribute('cx', px.toFixed(1)); dot.setAttribute('cy', py.toFixed(1));
      dot.style.opacity = '1';
      if (!tip) return;
      tip.innerHTML =
        `<strong>${intl(q.active)}</strong> ` +
        `<span class="act-tip-unit">${esc(t('active / hr'))}</span>` +
        `<span class="act-tip-when">${esc(fmtFull(q.t, p.period))}</span>`;
      tip.hidden = false;
      const cardW = host.clientWidth || W;
      const leftPx = (px / W) * cardW;
      tip.style.left = Math.max(8, Math.min(cardW - 8, leftPx)) + 'px';
    }
    function onLeave() {
      guide.style.opacity = '0';
      dot.style.opacity = '0';
      if (tip) tip.hidden = true;
    }
    overlay.addEventListener('mousemove', onMove);
    overlay.addEventListener('mouseleave', onLeave);
    overlay.addEventListener('touchstart', (e) => { if (e.touches[0]) onMove(e.touches[0]); }, { passive: true });
    overlay.addEventListener('touchmove', (e) => { if (e.touches[0]) onMove(e.touches[0]); }, { passive: true });
    overlay.addEventListener('touchend', onLeave);

    const last = points[points.length - 1];
    svg.setAttribute('aria-label',
      t('Active-player trend for this period; latest ~{n} per hour').replace('{n}', intl(last.active)));
  }

  // ─── Period <-> URL ────────────────────────────────────────────────
  // A `?period=` query param (NOT a #fragment - fragments never reach the
  // server, so they can't drive the per-period OG/Twitter card) selects the
  // graph and makes the address bar a copy-paste shareable link per period.
  // We still READ a #hash as a convenience for the in-page jump.
  function periodFromUrl() {
    try {
      const q = new URLSearchParams(location.search).get('period');
      const h = (location.hash || '').replace(/^#/, '');
      const cand = (q || h || '').toLowerCase();
      return PERIODS.includes(cand) ? cand : null;
    } catch (_) { return null; }
  }
  function reflectUrl(period) {
    try { history.replaceState(null, '', location.pathname + '?period=' + period); }
    catch (_) { /* history blocked - non-fatal */ }
  }

  // ─── Period loading ────────────────────────────────────────────────
  async function loadPeriod(period, reflect) {
    state.period = period;
    if (reflect) reflectUrl(period);
    // Reflect the active button.
    document.querySelectorAll('#act-periods button').forEach((b) => {
      const on = b.dataset.period === period;
      b.classList.toggle('active', on);
      b.setAttribute('aria-selected', String(on));
    });

    if (state.series[period]) {
      renderChart(state.series[period]);
      renderStats(state.series[period]);
      return;
    }
    const host = document.getElementById('act-chart');
    if (host) host.innerHTML = `<div class="act-loading">${esc(t('Loading…'))}</div>`;
    try {
      const data = await fetchJSON('/site/leaderboards/activity/series?period=' + encodeURIComponent(period));
      state.series[period] = data;
      // Guard against a race if the user clicked another period mid-fetch.
      if (state.period === period) { renderChart(data); renderStats(data); }
    } catch (_) {
      if (host) host.innerHTML = `<div class="act-empty">${esc(t('Could not load this range. Try again shortly.'))}</div>`;
    }
  }

  // ─── Boot ──────────────────────────────────────────────────────────
  function wire() {
    const tabs = document.getElementById('act-periods');
    if (tabs) {
      tabs.addEventListener('click', (e) => {
        const btn = e.target.closest('button[data-period]');
        if (btn) loadPeriod(btn.dataset.period, true);   // reflect into the URL
      });
    }
    // Manual #hash edits / back-forward jump to that period.
    window.addEventListener('hashchange', () => {
      const p = periodFromUrl();
      if (p && p !== state.period) loadPeriod(p);
    });
    // Responsive redraw (debounced) - the SVG is pixel-sized to the card.
    let rz;
    window.addEventListener('resize', () => {
      clearTimeout(rz);
      rz = setTimeout(() => { if (state.current) renderChart(state.current); }, 150);
    });
    // Re-render dynamic strings on language switch.
    document.addEventListener('btt-lang-changed', () => {
      renderLive();
      if (state.current) { renderChart(state.current); renderStats(state.current); }
    });
  }

  async function init() {
    wire();
    state.period = periodFromUrl() || state.period;   // deep-link from ?period=/#hash
    // Live hero (non-fatal) and the chosen-period series in parallel.
    fetchJSON('/site/leaderboards/activity')
      .then((d) => { state.live = d; renderLive(); })
      .catch(() => { /* hero stays hidden */ });
    await loadPeriod(state.period);
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
