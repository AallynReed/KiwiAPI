/* ═══════════════════════════════════════════════════════════════════════
   /market - Credits calculator (Credits tab)
   ───────────────────────────────────────────────────────────────────────
   Converts freely between flux, credits and US dollars. Every rate hangs
   off one observable number - what a Credit Pouch trades for - which is
   prefilled from our own market median and stays editable.

   Two credit bases, because a pouch's face value and what it costs to
   acquire are different numbers:

     face      250 credits    what opening a pouch actually gives you
     purchase  308.33 credits what a pouch effectively costs once the
                              $99.99 pack's bonus credits are spread over
                              the 60 pouches it contains

   They disagree by ~23%, so the basis is a visible toggle rather than a
   silent constant. Client-only apart from the one median lookup.
   ═══════════════════════════════════════════════════════════════════════ */

(function () {
  'use strict';

  const { esc, fetchJSON } = window.BTTUtil;

  // Published pack facts - the anchor the whole page is derived from.
  const PACK_USD = 99.99;
  const PACK_CREDITS = 18500;
  const POUCHES_PER_PACK = 60;
  const POUCH_FACE_CREDITS = 250;

  const USD_PER_POUCH = PACK_USD / POUCHES_PER_PACK;                    // 1.6665
  const PURCHASE_CREDITS_PER_POUCH = PACK_CREDITS / POUCHES_PER_PACK;   // 308.33

  const $ = (id) => document.getElementById(id);
  const $view = $('mkt-view-credits');
  if (!$view) return;

  const $pouch = $('mkt-cr-pouch');
  const $source = $('mkt-cr-source');
  const $basis = $('mkt-cr-basis');
  const $rates = $('mkt-cr-rates');
  const $flux = $('mkt-cr-flux');
  const $credits = $('mkt-cr-credits');
  const $usd = $('mkt-cr-usd');
  const $hour = $('mkt-cr-hour');
  const $hourOut = $('mkt-cr-hour-out');
  const $table = $('mkt-cr-table');
  const $chart = $('mkt-cr-chart');
  const $chartMeta = $('mkt-cr-chart-meta');

  const state = {
    // Deliberately null until the live median lands or the reader types one.
    // A plausible-looking default would be indistinguishable from real data
    // and goes stale silently - pouch prices have swung 3M -> 12M -> 50M.
    pouchFlux: null,
    basis: 'face',       // 'face' | 'purchase'
    live: null,          // latest median_each, else null
    liveCount: 0,
    manual: false,       // reader typed over the live price
    fluxPerHour: 500000,
    history: null,       // daily buckets, fetched once per page load
    shown: false,
    timer: null,
  };

  // The scraper posts hourly, so anything tighter is just noise on the
  // proxy. Polls only while the tab is both selected and foregrounded.
  const REFRESH_MS = 300000;

  boot();

  function boot() {
    if ($source) $source.textContent = t('Checking live listings…');
    if ($pouch) {
      $pouch.addEventListener('input', () => {
        const v = parseNum($pouch.value);
        state.pouchFlux = v && v > 0 ? v : null;
        state.manual = true;
        renderSource();
        render();
      });
    }

    if ($basis) {
      $basis.addEventListener('click', (e) => {
        const btn = e.target.closest('.mkt-cr-mode');
        if (!btn || btn.dataset.basis === state.basis) return;
        state.basis = btn.dataset.basis;
        for (const b of $basis.querySelectorAll('.mkt-cr-mode')) {
          const on = b.dataset.basis === state.basis;
          b.classList.toggle('active', on);
          b.setAttribute('aria-pressed', String(on));
        }
        render();
      });
    }

    wireConverter();

    if ($hour) {
      $hour.value = String(state.fluxPerHour);
      $hour.addEventListener('input', () => {
        const v = parseNum($hour.value);
        state.fluxPerHour = v && v > 0 ? v : null;
        renderHour();
      });
    }

    // Re-syncs a reader who typed over the price back onto live data.
    if ($source) {
      $source.addEventListener('click', (e) => {
        if (!e.target.closest('.mkt-cr-resync')) return;
        state.manual = false;
        applyLive();
      });
    }

    document.addEventListener('btt-mkt-view', (e) => {
      const on = e.detail === 'credits';
      if (on) { state.shown = true; startPolling(); loadHistory(); render(); }
      else stopPolling();
    });

    // The SVG measures its container, so it can only lay out once visible.
    let resizeTimer = null;
    window.addEventListener('resize', () => {
      clearTimeout(resizeTimer);
      resizeTimer = setTimeout(() => { if (state.shown && !$view.hidden) drawChart(); }, 140);
    });

    // A background tab has no reason to keep polling.
    document.addEventListener('visibilitychange', () => {
      if (document.hidden) stopPolling();
      else if (state.shown && !$view.hidden) startPolling();
    });

    document.addEventListener('btt-lang-changed', () => { if (state.shown) render(); });

    render();
  }

  /* ─── The rate ──────────────────────────────────────────────────────
     flux/credit follows the chosen basis; flux/dollar never does - a
     dollar buys a pouch regardless of how you value what's inside it. */
  function rates() {
    const pouch = state.pouchFlux;
    if (!pouch) return null;
    const creditsPerPouch = state.basis === 'face'
      ? POUCH_FACE_CREDITS
      : PURCHASE_CREDITS_PER_POUCH;
    return {
      creditsPerPouch,
      fluxPerCredit: pouch / creditsPerPouch,
      fluxPerDollar: pouch / USD_PER_POUCH,
    };
  }

  function startPolling() {
    fetchLive();
    if (state.timer) return;
    state.timer = setInterval(fetchLive, REFRESH_MS);
  }

  function stopPolling() {
    if (!state.timer) return;
    clearInterval(state.timer);
    state.timer = null;
  }

  async function fetchLive() {
    try {
      const d = await fetchJSON('/site/market/items/Credit%20Pouch/summary');
      const median = Number(d && d.median_each);
      if (!median || !isFinite(median) || median <= 0) return;
      state.live = Math.round(median);
      state.liveCount = Number(d.count) || 0;
      // A manual override wins until the reader asks to go back to live -
      // silently overwriting what someone typed is the one thing a
      // refreshing field must never do.
      if (state.manual) renderSource();
      else applyLive();
    } catch (err) {
      console.warn('[market-credits] live pouch price unavailable', err);
    } finally {
      renderSource();
    }
  }

  function applyLive() {
    if (state.live == null) return;
    state.pouchFlux = state.live;
    if ($pouch) $pouch.value = String(state.live);
    renderSource();
    render();
  }

  // The source line is a live region, so it only writes when the message
  // actually changes - otherwise a screen reader re-announces it on every
  // keystroke and every poll.
  function renderSource() {
    if (!$source) return;
    let mode, html;
    if (state.manual) {
      mode = 'manual';
      html = esc(t('Your own figure.'))
        + (state.live == null ? '' :
           ' <button type="button" class="mkt-cr-resync">'
           + esc(t('Use the live price')) + '</button>');
    } else if (state.live != null) {
      mode = 'live';
      html = esc(t('Live median from') + ' ' + fmt(state.liveCount) + ' '
                 + (state.liveCount === 1 ? t('listing') : t('listings')));
    } else {
      mode = 'empty';
      html = esc(t('No live price right now - enter one to continue.'));
    }
    if ($source.dataset.mode === mode && $source.dataset.sig === html) return;
    $source.dataset.mode = mode;
    $source.dataset.sig = html;
    $source.className = mode === 'live' ? 'mkt-cr-source is-live' : 'mkt-cr-source';
    $source.innerHTML = html;
  }

  /* ─── Converter ─────────────────────────────────────────────────────
     Three linked fields. Whichever one is being typed in drives the
     other two and is never rewritten under the cursor. */
  function wireConverter() {
    const link = (el, toFlux) => {
      if (!el) return;
      el.addEventListener('input', () => {
        const r = rates();
        const v = parseNum(el.value);
        if (!r || v == null) return clearExcept(el);
        writeAll(toFlux(v, r), el);
      });
    };
    link($flux, (v) => v);
    link($credits, (v, r) => v * r.fluxPerCredit);
    link($usd, (v, r) => v * r.fluxPerDollar);
  }

  function writeAll(flux, driving) {
    const r = rates();
    if (!r || !isFinite(flux)) return;
    if ($flux !== driving) $flux.value = fmt(Math.round(flux));
    if ($credits !== driving) $credits.value = fmt(round2(flux / r.fluxPerCredit));
    if ($usd !== driving) $usd.value = round2(flux / r.fluxPerDollar).toFixed(2);
  }

  function clearExcept(driving) {
    for (const el of [$flux, $credits, $usd]) {
      if (el && el !== driving) el.value = '';
    }
  }

  /* ─── Render ────────────────────────────────────────────────────────*/
  function render() {
    renderRates();
    renderTable();
    renderHour();
    drawChart();
    reconvert();
    rerunI18n();
  }

  // Keep the converter honest when the rate itself changes: flux is the
  // invariant, so re-derive credits and dollars from whatever flux holds.
  function reconvert() {
    if (!$flux) return;
    const v = parseNum($flux.value);
    if (v == null) return;
    writeAll(v, $flux);
  }

  function renderRates() {
    if (!$rates) return;
    const r = rates();
    if (!r) {
      $rates.innerHTML = '<p class="mkt-loading" data-i18n>'
        + t('Enter what a Credit Pouch sells for to see the rates.') + '</p>';
      return;
    }
    const card = (label, value, sub) => `
      <div class="mkt-cr-card">
        <span class="mkt-cr-card-label">${esc(t(label))}</span>
        <span class="mkt-cr-card-value">${esc(value)}</span>
        <span class="mkt-cr-card-sub">${esc(sub)}</span>
      </div>`;

    $rates.innerHTML =
      card('Flux per credit', fmt(round2(r.fluxPerCredit)),
           t('What one credit is worth in flux.')) +
      card('Flux per dollar', fmt(Math.round(r.fluxPerDollar)),
           t('What a dollar spent on pouches turns into.')) +
      card('Credits per pouch', fmt(round2(r.creditsPerPouch)),
           state.basis === 'face'
             ? t('Face value - what opening one gives you.')
             : t('What one effectively costs from the pack.')) +
      card('Dollars per pouch', '$' + USD_PER_POUCH.toFixed(2),
           t('The $99.99 pack split across its 60 pouches.'));
  }

  function renderTable() {
    if (!$table) return;
    const r = rates();
    if (!r) { $table.innerHTML = ''; return; }

    const rows = [
      [t('One Credit Pouch'), state.pouchFlux],
      [t('One million flux'), 1000000],
      // Labelled by the credits, not the price: on the face basis this row
      // lands well above $99.99, and that gap IS the point - buying credits
      // as flux costs more than buying them from the store.
      [t("The pack's 18,500 credits"), PACK_CREDITS * r.fluxPerCredit],
    ];

    $table.innerHTML = `
      <table class="mkt-cr-tbl">
        <thead><tr>
          <th scope="col">${esc(t('Amount'))}</th>
          <th scope="col" class="num">${esc(t('Flux'))}</th>
          <th scope="col" class="num">${esc(t('Credits'))}</th>
          <th scope="col" class="num">${esc(t('US Dollars'))}</th>
        </tr></thead>
        <tbody>${rows.map(([label, flux]) => `
          <tr>
            <th scope="row">${esc(label)}</th>
            <td class="num">${esc(fmt(Math.round(flux)))}</td>
            <td class="num">${esc(fmt(round2(flux / r.fluxPerCredit)))}</td>
            <td class="num">${esc('$' + round2(flux / r.fluxPerDollar).toFixed(2))}</td>
          </tr>`).join('')}
        </tbody>
      </table>`;
  }

  function renderHour() {
    if (!$hourOut) return;
    const r = rates();
    if (!r || !state.fluxPerHour) { $hourOut.innerHTML = ''; return; }
    const credits = state.fluxPerHour / r.fluxPerCredit;
    const usd = state.fluxPerHour / r.fluxPerDollar;
    $hourOut.innerHTML =
      '<span class="mkt-cr-hour-part"><strong>' + esc(fmt(round2(credits)))
      + '</strong> ' + esc(t('credits an hour')) + '</span>'
      + '<span class="mkt-cr-hour-part"><strong>' + esc('$' + round2(usd).toFixed(2))
      + '</strong> ' + esc(t('an hour')) + '</span>';
  }

  /* ─── 30-day evolution ──────────────────────────────────────────────
     Same daily buckets the Analytics timeline uses, divided through by
     credits-per-pouch so the axis reads in flux per credit and follows
     the basis toggle. The p25-p75 band comes along because a median on
     its own hides how wide the spread got.                             */
  async function loadHistory() {
    if (state.history) return;
    try {
      const d = await fetchJSON(
        '/site/market/analytics/timeline?name=Credit%20Pouch&days=30');
      state.history = (d && d.points) || [];
    } catch (err) {
      state.history = [];
      console.warn('[market-credits] history unavailable', err);
    }
    drawChart();
  }

  function drawChart() {
    if (!$chart) return;
    const pts = state.history;
    if (!pts) { $chart.innerHTML = ''; return; }
    const r = rates();
    if (!r || pts.length < 2) {
      $chart.innerHTML = '<p class="mkt-loading">'
        + esc(t('Not enough price history yet.')) + '</p>';
      if ($chartMeta) $chartMeta.textContent = '';
      return;
    }

    const W = Math.max(320, $chart.clientWidth || 640);
    const H = 240;
    const padL = 62, padR = 14, padT = 14, padB = 26;
    const plotW = W - padL - padR;
    const plotH = H - padT - padB;

    const cpp = r.creditsPerPouch;
    const rows = pts.map((p) => ({
      x: p.bucket,
      mid: p.p50 / cpp,
      lo: (p.p25 != null ? p.p25 : p.p50) / cpp,
      hi: (p.p75 != null ? p.p75 : p.p50) / cpp,
    }));

    const xMin = rows[0].x, xMax = rows[rows.length - 1].x;
    const yMax = Math.max(...rows.map((d) => d.hi)) * 1.08;
    const yMin = Math.min(...rows.map((d) => d.lo)) * 0.92;
    const xTo = (v) => padL + (xMax === xMin ? 0 : (v - xMin) / (xMax - xMin)) * plotW;
    const yTo = (v) => padT + (1 - (yMax === yMin ? 0.5 : (v - yMin) / (yMax - yMin))) * plotH;

    const svgNS = 'http://www.w3.org/2000/svg';
    const svg = document.createElementNS(svgNS, 'svg');
    svg.setAttribute('viewBox', '0 0 ' + W + ' ' + H);
    svg.setAttribute('preserveAspectRatio', 'xMidYMid meet');
    svg.setAttribute('role', 'img');
    svg.setAttribute('aria-label', t('Flux per credit over the last 30 days'));

    const el = (name, attrs, text) => {
      const n = document.createElementNS(svgNS, name);
      for (const k in attrs) n.setAttribute(k, attrs[k]);
      if (text != null) n.textContent = text;
      return n;
    };

    for (let i = 0; i <= 4; i++) {
      const v = yMin + ((yMax - yMin) * i) / 4;
      const y = yTo(v);
      svg.appendChild(el('line', { class: 'mkt-cr-grid', x1: padL, x2: W - padR, y1: y, y2: y }));
      svg.appendChild(el('text', {
        class: 'mkt-cr-axis', x: padL - 7, y: y + 3, 'text-anchor': 'end',
      }, abbrev(v)));
    }

    svg.appendChild(el('path', {
      class: 'mkt-cr-band',
      d: 'M' + rows.map((d) => xTo(d.x).toFixed(1) + ',' + yTo(d.hi).toFixed(1)).join('L')
        + 'L' + rows.slice().reverse()
          .map((d) => xTo(d.x).toFixed(1) + ',' + yTo(d.lo).toFixed(1)).join('L') + 'Z',
    }));
    svg.appendChild(el('path', {
      class: 'mkt-cr-line',
      d: 'M' + rows.map((d) => xTo(d.x).toFixed(1) + ',' + yTo(d.mid).toFixed(1)).join('L'),
    }));

    const last = rows[rows.length - 1];
    svg.appendChild(el('circle', {
      class: 'mkt-cr-dot', cx: xTo(last.x), cy: yTo(last.mid), r: 3.5,
    }));

    for (const [v, anchor] of [[xMin, 'start'], [xMax, 'end']]) {
      svg.appendChild(el('text', {
        class: 'mkt-cr-axis', x: xTo(v), y: H - 8, 'text-anchor': anchor,
      }, dayLabel(v)));
    }

    $chart.innerHTML = '';
    $chart.appendChild(svg);

    if ($chartMeta) {
      const first = rows[0].mid;
      const change = first ? ((last.mid - first) / first) * 100 : 0;
      const dir = change >= 0 ? t('up') : t('down');
      // "Latest day", not "now": this is the most recent daily bucket, while
      // the card above is the median across currently active listings. They
      // are different windows and will not match exactly.
      $chartMeta.textContent = t('Latest day') + ' ' + fmt(round2(last.mid)) + ' '
        + t('flux per credit') + ' - ' + dir + ' '
        + Math.abs(round2(change)) + '% ' + t('over 30 days');
    }
  }

  function dayLabel(sec) {
    const d = new Date(sec * 1000);
    return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
  }

  function abbrev(n) {
    const a = Math.abs(n);
    if (a >= 1e6) return round2(n / 1e6) + 'M';
    if (a >= 1e3) return round2(n / 1e3) + 'k';
    return String(round2(n));
  }

  /* ─── Helpers ───────────────────────────────────────────────────────*/
  // Accepts "3,000,000", "3 000 000", "3000000", "1.67". Returns null for
  // an empty or junk field so a blank box reads as "nothing yet" rather
  // than zero.
  function parseNum(raw) {
    const cleaned = String(raw == null ? '' : raw).replace(/[^0-9.]/g, '');
    if (!cleaned) return null;
    const n = parseFloat(cleaned);
    return isFinite(n) ? n : null;
  }

  function round2(n) { return Math.round(n * 100) / 100; }

  function fmt(n) {
    if (!isFinite(n)) return '-';
    return n.toLocaleString(undefined, { maximumFractionDigits: 2 });
  }

  function t(s) { return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s; }
  function rerunI18n() { if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh(); }
})();
