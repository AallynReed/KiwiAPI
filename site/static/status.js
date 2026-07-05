/* Trove server status page.
 *
 * Fetches /site/trove-status (current EU/US/PTS snapshot) and
 * /site/trove-status/history (per-env timeline) and renders:
 *   • a big overall banner (tracks Live),
 *   • per-environment + auth cards,
 *   • a downtime-history timeline bar + outage log (env-switchable).
 *
 * No build step / framework - same vanilla style as landing.js. i18n via
 * the global window.BTTi18n when present, else English passthrough.
 */
(() => {
  'use strict';

  const { esc } = window.BTTUtil;

  const tr = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);

  const STATUS_META = {
    online:      { cls: 'is-up',    label: 'Online' },
    down:        { cls: 'is-down',  label: 'Down' },
    // Legacy value from older history/snapshots → render as red "Down" too.
    maintenance: { cls: 'is-down',  label: 'Down' },
    unknown:     { cls: 'is-unk',   label: 'Checking…' },
  };

  let historyEnv = 'eu';

  // ─── Time helpers ──────────────────────────────────────────────────
  function fmtClock(unix) {
    if (!unix) return '';
    return new Date(unix * 1000).toLocaleString(undefined, {
      month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit',
    });
  }
  function fmtAgo(unix) {
    if (!unix) return '';
    const diff = Math.max(0, Math.floor(Date.now() / 1000) - unix);
    if (diff < 60) return tr('just now');
    if (diff < 3600) return tr('{n}m ago').replace('{n}', Math.round(diff / 60));
    if (diff < 86400) return tr('{n}h ago').replace('{n}', Math.round(diff / 3600));
    return tr('{n}d ago').replace('{n}', Math.round(diff / 86400));
  }
  function fmtDuration(seconds) {
    if (seconds == null) return '';
    const s = Math.max(0, Math.round(seconds));
    if (s < 60) return `${s}s`;
    if (s < 3600) return `${Math.round(s / 60)}m`;
    const h = Math.floor(s / 3600), m = Math.round((s % 3600) / 60);
    if (s < 86400) return m ? `${h}h ${m}m` : `${h}h`;
    const d = Math.floor(s / 86400), hh = Math.round((s % 86400) / 3600);
    return hh ? `${d}d ${hh}h` : `${d}d`;
  }

  // ─── Current snapshot ──────────────────────────────────────────────
  function renderCurrent(data) {
    const banner = document.getElementById('st-banner');
    const bannerText = banner && banner.querySelector('.st-banner-text');
    const bannerChecked = document.getElementById('st-banner-checked');
    const cards = document.getElementById('st-cards');
    if (!banner || !cards) return;

    const overall = (data && data.overall) || 'unknown';
    const meta = STATUS_META[overall] || STATUS_META.unknown;

    // Banner headline mirrors the LIVE environments. "down" overall is split into
    // "partially down" (some Live region still up) vs fully "down" for the wording.
    const liveEnvs = ['eu', 'us'];
    const anyLiveUp = liveEnvs.some(k => (data && data.environments && data.environments[k]
                                          && data.environments[k].status === 'online'));
    const headline = overall === 'online'  ? tr('All Trove servers operational')
                   : overall === 'unknown' ? tr('Checking Trove status…')
                   : anyLiveUp             ? tr('Trove is partially down')
                   :                         tr('Trove is down');
    banner.className = 'st-banner ' + meta.cls;
    if (bannerText) bannerText.textContent = headline;
    if (bannerChecked) {
      bannerChecked.textContent = data && data.checked_at
        ? tr('checked {ago}').replace('{ago}', fmtAgo(data.checked_at)) : '';
    }
    banner.hidden = false;

    // Per-environment cards (EU, US, PTS) + the shared auth tier.
    const envs = (data && data.environments) || {};
    const envCard = (key, title) => {
      const e = envs[key] || { status: 'unknown', game: null };
      const m = STATUS_META[e.status] || STATUS_META.unknown;
      const game = e.game || {};
      const latency = (typeof game.latency_ms === 'number' && e.online)
        ? `<span class="st-card-latency">${Math.round(game.latency_ms)} ms</span>` : '';
      const sub = e.status === 'online' ? tr('Game servers accepting connections')
                : (e.status === 'down' || e.status === 'maintenance') ? tr('Servers unreachable')
                : tr('Awaiting first probe');
      return `
        <article class="st-card ${m.cls}">
          <div class="st-card-head">
            <span class="st-card-dot" aria-hidden="true"></span>
            <span class="st-card-name">${esc(title)}</span>
            <span class="st-card-status">${esc(tr(m.label))}</span>
          </div>
          <p class="st-card-sub">${esc(sub)}</p>
          <div class="st-card-foot">
            ${latency}
          </div>
        </article>`;
    };

    const auth = (data && data.auth) || null;
    const authCls = auth ? (auth.online ? 'is-up' : 'is-down') : 'is-unk';
    const authLabel = auth ? (auth.online ? tr('Reachable') : tr('Unreachable')) : tr('Checking…');
    const authLatency = (auth && typeof auth.latency_ms === 'number' && auth.online)
      ? `<span class="st-card-latency">${Math.round(auth.latency_ms)} ms</span>` : '';
    const authCard = `
      <article class="st-card ${authCls}">
        <div class="st-card-head">
          <span class="st-card-dot" aria-hidden="true"></span>
          <span class="st-card-name">${esc(tr('Account login'))}</span>
          <span class="st-card-status">${esc(authLabel)}</span>
        </div>
        <p class="st-card-sub">${esc(tr('Shared auth gateway (auth.trionworlds.com)'))}</p>
        <div class="st-card-foot">${authLatency}</div>
      </article>`;

    cards.innerHTML = envCard('eu', tr('EU')) + envCard('us', tr('US'))
                    + envCard('pts', tr('PTS (Test Server)')) + authCard;
  }

  // ─── History timeline ──────────────────────────────────────────────
  function renderHistory(data) {
    const body = document.getElementById('st-history-body');
    if (!body) return;
    const segments = (data && data.segments) || [];
    if (!segments.length) {
      body.innerHTML = `<p class="st-empty">${esc(tr('No history recorded yet - the timeline fills in as the prober runs.'))}</p>`;
      return;
    }

    const winStart = data.window_start, winEnd = data.window_end;
    const span = Math.max(1, winEnd - winStart);

    // Uptime summary.
    const uptimePct = data.uptime == null ? null : (data.uptime * 100);
    const uptimeStr = uptimePct == null ? '-' : `${uptimePct.toFixed(uptimePct >= 99.95 ? 2 : 1)}%`;

    // Proportional timeline bar - one block per segment, width by duration.
    const blocks = segments.map((s) => {
      const left = ((s.started_at - winStart) / span) * 100;
      const end = (s.ended_at == null ? winEnd : s.ended_at);
      const width = Math.max(0.3, ((end - s.started_at) / span) * 100);
      const m = STATUS_META[s.status] || STATUS_META.unknown;
      const title = `${tr(m.label)} · ${fmtClock(s.started_at)} → ${s.ended_at == null ? tr('now') : fmtClock(s.ended_at)} · ${fmtDuration(s.duration_seconds)}`;
      return `<span class="st-bar-seg ${m.cls}" style="left:${left.toFixed(3)}%;width:${width.toFixed(3)}%" title="${esc(title)}"></span>`;
    }).join('');

    // Outage log - non-online segments, newest first.
    const outages = (data.outages || []).slice().reverse();
    const outageRows = outages.length
      ? outages.map((o) => {
          const m = STATUS_META[o.status] || STATUS_META.unknown;
          const ongoing = o.ended_at == null;
          return `
            <li class="st-outage ${m.cls}">
              <span class="st-outage-badge">${esc(tr(m.label))}</span>
              <span class="st-outage-when">${esc(fmtClock(o.started_at))} → ${ongoing ? esc(tr('ongoing')) : esc(fmtClock(o.ended_at))}</span>
              <span class="st-outage-dur">${esc(fmtDuration(o.duration_seconds))}</span>
            </li>`;
        }).join('')
      : `<li class="st-outage-none">${esc(tr('No outages in this window. 🎉'))}</li>`;

    body.innerHTML = `
      <div class="st-uptime">
        <span class="st-uptime-num">${esc(uptimeStr)}</span>
        <span class="st-uptime-label">${esc(tr('uptime · last {d} days').replace('{d}', data.days))}</span>
      </div>
      <div class="st-bar" role="img" aria-label="${esc(tr('Status timeline'))}">${blocks}</div>
      <div class="st-bar-axis">
        <span>${esc(fmtClock(winStart))}</span>
        <span>${esc(tr('now'))}</span>
      </div>
      <ul class="st-outages">${outageRows}</ul>`;
  }

  // ─── Fetch + schedule ──────────────────────────────────────────────
  async function loadCurrent() {
    try {
      const r = await fetch('/site/trove-status');
      if (!r.ok) throw new Error(r.status);
      renderCurrent(await r.json());
    } catch (_) { /* keep prior render */ }
  }
  async function loadHistory() {
    const body = document.getElementById('st-history-body');
    try {
      const r = await fetch(`/site/trove-status/history?env=${historyEnv}&days=7`);
      if (!r.ok) throw new Error(r.status);
      renderHistory(await r.json());
    } catch (_) {
      if (body) body.innerHTML = `<p class="st-empty">${esc(tr('Could not load history.'))}</p>`;
    }
  }

  // Env tab switching.
  const tabs = document.getElementById('st-env-tabs');
  if (tabs) {
    tabs.addEventListener('click', (e) => {
      const btn = e.target.closest('[data-env]');
      if (!btn) return;
      historyEnv = btn.dataset.env;
      for (const b of tabs.querySelectorAll('[data-env]')) {
        const on = b === btn;
        b.classList.toggle('active', on);
        b.setAttribute('aria-selected', String(on));
      }
      loadHistory();
    });
  }

  // i18n relabel re-render (cards/history hold translated text).
  document.addEventListener('btt-lang-changed', () => { loadCurrent(); loadHistory(); });

  loadCurrent();
  loadHistory();
  setInterval(loadCurrent, 60_000);
  setInterval(loadHistory, 120_000);
  document.addEventListener('visibilitychange', () => {
    if (!document.hidden) { loadCurrent(); loadHistory(); }
  });
})();
