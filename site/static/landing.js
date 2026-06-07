/* ═══════════════════════════════════════════════════════════════════════
   Better Trove Tools — landing page enhancement layer
   ───────────────────────────────────────────────────────────────────────
   Runs AFTER app.js (the download dropdown + nav-toggle logic) and i18n.js.
   We deliberately don't touch the hamburger / dropdown wiring — those are
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
   silently no-ops — the rest of the page keeps working.
   ═══════════════════════════════════════════════════════════════════════ */

(function () {
  'use strict';

  const RM = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  const API = 'https://api.aallyn.net';

  // ── 1. ready flag (drives hero entry animation) ────────────────────
  // Wait one rAF after DOMContentLoaded so the initial styles have applied
  // before the transition kicks off — otherwise the transition can fire
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
  // an opacity crossfade. The list is dynamic — drop a file in
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
    // class flip happens after the elements are in the layout tree —
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
    // Pause the cycle when the tab isn't visible — saves a tiny bit of
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
          // clientWidth / clientHeight are READ-ONLY getters — you can't
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
        const stop  = () => { if (rafId !== null) { cancelAnimationFrame(rafId); rafId = null; } };

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
  // Server time, trove day, chaos chest, hourly challenge — refreshed
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

  function fmtClock(date) {
    const pad = n => String(n).padStart(2, '0');
    return `${pad(date.getUTCHours())}:${pad(date.getUTCMinutes())}:${pad(date.getUTCSeconds())}`;
  }
  function fmtCountdown(seconds) {
    if (seconds <= 0) return '—';
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
      fields.serverTime.textContent = fmtClock(now);
      if (fields.serverDate) {
        fields.serverDate.textContent = now.toUTCString().slice(5, 16) + ' UTC';
      }
      if (dailyResetAt) {
        const left = dailyResetAt - Math.floor(now.getTime() / 1000);
        if (fields.resetIn) fields.resetIn.textContent = `daily reset in ${fmtCountdown(left)}`;
      }
    }
    requestAnimationFrame(tickClock);
  }
  requestAnimationFrame(tickClock);

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
      // when the API is briefly unreachable — log only once per session.
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

  // Kick off + schedule. requestIdleCallback first (so we don't fight the
  // initial paint), then a normal interval. If the tab returns from hidden,
  // refresh immediately so a backgrounded tab snaps to current.
  const kick = () => refreshLive();
  if ('requestIdleCallback' in window) {
    requestIdleCallback(kick, { timeout: 1500 });
  } else {
    setTimeout(kick, 300);
  }
  setInterval(refreshLive, 30_000);
  document.addEventListener('visibilitychange', () => {
    if (!document.hidden) refreshLive();
  });
})();
