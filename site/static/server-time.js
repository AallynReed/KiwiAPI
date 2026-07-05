/* Trove Server Time page (/server-time): analog+digital clock, world clocks,
   and a Discord <t:unix:STYLE> timestamp maker - all off one clock. Trove's day
   rolls over at 11:00 UTC, so the game clock is real UTC minus 11h ("UTC-11").
   The clock anchors to the API's authoritative UTC (never trusting a wrong local
   machine clock), falling back to local on fetch failure. UTC-11 logic + tz list
   are shared with landing.js. */
(() => {
  'use strict';

  const TROVE_OFFSET_MS = 11 * 3600000;
  const t = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);
  // i18n.js doesn't expose a locale, so format with the browser default.
  const LOCALE = undefined;
  const pad = (n) => String(n).padStart(2, '0');
  const $ = (id) => document.getElementById(id);
  const SVG_NS = 'http://www.w3.org/2000/svg';

  // id: 'trove' (fixed UTC-11) / 'local' (this device) / 'UTC' / an IANA zone.
  const ZONES = [
    { id: 'trove',               name: 'Trove Server',   sub: 'Daily reset clock' },
    { id: 'local',               name: 'Your time',      sub: 'This device' },
    { id: 'UTC',                 name: 'UTC',            sub: 'Universal time' },
    { id: 'America/Los_Angeles', name: 'US Pacific',     sub: 'Los Angeles' },
    { id: 'America/New_York',    name: 'US Eastern',     sub: 'New York' },
    { id: 'America/Sao_Paulo',   name: 'Brazil',         sub: 'Brasília' },
    { id: 'Europe/Lisbon',       name: 'UK / Portugal',  sub: 'London · Lisbon' },
    { id: 'Europe/Paris',        name: 'Central Europe', sub: 'Paris · Berlin · Madrid' },
    { id: 'Europe/Moscow',       name: 'Russia',         sub: 'Moscow' },
    { id: 'Asia/Shanghai',       name: 'China',          sub: 'Beijing' },
    { id: 'Asia/Tokyo',          name: 'Japan / Korea',  sub: 'Tokyo · Seoul' },
    { id: 'Australia/Sydney',    name: 'Australia',      sub: 'Sydney' },
  ];

  // Discord <t:UNIX:STYLE> rendering styles. `opts` mirrors how Discord renders
  // each style in the viewer's local zone; `rel` = the relative "in N hours";
  // `chip` is the short label used on the style selector, `label` the long one.
  const DISCORD_STYLES = [
    { code: 't', label: 'Short time', chip: 'Time',
      opts: { hour: 'numeric', minute: '2-digit' } },
    { code: 'T', label: 'Long time', chip: 'Time + seconds',
      opts: { hour: 'numeric', minute: '2-digit', second: '2-digit' } },
    { code: 'd', label: 'Short date', chip: 'Date',
      opts: { year: 'numeric', month: '2-digit', day: '2-digit' } },
    { code: 'D', label: 'Long date', chip: 'Date (long)',
      opts: { year: 'numeric', month: 'long', day: 'numeric' } },
    { code: 'f', label: 'Short date/time', chip: 'Date + time', def: true,
      opts: { year: 'numeric', month: 'long', day: 'numeric', hour: 'numeric', minute: '2-digit' } },
    { code: 'F', label: 'Long date/time', chip: 'Full',
      opts: { weekday: 'long', year: 'numeric', month: 'long', day: 'numeric', hour: 'numeric', minute: '2-digit' } },
    { code: 'R', label: 'Relative', chip: 'Relative', rel: true },
  ];

  // ── Authoritative clock ────────────────────────────────────────────────
  // performance.now() + epochOffset == server-now in ms. null until synced.
  let epochOffset = null;
  let dailyResetAt = null;   // unix seconds
  let weeklyResetAt = null;

  const nowMs = () => (epochOffset !== null ? performance.now() + epochOffset : Date.now());
  const troveDate = (ms) => new Date(ms - TROVE_OFFSET_MS); // read via getUTC* = Trove wall clock

  async function syncTime() {
    // Same-origin proxy first (trove.aallyn.net forwards it + the dev server
    // stubs it), then the public API host, then fall back to the local clock.
    const urls = ['/site/server-time', 'https://api.aallyn.net/v1/rotations/server-time'];
    for (const url of urls) {
      try {
        const ctrl = new AbortController();
        const timer = setTimeout(() => ctrl.abort(), 6000);
        const r = await fetch(url, { signal: ctrl.signal });
        clearTimeout(timer);
        if (!r.ok) continue;
        const d = await r.json();
        if (typeof d.now_unix === 'number') {
          epochOffset = d.now_unix * 1000 - performance.now();
          dailyResetAt = d.daily_reset_at || null;
          weeklyResetAt = d.weekly_reset_at || null;
          return;
        }
      } catch (e) { /* try the next source */ }
    }
    // No server: derive the daily reset (next 11:00 UTC) so the countdown lives.
    if (dailyResetAt === null) {
      const d = new Date();
      const reset = Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate(), 11, 0, 0);
      dailyResetAt = Math.floor((d.getTime() < reset ? reset : reset + 86400000) / 1000);
    }
  }

  // ── Time-zone helpers ──────────────────────────────────────────────────
  // Numeric 0-23 hour in a zone (for the day/night glyph). tz undefined = local.
  function zoneHour(date, tz) {
    const f = new Intl.DateTimeFormat('en-US', { hour: 'numeric', hourCycle: 'h23', ...(tz ? { timeZone: tz } : {}) });
    return parseInt(f.format(date), 10) % 24;
  }

  // Minutes east of UTC for an IANA zone at a given instant (DST-correct).
  function ianaOffsetMin(date, tz) {
    const dtf = new Intl.DateTimeFormat('en-US', {
      timeZone: tz, hourCycle: 'h23', year: 'numeric', month: '2-digit',
      day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit',
    });
    const p = {};
    for (const part of dtf.formatToParts(date)) p[part.type] = part.value;
    const asUTC = Date.UTC(+p.year, +p.month - 1, +p.day, (+p.hour) % 24, +p.minute, +p.second);
    return Math.round((asUTC - date.getTime()) / 60000);
  }

  function offsetLabel(date, id) {
    let mins;
    if (id === 'trove') mins = -11 * 60;
    else if (id === 'UTC') mins = 0;
    else if (id === 'local') mins = -date.getTimezoneOffset();
    else mins = ianaOffsetMin(date, id);
    const sign = mins >= 0 ? '+' : '-';
    const a = Math.abs(mins);
    return 'UTC' + sign + Math.floor(a / 60) + (a % 60 ? ':' + pad(a % 60) : '');
  }

  function zoneStrings(date, id) {
    const topt = { hourCycle: 'h23', hour: '2-digit', minute: '2-digit', second: '2-digit' };
    const dopt = { weekday: 'short', month: 'short', day: 'numeric' };
    if (id === 'trove') {
      const d = troveDate(date.getTime());
      return {
        h: d.getUTCHours(),
        time: d.toLocaleTimeString(LOCALE, { ...topt, timeZone: 'UTC' }),
        date: d.toLocaleDateString(LOCALE, { ...dopt, timeZone: 'UTC' }),
      };
    }
    const tz = id === 'local' ? undefined : id;
    if (tz) { topt.timeZone = tz; dopt.timeZone = tz; }
    return {
      h: zoneHour(date, tz),
      time: date.toLocaleTimeString(LOCALE, topt),
      date: date.toLocaleDateString(LOCALE, dopt),
    };
  }

  // ── Countdown formatting ───────────────────────────────────────────────
  function fmtCountdown(sec) {
    if (sec == null) return '—';
    if (sec <= 0) return t('now');
    const d = Math.floor(sec / 86400);
    const h = Math.floor((sec % 86400) / 3600);
    const m = Math.floor((sec % 3600) / 60);
    const s = Math.floor(sec % 60);
    if (d > 0) return `${d}d ${h}h ${m}m`;
    if (h > 0) return `${h}h ${pad(m)}m ${pad(s)}s`;
    if (m > 0) return `${m}m ${pad(s)}s`;
    return `${s}s`;
  }

  // ── Analog clock ───────────────────────────────────────────────────────
  function buildTicks() {
    const g = $('srv-ticks');
    if (!g) return;
    for (let i = 0; i < 60; i++) {
      const major = i % 5 === 0;
      const ang = (i / 60) * 2 * Math.PI;
      const r0 = major ? 73 : 80;
      const r1 = 84;
      const ln = document.createElementNS(SVG_NS, 'line');
      ln.setAttribute('x1', (100 + r0 * Math.sin(ang)).toFixed(2));
      ln.setAttribute('y1', (100 - r0 * Math.cos(ang)).toFixed(2));
      ln.setAttribute('x2', (100 + r1 * Math.sin(ang)).toFixed(2));
      ln.setAttribute('y2', (100 - r1 * Math.cos(ang)).toFixed(2));
      ln.setAttribute('class', major ? 'srv-tick srv-tick-major' : 'srv-tick');
      g.appendChild(ln);
    }
  }

  const handH = $('srv-hand-h'), handM = $('srv-hand-m'), handS = $('srv-hand-s');
  function moveHands(d) {
    const h = d.getUTCHours(), m = d.getUTCMinutes(), s = d.getUTCSeconds();
    const ms = d.getUTCMilliseconds();
    const secAng = (s + ms / 1000) * 6;
    const minAng = m * 6 + s * 0.1;
    const hrAng = (h % 12) * 30 + m * 0.5;
    if (handS) handS.setAttribute('transform', `rotate(${secAng} 100 100)`);
    if (handM) handM.setAttribute('transform', `rotate(${minAng} 100 100)`);
    if (handH) handH.setAttribute('transform', `rotate(${hrAng} 100 100)`);
  }

  // ── Main tick (text once a second, second hand smoother) ───────────────
  const bigtime = $('srv-bigtime'), bigdate = $('srv-bigdate');
  const dailyEl = $('srv-daily'), weeklyEl = $('srv-weekly');
  let lastSec = -1;

  function tick() {
    const ms = nowMs();
    const trove = troveDate(ms);
    moveHands(trove);
    const sec = Math.floor(ms / 1000);
    if (sec === lastSec) return;
    lastSec = sec;
    const h = trove.getUTCHours(), m = trove.getUTCMinutes(), s = trove.getUTCSeconds();
    if (bigtime) bigtime.textContent = `${pad(h)}:${pad(m)}:${pad(s)}`;
    if (bigdate) {
      bigdate.textContent = trove.toLocaleDateString(LOCALE, {
        weekday: 'long', year: 'numeric', month: 'long', day: 'numeric', timeZone: 'UTC' });
    }
    if (dailyEl) dailyEl.textContent = fmtCountdown(dailyResetAt != null ? dailyResetAt - sec : null);
    if (weeklyEl) weeklyEl.textContent = fmtCountdown(weeklyResetAt != null ? weeklyResetAt - sec : null);
    updateWorld(new Date(ms));
    updateConverterLive();
  }

  // ── World clocks ───────────────────────────────────────────────────────
  let worldCells = null;
  function buildWorld() {
    const grid = $('srv-grid');
    if (!grid) return;
    grid.innerHTML = '';
    worldCells = ZONES.map((z) => {
      const card = document.createElement('article');
      card.className = 'srv-wc' + (z.id === 'trove' ? ' is-trove' : '') + (z.id === 'local' ? ' is-local' : '');
      const head = document.createElement('div'); head.className = 'srv-wc-head';
      const icon = document.createElement('i'); icon.className = 'srv-wc-icon fa-solid fa-clock';
      const names = document.createElement('div'); names.className = 'srv-wc-names';
      const nm = document.createElement('div'); nm.className = 'srv-wc-name'; nm.textContent = t(z.name);
      const sb = document.createElement('div'); sb.className = 'srv-wc-sub'; sb.textContent = t(z.sub);
      names.append(nm, sb);
      const off = document.createElement('span'); off.className = 'srv-wc-off'; off.textContent = '—';
      head.append(icon, names, off);
      const time = document.createElement('div'); time.className = 'srv-wc-time'; time.textContent = '--:--:--';
      const date = document.createElement('div'); date.className = 'srv-wc-date'; date.textContent = '—';
      card.append(head, time, date);
      grid.appendChild(card);
      return { z, card, icon, off, time, date };
    });
  }

  function updateWorld(date) {
    if (!worldCells) return;
    for (const c of worldCells) {
      const p = zoneStrings(date, c.z.id);
      c.time.textContent = p.time;
      c.date.textContent = p.date;
      c.off.textContent = offsetLabel(date, c.z.id);
      const day = p.h >= 6 && p.h < 18;
      c.icon.className = 'srv-wc-icon fa-solid ' + (day ? 'fa-sun' : 'fa-moon');
      c.card.classList.toggle('is-day', day);
      c.card.classList.toggle('is-night', !day);
    }
  }

  // ── Discord timestamp maker ────────────────────────────────────────────
  let convRows = null;       // "every format" list rows
  let styleChips = null;     // the style-selector pills
  let selectedStyle = 'f';   // Discord's own default style is :f
  let currentUnix = null;    // the chosen instant (unix seconds)

  const VIEWER_TZ = (() => {
    try { return Intl.DateTimeFormat().resolvedOptions().timeZone || 'local time'; }
    catch (e) { return 'local time'; }
  })();

  const styleByCode = (code) => DISCORD_STYLES.find((s) => s.code === code) || DISCORD_STYLES[0];

  function renderStyle(unix, st) {
    if (unix == null) return '—';
    return st.rel ? relPreview(unix) : new Date(unix * 1000).toLocaleString(LOCALE, st.opts);
  }

  // Full "Friday, June 26, 2026 at 8:30 PM" label - the Discord hover tooltip.
  function fullLabel(unix) {
    if (unix == null) return '';
    return new Date(unix * 1000).toLocaleString(LOCALE, {
      weekday: 'long', year: 'numeric', month: 'long', day: 'numeric',
      hour: 'numeric', minute: '2-digit' });
  }

  // A wall-clock entered in `zoneId` → unix seconds of that instant.
  function inputToUnix(value, zoneId) {
    const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})/.exec(value || '');
    if (!m) return null;
    const Y = +m[1], Mo = +m[2], D = +m[3], H = +m[4], Mi = +m[5];
    if (zoneId === 'trove') return Math.floor(Date.UTC(Y, Mo - 1, D, H, Mi, 0) / 1000) + 11 * 3600;
    if (zoneId === 'UTC') return Math.floor(Date.UTC(Y, Mo - 1, D, H, Mi, 0) / 1000);
    if (zoneId === 'local') return Math.floor(new Date(Y, Mo - 1, D, H, Mi, 0).getTime() / 1000);
    // IANA: solve for the instant whose wall clock matches (two passes = DST-safe).
    let guess = Date.UTC(Y, Mo - 1, D, H, Mi, 0);
    for (let i = 0; i < 2; i++) {
      const off = ianaOffsetMin(new Date(guess), zoneId) * 60000;
      guess = Date.UTC(Y, Mo - 1, D, H, Mi, 0) - off;
    }
    return Math.floor(guess / 1000);
  }

  function relPreview(unix) {
    const diff = unix - Math.floor(nowMs() / 1000);
    const rtf = (typeof Intl.RelativeTimeFormat === 'function')
      ? new Intl.RelativeTimeFormat(LOCALE, { numeric: 'auto' }) : null;
    const abs = Math.abs(diff);
    const units = [['year', 31536000], ['month', 2592000], ['day', 86400],
      ['hour', 3600], ['minute', 60], ['second', 1]];
    for (const [u, span] of units) {
      if (abs >= span || u === 'second') {
        const v = Math.round(diff / span);
        // `${v + ' ' + u}` not `${v} ${u}` - the minifier strips the space between
        // adjacent interpolations in *.min.js (see minify_static.py guard).
        return rtf ? rtf.format(v, u) : `${v + ' ' + u}${Math.abs(v) === 1 ? '' : 's'}`;
      }
    }
    return '';
  }

  function buildConverter() {
    const note = $('srv-viewer-tz');
    if (note) note.textContent = t('Previewed in your time zone') + ' · ' + VIEWER_TZ;

    const sel = $('srv-tz');
    if (sel) {
      sel.innerHTML = '';
      for (const z of ZONES) {
        const o = document.createElement('option');
        o.value = z.id;
        o.textContent = t(z.name) + (z.id === 'trove' || z.id === 'local' || z.id === 'UTC' ? '' : ` · ${z.sub}`);
        sel.appendChild(o);
      }
    }

    // Style selector pills.
    const chipWrap = $('srv-dc-styles');
    if (chipWrap) {
      chipWrap.innerHTML = '';
      styleChips = DISCORD_STYLES.map((st) => {
        const b = document.createElement('button');
        b.type = 'button';
        b.className = 'srv-dc-style' + (st.code === selectedStyle ? ' is-active' : '');
        b.setAttribute('role', 'tab');
        b.setAttribute('aria-selected', st.code === selectedStyle ? 'true' : 'false');
        b.textContent = t(st.chip);
        b.addEventListener('click', () => selectStyle(st.code));
        chipWrap.appendChild(b);
        return { st, el: b };
      });
    }

    // "Every format" list.
    const out = $('srv-conv-out');
    if (out) {
      out.innerHTML = '';
      convRows = DISCORD_STYLES.map((st) => {
        const row = document.createElement('div');
        row.className = 'srv-cv-row' + (st.code === selectedStyle ? ' is-active' : '');
        const meta = document.createElement('div'); meta.className = 'srv-cv-meta';
        const lab = document.createElement('span'); lab.className = 'srv-cv-label'; lab.textContent = t(st.label);
        if (st.def) {
          const bd = document.createElement('span'); bd.className = 'srv-cv-badge'; bd.textContent = t('Default'); lab.appendChild(bd);
        }
        const preview = document.createElement('div'); preview.className = 'srv-cv-preview'; preview.textContent = '—';
        meta.append(lab, preview);
        const code = document.createElement('code'); code.className = 'srv-cv-code'; code.textContent = '—';
        const btn = document.createElement('button');
        btn.type = 'button'; btn.className = 'srv-cv-copy';
        btn.innerHTML = '<i class="fa-regular fa-copy" aria-hidden="true"></i>';
        btn.setAttribute('aria-label', t('Copy code'));
        btn.addEventListener('click', () => copy(code.textContent, btn));
        row.append(meta, code, btn);
        out.appendChild(row);
        return { st, row, preview, code };
      });
    }
    recompute();
  }

  function selectStyle(code) {
    selectedStyle = code;
    if (styleChips) for (const c of styleChips) {
      const on = c.st.code === code;
      c.el.classList.toggle('is-active', on);
      c.el.setAttribute('aria-selected', on ? 'true' : 'false');
    }
    if (convRows) for (const r of convRows) r.row.classList.toggle('is-active', r.st.code === code);
    updateDiscord();
    updatePrimary();
  }

  // Recompute the chosen instant + every static rendering (on input / zone change).
  function recompute() {
    const dt = $('srv-dt'), sel = $('srv-tz');
    const zoneId = sel ? sel.value : 'trove';
    currentUnix = dt ? inputToUnix(dt.value, zoneId) : null;
    if (convRows) for (const r of convRows) {
      if (currentUnix == null) { r.code.textContent = '—'; r.preview.textContent = '—'; continue; }
      r.code.textContent = `<t:${currentUnix}:${r.st.code}>`;
      r.preview.textContent = renderStyle(currentUnix, r.st);
    }
    updateDiscord();
    updatePrimary();
  }

  // The mock-message chips (selected style + the always-on relative).
  function updateDiscord() {
    const chip = $('srv-dc-chip'), chipRel = $('srv-dc-chip-rel'), sent = $('srv-dc-sent');
    const relWrap = document.querySelector('.srv-dc-rel-wrap');
    if (sent) {
      sent.textContent = t('Today at') + ' ' +
        new Date(nowMs()).toLocaleTimeString(LOCALE, { hour: 'numeric', minute: '2-digit' });
    }
    if (relWrap) relWrap.style.display = selectedStyle === 'R' ? 'none' : '';
    if (currentUnix == null) {
      if (chip) chip.textContent = '—';
      if (chipRel) chipRel.textContent = '—';
      return;
    }
    const st = styleByCode(selectedStyle);
    if (chip) { chip.textContent = renderStyle(currentUnix, st); chip.title = fullLabel(currentUnix); }
    if (chipRel) { chipRel.textContent = relPreview(currentUnix); chipRel.title = fullLabel(currentUnix); }
  }

  // The big copy code next to the preview = the selected style.
  function updatePrimary() {
    const codeEl = $('srv-dc-code');
    if (codeEl) codeEl.textContent = currentUnix == null ? '—' : `<t:${currentUnix}:${selectedStyle}>`;
  }

  // Per-second refresh of the bits that move on their own (relative renders).
  function updateConverterLive() {
    if (currentUnix == null) return;
    const chipRel = $('srv-dc-chip-rel');
    if (chipRel) chipRel.textContent = relPreview(currentUnix);
    if (selectedStyle === 'R') {
      const chip = $('srv-dc-chip');
      if (chip) chip.textContent = relPreview(currentUnix);
    }
    if (convRows) for (const r of convRows) if (r.st.rel) r.preview.textContent = renderStyle(currentUnix, r.st);
    const sent = $('srv-dc-sent');
    if (sent) {
      sent.textContent = t('Today at') + ' ' +
        new Date(nowMs()).toLocaleTimeString(LOCALE, { hour: 'numeric', minute: '2-digit' });
    }
  }

  // Fill the picker with the current wall-clock in `zoneId` and select it.
  function fillNow(zoneId) {
    const dt = $('srv-dt'), sel = $('srv-tz');
    const ms = nowMs();
    let Y, Mo, D, H, Mi;
    if (zoneId === 'trove') {
      const d = troveDate(ms);
      [Y, Mo, D, H, Mi] = [d.getUTCFullYear(), d.getUTCMonth() + 1, d.getUTCDate(), d.getUTCHours(), d.getUTCMinutes()];
    } else if (zoneId === 'UTC') {
      const d = new Date(ms);
      [Y, Mo, D, H, Mi] = [d.getUTCFullYear(), d.getUTCMonth() + 1, d.getUTCDate(), d.getUTCHours(), d.getUTCMinutes()];
    } else if (zoneId === 'local') {
      const d = new Date(ms);
      [Y, Mo, D, H, Mi] = [d.getFullYear(), d.getMonth() + 1, d.getDate(), d.getHours(), d.getMinutes()];
    } else {
      const f = new Intl.DateTimeFormat('en-US', { timeZone: zoneId, hourCycle: 'h23',
        year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' });
      const p = {};
      for (const part of f.formatToParts(new Date(ms))) p[part.type] = part.value;
      [Y, Mo, D, H, Mi] = [+p.year, +p.month, +p.day, (+p.hour) % 24, +p.minute];
    }
    if (dt) dt.value = `${Y}-${pad(Mo)}-${pad(D)}T${pad(H)}:${pad(Mi)}`;
    if (sel) sel.value = zoneId;
    recompute();
  }

  // The "Now (server time)" button grabs the current Trove server time.
  const setNow = () => fillNow('trove');

  function copy(text, btn) {
    if (!text || text === '—') return;
    const flash = () => {
      const old = btn.innerHTML;
      btn.classList.add('copied');
      btn.innerHTML = '<i class="fa-solid fa-check" aria-hidden="true"></i>';
      setTimeout(() => { btn.classList.remove('copied'); btn.innerHTML = old; }, 1200);
    };
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).then(flash).catch(() => fallbackCopy(text, flash));
    } else {
      fallbackCopy(text, flash);
    }
  }

  function fallbackCopy(text, done) {
    const ta = document.createElement('textarea');
    ta.value = text; ta.style.position = 'fixed'; ta.style.opacity = '0';
    document.body.appendChild(ta); ta.focus(); ta.select();
    try { document.execCommand('copy'); done(); } catch (e) { /* clipboard unavailable */ }
    document.body.removeChild(ta);
  }

  // Re-localize the JS-built text (zone names, labels) on a language switch.
  function relocalize() {
    const dt = $('srv-dt'), sel = $('srv-tz');
    const dv = dt ? dt.value : null;
    const sv = sel ? sel.value : 'trove';
    buildWorld();
    buildConverter();
    if (dt && dv) dt.value = dv;
    if (sel) sel.value = sv;
    lastSec = -1;
    recompute();
    updateWorld(new Date(nowMs()));
  }

  // ── Boot ───────────────────────────────────────────────────────────────
  function init() {
    buildTicks();
    buildWorld();
    buildConverter();
    fillNow('local');      // default the maker to the viewer's own time
    updateWorld(new Date(nowMs()));
    tick();
    syncTime();            // async; corrects the offset + reset times when it lands
    setInterval(tick, 250);          // 4/s keeps the second hand from stuttering
    setInterval(syncTime, 60000);    // re-anchor + refresh reset countdowns

    const dt = $('srv-dt'), sel = $('srv-tz'), nowBtn = $('srv-now'), pCopy = $('srv-dc-copy');
    if (dt) dt.addEventListener('input', recompute);
    if (sel) sel.addEventListener('change', recompute);
    if (nowBtn) nowBtn.addEventListener('click', setNow);
    if (pCopy) pCopy.addEventListener('click', () => copy($('srv-dc-code').textContent, pCopy));
    document.addEventListener('btt-lang-changed', relocalize);
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
