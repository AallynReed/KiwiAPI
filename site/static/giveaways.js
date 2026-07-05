/* ═══════════════════════════════════════════════════════════════════════
   giveaways.js - public /giveaways page
   ───────────────────────────────────────────────────────────────────────
   Reads the public list from /site/giveaways (same-origin, cached). Entering
   needs a signed-in SiteUser, so the Enter button + "your odds" go through
   window.BTTAuth (cross-origin to the API with the site token). Counts +
   statuses refresh on a 30s poll; the countdown ticks every second.
   ═══════════════════════════════════════════════════════════════════════ */
(function () {
  'use strict';

  const { esc } = window.BTTUtil;

  const Auth = window.BTTAuth || null;
  const $list = document.getElementById('gw-list');
  if (!$list) return;

  let items = [];
  let entered = new Set();
  let me = null;
  let ticker = null;

  const fmtNum = (n) => Number(n).toLocaleString();
  // Odds of winning as a percentage = 1 / entrants. e.g. 43 entrants -> "2.3%".
  function oddsText(count) {
    if (!count || count < 1) return '—';
    const pct = 100 / count;
    if (pct < 0.01) return '<0.01%';
    return (pct < 1 ? pct.toFixed(2) : pct.toFixed(1)).replace(/\.0+$/, '') + '%';
  }

  function countdown(toIso) {
    const ms = new Date(toIso).getTime() - Date.now();
    if (ms <= 0) return null;
    const s = Math.floor(ms / 1000);
    const d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600);
    const m = Math.floor((s % 3600) / 60), sec = s % 60;
    if (d > 0) return `${d}d ${h}h ${m}m`;
    if (h > 0) return `${h}h ${m}m ${sec}s`;
    return `${m}m ${sec}s`;
  }

  async function load() {
    let data;
    try {
      const r = await fetch('/site/giveaways');
      data = await r.json();
      items = data.items || [];
    } catch (_) {
      $list.innerHTML = `<p class="gw-empty">Couldn't load giveaways. Try again shortly.</p>`;
      return;
    }
    // Signed-in extras: who I am + which giveaways I've entered.
    if (Auth) {
      try { me = await Auth.getMe(); } catch (_) { me = null; }
      if (me) {
        try {
          const r = await Auth.callJSON('/v1/giveaways/mine');
          if (r.ok && r.data) entered = new Set(r.data.giveaway_ids || []);
        } catch (_) { /* leave entered as-is */ }
      }
    }
    render();
  }

  function card(g) {
    const youEntered = entered.has(g.id);
    let cta = '';
    if (g.status === 'open') {
      if (!me) cta = `<a class="gw-btn" href="/login?next=/giveaways">Sign in to enter</a>`;
      else if (youEntered) cta = `<button class="gw-btn entered" disabled>✓ You're entered</button>`;
      else cta = `<button class="gw-btn primary" data-enter="${g.id}">Enter giveaway</button>`;
    }

    let meta = '';
    if (g.status === 'open') {
      const odds = youEntered ? oddsText(g.entry_count) : oddsText(g.entry_count + 1);
      meta = `
        <div class="gw-stat"><span class="gw-stat-n">${fmtNum(g.entry_count)}</span><span class="gw-stat-l">entries</span></div>
        <div class="gw-stat"><span class="gw-stat-n">${odds}</span><span class="gw-stat-l">${youEntered ? 'your odds' : 'odds if you enter'}</span></div>
        <div class="gw-stat"><span class="gw-stat-n gw-cd" data-end="${esc(g.ends_at)}">…</span><span class="gw-stat-l">closes in</span></div>`;
    } else if (g.status === 'scheduled') {
      meta = `<div class="gw-stat"><span class="gw-stat-n gw-cd" data-start="${esc(g.starts_at)}">…</span><span class="gw-stat-l">opens in</span></div>`;
    } else if (g.status === 'drawn') {
      meta = `<div class="gw-winner">🎉 Winner: <strong>${esc(g.winner_username || '—')}</strong></div>`;
    } else if (g.status === 'closed') {
      meta = `<div class="gw-winner muted">Closed — no entrants</div>`;
    }

    return `
      <article class="gw-card status-${esc(g.status)}">
        <div class="gw-card-main">
          <span class="gw-badge gw-badge-${esc(g.status)}">${esc(g.status)}</span>
          <h3 class="gw-prize">${esc(g.prize_name)}</h3>
          <p class="gw-name">${esc(g.title)}</p>
          ${g.description ? `<p class="gw-desc">${esc(g.description)}</p>` : ''}
        </div>
        <div class="gw-card-side">
          <div class="gw-meta">${meta}</div>
          ${cta}
        </div>
      </article>`;
  }

  function section(title, list) {
    if (!list.length) return '';
    return `<section class="gw-section"><h2 class="gw-section-title">${title}</h2>
      <div class="gw-grid">${list.map(card).join('')}</div></section>`;
  }

  function render() {
    if (!items.length) {
      $list.innerHTML = `<p class="gw-empty">No giveaways right now — check back soon!</p>`;
      return;
    }
    const open = items.filter((g) => g.status === 'open');
    const upcoming = items.filter((g) => g.status === 'scheduled');
    const past = items.filter((g) => g.status === 'drawn' || g.status === 'closed');
    $list.innerHTML = section('Open now', open) + section('Upcoming', upcoming) + section('Past', past);
    $list.querySelectorAll('[data-enter]').forEach((b) =>
      b.addEventListener('click', () => enter(b.dataset.enter, b)));
    startTicker();
  }

  async function enter(id, btn) {
    if (!Auth) { location.href = '/login?next=/giveaways'; return; }
    btn.disabled = true;
    try {
      const r = await Auth.callJSON('/v1/giveaways/' + id + '/enter', { method: 'POST', json: {} });
      if (!r.ok) {
        btn.disabled = false;
        alert((r.data && r.data.error && r.data.error.message) || "Couldn't enter — try again.");
        return;
      }
      entered.add(id);
      const g = items.find((x) => x.id === id);
      if (g && r.data) g.entry_count = r.data.entry_count;
      render();
    } catch (_) {
      btn.disabled = false;
      alert("Couldn't enter — try again.");
    }
  }

  function startTicker() {
    if (ticker) clearInterval(ticker);
    const update = () => {
      $list.querySelectorAll('.gw-cd').forEach((el) => {
        const txt = countdown(el.dataset.end || el.dataset.start);
        el.textContent = txt == null ? (el.dataset.end ? 'closing…' : 'opening…') : txt;
      });
    };
    update();
    ticker = setInterval(update, 1000);
  }

  load().then(() => setInterval(load, 30000));  // keep counts + statuses fresh
})();
