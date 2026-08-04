/* ═══════════════════════════════════════════════════════════════════════
   /market - Listing fee calculator (Fees tab)
   ───────────────────────────────────────────────────────────────────────
   Owns two things:

     1. ``window.BTTMarketFee`` - the pure fee/tax math. market.js reuses
        it to price the Fee / Seller nets columns on every listing row,
        so the curve lives in exactly one place.
     2. The Fees tab UI - a price input, the four outcome cards, and an
        SVG curve of the fee across the whole price range.

   Client-only, no API: the curve is a published game constant rather
   than scraped data, so nothing here can go stale between deploys.
   ═══════════════════════════════════════════════════════════════════════ */

(function () {
  'use strict';

  const { esc } = window.BTTUtil;

  /* ─── The fee curve ──────────────────────────────────────────────────
     Trove charges a listing fee UP FRONT, computed as a cubic Bézier
     over the normalized listing price. The game's own code spells it as
     three nested lerps (that's De Casteljau's algorithm); the closed
     form below is the identical curve and reproduces every dev-published
     sample to the flux:

       15,000 → 51    250,000 → 77       1,000,000 → 165
       10M    → 2,513  25M    → 18,904   40M+      → 65,000

       fee(t) = 50(1-t)³ + 3·1500·t(1-t)² + 3·6000·t²(1-t) + 65000·t³
       t      = (min(price, SATURATION) - 1) / (SATURATION - 1)

     SATURATION (40M) is where the curve *reaches* its 65,000 cap - it is
     NOT the maximum listable price. Anything listed above 40M simply
     pays the flat cap. Getting this backwards is the single easiest way
     to misread the formula, hence the name.

     The published samples only match with truncation, not rounding
     (25M → 18904.0032 → 18,904), so Math.floor is deliberate.          */
  const PTS = [50, 1500, 6000, 65000];
  const SATURATION = 40000000;
  const FEE_CAP = PTS[3];
  const TAX_RATE = 0.10;

  function listingFee(price) {
    const p = Math.max(1, Math.floor(Number(price) || 0));
    const t = (Math.min(p, SATURATION) - 1) / (SATURATION - 1);
    const u = 1 - t;
    return Math.floor(
      PTS[0] * u * u * u +
      3 * PTS[1] * t * u * u +
      3 * PTS[2] * t * t * u +
      PTS[3] * t * t * t
    );
  }

  // 10% of the sale price, taken only when the listing actually sells.
  // The game hasn't published whether it floors or rounds this; the two
  // differ by at most 1 flux, and flooring keeps it consistent with the
  // listing fee above.
  function saleTax(price) {
    return Math.floor(Math.max(0, Number(price) || 0) * TAX_RATE);
  }

  // What the seller actually banks on a completed sale. The listing fee
  // is refunded on a sale, so it does NOT appear here - it only ever
  // costs you when the listing expires unsold.
  function netIfSold(price) {
    const p = Math.max(0, Math.floor(Number(price) || 0));
    return p - saleTax(p);
  }

  window.BTTMarketFee = {
    listingFee, saleTax, netIfSold,
    SATURATION, FEE_CAP, TAX_RATE,
  };

  /* ─── Fees tab UI ───────────────────────────────────────────────────
     Everything below is the tab itself. Bail out cleanly on any page
     that loads the math but not the markup.                            */
  const $ = (id) => document.getElementById(id);
  const $view = $('mkt-view-fees');
  if (!$view) return;

  const $input = $('mkt-fee-price');
  const $presets = $('mkt-fee-presets');
  const $cards = $('mkt-fee-cards');
  const $chart = $('mkt-fee-chart');
  const $chartMeta = $('mkt-fee-chart-meta');
  const $modes = $('mkt-fee-modes');

  const PRESETS = [15000, 250000, 1000000, 10000000, 40000000];

  const state = {
    price: 1000000, // null while the field is empty - see renderCards()
    mode: 'flux',   // 'flux' | 'rate'
    shown: false,   // has the tab been visible at least once
  };

  boot();

  function boot() {
    if ($input) {
      $input.value = String(state.price);
      // Digits only, so pasting "1,000,000" or "1 000 000" works. An empty
      // field means "no price yet", NOT a 1-flux listing - clamping a blank
      // box to 1 would flash a 5000% fee rate at anyone who clears it.
      $input.addEventListener('input', () => {
        const digits = $input.value.replace(/[^0-9]/g, '');
        state.price = digits ? Math.max(1, parseInt(digits, 10)) : null;
        render();
      });
    }
    if ($presets) {
      $presets.innerHTML = PRESETS.map((p) => `
        <button type="button" class="mkt-fee-preset" data-price="${p}">
          ${esc(abbrev(p))}
        </button>`).join('');
      $presets.addEventListener('click', (e) => {
        const btn = e.target.closest('.mkt-fee-preset');
        if (!btn) return;
        state.price = Number(btn.dataset.price);
        if ($input) $input.value = String(state.price);
        render();
      });
    }
    if ($modes) {
      $modes.addEventListener('click', (e) => {
        const btn = e.target.closest('.mkt-fee-mode');
        if (!btn || btn.dataset.mode === state.mode) return;
        state.mode = btn.dataset.mode;
        for (const b of $modes.querySelectorAll('.mkt-fee-mode')) {
          const on = b.dataset.mode === state.mode;
          b.classList.toggle('active', on);
          b.setAttribute('aria-pressed', String(on));
        }
        drawCurve();
      });
    }

    // The SVG measures its container, so it can only be laid out once the
    // panel is actually visible. market-analytics.js owns the tablist and
    // announces every switch.
    document.addEventListener('btt-mkt-view', (e) => {
      if (e.detail !== 'fees') return;
      state.shown = true;
      render();
    });

    let resizeTimer = null;
    window.addEventListener('resize', () => {
      clearTimeout(resizeTimer);
      resizeTimer = setTimeout(() => { if (state.shown) drawCurve(); }, 140);
    });

    document.addEventListener('btt-lang-changed', () => { if (state.shown) render(); });

    renderCards();
  }

  function render() {
    renderCards();
    drawCurve();
  }

  // ─── Outcome cards ─────────────────────────────────────────────────
  // Four numbers, in the order a seller actually asks them: what does it
  // cost me now, is that a lot, what do I walk away with, what do I lose
  // if nobody bites.
  function renderCards() {
    if (!$cards) return;
    const price = state.price;
    const blank = price == null;
    const fee = blank ? 0 : listingFee(price);
    const rate = blank ? 0 : (fee / price) * 100;

    const card = (label, value, sub, cls) => `
      <div class="mkt-fee-card${cls ? ' ' + cls : ''}">
        <span class="mkt-fee-card-label">${esc(t(label))}</span>
        <span class="mkt-fee-card-value">${esc(value)}</span>
        <span class="mkt-fee-card-sub">${esc(sub)}</span>
      </div>`;

    $cards.innerHTML =
      card('Listing fee', blank ? '-' : full(fee),
           price >= SATURATION ? t('Capped - the fee stops growing past 40M')
                               : t('Charged the moment you list'),
           'mkt-fee-card-cost') +
      card('Fee rate',
           blank ? '-' : (rate < 0.01 ? '<0.01%' : rate.toFixed(rate < 1 ? 3 : 2) + '%'),
           t('Share of the asking price'), '') +
      card('If it sells', blank ? '-' : full(netIfSold(price)),
           t('After the 10% tax; fee refunded'), 'mkt-fee-card-good') +
      card('If it expires', blank ? '-' : '-' + full(fee),
           t('No tax, but the fee is gone'), 'mkt-fee-card-bad');
  }

  // ─── Fee curve ─────────────────────────────────────────────────────
  // Log-x across the whole usable price range. Two readings: the raw fee
  // in flux (a rising S), and the fee as a share of the price - which is
  // the shape most people don't expect, a U that bottoms out near 1M and
  // climbs again toward the cap.
  const X_MIN = 1000;
  const X_MAX = 50000000;

  function drawCurve() {
    if (!$chart) return;
    $chart.innerHTML = '';

    const W = Math.max(280, Math.round($chart.clientWidth || 600));
    const H = Math.max(180, Math.round($chart.clientHeight || 240));
    const padL = 54, padR = 14, padT = 12, padB = 28;
    const plotW = W - padL - padR;
    const plotH = H - padT - padB;
    if (plotW <= 0 || plotH <= 0) return;

    const rate = state.mode === 'rate';
    const lg = Math.log10;
    const xToPx = (v) =>
      padL + ((lg(Math.min(X_MAX, Math.max(X_MIN, v))) - lg(X_MIN)) /
              (lg(X_MAX) - lg(X_MIN))) * plotW;

    // Flux mode is linear (0 → the 65k cap). Rate mode spans three orders
    // of magnitude (~0.016% to 5%), so it needs a log axis or the whole
    // right-hand climb flattens into the baseline.
    const yMinR = 0.01, yMaxR = 10;   // percent, log decades
    const yToPx = rate
      ? (v) => padT + (1 - (lg(Math.min(yMaxR, Math.max(yMinR, v))) - lg(yMinR)) /
                           (lg(yMaxR) - lg(yMinR))) * plotH
      : (v) => padT + (1 - Math.min(1, Math.max(0, v / FEE_CAP))) * plotH;

    const svgNS = 'http://www.w3.org/2000/svg';
    const svg = document.createElementNS(svgNS, 'svg');
    svg.setAttribute('viewBox', `0 0 ${W} ${H}`);
    svg.setAttribute('preserveAspectRatio', 'xMidYMid meet');

    const el = (name, attrs, text) => {
      const n = document.createElementNS(svgNS, name);
      for (const k in attrs) n.setAttribute(k, attrs[k]);
      if (text != null) n.textContent = text;
      return n;
    };

    // Y grid + labels.
    const yTicks = rate
      ? [0.01, 0.1, 1, 10]
      : [0, FEE_CAP * 0.25, FEE_CAP * 0.5, FEE_CAP * 0.75, FEE_CAP];
    for (const v of yTicks) {
      const y = yToPx(v);
      svg.appendChild(el('line', {
        class: 'mkt-fee-grid', x1: padL, x2: W - padR, y1: y, y2: y,
      }));
      svg.appendChild(el('text', {
        class: 'mkt-fee-axis', x: padL - 6, y: y + 3, 'text-anchor': 'end',
      }, rate ? fmtPct(v) : abbrev(v)));
    }

    // X labels - one per decade.
    for (let d = 3; d <= 7; d++) {
      const v = Math.pow(10, d);
      if (v < X_MIN || v > X_MAX) continue;
      const x = xToPx(v);
      svg.appendChild(el('text', {
        class: 'mkt-fee-axis', x, y: H - 8,
        'text-anchor': d === 3 ? 'start' : 'middle',
      }, abbrev(v)));
    }

    // The curve itself.
    const SAMPLES = 220;
    const pts = [];
    for (let i = 0; i <= SAMPLES; i++) {
      const price = Math.pow(10, lg(X_MIN) + (lg(X_MAX) - lg(X_MIN)) * (i / SAMPLES));
      const fee = listingFee(price);
      pts.push([xToPx(price), yToPx(rate ? (fee / price) * 100 : fee)]);
    }
    svg.appendChild(el('path', {
      class: 'mkt-fee-line',
      d: 'M' + pts.map(([x, y]) => `${x.toFixed(1)},${y.toFixed(1)}`).join('L'),
    }));

    // Where the cap kicks in. Only meaningful on the flux reading - on
    // the rate curve 40M is just a point on the way up, not a ceiling.
    if (!rate) {
      const capX = xToPx(SATURATION);
      svg.appendChild(el('line', {
        class: 'mkt-fee-cap', x1: capX, x2: capX, y1: padT, y2: padT + plotH,
      }));
      svg.appendChild(el('text', {
        class: 'mkt-fee-cap-label', x: capX - 6, y: padT + 12, 'text-anchor': 'end',
      }, t('cap')));
    }

    // Marker for the price currently in the box (none while it's empty).
    const p = state.price;
    if (p != null && p >= X_MIN && p <= X_MAX) {
      const fee = listingFee(p);
      const mx = xToPx(p);
      const my = yToPx(rate ? (fee / p) * 100 : fee);
      svg.appendChild(el('line', {
        class: 'mkt-fee-marker', x1: mx, x2: mx, y1: my, y2: padT + plotH,
      }));
      svg.appendChild(el('circle', { class: 'mkt-fee-dot', cx: mx, cy: my, r: 4 }));
    }

    svg.setAttribute('role', 'img');
    svg.setAttribute('aria-label', rate
      ? t('Listing fee as a share of the asking price, from 1k to 50M flux')
      : t('Listing fee in flux, from 1k to 50M asking price'));
    $chart.appendChild(svg);

    if ($chartMeta) {
      $chartMeta.textContent = (p != null && p > X_MAX)
        ? t('Above 40M the fee is flat at 65,000 flux.')
        : t('The fee is cheapest, relative to price, around the 1M mark.');
    }
  }

  // ─── Formatting ────────────────────────────────────────────────────
  function full(n) { return Number(n || 0).toLocaleString(); }

  function abbrev(n) {
    n = Number(n || 0);
    if (n >= 1000000) return (n / 1000000).toFixed(n % 1000000 ? 1 : 0) + 'M';
    if (n >= 1000) return (n / 1000).toFixed(n % 1000 ? 1 : 0) + 'k';
    return String(Math.round(n));
  }

  function fmtPct(v) {
    return (v < 1 ? v.toFixed(2).replace(/0+$/, '').replace(/\.$/, '') : String(v)) + '%';
  }

  function t(s) {
    return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s;
  }
})();
