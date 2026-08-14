/* ═══════════════════════════════════════════════════════════════════════
   /tomes - what each tome pays out, priced at live market medians
   ───────────────────────────────────────────────────────────────────────
   Three lists, because the tomes answer three different questions:

     regular    repeatable without limit -> ranked, the top one is the best
                flux per dungeon run available
     legendary  one of each per week -> a checklist against the reset, still
                ranked so limited run time goes to the best ones first
     unpriced   pays out something untradeable -> shown, never guessed at

   Tick state is localStorage only. It is a personal checklist, not an
   account feature, so there is no reason for it to reach the server.
   ═══════════════════════════════════════════════════════════════════════ */

(function () {
  'use strict';

  const { esc, fetchJSON, apiUrl } = window.BTTUtil;

  const TICK_KEY = 'btt.tomes.ticked';

  const $ = (id) => document.getElementById(id);
  const $regular = $('tm-regular');
  const $legendary = $('tm-legendary');
  const $unpriced = $('tm-unpriced');
  const $reset = $('tm-reset');
  const $progress = $('tm-progress');
  const $clear = $('tm-clear');

  const state = {
    data: null,
    ticked: loadTicks(),
    resetTimer: null,
  };

  boot();

  async function boot() {
    if ($clear) {
      $clear.addEventListener('click', () => {
        state.ticked = { week: weekKey(), names: [] };
        saveTicks();
        render();
      });
    }
    document.addEventListener('btt-lang-changed', render);

    for (const el of [$regular, $legendary, $unpriced]) {
      if (el) el.innerHTML = '<p class="tm-loading" data-i18n>Loading…</p>';
    }
    try {
      state.data = await fetchJSON('/site/tomes');
    } catch (err) {
      for (const el of [$regular, $legendary, $unpriced]) {
        if (el) el.innerHTML = '<p class="tm-error">' + esc(t('Failed to load')) + '</p>';
      }
      return;
    }
    startResetClock();
    render();
  }

  /* ─── Weekly ticks ──────────────────────────────────────────────────
     Stamped with the reset the ticks belong to, so the list clears itself
     the moment a new week starts rather than carrying stale ones over. */
  function weekKey() {
    return state.data ? String(state.data.weekly_reset_at) : 'pending';
  }

  function loadTicks() {
    try {
      const raw = JSON.parse(localStorage.getItem(TICK_KEY) || 'null');
      if (raw && Array.isArray(raw.names)) return raw;
    } catch (_) { /* private mode, or someone edited it - start clean */ }
    return { week: null, names: [] };
  }

  function saveTicks() {
    try {
      localStorage.setItem(TICK_KEY, JSON.stringify(state.ticked));
    } catch (_) { /* private mode - ticks just will not persist */ }
  }

  function ticks() {
    if (state.ticked.week !== weekKey()) {
      state.ticked = { week: weekKey(), names: [] };
      saveTicks();
    }
    return new Set(state.ticked.names);
  }

  function toggle(name) {
    const set = ticks();
    if (set.has(name)) set.delete(name);
    else set.add(name);
    state.ticked = { week: weekKey(), names: [...set] };
    saveTicks();
    render();
  }

  /* ─── Reset countdown ───────────────────────────────────────────────*/
  function startResetClock() {
    if (state.resetTimer) clearInterval(state.resetTimer);
    drawReset();
    state.resetTimer = setInterval(drawReset, 1000);
  }

  function drawReset() {
    if (!$reset || !state.data) return;
    const left = state.data.weekly_reset_at - Math.floor(Date.now() / 1000);
    if (left <= 0) {
      $reset.innerHTML = '<span class="tm-reset-label">'
        + esc(t('Legendary tomes have reset')) + '</span>';
      return;
    }
    const d = Math.floor(left / 86400);
    const h = Math.floor((left % 86400) / 3600);
    const m = Math.floor((left % 3600) / 60);
    const s = left % 60;
    const parts = d > 0 ? [d + 'd', h + 'h', m + 'm'] : [h + 'h', m + 'm', s + 's'];
    $reset.innerHTML = '<span class="tm-reset-label">' + esc(t('Resets in')) + '</span>'
      + '<span class="tm-reset-clock">' + esc(parts.join(' ')) + '</span>';
  }

  /* ─── Render ────────────────────────────────────────────────────────*/
  function render() {
    if (!state.data) return;
    const all = state.data.tomes;
    const priced = (t2) => t2.status === 'priced';

    const regular = all.filter((x) => x.type === 'regular' && priced(x)).sort(byValue);
    const legendary = all.filter((x) => x.type === 'legendary' && priced(x)).sort(byValue);
    const unpriced = all.filter((x) => !priced(x)).sort((a, b) =>
      a.type === b.type ? a.name.localeCompare(b.name) : (a.type === 'regular' ? -1 : 1));

    $regular.innerHTML = list(regular, { rank: true });
    $legendary.innerHTML = list(legendary, { rank: true, tick: true });
    $unpriced.innerHTML = list(unpriced, { reason: true });

    wireTicks();
    wireIcons();
    renderProgress(legendary);
    rerunI18n();
  }

  function byValue(a, b) { return (b.value || 0) - (a.value || 0); }

  function renderProgress(legendary) {
    if (!$progress) return;
    const set = ticks();
    const done = legendary.filter((x) => set.has(x.name)).length;
    const total = state.data.tomes.filter((x) => x.type === 'legendary').length;
    $progress.textContent = done + ' / ' + total + ' ' + t('done this week');
  }

  function rewardText(r) {
    const via = r.via ? ' (' + t('priced from') + ' ' + r.via + ')' : '';
    return fmt(r.amount) + ' x ' + r.item + via;
  }

  function reasonFor(x) {
    if (x.status === 'no_payout_data') return t('Payout not recorded');
    if (x.status === 'untradeable') {
      return t('Not evaluated') + ' - ' + x.untradeable.join(', ')
        + ' ' + t('cannot be traded');
    }
    if (x.status === 'unlisted') {
      return t('Nothing listed right now') + ' - ' + x.unlisted.join(', ');
    }
    return '';
  }

  /* ─── Rows ──────────────────────────────────────────────────────────
     Icon | name over payout | value over unit price. A tome is a thing
     you recognise by its art long before you read its name, so the icon
     leads. Tomes we could not pin to game art get a book glyph rather
     than a borrowed icon - a confidently wrong one reads as fact. */
  function iconHTML(x) {
    if (!x.blueprint) {
      return '<span class="tm-icon is-fallback" aria-hidden="true">'
        + '<i class="fa-solid fa-book"></i></span>';
    }
    const src = apiUrl('/site/codexes/render?blueprint='
      + encodeURIComponent(x.blueprint)
      + '&branch=' + encodeURIComponent(state.data.icon_branch || 'live-us')
      + '&dim=96');
    return '<span class="tm-icon"><img loading="lazy" decoding="async" alt=""'
      + ' src="' + esc(src) + '"></span>';
  }

  function list(rows, opts) {
    if (!rows.length) {
      return '<p class="tm-empty">' + esc(t('Nothing to show here.')) + '</p>';
    }
    const best = state.data.best_regular;
    const set = opts.tick ? ticks() : null;
    // Scored against the best tome in THIS list, so the number stays 0-100%
    // and reads as "how close to the best option here" - the same question
    // the gem evaluator's quality % answers. Comparing legendaries to the best
    // regular instead would put good ones in the thousands of percent.
    const top = rows.reduce((m, x) => Math.max(m, x.value || 0), 0);

    const head = '<div class="tm-listhead">'
      + '<span>' + esc(t('Tome')) + '</span>'
      + '<span>' + esc(opts.reason ? t('Why not') : t('Sell value')) + '</span>'
      + '</div>';

    const body = rows.map((x) => {
      const done = set && set.has(x.name);
      const weak = opts.rank && best != null && x.value != null
        && x.value < best && x.type === 'legendary';
      const payout = x.rewards.map(rewardText).join('<br>') || '-';

      // Per-unit only where it is unambiguous: with two rewards there is no
      // single "each" to quote, so it is left off rather than picked from one.
      const one = x.rewards.length === 1 ? x.rewards[0] : null;
      const each = one && one.unit_price != null
        ? '<span class="tm-each">(' + esc(fmt(round2(one.unit_price))) + ' '
          + esc(t('ea')) + ')</span>'
        : '';

      const pct = (!opts.reason && top > 0 && x.value != null)
        ? (x.value / top) * 100 : null;

      const value = opts.reason
        ? '<span class="tm-reason">' + esc(reasonFor(x)) + '</span>'
        : '<span class="tm-total">' + esc(fmt(Math.round(x.value))) + '</span>' + each;

      // The fill sits behind the row rather than beside it - a value bar that
      // took its own column would crowd out the payout line on a phone.
      const fill = pct == null ? ''
        : '<span class="tm-fill" style="width:' + pct.toFixed(2) + '%"></span>';

      return '<article class="tm-row' + (done ? ' is-done' : '') + '">' + fill
        + (opts.tick
            ? '<input type="checkbox" class="tm-tick" data-tome="' + esc(x.name) + '"'
              + (done ? ' checked' : '') + ' aria-label="' + esc(x.name) + '">'
            : '')
        + iconHTML(x)
        + '<div class="tm-info">'
        + '<span class="tm-name">' + esc(x.name)
        + (weak ? '<span class="tm-flag" title="'
                  + esc(t('Worth less than the best regular tome, which you can farm as often as you like'))
                  + '">' + esc(t('below a regular tome')) + '</span>' : '')
        + '</span>'
        + '<span class="tm-payout">' + payout + '</span>'
        + (x.note ? '<span class="tm-note">' + esc(x.note) + '</span>' : '')
        + '</div>'
        + '<div class="tm-value">' + value + '</div>'
        + '</article>';
    }).join('');

    return head + '<div class="tm-rows">' + body + '</div>';
  }

  function wireTicks() {
    for (const box of document.querySelectorAll('.tm-tick')) {
      box.addEventListener('change', () => toggle(box.dataset.tome));
    }
  }

  // A blueprint that no longer renders would leave a broken-image box, so the
  // icon falls back to the glyph instead. Bound here rather than with an inline
  // onerror so nothing depends on the CSP allowing inline handlers.
  function wireIcons() {
    for (const img of document.querySelectorAll('.tm-icon img')) {
      img.addEventListener('error', () => {
        const wrap = img.closest('.tm-icon');
        if (!wrap) return;
        wrap.classList.add('is-fallback');
        wrap.innerHTML = '<i class="fa-solid fa-book" aria-hidden="true"></i>';
      }, { once: true });
    }
  }

  /* ─── Helpers ───────────────────────────────────────────────────────*/
  function round2(n) { return Math.round(n * 100) / 100; }

  function fmt(n) {
    if (n == null || !isFinite(n)) return '-';
    return n.toLocaleString(undefined, { maximumFractionDigits: 0 });
  }

  function t(s) { return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s; }
  function rerunI18n() { if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh(); }
})();
