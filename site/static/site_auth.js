/* ═══════════════════════════════════════════════════════════════════════
   site_auth.js - public-facing user accounts client
   ───────────────────────────────────────────────────────────────────────
   Loaded on EVERY site page (via the navbar widget) and is the active
   driver on /login + /signup. Wires up:

     • localStorage token storage with refresh-on-401
     • Login + signup forms (when present on the page)
     • The "Sign in" / signed-in avatar+dropdown in the navbar
     • A small global API (window.BTTAuth) the dashboard imports from

   The backend lives at https://api.aallyn.net/v1/site-auth/* and is
   CORS-allowlisted for *.aallyn.net (see app/core/config.py). The
   page-side fetch path stays a single hop - no /site/auth/* proxy.
   ═══════════════════════════════════════════════════════════════════════ */

(function () {
  'use strict';

  const API = 'https://api.aallyn.net';
  const STORAGE_PREFIX = 'btt_site_auth';
  const KEY_ACCESS = `${STORAGE_PREFIX}_access`;
  const KEY_REFRESH = `${STORAGE_PREFIX}_refresh`;
  const KEY_USER = `${STORAGE_PREFIX}_user`;        // cached /me snapshot

  // Captcha provider config. Lazy-loaded on first use of getCaptchaConfig().
  // /config is unauthenticated, so it's safe to fetch from any page.
  let _captchaConfig = null;
  let _captchaConfigPromise = null;
  async function getCaptchaConfig() {
    if (_captchaConfig) return _captchaConfig;
    if (_captchaConfigPromise) return _captchaConfigPromise;
    _captchaConfigPromise = (async () => {
      try {
        const r = await fetch(API + '/config');
        if (!r.ok) return null;
        const data = await r.json();
        _captchaConfig = {
          provider: data.captcha_provider || 'turnstile',
          sitekey:  data.captcha_sitekey  || null,
        };
        return _captchaConfig;
      } catch (_) { return null; }
    })().finally(() => { _captchaConfigPromise = null; });
    return _captchaConfigPromise;
  }

  // ─── Captcha widget loader ─────────────────────────────────────────
  // Both hCaptcha and Turnstile expose the same widget primitive at
  // ``window.hcaptcha`` / ``window.turnstile``: ``.render(container,
  // {sitekey, callback, ...})`` returns a widget id, ``.reset(id)``
  // recycles the token. The form code stores the most recent token in
  // a closure and clears it on submit so each request gets a fresh one
  // (both providers issue single-use tokens).
  const _captchaScripts = {
    hcaptcha:  'https://hcaptcha.com/1/api.js?render=explicit',
    turnstile: 'https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit',
  };
  let _captchaScriptLoaded = null;
  async function loadCaptchaScript(provider) {
    if (_captchaScriptLoaded === provider) return true;
    if (_captchaScriptLoaded) return _captchaScriptLoaded === provider;
    const src = _captchaScripts[provider];
    if (!src) return false;
    await new Promise((resolve, reject) => {
      const s = document.createElement('script');
      s.src = src;
      s.async = true;
      s.defer = true;
      s.onload = () => resolve();
      s.onerror = () => reject(new Error('captcha script failed to load'));
      document.head.appendChild(s);
    });
    _captchaScriptLoaded = provider;
    return true;
  }

  // Render a captcha widget into ``$mount``, returning a getter that
  // yields the current token (or null if the widget isn't ready yet).
  // No-op when captcha isn't configured - returns a getter that always
  // gives null, which the server treats as "no captcha required" given
  // its disable-when-unconfigured semantics.
  async function mountCaptcha($mount) {
    const cfg = await getCaptchaConfig();
    if (!cfg || !cfg.sitekey) return () => null;
    try {
      await loadCaptchaScript(cfg.provider);
    } catch (_) { return () => null; }
    let token = null;
    let widgetId = null;
    // Both providers' render API resolves the global as soon as the
    // script tag's onload fires AND the provider's own ready callback
    // has run. Poll briefly.
    const globalName = cfg.provider === 'hcaptcha' ? 'hcaptcha' : 'turnstile';
    for (let i = 0; i < 40 && !window[globalName]; i++) {
      await new Promise((r) => setTimeout(r, 50));
    }
    const widget = window[globalName];
    if (!widget) return () => null;
    widgetId = widget.render($mount, {
      sitekey: cfg.sitekey,
      theme: 'dark',
      callback: (t_) => { token = t_; },
      'expired-callback': () => { token = null; },
      'error-callback':   () => { token = null; },
    });
    return () => {
      const out = token;
      // Reset the widget so subsequent submits don't reuse a token.
      // Providers refuse a second use anyway, but the widget would
      // otherwise show a "checked" state after a failed call.
      if (out && widget.reset && widgetId !== null) {
        try { widget.reset(widgetId); } catch (_) {}
        token = null;
      }
      return out;
    };
  }

  // ─── Token storage ─────────────────────────────────────────────────
  // localStorage rather than cookies so we don't need CSRF tokens on
  // every form submit. Same trade-off the dev portal makes. JWTs in
  // localStorage are XSS-readable; the CSP locks the page down enough
  // that injected scripts are a non-trivial bar.
  const tokens = {
    get access() {
      try { return localStorage.getItem(KEY_ACCESS); } catch (_) { return null; }
    },
    get refresh() {
      try { return localStorage.getItem(KEY_REFRESH); } catch (_) { return null; }
    },
    save(access, refresh) {
      try {
        if (access)  localStorage.setItem(KEY_ACCESS,  access);
        if (refresh) localStorage.setItem(KEY_REFRESH, refresh);
      } catch (_) {}
    },
    clear() {
      try {
        localStorage.removeItem(KEY_ACCESS);
        localStorage.removeItem(KEY_REFRESH);
        localStorage.removeItem(KEY_USER);
      } catch (_) {}
    },
  };

  // ─── Authenticated fetch with refresh-on-401 ───────────────────────
  async function call(path, opts = {}) {
    const url = path.startsWith('http') ? path : API + path;
    const headers = Object.assign({}, opts.headers || {});
    if (opts.json !== undefined) {
      headers['Content-Type'] = 'application/json';
    }
    if (opts.auth !== false && tokens.access) {
      headers['Authorization'] = 'Bearer ' + tokens.access;
    }
    const init = {
      method: opts.method || (opts.json !== undefined ? 'POST' : 'GET'),
      headers,
      body: opts.json !== undefined ? JSON.stringify(opts.json) : opts.body,
    };
    let res = await fetch(url, init);
    // 401 → refresh once and retry. We never refresh while ALREADY
    // refreshing to avoid loops; the second 401 in a row clears state
    // and bubbles up.
    if (res.status === 401 && opts.auth !== false && !opts._retried && tokens.refresh) {
      const ok = await refresh();
      if (ok) {
        headers['Authorization'] = 'Bearer ' + tokens.access;
        res = await fetch(url, Object.assign({}, init, { headers }));
      } else {
        tokens.clear();
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
    if (!tokens.refresh) return false;
    try {
      const r = await fetch(API + '/v1/site-auth/refresh', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ refresh_token: tokens.refresh }),
      });
      if (!r.ok) return false;
      const data = await r.json();
      tokens.save(data.access_token, data.refresh_token);
      return true;
    } catch (_) {
      return false;
    }
  }

  async function logout() {
    if (tokens.refresh) {
      try {
        await fetch(API + '/v1/site-auth/logout', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ refresh_token: tokens.refresh }),
        });
      } catch (_) { /* best-effort */ }
    }
    tokens.clear();
    // Broadcast so the navbar widget on this page re-renders without
    // a hard reload. Other tabs pick it up via the 'storage' event.
    document.dispatchEvent(new CustomEvent('btt-auth-changed', { detail: { user: null } }));
  }

  // ─── /me cache ─────────────────────────────────────────────────────
  let _meCache = null;
  let _meInflight = null;

  async function getMe({ force = false } = {}) {
    if (!tokens.access) return null;
    if (!force && _meCache) return _meCache;
    if (_meInflight) return _meInflight;
    _meInflight = (async () => {
      const r = await callJSON('/v1/site-auth/me');
      if (r.ok && r.data) {
        _meCache = r.data;
        try { localStorage.setItem(KEY_USER, JSON.stringify(r.data)); } catch (_) {}
        document.dispatchEvent(new CustomEvent('btt-auth-changed', { detail: { user: r.data } }));
        return r.data;
      }
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
    $el.innerHTML = `
      <div class="nav-account-menu">
        <button type="button" class="nav-account-trigger" id="nav-account-trigger"
                aria-haspopup="true" aria-expanded="false" aria-controls="nav-account-panel">
          <span class="nav-account-avatar">${esc(initials)}</span>
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

  // ─── Form wiring (login + signup) ──────────────────────────────────
  function wireLoginForm() {
    const $form = document.getElementById('login-form');
    if (!$form) return;
    const $err = document.getElementById('login-error');
    const $submit = document.getElementById('login-submit');
    const $captcha = document.getElementById('login-captcha');

    // Mount captcha (no-op when unconfigured). The returned getter
    // gives the current token or null - we pass it to the server
    // either way and let the server's verify_captcha decide.
    let getCaptchaToken = () => null;
    if ($captcha) {
      mountCaptcha($captcha).then((fn) => { getCaptchaToken = fn; });
    }

    $form.addEventListener('submit', async (e) => {
      e.preventDefault();
      $err.hidden = true;
      $submit.disabled = true;
      const identifier = document.getElementById('login-identifier').value.trim();
      const password = document.getElementById('login-password').value;
      const captcha_token = getCaptchaToken();
      try {
        const r = await callJSON('/v1/site-auth/login', {
          method: 'POST',
          json: { identifier, password, captcha_token },
          auth: false,
        });
        if (!r.ok) {
          $err.textContent = errorMessage(r.data) || t('Sign-in failed.');
          $err.hidden = false;
          return;
        }
        tokens.save(r.data.access_token, r.data.refresh_token);
        await getMe({ force: true });
        // Redirect target: ?next=… if present (set by 401 handler),
        // otherwise /dashboard.
        const next = new URLSearchParams(location.search).get('next');
        location.href = next && next.startsWith('/') ? next : '/dashboard';
      } finally {
        $submit.disabled = false;
      }
    });

    // Inline resend-verification link - uses the email half of the
    // identifier (if it looks like an email) for the resend call.
    const $resend = document.getElementById('resend-verify-link');
    if ($resend) {
      $resend.addEventListener('click', async (e) => {
        e.preventDefault();
        const raw = document.getElementById('login-identifier').value.trim();
        if (raw.indexOf('@') < 0) {
          $err.textContent = t('Enter your email above first, then click resend.');
          $err.hidden = false;
          return;
        }
        const r = await callJSON('/v1/site-auth/resend-verification', {
          method: 'POST',
          json: { email: raw },
          auth: false,
        });
        $err.textContent = (r.data && r.data.message) || t('Verification email sent.');
        $err.hidden = false;
      });
    }
  }

  function wireSignupForm() {
    const $form = document.getElementById('signup-form');
    if (!$form) return;
    const $err = document.getElementById('signup-error');
    const $submit = document.getElementById('signup-submit');
    const $captcha = document.getElementById('signup-captcha');

    let getCaptchaToken = () => null;
    if ($captcha) {
      mountCaptcha($captcha).then((fn) => { getCaptchaToken = fn; });
    }

    $form.addEventListener('submit', async (e) => {
      e.preventDefault();
      $err.hidden = true;
      $submit.disabled = true;
      const username = document.getElementById('signup-username').value.trim();
      const email = document.getElementById('signup-email').value.trim();
      const password = document.getElementById('signup-password').value;
      const displayName = (document.getElementById('signup-display-name').value || '').trim();
      const captcha_token = getCaptchaToken();
      try {
        const r = await callJSON('/v1/site-auth/signup', {
          method: 'POST',
          json: {
            username,
            email,
            password,
            display_name: displayName || null,
            captcha_token,
          },
          auth: false,
        });
        if (!r.ok) {
          $err.textContent = errorMessage(r.data) || t('Sign-up failed.');
          $err.hidden = false;
          return;
        }
        // Account created - log them in immediately. The user can browse
        // the dashboard while their email verifies in the background.
        // The captcha token we just used is already consumed; the login
        // endpoint will skip the gate if captcha is fully disabled.
        // Otherwise we'd need a fresh token, which the user would have
        // to solve again - for now we accept the corner case of an
        // immediate-after-signup auto-login bouncing on captcha and
        // having the user land on /login.
        const login = await callJSON('/v1/site-auth/login', {
          method: 'POST',
          json: { identifier: username, password, captcha_token: null },
          auth: false,
        });
        if (login.ok) {
          tokens.save(login.data.access_token, login.data.refresh_token);
          await getMe({ force: true });
        }
        location.href = '/dashboard';
      } finally {
        $submit.disabled = false;
      }
    });
  }

  // ─── i18n helpers ──────────────────────────────────────────────────
  function t(s) {
    return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s;
  }
  function rerunI18n() {
    if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh();
  }

  function esc(s) {
    return String(s ?? '').replace(/[&<>"']/g, (c) =>
      ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
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

  // ─── Cross-tab + storage sync ──────────────────────────────────────
  // If the user logs out (or in) in another tab, mirror the change here.
  window.addEventListener('storage', (e) => {
    if (e.key === KEY_ACCESS || e.key === KEY_REFRESH || e.key === KEY_USER) {
      _meCache = null;
      bootNav();
    }
  });

  // ─── Boot ──────────────────────────────────────────────────────────
  async function bootNav() {
    // First paint: use cached user (if any) so the avatar shows
    // immediately. Then refresh from the server.
    const cached = getCachedUser();
    renderNav(cached);
    if (!tokens.access) { renderNav(null); return; }
    const fresh = await getMe();
    renderNav(fresh);
  }

  function boot() {
    bootNav();
    wireLoginForm();
    wireSignupForm();
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
    tokens,
    API,
    errorMessage,
    mountCaptcha,
    getCaptchaConfig,
  };
})();
