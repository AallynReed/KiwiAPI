/* site_auth.js - public user-accounts client. Loaded on every site page (navbar
   widget) and the active driver on /login; sign-in is Discord-only. Exposes
   window.BTTAuth for the dashboard. The backend at api.aallyn.net/v1/site-auth/*
   is CORS-allowlisted for *.aallyn.net with credentials (app/core/config.py), so
   page fetches stay a single hop.

   The session is HttpOnly cookies (app/site_auth/cookies.py), NOT localStorage -
   there is deliberately no token here for a script to read or exfiltrate. What
   that costs, and how each piece pays for it, is commented inline below. */

(function () {
  'use strict';

  const { esc } = window.BTTUtil;

  // Same rule the rest of the site uses (_site_util.js): the API origin in
  // production, same-origin ("") in dev - where the dev server reverse-proxies
  // /v1/site-auth/* so the session cookies actually apply to localhost.
  const API = window.API_BASE === undefined ? 'https://api.aallyn.net' : window.API_BASE;
  const STORAGE_PREFIX = 'btt_site_auth';
  const KEY_ACCESS = `${STORAGE_PREFIX}_access`;
  const KEY_REFRESH = `${STORAGE_PREFIX}_refresh`;
  const KEY_USER = `${STORAGE_PREFIX}_user`;        // cached /me snapshot

  // ─── Session storage ───────────────────────────────────────────────
  // The session lives in HttpOnly cookies set by /v1/site-auth/* (see
  // app/site_auth/cookies.py). Script cannot read them, which is the point:
  // an HTML injection can no longer walk off with a 30-day refresh token.
  //
  // Because we can't see the real cookies, the server also sets a
  // non-HttpOnly, valueless HINT cookie. It is the only thing that tells an
  // anonymous visitor apart from a signed-in one without spending a request,
  // and it holds no secret - losing it costs one wasted /me, nothing more.
  const HINT_COOKIE = 'kiwi_site_session';

  function hasHint() {
    return document.cookie.split('; ').some((c) => c.startsWith(HINT_COOKIE + '='));
  }

  // Expire the hint locally. The server clears it on logout, but a refresh
  // that comes back 401 (session revoked, 30 days elapsed) is rendered by the
  // error handler and carries no Set-Cookie - without this the client would
  // keep believing it had a session and re-probe /me on every page load.
  function dropHint() {
    const base = HINT_COOKIE + '=; Max-Age=0; Path=/; SameSite=Lax';
    document.cookie = base;
    const host = location.hostname.split('.');
    if (host.length >= 3) document.cookie = base + '; Domain=.' + host.slice(-2).join('.');
  }

  // Pre-cookie sessions kept their tokens in localStorage. That is the exposure
  // the cookie migration removed, so they are no longer honoured: the keys are
  // purged on load and the holder signs in again to get an HttpOnly session.
  function purgeLegacy() {
    try {
      localStorage.removeItem(KEY_ACCESS);
      localStorage.removeItem(KEY_REFRESH);
    } catch (_) {}
  }
  purgeLegacy();

  // Is there anything worth asking the server about? Cheap and synchronous.
  function hasSession() {
    return hasHint();
  }

  function clearSession() {
    dropHint();
    try { localStorage.removeItem(KEY_USER); } catch (_) {}
    _meCache = null;
  }

  // ─── Authenticated fetch with refresh-on-401 ───────────────────────
  async function call(path, opts = {}) {
    const url = path.startsWith('http') ? path : API + path;
    const headers = Object.assign({}, opts.headers || {});
    if (opts.json !== undefined) {
      headers['Content-Type'] = 'application/json';
    }
    const init = {
      method: opts.method || (opts.json !== undefined ? 'POST' : 'GET'),
      headers,
      body: opts.json !== undefined ? JSON.stringify(opts.json) : opts.body,
      // The session cookie is the credential now, and this is cross-origin
      // (trove.aallyn.net -> api.aallyn.net), so it has to be asked for
      // explicitly. The API allowlists our origin with allow_credentials.
      credentials: 'include',
    };
    let res = await fetch(url, init);
    // 401 → refresh once and retry. We never refresh while ALREADY
    // refreshing to avoid loops; the second 401 in a row clears state
    // and bubbles up.
    if (res.status === 401 && opts.auth !== false && !opts._retried && hasSession()) {
      const ok = await refresh();
      if (ok) {
        res = await fetch(url, init);
      } else {
        clearSession();
      }
    }
    return res;
  }

  async function callJSON(path, opts) {
    const res = await call(path, opts);
    let data = null;
    try { data = await res.json(); } catch (_) { /* no body */ }
    return { ok: res.ok, status: res.status, data };
  }

  async function refresh() {
    if (!hasSession()) return false;
    try {
      const r = await fetch(API + '/v1/site-auth/refresh', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'include',
        // Empty body - the server reads the refresh token from the HttpOnly
        // cookie. Nothing here ever holds one.
        body: '{}',
      });
      if (!r.ok) { clearSession(); return false; }
      return true;
    } catch (_) {
      return false;
    }
  }

  async function logout() {
    try {
      await fetch(API + '/v1/site-auth/logout', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'include',
        body: '{}',
      });
    } catch (_) { /* best-effort - the local clear below still happens */ }
    clearSession();
    // Broadcast so the navbar widget on this page re-renders without a hard
    // reload, and so other tabs drop their session too.
    announce();
    document.dispatchEvent(new CustomEvent('btt-auth-changed', { detail: { user: null } }));
  }

  // ─── /me cache ─────────────────────────────────────────────────────
  let _meCache = null;
  let _meInflight = null;

  async function getMe({ force = false } = {}) {
    if (!hasSession()) return null;
    if (!force && _meCache) return _meCache;
    if (_meInflight) return _meInflight;
    _meInflight = (async () => {
      const r = await callJSON('/v1/site-auth/me');
      if (r.ok && r.data) {
        _meCache = r.data;
        try { localStorage.setItem(KEY_USER, JSON.stringify(r.data)); } catch (_) {}
        announce();
        document.dispatchEvent(new CustomEvent('btt-auth-changed', { detail: { user: r.data } }));
        return r.data;
      }
      // The hint outlived the session (revoked elsewhere, or 30 days passed).
      // Drop it so we stop probing on every page load.
      if (r.status === 401) clearSession();
      return null;
    })().finally(() => { _meInflight = null; });
    return _meInflight;
  }

  // Synchronous best-guess from the cached /me snapshot. Useful for
  // the navbar widget on first paint - avoids a flash of "Sign in"
  // for logged-in users between page nav and the /me fetch landing.
  function getCachedUser() {
    if (_meCache) return _meCache;
    try {
      const raw = localStorage.getItem(KEY_USER);
      if (raw) { _meCache = JSON.parse(raw); return _meCache; }
    } catch (_) {}
    return null;
  }

  // ─── Navbar account widget ─────────────────────────────────────────
  // Renders into <span id="nav-account">. Two states:
  //   • signed-out: "Sign in" pill linking to /login
  //   • signed-in: avatar with initials + dropdown (Dashboard, Sign out)
  function renderNav(user) {
    const $el = document.getElementById('nav-account');
    if (!$el) return;
    if (!user) {
      $el.dataset.state = 'out';
      $el.innerHTML = `
        <a class="nav-account-signin" href="/login" data-i18n>Sign in</a>`;
      rerunI18n();
      return;
    }
    $el.dataset.state = 'in';
    const label = user.display_name || user.username || '?';
    const initials = (label.match(/[a-zA-Z0-9]/g) || ['?'])[0].toUpperCase();
    // Prefer the user's Discord avatar; fall back to an initials chip.
    const avatar = user.avatar_url
      ? `<img class="nav-account-avatar" src="${esc(user.avatar_url)}" alt="" referrerpolicy="no-referrer">`
      : `<span class="nav-account-avatar">${esc(initials)}</span>`;
    $el.innerHTML = `
      ${bellHTML()}
      <div class="nav-account-menu">
        <button type="button" class="nav-account-trigger" id="nav-account-trigger"
                aria-haspopup="true" aria-expanded="false" aria-controls="nav-account-panel">
          ${avatar}
          <span class="nav-account-name">${esc(label)}</span>
          <i class="fa-solid fa-chevron-down" aria-hidden="true"></i>
        </button>
        <div class="nav-account-panel" id="nav-account-panel" role="menu" hidden>
          <a class="nav-account-item" href="/dashboard" role="menuitem">
            <i class="fa-solid fa-gauge-high" aria-hidden="true"></i>
            <span data-i18n>Dashboard</span>
          </a>
          <button type="button" class="nav-account-item nav-account-item-danger"
                  id="nav-account-signout" role="menuitem">
            <i class="fa-solid fa-right-from-bracket" aria-hidden="true"></i>
            <span data-i18n>Sign out</span>
          </button>
        </div>
      </div>`;
    rerunI18n();

    wireBell();

    const $trigger = document.getElementById('nav-account-trigger');
    const $panel = document.getElementById('nav-account-panel');
    const $signout = document.getElementById('nav-account-signout');
    const setOpen = (open) => {
      $trigger.setAttribute('aria-expanded', String(open));
      $panel.hidden = !open;
    };
    setOpen(false);
    $trigger.addEventListener('click', (e) => {
      e.stopPropagation();
      setOpen($panel.hidden);
    });
    document.addEventListener('click', (e) => {
      if (!$el.contains(e.target)) setOpen(false);
    });
    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && !$panel.hidden) { setOpen(false); $trigger.focus(); }
    });
    $signout.addEventListener('click', async () => {
      await logout();
      // Logged out from /dashboard → bounce to login. Anywhere else,
      // stay on the current page and let the navbar refresh.
      if (location.pathname === '/dashboard') location.href = '/login';
    });
  }

  // ─── Mod-issue notifications (navbar bell) ─────────────────────────
  // A creator has to hear that someone filed a bug on their mod, and the person
  // who filed it has to hear the answer. The feed is derived server-side from
  // the threads you take part in (app/trove/mods_hub/issues.py) - there is no
  // per-user notification store to keep in sync, and nothing to clean up.
  //
  // Rendered inside the account widget, so it exists exactly when a session
  // does. A 404 (feature switched off site-wide) simply leaves the bell empty.
  let _notif = null;

  function bellHTML() {
    return `
      <div class="nav-bell" id="nav-bell" hidden>
        <button type="button" class="nav-bell-trigger" id="nav-bell-trigger"
                aria-haspopup="true" aria-expanded="false" aria-controls="nav-bell-panel"
                aria-label="${esc(t('Notifications'))}" title="${esc(t('Notifications'))}">
          <i class="fa-solid fa-bell" aria-hidden="true"></i>
          <span class="nav-bell-count" id="nav-bell-count" hidden></span>
        </button>
        <div class="nav-bell-panel" id="nav-bell-panel" role="menu" hidden>
          <p class="nav-bell-head">${esc(t('Issues & requests'))}</p>
          <div class="nav-bell-list" id="nav-bell-list"></div>
        </div>
      </div>`;
  }

  function wireBell() {
    const $bell = document.getElementById('nav-bell');
    const $trigger = document.getElementById('nav-bell-trigger');
    const $panel = document.getElementById('nav-bell-panel');
    if (!$bell || !$trigger || !$panel) return;
    const setOpen = (open) => {
      $trigger.setAttribute('aria-expanded', String(open));
      $panel.hidden = !open;
      // Opening the panel IS reading it: the server keeps one watermark per
      // account rather than a read flag per row.
      if (open && _notif && _notif.unread) markNotificationsSeen();
    };
    $trigger.addEventListener('click', (e) => {
      e.stopPropagation();
      setOpen($panel.hidden);
    });
    document.addEventListener('click', (e) => {
      if (!$bell.contains(e.target)) setOpen(false);
    });
    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && !$panel.hidden) { setOpen(false); $trigger.focus(); }
    });
    loadNotifications();
  }

  async function loadNotifications() {
    try {
      const r = await fetch(API + '/site/mods/notifications', { credentials: 'include' });
      if (!r.ok) return;                       // feature off, or no session - stay hidden
      _notif = await r.json();
      paintNotifications();
    } catch (_) { /* the bell just stays hidden */ }
  }

  function paintNotifications() {
    const $bell = document.getElementById('nav-bell');
    const $count = document.getElementById('nav-bell-count');
    const $list = document.getElementById('nav-bell-list');
    if (!$bell || !$count || !$list || !_notif) return;
    const items = _notif.items || [];
    if (!items.length) { $bell.hidden = true; return; }
    $bell.hidden = false;
    $count.hidden = !_notif.unread;
    $count.textContent = _notif.unread > 9 ? '9+' : String(_notif.unread || '');
    $list.innerHTML = items.map((n) => `
      <a class="nav-bell-item ${n.unread ? 'is-new' : ''}" href="${esc(n.url)}" role="menuitem">
        <i class="fa-solid ${n.status === 'closed' ? 'fa-circle-check'
          : (n.kind === 'request' ? 'fa-lightbulb' : 'fa-circle-dot')}" aria-hidden="true"></i>
        <span class="nav-bell-text">
          <span class="nav-bell-title">${esc(n.title)}</span>
          <span class="nav-bell-sub">${esc(n.mod_title)} · #${esc(String(n.number))}</span>
        </span>
      </a>`).join('');
    rerunI18n();
  }

  async function markNotificationsSeen() {
    const $count = document.getElementById('nav-bell-count');
    if ($count) $count.hidden = true;
    if (_notif) {
      _notif.unread = 0;
      (_notif.items || []).forEach((n) => { n.unread = false; });
    }
    try { await call('/v1/mods/hub/me/issue-notifications/seen', { method: 'POST', json: {} }); }
    catch (_) { /* the badge is already down; it comes back on the next load if not */ }
  }

  // ─── Discord sign-in button ────────────────────────────────────────
  // The template ships the production URL so the button still works with no JS.
  // This re-points it at whatever API origin this deployment actually uses (dev
  // would otherwise sign you into production), and carries ``next`` along -
  // Discord round-trips through its own domain, so the server has to hand it
  // back to us afterwards (app/site_auth/oauth.py).
  function wireDiscordButton() {
    const $btn = document.querySelector('.acc-oauth-discord');
    if (!$btn) return;
    let url = API + '/v1/site-auth/oauth/discord/start';
    const next = new URLSearchParams(location.search).get('next');
    if (next && next.startsWith('/') && !next.startsWith('//')) {
      url += '?next=' + encodeURIComponent(next);
    }
    $btn.setAttribute('href', url);
  }

  // ─── Discord OAuth return ──────────────────────────────────────────

  // Complete a Discord OAuth return: the callback lands back on the site with
  // #discord=<one-time-code> in the fragment. Swap it for site tokens, then go
  // to the dashboard. Runs on every page load (no-op without the fragment) so
  // it still fires when the password form is hidden (Discord-only mode).
  async function handleOAuthReturn() {
    const m = location.hash.match(/(?:^|[#&])discord=([^&]+)/);
    if (!m) return;
    history.replaceState(null, '', location.pathname + location.search);
    try {
      const r = await callJSON('/v1/site-auth/oauth/exchange', {
        method: 'POST', json: { code: decodeURIComponent(m[1]) }, auth: false,
      });
      if (!r.ok) throw new Error('exchange');
      // Nothing is stored here on purpose: the response set the HttpOnly
      // session cookies (and the hint), so the tokens in r.data are for
      // non-browser clients only. Writing them to localStorage is exactly the
      // exposure this migration removed.
      if (!hasHint()) throw new Error('no-session-cookie');
      await getMe({ force: true });
      const rawNext = new URLSearchParams(location.search).get('next') || '';
      // Resolve against our own origin and only keep the path if it stays
      // same-origin - defeats "//evil.com", "https://evil.com" and "javascript:".
      let safeNext = '/dashboard';
      try {
        const u = new URL(rawNext, location.origin);
        if (u.origin === location.origin) safeNext = u.pathname + u.search + u.hash;
      } catch (_) { /* malformed - keep the default */ }
      location.href = safeNext;
    } catch (_) {
      const $err = document.getElementById('login-error');
      if ($err) {
        $err.textContent = t('Discord sign-in failed. Please try again.');
        $err.hidden = false;
      }
    }
  }

  // ─── i18n helpers ──────────────────────────────────────────────────
  function t(s) {
    return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s;
  }
  function rerunI18n() {
    if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh();
  }

  // Translate a server error envelope into a clean human string. The
  // backend returns either { error: { message } } or pydantic's
  // { detail: [...] } depending on which validation layer fired.
  function errorMessage(data) {
    if (!data) return null;
    if (data.error && data.error.message) return data.error.message;
    if (typeof data.detail === 'string') return data.detail;
    if (Array.isArray(data.detail) && data.detail.length) {
      const first = data.detail[0];
      return first.msg || JSON.stringify(first);
    }
    return null;
  }

  // ─── Cross-tab sync ────────────────────────────────────────────────
  // The session used to live in localStorage, so a sign-in/out in another tab
  // arrived for free as a 'storage' event. Cookies fire no such event, so say
  // it explicitly over BroadcastChannel. The 'storage' listener stays for the
  // legacy KEY_USER snapshot (and older browsers without BroadcastChannel).
  let _channel = null;
  try {
    _channel = new BroadcastChannel('btt-site-auth');
    _channel.addEventListener('message', () => {
      _meCache = null;
      bootNav();
    });
  } catch (_) { /* unsupported - the storage event below still covers most cases */ }

  function announce() {
    try { if (_channel) _channel.postMessage(1); } catch (_) {}
  }

  window.addEventListener('storage', (e) => {
    if (e.key === KEY_USER) {
      _meCache = null;
      bootNav();
    }
  });

  // ─── Boot ──────────────────────────────────────────────────────────
  async function bootNav() {
    // Paint from cache first (no Sign-in flash), then refresh from the server.
    const cached = getCachedUser();
    renderNav(cached);
    // The hint cookie is what keeps an anonymous visitor from spending a /me
    // request on every page just to be told they're anonymous.
    if (!hasSession()) { renderNav(null); return; }
    const fresh = await getMe();
    renderNav(fresh);
  }

  function boot() {
    bootNav();
    wireDiscordButton();
    handleOAuthReturn();
    // Re-render the navbar on language change so localized strings refresh.
    document.addEventListener('btt-lang-changed', () => {
      renderNav(getCachedUser());
    });
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', boot);
  } else {
    boot();
  }

  // ─── Public surface for the dashboard JS to import ─────────────────
  window.BTTAuth = {
    call,
    callJSON,
    refresh,
    logout,
    getMe,
    getCachedUser,
    hasSession,
    API,
    errorMessage,
    // Kept only so a stale cached page script reading `.access` gets null
    // instead of throwing. The session is HttpOnly cookies - there is nothing
    // here to read. Callers wanting "am I signed in?" use hasSession().
    tokens: { access: null, refresh: null },
  };
})();
