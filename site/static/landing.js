/* ═══════════════════════════════════════════════════════════════════════
   Better Trove Tools - landing page enhancement layer
   ───────────────────────────────────────────────────────────────────────
   Runs AFTER app.js (the download dropdown + nav-toggle logic) and i18n.js.
   We deliberately don't touch the hamburger / dropdown wiring - those are
   already proven; this file owns the visual+data enhancements that are
   new in the 2026-06-07 redesign:

     1. ready-class on <body> so the hero title's staggered entry plays
        once layout has settled (avoids the first paint flashing)
     2. canvas particle background (low-density, paused when hidden or
        when prefers-reduced-motion)
     3. IntersectionObserver-driven .in-view triggers for spotlights +
        bento tiles
     4. animated number counters for the stats strip
     5. live API data for the strip (server time, trove day, chaos chest,
        challenge), refreshed every 30s
     6. nav .scrolled flip when scrolled past the hero (drives the
        glassmorphic opacity boost in CSS)

   Everything is guarded so missing DOM (e.g. if HTML is trimmed) just
   silently no-ops - the rest of the page keeps working.
   ═══════════════════════════════════════════════════════════════════════ */

(function () {
  'use strict';

  const RM = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  const API = 'https://api.aallyn.net';

  // ── 1. ready flag (drives hero entry animation) ────────────────────
  // Wait one rAF after DOMContentLoaded so the initial styles have applied
  // before the transition kicks off - otherwise the transition can fire
  // mid-paint and skip the "from" state visually.
  requestAnimationFrame(() => requestAnimationFrame(() => {
    document.body.classList.add('ready');
  }));

  // ── 2. Nav scrolled flag ───────────────────────────────────────────
  // Pure CSS sticky already keeps it pinned; this class just lets us
  // bump the bg opacity once the user has scrolled past the very top.
  const navbar = document.querySelector('.navbar');
  if (navbar) {
    let ticking = false;
    const update = () => {
      navbar.classList.toggle('scrolled', window.scrollY > 40);
      ticking = false;
    };
    window.addEventListener('scroll', () => {
      if (!ticking) { requestAnimationFrame(update); ticking = true; }
    }, { passive: true });
    update();
  }

  // ── 3. IntersectionObserver triggers ───────────────────────────────
  // Adds .in-view the first time an element crosses 15% of the viewport;
  // CSS handles the rest (fade-in, scale-in, etc). One-shot per element.
  if ('IntersectionObserver' in window) {
    const io = new IntersectionObserver((entries) => {
      for (const e of entries) {
        if (e.isIntersecting) {
          e.target.classList.add('in-view');
          io.unobserve(e.target);
        }
      }
    }, { rootMargin: '0px 0px -10% 0px', threshold: 0.15 });
    document.querySelectorAll('.spotlight, .bento-tile').forEach(el => io.observe(el));
  } else {
    // Older browsers: just surface everything immediately.
    document.querySelectorAll('.spotlight, .bento-tile').forEach(el => el.classList.add('in-view'));
  }

  // ── 4. Animated number counters ────────────────────────────────────
  // Each .stat-num with data-target counts from 0 → target over ~1.4s
  // using an ease-out curve. Reduced motion → set the final value
  // directly and skip the animation.
  if ('IntersectionObserver' in window) {
    const statIO = new IntersectionObserver((entries) => {
      for (const e of entries) {
        if (!e.isIntersecting) continue;
        const el = e.target;
        const target = parseInt(el.dataset.target, 10);
        statIO.unobserve(el);
        if (!Number.isFinite(target)) continue;
        if (RM) { el.textContent = target.toLocaleString(); continue; }
        const start = performance.now();
        const dur = 1400;
        const ease = t => 1 - Math.pow(1 - t, 3);  // easeOutCubic
        const tick = (now) => {
          const t = Math.min(1, (now - start) / dur);
          el.textContent = Math.round(target * ease(t)).toLocaleString();
          if (t < 1) requestAnimationFrame(tick);
        };
        requestAnimationFrame(tick);
      }
    }, { threshold: 0.4 });
    document.querySelectorAll('.stat-num[data-target]').forEach(el => statIO.observe(el));
  }

  // ── 4.5 Hero screenshot slideshow ──────────────────────────────────
  // Fetches /site/screenshots.json (served by app/site/router.py), builds
  // an <img> per screenshot inside .hero-screens, and cycles them with
  // an opacity crossfade. The list is dynamic - drop a file in
  // site/static/trove-screens/ and it appears on next page load (the
  // endpoint caches for 60s).
  //
  // Reduced motion: still shows the first image (so the visual depth is
  // there), just skips the cycle so the page is static.
  (async () => {
    const slot = document.querySelector('.hero-screens');
    if (!slot) return;
    let urls = [];
    try {
      const r = await fetch('/site/screenshots.json', { cache: 'default' });
      if (!r.ok) throw new Error('HTTP ' + r.status);
      const data = await r.json();
      urls = Array.isArray(data.screenshots) ? data.screenshots : [];
    } catch (e) {
      console.warn('[landing] screenshot list fetch failed:', e.message);
      return;
    }
    if (urls.length === 0) return;

    // Build the <img> tags. Preload eagerly so the crossfade doesn't
    // stutter on the FIRST switch (every cycle after benefits from the
    // browser's HTTP cache). decoding="async" keeps decode off the main
    // thread so layout isn't blocked while a heavy screenshot resolves.
    const imgs = urls.map(src => {
      const i = document.createElement('img');
      i.src = src;
      i.alt = '';
      i.loading = 'eager';
      i.decoding = 'async';
      slot.appendChild(i);
      return i;
    });

    // First image visible immediately. requestAnimationFrame so the
    // class flip happens after the elements are in the layout tree -
    // otherwise the very first paint can skip the opacity transition.
    requestAnimationFrame(() => imgs[0].classList.add('active'));

    if (RM || imgs.length === 1) return;  // no cycle needed

    let cur = 0;
    const PERIOD = 8000;  // ms each image stays before crossfading out
    const advance = () => {
      const next = (cur + 1) % imgs.length;
      imgs[next].classList.add('active');
      imgs[cur].classList.remove('active');
      cur = next;
    };
    let timer = setInterval(advance, PERIOD);
    // Pause the cycle when the tab isn't visible - saves a tiny bit of
    // CPU but mostly avoids a "skip many at once" jolt when the tab
    // returns from being backgrounded for an hour.
    document.addEventListener('visibilitychange', () => {
      if (document.hidden) { clearInterval(timer); timer = null; }
      else if (!timer) { timer = setInterval(advance, PERIOD); }
    });
  })();

  // ── 5. Canvas particle background ──────────────────────────────────
  // Sparse drifting dots layered behind the orbs. Cheap: rAF, no shaders,
  // ~80 particles. Pauses when tab is hidden + when reduced-motion is on.
  // Wrapped in try/catch defensively: a canvas context failure (locked-down
  // browser, GPU disabled in headless contexts, etc.) used to take down the
  // live-data fetch below because the throw aborted the whole IIFE.
  try {
    const canvas = document.querySelector('canvas.bg-fx');
    if (canvas && !RM) {
      const ctx = canvas.getContext('2d', { alpha: true });
      if (ctx) {
        const dpr = Math.min(window.devicePixelRatio || 1, 2);
        let parts = [];
        let w = 0, h = 0, rafId = null;

        const seed = () => {
          const count = Math.min(80, Math.floor((w * h) / 22000));
          parts = Array.from({ length: count }, () => ({
            x: Math.random() * w,
            y: Math.random() * h,
            // Slow vertical drift, slight horizontal wobble.
            vx: (Math.random() - 0.5) * 0.06,
            vy: (Math.random() - 0.5) * 0.08 - 0.04,
            r: Math.random() * 1.4 + 0.4,
            a: Math.random() * 0.4 + 0.15,
          }));
        };
        const resize = () => {
          // clientWidth / clientHeight are READ-ONLY getters - you can't
          // assign through them. The displayed size already comes from CSS
          // (canvas.bg-fx is width:100% height:100%); we only need to set
          // the backing-store size (canvas.width / .height in device px).
          w = window.innerWidth;
          h = window.innerHeight;
          canvas.width  = Math.floor(w * dpr);
          canvas.height = Math.floor(h * dpr);
          ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
          seed();
        };

        const tick = () => {
          ctx.clearRect(0, 0, w, h);
          ctx.fillStyle = 'rgba(120, 180, 255, 1)';
          for (const p of parts) {
            p.x += p.vx;
            p.y += p.vy;
            if (p.x < -5) p.x = w + 5; else if (p.x > w + 5) p.x = -5;
            if (p.y < -5) p.y = h + 5; else if (p.y > h + 5) p.y = -5;
            ctx.globalAlpha = p.a;
            ctx.beginPath();
            ctx.arc(p.x, p.y, p.r, 0, Math.PI * 2);
            ctx.fill();
          }
          ctx.globalAlpha = 1;
          rafId = requestAnimationFrame(tick);
        };

        const start = () => { if (rafId === null) rafId = requestAnimationFrame(tick); };
        const stop = () => { if (rafId !== null) { cancelAnimationFrame(rafId); rafId = null; } };

        window.addEventListener('resize', () => { resize(); }, { passive: true });
        document.addEventListener('visibilitychange', () => {
          // No point burning CPU when the user can't see it.
          if (document.hidden) stop(); else start();
        });
        resize();
        start();
      }
    }
  } catch (e) {
    console.warn('[landing] canvas fx disabled:', e && e.message);
  }

  // ── 6. Live API data ───────────────────────────────────────────────
  // Server time, trove day, chaos chest, hourly challenge - refreshed
  // every 30 seconds. Server time itself ticks every second client-side
  // off a base offset so the second-hand doesn't only update on the API
  // poll. Failures keep the previous value rather than blanking the card.

  const $live = (key) => document.querySelector(`[data-live="${key}"]`);
  const cards = {
    serverTime: document.querySelector('[data-live="server-time-card"]'),
    troveDay:   document.querySelector('[data-live="trove-day-card"]'),
    chaos:      document.querySelector('[data-live="chaos-card"]'),
    challenge:  document.querySelector('[data-live="challenge-card"]'),
  };
  const fields = {
    serverTime: $live('server-time'),
    serverDate: $live('server-date'),
    troveDay:   $live('trove-day'),
    resetIn:    $live('reset-in'),
    chaosName:  $live('chaos-name'),
    chaosEnds:  $live('chaos-ends'),
    challName:  $live('challenge-name'),
    challKind:  $live('challenge-kind'),
  };

  // Anchor the live clock to the API's authoritative UTC, not the local
  // machine's clock (which can drift / be wrong). serverEpochOffset is
  // ms-since-load → server-time-now.
  let serverEpochOffset = null;
  let dailyResetAt = null;  // unix seconds

  // Trove's clock runs on UTC-11 (the daily reset is 11:00 UTC == midnight
  // UTC-11), so the "Server Time" card shows UTC-11, and hovering it reveals
  // the same instant across common player timezones. Ported from
  // BetterTroveTools (web/js/main.js globalTimezones).
  const TROVE_OFFSET_MS = 11 * 3600000;
  const GLOBAL_TIMEZONES = [
    { id: 'trove',               name: 'Trove Server (reset)' },
    { id: 'local',               name: 'Local Time' },
    { id: 'UTC',                 name: 'UTC' },
    { id: 'America/Sao_Paulo',   name: 'Brazil (Brasilia)' },
    { id: 'America/New_York',    name: 'US Eastern' },
    { id: 'America/Los_Angeles', name: 'US Pacific' },
    { id: 'Europe/Lisbon',       name: 'Portugal / UK' },
    { id: 'Europe/Paris',        name: 'Central Europe (FR, DE, ES)' },
    { id: 'Europe/Moscow',       name: 'Russia (Moscow)' },
    { id: 'Asia/Shanghai',       name: 'China (Beijing)' },
    { id: 'Asia/Tokyo',          name: 'Japan & South Korea' },
    { id: 'Australia/Sydney',    name: 'Australia (Sydney)' },
  ];
  const tzT = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);

  function fmtClock(date) {
    const pad = n => String(n).padStart(2, '0');
    return `${pad(date.getUTCHours())}:${pad(date.getUTCMinutes())}:${pad(date.getUTCSeconds())}`;
  }
  function fmtCountdown(seconds) {
    if (seconds <= 0) return '-';
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    if (h > 0) return `${h}h ${m}m`;
    return `${m}m`;
  }
  function flash(card) {
    if (!card) return;
    card.classList.remove('flash');
    void card.offsetWidth;  // restart the keyframe
    card.classList.add('flash');
  }

  function tickClock() {
    if (fields.serverTime && serverEpochOffset !== null) {
      const now = new Date(performance.now() + serverEpochOffset);
      // Trove runs on UTC-11: shift the authoritative UTC instant back 11h and
      // render it as UTC (fmtClock uses getUTC*), so the card shows Trove time.
      const trove = new Date(now.getTime() - TROVE_OFFSET_MS);
      fields.serverTime.textContent = fmtClock(trove);
      if (fields.serverDate) {
        fields.serverDate.textContent = trove.toUTCString().slice(5, 16) + ' · UTC-11';
      }
      if (dailyResetAt) {
        const left = dailyResetAt - Math.floor(now.getTime() / 1000);
        if (fields.resetIn) fields.resetIn.textContent = `daily reset in ${fmtCountdown(left)}`;
      }
      updateTzTooltip(now);
    }
    requestAnimationFrame(tickClock);
  }
  requestAnimationFrame(tickClock);

  // ── Timezone tooltip on the "Server Time" card ────────────────────────
  // On hover/focus, show the current instant across common player timezones
  // (Trove highlighted). The card has overflow:hidden, so the tooltip lives on
  // <body> as a position:fixed element placed just under the card.
  let tzTip = null, tzRows = null, tzVisible = false, tzLastSec = -1;

  function nowInstant() {
    // Prefer the API's authoritative UTC; fall back to the local clock so a
    // hover still works before the first server-time poll lands.
    return serverEpochOffset !== null
      ? new Date(performance.now() + serverEpochOffset)
      : new Date();
  }

  function buildTzTip() {
    if (tzTip || !cards.serverTime) return;
    tzTip = document.createElement('div');
    tzTip.className = 'tz-tip';
    tzTip.setAttribute('role', 'tooltip');
    tzTip.hidden = true;
    const head = document.createElement('div');
    head.className = 'tz-tip-head';
    head.textContent = tzT('Times around the world');
    tzTip.appendChild(head);
    tzRows = GLOBAL_TIMEZONES.map((tz) => {
      const row = document.createElement('div');
      row.className = 'tz-row' + (tz.id === 'trove' ? ' highlight' : '');
      const name = document.createElement('div');
      name.className = 'tz-name';
      name.textContent = tzT(tz.name);
      const right = document.createElement('div');
      right.className = 'tz-right';
      const time = document.createElement('div');
      time.className = 'tz-time';
      const date = document.createElement('div');
      date.className = 'tz-date';
      right.append(time, date);
      row.append(name, right);
      tzTip.appendChild(row);
      return { tz, time, date };
    });
    document.body.appendChild(tzTip);
  }

  function renderTz(utc) {
    if (!tzRows) return;
    const locale = (window.BTTi18n && window.BTTi18n.currentLocale)
      ? window.BTTi18n.currentLocale.replace('_', '-') : undefined;
    const tOpts = { hour12: false, hour: '2-digit', minute: '2-digit', second: '2-digit' };
    const dOpts = { weekday: 'short', month: 'short', day: 'numeric' };
    for (const { tz, time, date } of tzRows) {
      try {
        if (tz.id === 'trove') {
          const trove = new Date(utc.getTime() - TROVE_OFFSET_MS);
          time.textContent = trove.toLocaleTimeString(locale, { ...tOpts, timeZone: 'UTC' });
          date.textContent = trove.toLocaleDateString(locale, { ...dOpts, timeZone: 'UTC' });
        } else if (tz.id === 'local') {
          time.textContent = utc.toLocaleTimeString(locale, tOpts);
          date.textContent = utc.toLocaleDateString(locale, dOpts);
        } else {
          time.textContent = utc.toLocaleTimeString(locale, { ...tOpts, timeZone: tz.id });
          date.textContent = utc.toLocaleDateString(locale, { ...dOpts, timeZone: tz.id });
        }
      } catch (e) {
        time.textContent = '--:--:--';
        date.textContent = '---';
      }
    }
  }

  function updateTzTooltip(utc) {
    if (!tzVisible) return;
    const sec = Math.floor(utc.getTime() / 1000);
    if (sec === tzLastSec) return;   // displayed values only change each second
    tzLastSec = sec;
    renderTz(utc);
  }

  function positionTz() {
    if (!tzTip || !cards.serverTime) return;
    const r = cards.serverTime.getBoundingClientRect();
    const w = tzTip.offsetWidth || 300;
    const left = Math.max(12, Math.min(r.left, window.innerWidth - w - 12));
    tzTip.style.left = Math.round(left) + 'px';
    tzTip.style.top = Math.round(r.bottom + 10) + 'px';
  }

  function showTz() {
    if (!cards.serverTime) return;
    buildTzTip();
    tzVisible = true;
    tzLastSec = -1;
    renderTz(nowInstant());
    tzTip.hidden = false;
    positionTz();
    requestAnimationFrame(() => { if (tzTip) tzTip.classList.add('show'); });
  }

  function hideTz() {
    tzVisible = false;
    if (tzTip) { tzTip.classList.remove('show'); tzTip.hidden = true; }
  }

  if (cards.serverTime) {
    cards.serverTime.tabIndex = 0;
    cards.serverTime.addEventListener('mouseenter', showTz);
    cards.serverTime.addEventListener('mouseleave', hideTz);
    cards.serverTime.addEventListener('focus', showTz);
    cards.serverTime.addEventListener('blur', hideTz);
    window.addEventListener('scroll', () => { if (tzVisible) positionTz(); }, { passive: true });
    window.addEventListener('resize', () => { if (tzVisible) positionTz(); });
  }

  async function safeFetch(path) {
    try {
      const ctrl = new AbortController();
      const t = setTimeout(() => ctrl.abort(), 8000);
      const r = await fetch(API + path, { signal: ctrl.signal });
      clearTimeout(t);
      if (!r.ok) throw new Error(`HTTP ${r.status}`);
      return await r.json();
    } catch (e) {
      // Don't pollute the console with one expected error per refresh
      // when the API is briefly unreachable - log only once per session.
      if (!safeFetch._warned) {
        console.warn('[landing] live data fetch failed:', e.message);
        safeFetch._warned = true;
      }
      return null;
    }
  }

  async function refreshLive() {
    const [time, chaos, challenge] = await Promise.all([
      safeFetch('/v1/rotations/server-time'),
      safeFetch('/v1/rotations/chaos-chest'),
      safeFetch('/v1/rotations/challenge/current'),
    ]);

    // Server time → set offset, day, daily reset.
    if (time && typeof time.now_unix === 'number') {
      serverEpochOffset = time.now_unix * 1000 - performance.now();
      dailyResetAt = time.daily_reset_at || null;
      if (fields.troveDay && time.trove_day) {
        const prev = fields.troveDay.textContent;
        fields.troveDay.textContent = time.trove_day;
        if (prev && prev !== time.trove_day) flash(cards.troveDay);
      }
    }

    // Chaos chest → name + "ends in N days" sub.
    if (chaos) {
      const name = (chaos.item && chaos.item.name) || 'Unknown';
      const prev = fields.chaosName ? fields.chaosName.textContent : '';
      if (fields.chaosName) fields.chaosName.textContent = name;
      if (fields.chaosEnds && typeof chaos.seconds_remaining === 'number') {
        const days = Math.max(0, Math.floor(chaos.seconds_remaining / 86400));
        fields.chaosEnds.textContent = days > 0 ? `${days} day${days === 1 ? '' : 's'} left` : 'rotates today';
      }
      if (prev && prev !== name && prev !== 'loading…') flash(cards.chaos);
    }

    // Hourly challenge → name + type tag.
    if (challenge) {
      const name = challenge.name || 'No active challenge';
      const prev = fields.challName ? fields.challName.textContent : '';
      if (fields.challName) fields.challName.textContent = name;
      if (fields.challKind) {
        if (challenge.type) {
          // Capitalise: "collection" → "Collection", "rampage" → "Rampage", etc.
          const cap = challenge.type.charAt(0).toUpperCase() + challenge.type.slice(1);
          const left = typeof challenge.seconds_remaining === 'number' && challenge.active
            ? `${cap} · ${Math.max(0, Math.floor(challenge.seconds_remaining / 60))}m left`
            : cap;
          fields.challKind.textContent = left;
        } else {
          fields.challKind.textContent = 'waiting for next window';
        }
      }
      if (prev && prev !== name && prev !== 'loading…') flash(cards.challenge);
    }
  }

  // ─── Trove server status pill ──────────────────────────────────────
  // Fetches /site/trove-status (same-origin proxy of the API prober) and
  // paints the hero status pill. The prober runs server-side every ~60s;
  // we poll a touch faster so the dot tracks it. Hidden until the first
  // resolved fetch so it never flashes a wrong state.
  const $status = document.getElementById('trove-status');
  const $statusText = $status && $status.querySelector('.trove-status-text');
  // Translate via the global i18n helper when present, else pass through
  // the English source (landing.js's local `t` vars are animation/timer
  // scratch, not the i18n function - hence this dedicated helper).
  const tr = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);

  function paintStatus(data) {
    if (!$status || !$statusText) return;
    const overall = (data && data.overall) || 'unknown';
    if (overall === 'unknown') { $status.hidden = true; return; }

    // Map verdict → {class, label, title}. When worlds_checked is false
    // we only verified the auth tier, so "online" is honest-but-partial;
    // the title spells that out without cluttering the pill.
    // The pill reads "Trove servers · <state>" (the prefix is static in
    // the markup), so the JS only sets the state word + colour. Driven by
    // the LIVE environment's verdict from the multi-env payload, falling
    // back to the legacy flat ``overall`` shape for safety.
    // Binary verdict: online (green) or down (red). 'maintenance' is a legacy
    // value from older snapshots → treated as down. A partial outage (some Live
    // region still up) is spelled out in the title.
    let cls, label, title;
    if (overall === 'online') {
      cls = 'is-up';
      label = tr('Online');
      title = tr('Trove login and game servers are responding.');
    } else {
      const envs = (data && data.environments) || {};
      const anyLiveUp = ['eu', 'us'].some(k => envs[k] && envs[k].status === 'online');
      cls = 'is-down';
      label = tr('Down');
      title = anyLiveUp
        ? tr('Some Trove servers are unreachable.')
        : tr('Trove servers are unreachable.');
    }
    $status.classList.remove('is-up', 'is-maint', 'is-down');
    $status.classList.add(cls);
    $statusText.textContent = label;
    $status.title = title;
    $status.hidden = false;
  }

  async function refreshStatus() {
    try {
      const ctrl = new AbortController();
      const tm = setTimeout(() => ctrl.abort(), 8000);
      const r = await fetch('/site/trove-status', { signal: ctrl.signal });
      clearTimeout(tm);
      if (!r.ok) throw new Error(`HTTP ${r.status}`);
      paintStatus(await r.json());
    } catch (_) {
      // Leave the pill in its previous state on a transient failure; if
      // it was never shown, it stays hidden (no false signal).
    }
  }

  // Kick off + schedule. requestIdleCallback first (so we don't fight the
  // initial paint), then a normal interval. If the tab returns from hidden,
  // refresh immediately so a backgrounded tab snaps to current.
  const kick = () => { refreshLive(); refreshStatus(); };
  if ('requestIdleCallback' in window) {
    requestIdleCallback(kick, { timeout: 1500 });
  } else {
    setTimeout(kick, 300);
  }
  setInterval(refreshLive, 30_000);
  setInterval(refreshStatus, 60_000);
  document.addEventListener('visibilitychange', () => {
    if (!document.hidden) { refreshLive(); refreshStatus(); }
  });

  // The support widget used to be wired up here, but the markup ships
  // on every page (leaderboards/commands/updates/index/support) and
  // landing.js only loads on /. Moved into app.js - see section 7
  // there. Keeping this comment as a breadcrumb for the next person
  // who greps for "support-widget" in landing.js.
})();
