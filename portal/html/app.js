"use strict";

const app = document.getElementById("app");
const state = { config: null, user: null, tab: "tokens" };

// The frontend (dev.aallyn.net) talks to the API (api.aallyn.net) cross-origin.
const API_BASE = "https://api.aallyn.net";

// --- Theme (light / dark, persisted) ---------------------------------------

const THEME_KEY = "kiwi_theme";
function currentTheme() { return localStorage.getItem(THEME_KEY) === "light" ? "light" : "dark"; }
function applyTheme(t) { document.documentElement.setAttribute("data-theme", t); localStorage.setItem(THEME_KEY, t); }
// Apply immediately (before first paint) to avoid a flash of the wrong theme.
document.documentElement.setAttribute("data-theme", currentTheme());

function themeBtn() {
  const icon = currentTheme() === "dark" ? "☀" : "☾";
  return `<button class="theme-toggle" type="button" aria-label="Toggle light or dark theme" title="Toggle theme">${icon}</button>`;
}
function syncThemeToggles() {
  const icon = currentTheme() === "dark" ? "☀" : "☾";
  document.querySelectorAll(".theme-toggle").forEach((b) => { b.textContent = icon; });
}
document.addEventListener("click", (e) => {
  if (e.target.closest(".theme-toggle")) {
    applyTheme(currentTheme() === "dark" ? "light" : "dark");
    syncThemeToggles();
  }
});

// --- API helper ------------------------------------------------------------

const API = {
  token: localStorage.getItem("kiwi_jwt"),
  refresh: localStorage.getItem("kiwi_refresh"),

  setTokens(access, refresh) {
    this.token = access;
    localStorage.setItem("kiwi_jwt", access);
    if (refresh) { this.refresh = refresh; localStorage.setItem("kiwi_refresh", refresh); }
  },
  clear() {
    this.token = null; this.refresh = null;
    localStorage.removeItem("kiwi_jwt");
    localStorage.removeItem("kiwi_refresh");
  },

  // Try once to swap an expired access token for a fresh one (rotates refresh).
  async _tryRefresh() {
    if (!this.refresh) return false;
    try {
      const res = await fetch(API_BASE + "/auth/refresh", {
        method: "POST", headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ refresh_token: this.refresh }),
      });
      if (!res.ok) return false;
      const data = await res.json();
      this.setTokens(data.access_token, data.refresh_token);
      return true;
    } catch (_) { return false; }
  },

  call(path, opts = {}) { return this._call(path, opts, true); },

  // Multipart upload - separate from `call` because file ingest endpoints
  // (leaderboards, market) take a `file` field, and FormData mustn't be
  // JSON-serialised. Mirrors `_call`'s auth-refresh-on-401 behaviour.
  multipart(path, formData, opts = {}) { return this._multipart(path, formData, opts, true); },

  async _multipart(path, formData, { query = null } = {}, allowRefresh = true) {
    const headers = {};
    if (this.token) headers["Authorization"] = "Bearer " + this.token;
    let url = API_BASE + path;
    if (query) {
      const qs = new URLSearchParams();
      for (const [k, v] of Object.entries(query)) {
        if (v != null && v !== "") qs.set(k, String(v));
      }
      const s = qs.toString();
      if (s) url += "?" + s;
    }
    // Don't set Content-Type - fetch derives the multipart boundary itself.
    const res = await fetch(url, { method: "POST", headers, body: formData });
    let data = null;
    try { data = await res.json(); } catch (_) { /* no body */ }
    if (res.status === 401 && allowRefresh) {
      if (await this._tryRefresh()) return this._multipart(path, formData, { query }, false);
      this.clear();
      const wasLoggedIn = !!state.user;
      state.user = null;
      location.hash = "";
      renderAuth("login");
      if (wasLoggedIn) toast("Your session expired - please log in again.", "err");
      throw { code: "session_expired", message: "Session expired" };
    }
    if (!res.ok) throw (data && data.error) || { code: String(res.status), message: `HTTP ${res.status}` };
    return data;
  },

  async _call(path, { method = "GET", body = null, auth = true } = {}, allowRefresh = true) {
    const headers = {};
    if (body) headers["Content-Type"] = "application/json";
    if (auth && this.token) headers["Authorization"] = "Bearer " + this.token;
    const res = await fetch(API_BASE + path, { method, headers, body: body ? JSON.stringify(body) : undefined });
    let data = null;
    try { data = await res.json(); } catch (_) { /* no body */ }

    if (res.status === 401 && auth && allowRefresh) {
      if (await this._tryRefresh()) return this._call(path, { method, body, auth }, false);
      // Refresh failed - the session is truly gone. Bounce to login gracefully.
      this.clear();
      const wasLoggedIn = !!state.user;
      state.user = null;
      location.hash = "";
      renderAuth("login");
      if (wasLoggedIn) toast("Your session expired - please log in again.", "err");
      throw { code: "session_expired", message: "Session expired" };
    }
    if (!res.ok) throw (data && data.error) || { code: String(res.status), message: `HTTP ${res.status}` };
    return data;
  },
};

// --- Utilities -------------------------------------------------------------

function esc(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}
function fmt(iso) { return iso ? new Date(iso).toLocaleString() : "-"; }
function fmtDay(iso) { return iso ? new Date(iso).toLocaleDateString() : "-"; }

// Local password-strength heuristic (no network - the server still does the
// authoritative HaveIBeenPwned breach check and rejects compromised passwords).
function passwordStrength(pw) {
  if (!pw) return { score: 0, label: "", pct: 0 };
  let score = 0;
  if (pw.length >= 8) score++;
  if (pw.length >= 12) score++;
  if (/[a-z]/.test(pw) && /[A-Z]/.test(pw)) score++;
  if (/\d/.test(pw)) score++;
  if (/[^A-Za-z0-9]/.test(pw)) score++;
  if (/^(?:password|qwerty|12345|letmein|admin|welcome)/i.test(pw)) score = Math.min(score, 1);
  score = Math.max(0, Math.min(score, 4));
  return { score, label: ["Very weak", "Weak", "Fair", "Good", "Strong"][score], pct: (score / 4) * 100 };
}

function attachStrengthMeter(input) {
  if (!input) return;
  const meter = document.createElement("div");
  meter.className = "pw-meter";
  meter.innerHTML = `<div class="pw-meter-bar"><span></span></div><div class="pw-meter-label"></div>`;
  input.insertAdjacentElement("afterend", meter);
  const bar = meter.querySelector("span");
  const label = meter.querySelector(".pw-meter-label");
  const colors = ["#f85149", "#f85149", "#d29922", "#3fb950", "#3fb950"];
  const update = () => {
    const s = passwordStrength(input.value);
    bar.style.width = s.pct + "%";
    bar.style.background = colors[s.score];
    label.textContent = input.value ? s.label : "";
  };
  input.addEventListener("input", update);
  update();
}

function toast(msg, kind = "ok") {
  const el = document.getElementById("toast");
  el.textContent = msg;
  el.className = "toast " + kind;
  el.hidden = false;
  clearTimeout(toast._t);
  toast._t = setTimeout(() => { el.hidden = true; }, 4000);
}

// --- Captcha (Turnstile / hCaptcha, provider-driven) -----------------------

const captcha = { widgetId: null, token: null };

function loadCaptchaScript(provider) {
  return new Promise((resolve) => {
    const ready = provider === "hcaptcha" ? window.hcaptcha : window.turnstile;
    if (ready) return resolve();
    const s = document.createElement("script");
    s.src = provider === "hcaptcha"
      ? "https://js.hcaptcha.com/1/api.js?render=explicit"
      : "https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit";
    s.async = true; s.defer = true;
    s.onload = () => resolve();
    s.onerror = () => resolve();
    document.head.appendChild(s);
  });
}

async function mountCaptcha(el) {
  captcha.token = null; captcha.widgetId = null;
  const { captcha_provider: provider, captcha_sitekey: key } = state.config;
  if (!key) { el.innerHTML = ""; return; }
  await loadCaptchaScript(provider);
  const lib = provider === "hcaptcha" ? window.hcaptcha : window.turnstile;
  if (!lib) { el.innerHTML = "<p class='err-text'>Captcha failed to load.</p>"; return; }
  captcha.widgetId = lib.render(el, {
    sitekey: key,
    callback: (t) => { captcha.token = t; },
    "expired-callback": () => { captcha.token = null; },
  });
}
function resetCaptcha() {
  const { captcha_provider: p } = state.config || {};
  const lib = p === "hcaptcha" ? window.hcaptcha : window.turnstile;
  if (lib && captcha.widgetId != null) { try { lib.reset(captcha.widgetId); } catch (_) {} }
  captcha.token = null;
}

// --- Auth views ------------------------------------------------------------

function renderAuth(tab = "login") {
  document.body.classList.remove("app-shell");  // auth view = centered card, normal page flow
  app.innerHTML = `
    <div class="auth-wrap">${themeBtn()}<div class="auth-card">
      <div class="brand"><span class="mark">◆</span> ${esc(state.config?.app_name || "Kiwi API")}</div>
      <p class="sub">Developer portal</p>
      <div class="tabs">
        <button data-tab="login" class="${tab === "login" ? "active" : ""}">Log in</button>
        <button data-tab="signup" class="${tab === "signup" ? "active" : ""}">Sign up</button>
      </div>
      <div id="auth-body"></div>
      ${state.config.github_oauth_enabled ? `
        <div class="oauth-sep">or</div>
        <button class="btn" id="github-btn" style="width:100%">Sign in with GitHub</button>` : ""}
    </div></div>`;
  app.querySelectorAll(".tabs button").forEach((b) =>
    b.addEventListener("click", () => renderAuth(b.dataset.tab)));
  const gh = document.getElementById("github-btn");
  if (gh) gh.addEventListener("click", () => { window.location = API_BASE + "/auth/oauth/github/start"; });
  syncThemeToggles();
  tab === "login" ? renderLogin() : renderSignup();
}

function renderLogin() {
  document.getElementById("auth-body").innerHTML = `
    <form id="login-form">
      <label>Email</label><input type="email" name="email" required autocomplete="username">
      <label>Password</label><input type="password" name="password" required autocomplete="current-password">
      <div id="cap" style="margin-top:16px"></div>
      <div class="err-text" id="login-err"></div>
      <button class="btn primary" style="width:100%;margin-top:16px">Log in</button>
      <p class="field-help" style="text-align:center;margin-top:14px">
        <a href="#" id="forgot-link">Forgot your password?</a></p>
    </form>`;
  mountCaptcha(document.getElementById("cap"));
  document.getElementById("forgot-link").addEventListener("click", (e) => { e.preventDefault(); renderForgot(); });
  document.getElementById("login-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    const f = e.target; const err = document.getElementById("login-err"); err.textContent = "";
    if (state.config.captcha_sitekey && !captcha.token) { err.textContent = "Please complete the captcha."; return; }
    const btn = f.querySelector("button"); btn.disabled = true;
    try {
      const r = await API.call("/auth/login", { auth: false, method: "POST",
        body: { email: f.email.value, password: f.password.value, captcha_token: captcha.token } });
      API.setTokens(r.access_token, r.refresh_token);
      await loadDashboard();
    } catch (ex) {
      err.textContent = ex.message;
      if (ex.code === "email_unverified") {
        err.innerHTML = `${esc(ex.message)} <a href="#" id="resend-link">Resend verification email</a>`;
        document.getElementById("resend-link").addEventListener("click", async (e2) => {
          e2.preventDefault();
          try {
            const rr = await API.call("/auth/resend-verification", { auth: false, method: "POST", body: { email: f.email.value } });
            toast(rr.message, "ok");
          } catch (ex2) { toast(ex2.message, "err"); }
        });
      }
      resetCaptcha(); btn.disabled = false;
    }
  });
}

function renderSignup() {
  document.getElementById("auth-body").innerHTML = `
    <form id="signup-form">
      <label>Email</label><input type="email" name="email" required autocomplete="username">
      <label>Display name <span class="muted">(optional)</span></label><input type="text" name="display_name">
      <label>Password</label><input type="password" name="password" required minlength="8" autocomplete="new-password">
      <div id="cap" style="margin-top:16px"></div>
      <div class="err-text" id="signup-err"></div>
      <button class="btn primary" style="width:100%;margin-top:16px">Create account</button>
    </form>`;
  attachStrengthMeter(document.querySelector('#signup-form input[name="password"]'));
  mountCaptcha(document.getElementById("cap"));
  document.getElementById("signup-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    const f = e.target; const err = document.getElementById("signup-err"); err.textContent = "";
    if (state.config.captcha_sitekey && !captcha.token) { err.textContent = "Please complete the captcha."; return; }
    const btn = f.querySelector("button"); btn.disabled = true;
    try {
      await API.call("/auth/signup", { auth: false, method: "POST", body: {
        email: f.email.value, password: f.password.value,
        display_name: f.display_name.value || null, captcha_token: captcha.token,
      } });
      // Mirrors EMAIL_SPAM_NOTICE in app/auth/router.py - signup returns the user
      // object (no message), so the spam guidance is added here on the portal side.
      toast("Account created - verify your email to finish signing up. Don't see it? "
        + "Check your spam folder and mark it 'Not spam' so future emails reach your inbox.", "ok");
      renderAuth("login");
    } catch (ex) { err.textContent = ex.message; resetCaptcha(); btn.disabled = false; }
  });
}

function renderForgot() {
  document.getElementById("auth-body").innerHTML = `
    <form id="forgot-form">
      <p class="field-help">Enter your email and we'll send a reset link.</p>
      <label>Email</label><input type="email" name="email" required>
      <div id="cap" style="margin-top:16px"></div>
      <div class="err-text" id="forgot-err"></div>
      <button class="btn primary" style="width:100%;margin-top:16px">Send reset link</button>
      <p class="field-help" style="text-align:center;margin-top:14px"><a href="#" id="back-login">Back to log in</a></p>
    </form>`;
  mountCaptcha(document.getElementById("cap"));
  document.getElementById("back-login").addEventListener("click", (e) => { e.preventDefault(); renderLogin(); });
  document.getElementById("forgot-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    const err = document.getElementById("forgot-err"); err.textContent = "";
    if (state.config.captcha_sitekey && !captcha.token) { err.textContent = "Please complete the captcha."; return; }
    try {
      const r = await API.call("/auth/forgot-password", { auth: false, method: "POST",
        body: { email: e.target.email.value, captcha_token: captcha.token } });
      toast(r.message, "ok"); renderLogin();
    } catch (ex) { err.textContent = ex.message; resetCaptcha(); }
  });
}

// --- Dashboard -------------------------------------------------------------

const TABS = ["tokens", "activity", "account", "overview", "pageviews", "events", "users", "siteusers", "config", "leaderboards", "ingest", "giveaways", "discord", "supporters", "claims", "mods", "codexes", "botstats"];
const MASTER_TABS = new Set(["overview", "pageviews", "events", "users", "siteusers", "config", "leaderboards", "ingest", "giveaways", "discord", "supporters", "claims", "mods", "codexes", "botstats"]);

// Inline SVG icons (the portal ships no icon font). 16px, currentColor stroke.
const ICONS = {
  tokens:       '<circle cx="8" cy="15" r="4"/><path d="M11 12.5 20 3.5M17.5 6l2 2M19.5 4l1.5 1.5"/>',
  activity:     '<path d="M3 12h4l3 7 4-15 3 8h4"/>',
  account:      '<circle cx="12" cy="8" r="4"/><path d="M4 20c0-4 4-6 8-6s8 2 8 6"/>',
  overview:     '<rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/><rect x="14" y="14" width="7" height="7" rx="1"/>',
  pageviews:    '<path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12Z"/><circle cx="12" cy="12" r="3"/>',
  events:       '<circle cx="4" cy="6" r="1.3"/><circle cx="4" cy="12" r="1.3"/><circle cx="4" cy="18" r="1.3"/><path d="M9 6h11M9 12h11M9 18h7"/>',
  users:        '<circle cx="8.5" cy="8" r="3.2"/><path d="M3 19c0-3 2.4-4.7 5.5-4.7S14 16 14 19"/><path d="M15 5.2A3.2 3.2 0 0 1 15 12M16 14.6c2.6.3 4 1.9 4 4.4"/>',
  config:       '<path d="M4 7h7M17.5 7H20M4 17h7M17.5 17H20"/><circle cx="14" cy="7" r="2.4"/><circle cx="14" cy="17" r="2.4"/>',
  modules:      '<path d="M12 3 3 7.5 12 12l9-4.5L12 3Z"/><path d="M3 12l9 4.5 9-4.5M3 16.5 12 21l9-4.5"/>',
  leaderboards: '<rect x="3" y="11" width="5" height="9" rx="1"/><rect x="9.5" y="5" width="5" height="15" rx="1"/><rect x="16" y="14" width="5" height="6" rx="1"/>',
  ingest:       '<path d="M12 3v11m0 0 4-4m-4 4-4-4"/><path d="M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2"/>',
  giveaways:    '<rect x="3" y="8" width="18" height="4" rx="1"/><path d="M5 12v9h14v-9M12 8v13"/><path d="M12 8S11 4 8.5 4a2 2 0 1 0 0 4H12zM12 8s1-4 3.5-4a2 2 0 1 1 0 4H12z"/>',
  discord:      '<path d="M4 6h16a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1H9l-4 4v-4H4a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1Z"/><circle cx="9.5" cy="11.5" r="1"/><circle cx="14.5" cy="11.5" r="1"/>',
  supporters:   '<path d="M12 20.3 4.6 12.9a4.4 4.4 0 0 1 6.2-6.2l1.2 1.2 1.2-1.2a4.4 4.4 0 0 1 6.2 6.2L12 20.3Z"/>',
  claims:       '<path d="M12 3 5 6v5c0 4 3 6.5 7 8 4-1.5 7-4 7-8V6l-7-3Z"/><path d="M9.5 12l2 2 3.5-3.5"/>',
  botstats:     '<path d="M4 20V4M4 20h16"/><rect x="7" y="12" width="3" height="5"/><rect x="12" y="8" width="3" height="9"/><rect x="17" y="14" width="3" height="3"/>',
  mods:         '<path d="M12 3 3 7.5 12 12l9-4.5L12 3Z"/><path d="M3 12l9 4.5 9-4.5M3 16.5 12 21l9-4.5"/>',
  codexes:      '<path d="M4 5a2 2 0 0 1 2-2h12v16H6a2 2 0 0 0-2 2V5Z"/><path d="M8 7h7M8 10h7"/>',
  siteusers:    '<circle cx="12" cy="7.5" r="3.3"/><path d="M5.5 20c0-3.4 2.9-5.3 6.5-5.3S18.5 16.6 18.5 20"/><path d="M3 4.5h18"/>',
};
function icon(name) {
  return `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${ICONS[name] || ""}</svg>`;
}

// Per-tab metadata: sidebar label + breadcrumb group. The Admin-panel block
// only renders for masters; "Modules" is a labelled subgroup so new per-feature
// admin pages slot in without touching the shell.
const TAB_META = {
  tokens:       { group: "API management", label: "Tokens" },
  activity:     { group: "API management", label: "Activity" },
  account:      { group: "API management", label: "Account" },
  overview:     { group: "Admin panel", label: "Overview" },
  pageviews:    { group: "Admin panel", label: "Site Analytics" },
  events:       { group: "Admin panel", label: "Events" },
  users:        { group: "Admin panel", label: "Users" },
  siteusers:    { group: "Admin panel", label: "Dashboard users" },
  config:       { group: "Admin panel", label: "Configuration" },
  leaderboards: { group: "Admin panel · Modules", label: "Leaderboards" },
  ingest:       { group: "Admin panel · Modules", label: "Ingest" },
  giveaways:    { group: "Admin panel · Modules", label: "Giveaways" },
  discord:      { group: "Admin panel · Modules", label: "Discord" },
  supporters:   { group: "Admin panel · Modules", label: "Supporters" },
  claims:       { group: "Admin panel · Modules", label: "Trove claims" },
  mods:         { group: "Admin panel · Modules", label: "Mods hub" },
  codexes:      { group: "Admin panel · Modules", label: "Codexes" },
  botstats:     { group: "Admin panel · Modules", label: "Bot stats" },
};

function tabFromHash() {
  let h = location.hash.replace(/^#/, "");
  if (h === "admin") h = "overview";  // legacy alias - old #admin deep-links
  return TABS.includes(h) ? h : null;
}

async function loadDashboard() {
  state.user = await API.call("/auth/me");
  // Honour a deep-linked tab (e.g. dev.aallyn.net/#activity).
  const hashed = tabFromHash();
  if (hashed) state.tab = hashed;
  renderDashboard();
}

function renderDashboard() {
  document.body.classList.add("app-shell");  // full-height app shell (sidebar + scrolling main)
  const u = state.user;
  const verified = u.is_verified
    ? '<span class="badge ok">verified</span>'
    : '<span class="badge warn">unverified</span>';
  // Non-admins landing on a master-only tab fall back to tokens.
  if (MASTER_TABS.has(state.tab) && !u.is_superuser) state.tab = "tokens";
  if (!TABS.includes(state.tab)) state.tab = "tokens";

  const navItem = (tab, sub) =>
    `<button class="nav-item${sub ? " nav-sub" : ""}" data-tab="${tab}" role="tab">${icon(tab)}<span>${TAB_META[tab].label}</span></button>`;
  const adminNav = u.is_superuser ? `
          <p class="nav-group">Admin panel <span class="badge muted">master</span></p>
          ${navItem("overview")}
          ${navItem("pageviews")}
          ${navItem("events")}
          ${navItem("users")}
          ${navItem("siteusers")}
          ${navItem("config")}
          <p class="nav-subgroup">${icon("modules")}<span>Modules</span></p>
          ${navItem("leaderboards", true)}
          ${navItem("ingest", true)}
          ${navItem("giveaways", true)}
          ${navItem("discord", true)}
          ${navItem("supporters", true)}
          ${navItem("claims", true)}
          ${navItem("mods", true)}
          ${navItem("codexes", true)}` : "";

  app.innerHTML = `
    <div class="topbar">
      <div class="brand"><span class="mark">◆</span> ${esc(state.config?.app_name || "Kiwi API")}</div>
      <div class="who">${esc(u.email)} ${verified} ${themeBtn()}
        <a class="portal-support" href="https://trove.aallyn.net/support" target="_blank" rel="noopener" title="Support the project" aria-label="Support">♥</a>
        <button class="btn small" id="logout">Log out</button></div>
    </div>
    <div class="shell">
      <aside class="sidebar">
        <nav role="tablist">
          <p class="nav-group">API management</p>
          ${navItem("tokens")}
          ${navItem("activity")}
          ${navItem("account")}
          ${adminNav}
        </nav>
      </aside>
      <main class="main">
        <div class="crumb" id="crumb"></div>
        <div id="tab-body"></div>
      </main>
    </div>`;
  document.getElementById("logout").addEventListener("click", async () => {
    try { await API.call("/auth/logout", { method: "POST", auth: false, body: { refresh_token: API.refresh } }); } catch (_) {}
    API.clear(); location.hash = ""; renderAuth("login");
  });
  // Nav clicks navigate via the URL hash, so the back button moves between tabs.
  app.querySelectorAll(".nav-item").forEach((b) =>
    b.addEventListener("click", () => {
      if (location.hash.replace(/^#/, "") === b.dataset.tab) selectTab();  // same tab -> just refresh
      else location.hash = b.dataset.tab;
    }));
  syncThemeToggles();
  // Reflect the current tab in the URL without adding a history entry on load.
  if (tabFromHash() !== state.tab) history.replaceState(null, "", "#" + state.tab);
  selectTab();
}

function selectTab() {
  const bodyEl = document.getElementById("tab-body");
  if (!bodyEl) return;  // dashboard not mounted (e.g. logged out)
  app.querySelectorAll(".nav-item").forEach((b) => {
    const on = b.dataset.tab === state.tab;
    b.classList.toggle("active", on);
    b.setAttribute("aria-selected", on ? "true" : "false");
  });
  const meta = TAB_META[state.tab];
  const crumb = document.getElementById("crumb");
  if (crumb && meta) {
    crumb.innerHTML =
      `<span>${esc(meta.group)}</span><span class="crumb-sep">›</span><span class="cur">${esc(meta.label)}</span>`;
  }
  if (state.tab === "tokens") renderTokens();
  else if (state.tab === "activity") renderActivity();
  else if (state.tab === "account") renderAccount();
  else if (state.tab === "overview") renderOverview();
  else if (state.tab === "pageviews") renderPageviews();
  else if (state.tab === "events") renderEvents();
  else if (state.tab === "users") renderUsers();
  else if (state.tab === "siteusers") renderSiteUsers();
  else if (state.tab === "config") renderConfigTab();
  else if (state.tab === "leaderboards") renderLeaderboards();
  else if (state.tab === "ingest") renderIngest();
  else if (state.tab === "giveaways") renderGiveaways();
  else if (state.tab === "discord") renderDiscord();
  else if (state.tab === "supporters") renderSupporters();
  else if (state.tab === "claims") renderClaims();
  else if (state.tab === "mods") renderModsModeration();
  else if (state.tab === "codexes") renderCodexes();
  else if (state.tab === "botstats") renderBotStats();
  else renderTokens();
}

// Browser back/forward (and manual hash edits) switch tabs when logged in.
window.addEventListener("hashchange", () => {
  if (!state.user) return;
  const t = tabFromHash();
  if (t && t !== state.tab) { state.tab = t; selectTab(); }
});

// --- Modal helper ----------------------------------------------------------

let _modalSeq = 0;

function modal(title, innerHtml, onConfirm, confirmLabel = "Confirm") {
  const titleId = `modal-title-${++_modalSeq}`;
  const lastFocused = document.activeElement;
  const overlay = document.createElement("div");
  overlay.className = "modal-overlay";
  overlay.innerHTML = `
    <div class="modal" role="dialog" aria-modal="true" aria-labelledby="${titleId}">
      <h3 id="${titleId}">${esc(title)}</h3>
      <div class="modal-body">${innerHtml}</div>
      <div class="err-text modal-err"></div>
      <div class="modal-actions">
        <button class="btn" data-cancel type="button">Cancel</button>
        <button class="btn primary" data-confirm type="button">${esc(confirmLabel)}</button>
      </div>
    </div>`;
  document.body.appendChild(overlay);

  const focusable = () => Array.from(overlay.querySelectorAll(
    'a[href], button:not([disabled]), input:not([disabled]), select, textarea, [tabindex]:not([tabindex="-1"])'
  )).filter((el) => el.offsetParent !== null);

  const close = () => {
    document.removeEventListener("keydown", onKey, true);
    overlay.remove();
    if (lastFocused && lastFocused.focus) lastFocused.focus();  // restore focus
  };

  function onKey(e) {
    if (e.key === "Escape") { e.preventDefault(); close(); return; }
    if (e.key === "Tab") {  // focus trap
      const items = focusable();
      if (!items.length) return;
      const first = items[0], last = items[items.length - 1];
      if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
      else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
    }
  }
  document.addEventListener("keydown", onKey, true);

  overlay.querySelector("[data-cancel]").addEventListener("click", close);
  overlay.addEventListener("mousedown", (e) => { if (e.target === overlay) close(); });
  overlay.querySelector("[data-confirm]").addEventListener("click", async () => {
    const errEl = overlay.querySelector(".modal-err"); errEl.textContent = "";
    const btn = overlay.querySelector("[data-confirm]"); btn.disabled = true;
    try {
      await onConfirm(overlay);
      close();
    } catch (ex) { errEl.textContent = ex.message || "Something went wrong."; btn.disabled = false; }
  });

  // Focus the first field (or the confirm button) when the dialog opens.
  const first = overlay.querySelector(".modal-body input, .modal-body select, .modal-body textarea")
    || overlay.querySelector("[data-confirm]");
  if (first) first.focus();
  return overlay;
}

function openRevokeToken(token, after) {
  const reasons = state.config.revoke_reasons || [];
  const opts = reasons.map((r) => `<option value="${esc(r)}">${esc(r)}</option>`).join("");
  const overlay = modal("Revoke token", `
    <p class="hint">Revoking <b>${esc(token.name)}</b> (<span class="mono">${esc(token.prefix)}…</span>) is permanent.
      A reason is required.</p>
    <label>Reason</label>
    <select id="revoke-reason">${opts}<option value="__other">Other…</option></select>
    <input id="revoke-custom" placeholder="Custom reason" style="display:none;margin-top:8px" maxlength="200">
  `, async () => {
    const sel = document.getElementById("revoke-reason");
    let reason = sel.value === "__other" ? document.getElementById("revoke-custom").value.trim() : sel.value;
    if (!reason) throw new Error("Please provide a reason.");
    await API.call(`/tokens/${token.id}/revoke`, { method: "POST", body: { reason } });
    toast("Token revoked.", "ok");
    after();
  }, "Revoke");
  const sel = overlay.querySelector("#revoke-reason");
  sel.addEventListener("change", () => {
    overlay.querySelector("#revoke-custom").style.display = sel.value === "__other" ? "block" : "none";
  });
}

function openEditToken(token, after) {
  // Pinned IPs are stored HASHED - we can't show what's currently set, only
  // the count. Submitting REPLACES the whole list; submitting an empty box
  // drops every IP restriction on the token.
  const pinHint = token.allowed_ip_count
    ? `<span class="muted">${token.allowed_ip_count} pinned (hidden - IPs are hashed server-side)</span>`
    : `<span class="muted">none pinned</span>`;
  modal("Edit token", `
    <label>Name</label>
    <input id="edit-name" value="${esc(token.name)}" maxlength="80">
    <label>Allowed IPs <span class="muted">(optional - one per line. Replaces the whole list; leave empty to drop all pinning.)</span></label>
    <p class="field-help">Current: ${pinHint}</p>
    <textarea id="edit-ips" rows="3" placeholder="203.0.113.4"></textarea>
    <p class="field-help">The secret and scopes can't be changed.</p>
  `, async () => {
    const name = document.getElementById("edit-name").value.trim();
    const ips = document.getElementById("edit-ips").value.split(/[\n,]+/).map((s) => s.trim()).filter(Boolean);
    if (!name) throw new Error("Name can't be empty.");
    await API.call(`/tokens/${token.id}`, { method: "PATCH", body: { name, allowed_ips: ips } });
    toast("Token updated.", "ok");
    after();
  }, "Save");
}

function openRotateToken(token, after) {
  modal("Rotate token", `
    <p class="hint">Issue a brand-new secret for <b>${esc(token.name)}</b>
      (<span class="mono">${esc(token.prefix)}…</span>). The name, scopes, allowed IPs and
      expiry stay the same, but the <b>old secret stops working immediately</b>.</p>
    <p class="field-help">Use this if a key may have leaked. Update the secret wherever it's used.</p>
  `, async () => {
    const t = await API.call(`/tokens/${token.id}/rotate`, { method: "POST" });
    // Stash the one-time secret so it survives the list re-render below.
    state.lastRotated = { id: t.id, token: t.token };
    toast("Token rotated - copy the new secret.", "ok");
    after();
  }, "Rotate");
}

// --- Tokens tab ------------------------------------------------------------

function curlExample(token) {
  return `# Use this token once data endpoints are published:\ncurl ${API_BASE}/v1/<endpoint> \\\n  -H "Authorization: Bearer ${token}"`;
}

const USAGE_SNIPPET = `export KIWI_TOKEN="kiwi_…"\n\n# Authenticate any future data endpoint with the token:\ncurl ${API_BASE}/v1/<endpoint> \\\n  -H "Authorization: Bearer $KIWI_TOKEN"`;

// Render a one-time secret with a copy button and a ready-to-run curl example.
function revealSecret(slot, token, label) {
  slot.innerHTML = `
    <div class="secret">
      <strong>${esc(label)}</strong>
      <span class="mono" id="secret-val">${esc(token)}</span>
      <button class="btn small" id="copy-secret">Copy token</button>
      <details style="margin-top:12px">
        <summary class="curl-summary">Show curl example</summary>
        <pre class="curl-block"><code>${esc(curlExample(token))}</code></pre>
        <button class="btn small" id="copy-curl">Copy curl</button>
      </details>
    </div>`;
  slot.querySelector("#copy-secret").addEventListener("click", () =>
    navigator.clipboard.writeText(token).then(() => toast("Token copied.", "ok")));
  slot.querySelector("#copy-curl").addEventListener("click", () =>
    navigator.clipboard.writeText(curlExample(token)).then(() => toast("curl copied.", "ok")));
  slot.scrollIntoView({ behavior: "smooth", block: "center" });
}

async function renderTokens() {
  const body = document.getElementById("tab-body");
  body.innerHTML = `<div class="loading">Loading tokens…</div>`;
  let tokens = [];
  try { tokens = await API.call("/tokens"); } catch (ex) { body.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`; return; }

  const unverified = !state.user.is_verified && state.config.require_verified_for_tokens;
  const banner = unverified ? `
    <div class="banner">
      <span>Verify your email to create API tokens.</span>
      <button class="btn small" id="resend">Resend email</button>
    </div>` : "";

  const scopeBadge = (t) => t.scopes === 0
    ? '<span class="badge ok">all</span>'
    : (t.scope_names.length ? t.scope_names.map((s) => `<code>${esc(s)}</code>`).join(" ") : `<code>${t.scopes}</code>`);

  const active = tokens.filter((t) => !t.revoked);
  const revoked = tokens.filter((t) => t.revoked);

  const activeRows = active.length ? active.map((t) => {
    const expired = t.expires_at && new Date(t.expires_at) < new Date();
    return `
    <tr>
      <td>${esc(t.name)}</td>
      <td class="mono">${esc(t.prefix)}…</td>
      <td>${scopeBadge(t)}</td>
      <td>${t.request_count}</td>
      <td class="muted">${fmt(t.last_used_at)}</td>
      <td class="muted">${t.expires_at ? fmtDay(t.expires_at) : "never"}</td>
      <td>${expired ? '<span class="badge warn">expired</span>' : '<span class="badge ok">active</span>'}</td>
      <td style="white-space:nowrap">
        <button class="btn small" data-edit="${t.id}">Edit</button>
        <button class="btn small" data-rotate="${t.id}">Rotate</button>
        <button class="btn small danger" data-revoke="${t.id}">Revoke</button>
      </td>
    </tr>`; }).join("") : `<tr><td colspan="8" class="muted">No active tokens.</td></tr>`;

  const revokedRows = revoked.map((t) => `
    <tr>
      <td>${esc(t.name)}</td>
      <td class="mono">${esc(t.prefix)}…</td>
      <td class="muted">${esc(t.revoke_reason || "-")}</td>
      <td class="muted">${fmt(t.revoked_at)}</td>
    </tr>`).join("");

  const scopeChecks = (state.config.scopes || []).map((s) =>
    `<label class="chk"><input type="checkbox" name="scope" value="${s.bit}"> <b>${esc(s.key)}</b> <span class="muted">${esc(s.description)} · bit <code>${s.bit}</code></span></label>`
  ).join("") || '<span class="muted">No scopes defined yet.</span>';

  body.innerHTML = `
    ${banner}
    <div class="card">
      <h2>Create a token</h2>
      <p class="hint">The secret is shown once. Store it somewhere safe. You can create up to ${state.config.token_creation_daily_limit} tokens per day.</p>
      <form id="create-form">
        <label>Name</label>
        <input name="name" required placeholder="my-laptop" ${unverified ? "disabled" : ""}>

        <label>Scopes <span class="muted">(at least one, or All scopes)</span></label>
        <div class="checks">${scopeChecks}</div>
        <label class="chk"><input type="checkbox" id="all-scopes"> <b>All scopes</b>
          <span class="muted">grant every scope, including ones added later (mask 0)</span></label>
        <p class="field-help" id="mask-preview"></p>

        <label>Allowed IPs <span class="muted">(optional - one exact IP per line. Stored hashed; you won't be able to see them again.)</span></label>
        <textarea name="ips" rows="2" placeholder="203.0.113.4" ${unverified ? "disabled" : ""}></textarea>

        <label>Expires</label>
        <select name="expiry" ${unverified ? "disabled" : ""}>
          <option value="30" selected>30 days</option>
          <option value="60">60 days</option>
          <option value="90">90 days</option>
          <option value="0">Unlimited</option>
        </select>

        <div class="err-text" id="create-err"></div>
        <button class="btn primary" style="margin-top:14px" ${unverified ? "disabled" : ""}>Create token</button>
      </form>
      <div id="new-token"></div>
    </div>
    <div class="card">
      <h2>Your tokens</h2>
      <table>
        <thead><tr><th>Name</th><th>Prefix</th><th>Scopes</th><th>Requests</th><th>Last used</th><th>Expires</th><th>Status</th><th></th></tr></thead>
        <tbody>${activeRows}</tbody>
      </table>
    </div>
    <div class="card">
      <h2>Using your tokens</h2>
      <p class="hint">Send the token as a Bearer credential - there's no login call, the token <em>is</em> the credential.</p>
      <pre class="curl-block"><code>${esc(USAGE_SNIPPET)}</code></pre>
      <button class="btn small" id="copy-usage">Copy</button>
      <p class="field-help">Full HTTP reference at <a href="https://docs.aallyn.net" target="_blank" rel="noopener">docs.aallyn.net</a>.</p>
    </div>
    ${revoked.length ? `
    <details class="card revoked-section">
      <summary>Revoked tokens (${revoked.length})</summary>
      <table style="margin-top:12px">
        <thead><tr><th>Name</th><th>Prefix</th><th>Reason</th><th>Revoked</th></tr></thead>
        <tbody>${revokedRows}</tbody>
      </table>
    </details>` : ""}`;

  if (unverified) {
    document.getElementById("resend").addEventListener("click", async (e) => {
      e.target.disabled = true;
      try { const r = await API.call("/auth/resend-verification", { method: "POST" }); toast(r.message, "ok"); }
      catch (ex) { toast(ex.message, "err"); e.target.disabled = false; }
    });
  }

  // A freshly rotated secret is shown once, the same way a newly created one is.
  if (state.lastRotated) {
    const { token } = state.lastRotated;
    state.lastRotated = null;
    const slot = document.getElementById("new-token");
    if (slot) revealSecret(slot, token, "Rotated secret - copy it now, it won't be shown again:");
  }

  document.getElementById("copy-usage").addEventListener("click", () =>
    navigator.clipboard.writeText(USAGE_SNIPPET).then(() => toast("Copied.", "ok")));

  const byId = Object.fromEntries(tokens.map((t) => [t.id, t]));
  body.querySelectorAll("[data-edit]").forEach((b) =>
    b.addEventListener("click", () => openEditToken(byId[b.dataset.edit], renderTokens)));
  body.querySelectorAll("[data-rotate]").forEach((b) =>
    b.addEventListener("click", () => openRotateToken(byId[b.dataset.rotate], renderTokens)));
  body.querySelectorAll("[data-revoke]").forEach((b) =>
    b.addEventListener("click", () => openRevokeToken(byId[b.dataset.revoke], renderTokens)));

  const form = document.getElementById("create-form");
  if (form && !unverified) {
    const allBox = document.getElementById("all-scopes");
    const preview = document.getElementById("mask-preview");
    const scopeBoxes = () => form.querySelectorAll('input[name="scope"]');
    const computeMask = () => {
      if (allBox.checked) return 0;
      let m = 0;
      form.querySelectorAll('input[name="scope"]:checked').forEach((c) => { m |= Number(c.value); });
      return m;
    };
    const updatePreview = () => {
      if (allBox.checked) { preview.textContent = "Mask: 0 (all scopes)"; return; }
      const m = computeMask();
      preview.textContent = m === 0 ? "Mask: 0 - pick a scope, or enable All scopes" : `Mask: ${m}`;
    };
    // "All scopes" supersedes the individual picks.
    allBox.addEventListener("change", () => {
      scopeBoxes().forEach((c) => { c.disabled = allBox.checked; });
      updatePreview();
    });
    scopeBoxes().forEach((c) => c.addEventListener("change", updatePreview));
    updatePreview();

    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      const f = e.target; const err = document.getElementById("create-err"); err.textContent = "";
      const all_scopes = allBox.checked;
      const mask = computeMask();
      const allowed_ips = f.ips.value.split(/[\n,]+/).map((s) => s.trim()).filter(Boolean);
      if (!all_scopes && mask === 0) { err.textContent = "Pick at least one scope, or enable All scopes."; return; }
      const expiry = f.expiry.value;
      const body = {
        name: f.name.value, scopes: mask, allowed_ips,
        expires_in_days: expiry === "0" ? null : Number(expiry),
      };
      try {
        const t = await API.call("/tokens", { method: "POST", body });
        revealSecret(document.getElementById("new-token"), t.token,
          "New token - copy it now, it won't be shown again:");
        f.reset();
        scopeBoxes().forEach((c) => { c.disabled = false; });  // re-enable after reset
        updatePreview();
        // The revealed secret stays put; the token list refreshes next tab visit.
      } catch (ex) { err.textContent = ex.message; }
    });
  }
}

// --- Activity tab ----------------------------------------------------------

async function renderActivity(days = 7) {
  const body = document.getElementById("tab-body");
  body.innerHTML = `<div class="loading">Loading activity…</div>`;
  let a;
  try { a = await API.call(`/tokens/activity?days=${days}`); }
  catch (ex) { body.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`; return; }

  const maxDay = Math.max(1, ...a.by_day.map((d) => d.count));
  const bars = a.by_day.length ? a.by_day.map((d) =>
    `<div class="bar" title="${esc(d.date)}: ${d.count} req" style="height:${(d.count / maxDay) * 100}%"></div>`).join("") : "";
  const labels = a.by_day.map((d) => `<span>${esc(d.date.slice(5))}</span>`).join("");

  const eps = a.by_endpoint.length ? a.by_endpoint.map((e) => `
    <tr><td class="mono">${esc(e.method)} ${esc(e.route)}</td><td>${e.count}</td>
    <td>${e.error_count}</td><td>${e.avg_duration_ms} ms</td></tr>`).join("")
    : `<tr><td colspan="4" class="muted">No requests in this window.</td></tr>`;

  body.innerHTML = `
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px">
        <h2 style="flex:1;margin:0">Usage - last ${days} days</h2>
        <button class="btn small" id="csv-btn" style="flex:0 0 auto">Download CSV</button>
        <select id="days" style="max-width:140px;flex:0 0 auto">
          <option value="1">24 hours</option><option value="7">7 days</option>
          <option value="30">30 days</option><option value="90">90 days</option>
        </select>
      </div>
      <div class="stat-grid">
        <div class="stat"><div class="n">${a.total_requests}</div><div class="l">Requests</div></div>
        <div class="stat"><div class="n">${a.error_count}</div><div class="l">Errors</div></div>
        <div class="stat"><div class="n">${a.avg_duration_ms} ms</div><div class="l">Avg latency</div></div>
      </div>
      <div class="bars">${bars}</div>
      <div class="bar-labels">${labels}</div>
    </div>
    <div class="card">
      <h2>By endpoint</h2>
      <table>
        <thead><tr><th>Endpoint</th><th>Requests</th><th>Errors</th><th>Avg latency</th></tr></thead>
        <tbody>${eps}</tbody>
      </table>
    </div>`;
  const sel = document.getElementById("days"); sel.value = String(days);
  sel.addEventListener("change", () => renderActivity(Number(sel.value)));

  document.getElementById("csv-btn").addEventListener("click", () => {
    downloadCSV(
      `kiwi-activity-${days}d.csv`,
      ["date", "requests", "errors", "avg_ms"],
      a.by_day.map((d) => [d.date, d.count, d.error_count, d.avg_duration_ms]),
    );
    toast("Activity CSV downloaded.", "ok");
  });
}

// --- Account tab -----------------------------------------------------------

async function loadSessions() {
  const el = document.getElementById("sessions-list");
  if (!el) return;
  let sessions;
  try { sessions = await API.call("/auth/sessions"); }
  catch (ex) { el.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`; return; }
  if (!sessions.length) { el.innerHTML = '<p class="muted">No active sessions.</p>'; return; }
  el.innerHTML = `<table>
    <thead><tr><th>Device</th><th>IP</th><th>Last used</th><th></th></tr></thead>
    <tbody>${sessions.map((s) => `
      <tr>
        <td>${esc((s.user_agent || "Unknown").slice(0, 48))} ${s.current ? '<span class="badge ok">this device</span>' : ""}</td>
        <td class="mono">${esc(s.ip || "-")}</td>
        <td class="muted">${fmt(s.last_used_at)}</td>
        <td>${s.current ? "" : `<button class="btn small danger" data-session="${s.id}">Revoke</button>`}</td>
      </tr>`).join("")}</tbody>
  </table>`;
  el.querySelectorAll("[data-session]").forEach((b) =>
    b.addEventListener("click", async () => {
      try {
        await API.call(`/auth/sessions/${b.dataset.session}`, { method: "DELETE" });
        toast("Session revoked.", "ok"); loadSessions();
      } catch (ex) { toast(ex.message, "err"); }
    }));
}

function renderAccount() {
  const u = state.user;
  const body = document.getElementById("tab-body");
  body.innerHTML = `
    <div class="card">
      <h2>Account</h2>
      <table>
        <tr><td class="muted">Email</td><td>${esc(u.email)} ${u.is_verified
          ? '<span class="badge ok">verified</span>' : '<span class="badge warn">unverified</span>'}</td></tr>
        <tr><td class="muted">Display name</td><td>${esc(u.display_name || "-")}
          <button class="btn small" id="edit-profile" style="margin-left:8px">Edit</button></td></tr>
        <tr><td class="muted">Member since</td><td>${fmtDay(u.created_at)}</td></tr>
        <tr><td class="muted">Last login</td><td>${fmt(u.last_login_at)}</td></tr>
      </table>
      ${u.is_verified ? "" : `<button class="btn small" id="resend2" style="margin-top:14px">Resend verification email</button>`}
    </div>

    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px">
        <h2 style="flex:1;margin:0">Active sessions</h2>
        <button class="btn small danger" id="logout-all" style="flex:0 0 auto">Log out everywhere</button>
      </div>
      <div id="sessions-list"><div class="muted">Loading…</div></div>
    </div>

    <div class="card">
      <h2>Change password</h2>
      <form id="pw-form">
        <label>Current password</label><input type="password" name="current" required autocomplete="current-password">
        <label>New password</label><input type="password" name="next" required minlength="8" autocomplete="new-password">
        <div class="err-text" id="pw-err"></div>
        <button class="btn primary" style="margin-top:14px">Update password</button>
      </form>
    </div>

    <div class="card">
      <h2>Change email</h2>
      <p class="hint">We'll send a confirmation link to the new address; the change applies once you click it.</p>
      <form id="email-form">
        <label>New email</label><input type="email" name="email" required>
        <label>Current password</label><input type="password" name="password" required autocomplete="current-password">
        <div class="err-text" id="email-err"></div>
        <button class="btn primary" style="margin-top:14px">Send confirmation</button>
      </form>
    </div>

    <div class="card">
      <h2>Your data</h2>
      <p class="hint">Download everything we hold about your account, or delete it permanently.</p>
      <div class="row">
        <button class="btn" id="export-btn" style="flex:0 0 auto">Download my data</button>
        <button class="btn danger" id="delete-btn" style="flex:0 0 auto">Delete account…</button>
      </div>
    </div>`;

  attachStrengthMeter(document.querySelector('#pw-form input[name="next"]'));

  document.getElementById("edit-profile").addEventListener("click", () => {
    modal("Edit profile", `
      <label>Display name</label>
      <input id="pf-name" maxlength="80" value="${esc(state.user.display_name || "")}" placeholder="Your name">
      <p class="field-help">Shown in your account. Leave blank to remove it.</p>
    `, async () => {
      const name = document.getElementById("pf-name").value.trim();
      state.user = await API.call("/auth/me", { method: "PATCH", body: { display_name: name || null } });
      toast("Profile updated.", "ok");
      selectTab();  // re-render the account view with the new value
    }, "Save");
  });

  const resend2 = document.getElementById("resend2");
  if (resend2) resend2.addEventListener("click", async () => {
    resend2.disabled = true;
    try { const r = await API.call("/auth/resend-verification", { auth: false, method: "POST", body: { email: state.user.email } }); toast(r.message, "ok"); }
    catch (ex) { toast(ex.message, "err"); resend2.disabled = false; }
  });

  document.getElementById("pw-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    const f = e.target; const err = document.getElementById("pw-err"); err.textContent = "";
    try {
      // Returns fresh tokens - every other session was just logged out.
      const r = await API.call("/auth/change-password", { method: "POST",
        body: { current_password: f.current.value, new_password: f.next.value } });
      API.setTokens(r.access_token, r.refresh_token);
      toast("Password updated. Other sessions were logged out.", "ok");
      f.reset();
      loadSessions();
    } catch (ex) { err.textContent = ex.message; }
  });

  document.getElementById("logout-all").addEventListener("click", async () => {
    if (!confirm("Log out of all sessions on every device?")) return;
    try { await API.call("/auth/logout-all", { method: "POST" }); } catch (_) {}
    API.clear(); renderAuth("login");
  });

  document.getElementById("export-btn").addEventListener("click", async () => {
    try { downloadJSON(await API.call("/auth/me/export"), "kiwi-export.json"); toast("Export downloaded.", "ok"); }
    catch (ex) { toast(ex.message, "err"); }
  });

  document.getElementById("delete-btn").addEventListener("click", () => {
    modal("Delete account", `
      <p class="hint">This permanently deletes your account, tokens, sessions, and usage data.
        This cannot be undone.</p>
      <label>Type your email to confirm</label>
      <input id="del-email" placeholder="${esc(state.user.email)}">
    `, async () => {
      const email = document.getElementById("del-email").value.trim();
      await API.call("/auth/delete-account", { method: "POST", body: { confirm_email: email } });
      API.clear();
      toast("Your account has been deleted.", "ok");
      renderAuth("login");
    }, "Delete forever");
  });

  loadSessions();

  document.getElementById("email-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    const f = e.target; const err = document.getElementById("email-err"); err.textContent = "";
    try {
      const r = await API.call("/auth/change-email", { method: "POST",
        body: { new_email: f.email.value, password: f.password.value } });
      toast(r.message, "ok"); f.reset();
    } catch (ex) { err.textContent = ex.message; }
  });
}

// --- Admin tab (superusers only) -------------------------------------------

const adminScope = (t) => t.scopes === 0
  ? '<span class="badge ok">all</span>'
  : (t.scope_names.length ? t.scope_names.map((s) => `<code>${esc(s)}</code>`).join(" ") : `<code>${t.scopes}</code>`);

// Admin · Overview - at-a-glance KPIs for the selected window.
async function renderOverview(days = 30) {
  const body = document.getElementById("tab-body");
  body.innerHTML = `<div class="loading">Loading overview…</div>`;
  let overview;
  try { overview = await API.call(`/admin/activity?days=${days}`); }
  catch (ex) { body.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`; return; }

  body.innerHTML = `
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px">
        <h2 style="flex:1;margin:0">Overview - last ${days} days</h2>
        <select id="ov-days" style="max-width:140px;flex:0 0 auto">
          <option value="1">24 hours</option><option value="7">7 days</option>
          <option value="30">30 days</option><option value="90">90 days</option>
        </select>
      </div>
      <div class="stat-grid">
        <div class="stat"><div class="n">${overview.total_requests}</div><div class="l">Requests</div></div>
        <div class="stat"><div class="n">${overview.error_count}</div><div class="l">Errors</div></div>
        <div class="stat"><div class="n">${overview.rate_limited}</div><div class="l">Rate-limit hits</div></div>
      </div>
    </div>`;
  const sel = document.getElementById("ov-days"); sel.value = String(days);
  sel.addEventListener("change", () => renderOverview(Number(sel.value)));
}

// Admin · Site Analytics - cookieless page views + unique visitors for the public
// showcase site. Unique visitors are counted once per UTC day (IP+UA salted hash);
// dynamic pages are grouped by route template (e.g. /player/{name}).
async function renderPageviews(days = 30) {
  const body = document.getElementById("tab-body");
  body.innerHTML = `<div class="loading">Loading site analytics…</div>`;
  let stats;
  try { stats = await API.call(`/admin/pageviews?days=${days}`); }
  catch (ex) { body.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`; return; }

  const num = (n) => Number(n || 0).toLocaleString();
  const pRow = (p) => `
    <tr>
      <td class="mono">${esc(p.path)}</td>
      <td>${num(p.views)}</td>
      <td>${num(p.unique_visitors)}</td>
    </tr>`;

  body.innerHTML = `
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px">
        <h2 style="flex:1;margin:0">Site analytics - last ${days} days</h2>
        <select id="pv-days" style="max-width:140px;flex:0 0 auto">
          <option value="1">24 hours</option><option value="7">7 days</option>
          <option value="30">30 days</option><option value="90">90 days</option>
        </select>
      </div>
      <div class="stat-grid">
        <div class="stat"><div class="n">${num(stats.total_views)}</div><div class="l">Page views</div></div>
        <div class="stat"><div class="n">${num(stats.unique_visitors)}</div><div class="l">Unique visitors</div></div>
        <div class="stat"><div class="n">${num(stats.views_today)}</div><div class="l">Views today</div></div>
      </div>
      <p class="hint">Public showcase-site page loads, one row per real page URL (each mod and player page individually), top ${stats.pages.length} by views. Unique visitors are counted once per day (cookieless IP+UA hash, no cookie stored); static assets and JSON proxies aren't counted.</p>
      <table>
        <thead><tr><th>Page</th><th>Views</th><th>Unique visitors</th></tr></thead>
        <tbody id="pv-rows"></tbody>
      </table>
    </div>`;

  const rowsEl = document.getElementById("pv-rows");
  rowsEl.innerHTML = stats.pages.length
    ? stats.pages.map(pRow).join("")
    : `<tr><td colspan="3" class="muted">No page views in this window yet.</td></tr>`;

  const sel = document.getElementById("pv-days"); sel.value = String(days);
  sel.addEventListener("change", () => renderPageviews(Number(sel.value)));
}

// Admin · Users - searchable roster; click a row to drill into one account.
async function renderUsers(days = 30) {
  const body = document.getElementById("tab-body");
  body.innerHTML = `<div class="loading">Loading users…</div>`;
  let users;
  try { users = await API.call(`/admin/users?days=${days}`); }
  catch (ex) { body.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`; return; }

  const uRow = (u) => `
    <tr class="clickable" data-user="${u.id}">
      <td>${esc(u.email)}
        ${u.is_superuser ? '<span class="badge ok">admin</span>' : ""}
        ${!u.is_active ? '<span class="badge off">inactive</span>' : (u.is_verified ? "" : '<span class="badge warn">unverified</span>')}</td>
      <td>${u.token_count}</td>
      <td>${u.total_requests}</td>
      <td>${u.rate_limited}</td>
      <td class="muted">${fmt(u.last_used_at)}</td>
    </tr>`;

  body.innerHTML = `
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px">
        <h2 style="flex:1;margin:0">Users (${users.length})</h2>
        <input id="user-search" placeholder="Search email…" style="max-width:220px;flex:0 0 auto">
        <select id="users-days" style="max-width:130px;flex:0 0 auto">
          <option value="1">24 hours</option><option value="7">7 days</option>
          <option value="30">30 days</option><option value="90">90 days</option>
        </select>
      </div>
      <p class="hint">Click a user to see their tokens and activity. Request / rate-limit counts are over the selected window.</p>
      <table>
        <thead><tr><th>User</th><th>Tokens</th><th>Requests</th><th>RL hits</th><th>Last used</th></tr></thead>
        <tbody id="user-rows"></tbody>
      </table>
    </div>`;

  const daysSel = document.getElementById("users-days"); daysSel.value = String(days);
  daysSel.addEventListener("change", () => renderUsers(Number(daysSel.value)));

  const userRowsEl = document.getElementById("user-rows");
  const paintUsers = (q) => {
    const list = q ? users.filter((u) => u.email.toLowerCase().includes(q)) : users;
    userRowsEl.innerHTML = list.length
      ? list.map(uRow).join("")
      : `<tr><td colspan="5" class="muted">No matching users.</td></tr>`;
    userRowsEl.querySelectorAll("[data-user]").forEach((r) =>
      r.addEventListener("click", () => renderAdminUser(r.dataset.user)));
  };
  paintUsers("");
  document.getElementById("user-search").addEventListener("input", (e) =>
    paintUsers(e.target.value.trim().toLowerCase()));
}

// --- Dashboard (site) users ------------------------------------------------
// Discord-signup accounts that own the public site (mods/modpacks/profiles),
// distinct from the dev-portal API users above. Search is server-side; the
// shell is painted once and only the rows repaint so the search box keeps focus.

async function renderSiteUsers() {
  const body = document.getElementById("tab-body");
  body.innerHTML = `
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px">
        <h2 style="flex:1;margin:0" id="su-title">Dashboard users</h2>
        <input id="su-search" placeholder="Search username / handle / email…" style="max-width:300px;flex:0 0 auto" autocomplete="off">
      </div>
      <p class="hint">Discord-signup accounts that own the public site — mods, modpacks and profiles. Separate from the API users above. Click a user to manage. Counts are mods / modpacks owned.</p>
      <table>
        <thead><tr><th>Trove username</th><th>Discord handle</th><th>Email</th><th>Mods / Packs</th><th>Joined</th></tr></thead>
        <tbody id="su-rows"><tr><td colspan="5" class="loading">Loading…</td></tr></tbody>
      </table>
      <p class="field-help" id="su-more"></p>
    </div>`;

  const rowsEl = document.getElementById("su-rows");
  const titleEl = document.getElementById("su-title");
  const moreEl = document.getElementById("su-more");
  const searchEl = document.getElementById("su-search");

  const row = (u) => `
    <tr class="clickable" data-user="${u.id}">
      <td><b>${esc(u.username)}</b>
        ${!u.is_active ? '<span class="badge off">deactivated</span>' : ""}
        ${u.claim_verified ? '<span class="badge ok">claim</span>' : ""}</td>
      <td class="muted">${u.discord_handle ? "@" + esc(u.discord_handle) : "—"}</td>
      <td class="muted">${esc(u.email || "—")}</td>
      <td>${u.mod_count} / ${u.modpack_count}</td>
      <td class="muted">${fmt(u.created_at)}</td>
    </tr>`;

  let reqSeq = 0;
  const load = async (q) => {
    const mine = ++reqSeq;
    let data;
    try { data = await API.call(`/admin/site-users?limit=100${q ? "&q=" + encodeURIComponent(q) : ""}`); }
    catch (ex) { if (mine === reqSeq) rowsEl.innerHTML = `<tr><td colspan="5" class="err-text">${esc(ex.message)}</td></tr>`; return; }
    if (mine !== reqSeq) return;  // a newer search superseded this one
    const users = data.items || [];
    state._siteUsers = {};
    users.forEach((u) => { state._siteUsers[u.id] = u; });
    titleEl.textContent = `Dashboard users${data.total != null ? ` (${data.total})` : ""}`;
    rowsEl.innerHTML = users.length
      ? users.map(row).join("")
      : `<tr><td colspan="5" class="muted">No matching users.</td></tr>`;
    rowsEl.querySelectorAll("[data-user]").forEach((r) =>
      r.addEventListener("click", () => openSiteUser(state._siteUsers[r.dataset.user], () => load(searchEl.value.trim()))));
    moreEl.textContent = (data.total > users.length)
      ? `Showing ${users.length} of ${data.total}. Refine your search to narrow the list.` : "";
  };

  let t;
  searchEl.addEventListener("input", () => { clearTimeout(t); t = setTimeout(() => load(searchEl.value.trim()), 300); });
  load("");
}

function openSiteUser(u, after) {
  if (!u) return;
  const overlay = document.createElement("div");
  overlay.className = "modal-overlay";
  const badges = `${u.is_active ? '<span class="badge ok">active</span>' : '<span class="badge off">deactivated</span>'} ${u.is_verified ? '<span class="badge ok">verified</span>' : '<span class="badge warn">unverified</span>'}`;
  const claim = u.claimed_trove_name
    ? `${esc(u.claimed_trove_name)}${u.claim_verified ? ' <span class="badge ok">verified</span>' : ' <span class="badge warn">unverified</span>'}` : "—";
  overlay.innerHTML = `
    <div class="modal" role="dialog" aria-modal="true">
      <h3>@${esc(u.username)} ${badges}</h3>
      <div class="modal-body">
        <table style="width:100%"><tbody>
          <tr><td class="muted">Discord handle</td><td>${u.discord_handle ? "@" + esc(u.discord_handle) : "—"}</td></tr>
          <tr><td class="muted">Email</td><td>${esc(u.email || "—")}</td></tr>
          <tr><td class="muted">Display name</td><td>${esc(u.display_name || "—")}</td></tr>
          <tr><td class="muted">Trove claim</td><td>${claim}</td></tr>
          <tr><td class="muted">Owns</td><td>${u.mod_count} mods · ${u.modpack_count} modpacks</td></tr>
          <tr><td class="muted">Joined</td><td>${fmt(u.created_at)}</td></tr>
          <tr><td class="muted">Last login</td><td>${fmt(u.last_login_at)}</td></tr>
          <tr><td class="muted">User ID</td><td class="mono">${esc(u.id)}</td></tr>
        </tbody></table>
        <div class="err-text su-err" style="margin-top:8px"></div>
        <div class="row" style="flex-wrap:wrap;gap:8px;margin-top:10px">
          <button class="btn small" data-act="username">Change username…</button>
          <button class="btn small" data-act="claimed">Set Trove name…</button>
          <button class="btn small" data-act="refresh-discord"${u.discord_id ? "" : " disabled title='No linked Discord id'"}>Refresh Discord</button>
          <button class="btn small" data-act="logout">Force log out</button>
          <button class="btn small ${u.is_active ? "danger" : "primary"}" data-act="toggle">${u.is_active ? "Deactivate" : "Activate"}</button>
        </div>
        <p class="field-help">Deactivating blocks sign-in and ends every active session immediately. Force log-out ends sessions without disabling the account.</p>
      </div>
      <div class="modal-actions"><button class="btn" data-cancel type="button">Close</button></div>
    </div>`;
  document.body.appendChild(overlay);
  const close = () => overlay.remove();
  const errEl = overlay.querySelector(".su-err");
  overlay.querySelector("[data-cancel]").addEventListener("click", close);
  overlay.addEventListener("mousedown", (e) => { if (e.target === overlay) close(); });

  const run = async (btn, fn) => {
    errEl.textContent = ""; btn.disabled = true;
    try { await fn(); close(); if (after) after(); }
    catch (ex) { errEl.textContent = ex.message || "Something went wrong."; btn.disabled = false; }
  };

  overlay.querySelector('[data-act="logout"]').addEventListener("click", (e) => run(e.target, async () => {
    await API.call(`/admin/site-users/${u.id}/logout`, { method: "POST" });
    toast("All sessions ended.", "ok");
  }));
  overlay.querySelector('[data-act="toggle"]').addEventListener("click", (e) => run(e.target, async () => {
    const path = u.is_active ? "deactivate" : "activate";
    await API.call(`/admin/site-users/${u.id}/${path}`, { method: "POST" });
    toast(u.is_active ? "Account deactivated." : "Account activated.", "ok");
  }));
  overlay.querySelector('[data-act="refresh-discord"]').addEventListener("click", (e) => {
    if (e.target.disabled) return;
    run(e.target, async () => {
      const res = await API.call(`/admin/site-users/${u.id}/refresh-discord`, { method: "POST" });
      toast(`Discord handle: @${res.discord_handle || "—"}`, "ok");
    });
  });
  overlay.querySelector('[data-act="username"]').addEventListener("click", () => {
    close();
    openSetSiteUsername(u, after);
  });
  overlay.querySelector('[data-act="claimed"]').addEventListener("click", () => {
    close();
    openSetClaimedName(u, after);
  });
}

function openSetClaimedName(u, after) {
  const current = u.claimed_trove_name || "";
  modal("Set Trove name", `
    <p class="hint">Set <b>@${esc(u.username)}</b>'s claimed Trove (leaderboard) name. Setting it marks the claim
      <b>admin-verified</b>; leave it empty to clear the claim.</p>
    <label>Trove name</label>
    <input id="su-claimed" value="${esc(current)}" maxlength="64" autocomplete="off" placeholder="In-game name">
    <p class="field-help">Matched case-insensitively against captured leaderboard rows.</p>
  `, async () => {
    const name = document.getElementById("su-claimed").value.trim();
    const res = await API.call(`/admin/site-users/${u.id}/claimed-name`, { method: "POST", body: { name } });
    toast(res.claimed_trove_name ? `Trove name set to ${res.claimed_trove_name}.` : "Trove claim cleared.", "ok");
    if (after) after();
  }, "Save");
}

function openSetSiteUsername(u, after) {
  modal("Change Trove username", `
    <p class="hint">Override <b>@${esc(u.username)}</b>'s frozen Trove username. This renames the handle on their mods and modpacks (their URLs change to match) and resolves any pending request they have.</p>
    <label>New username</label>
    <input id="su-new-username" value="${esc(u.username)}" maxlength="24" autocomplete="off">
    <p class="field-help">3–24 characters: lowercase letters, numbers, underscores or periods (a period can't start it or repeat).</p>
  `, async () => {
    const name = document.getElementById("su-new-username").value.trim().toLowerCase();
    if (!name) throw new Error("Enter a username.");
    const res = await API.call(`/admin/site-users/${u.id}/username`, { method: "POST", body: { username: name } });
    toast(res.changed === false ? "No change — same username." : `Username set to @${res.username}.`, "ok");
    if (after) after();
  }, "Save");
}

// Admin · Events - cursor-paginated audit log across all users.
async function renderEvents() {
  const body = document.getElementById("tab-body");
  body.innerHTML = `<div class="loading">Loading events…</div>`;
  let users;
  // Fetch the roster once for the user_id -> email lookup the rows display.
  try { users = await API.call(`/admin/users?days=90`); }
  catch (ex) { body.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`; return; }
  const emailById = Object.fromEntries(users.map((u) => [u.id, u.email]));

  body.innerHTML = `
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px">
        <h2 style="flex:1;margin:0">Recent events</h2>
        <select id="ev-status" style="max-width:170px;flex:0 0 auto">
          <option value="">All statuses</option>
          <option value="429">429 - rate-limited</option>
          <option value="401">401</option><option value="403">403</option><option value="500">500</option>
        </select>
      </div>
      <p class="hint">Audit log of recent API requests across all users. Filter by status; page with "Load more".</p>
      <table>
        <thead><tr><th>When</th><th>User</th><th>Method</th><th>Route</th><th>Status</th></tr></thead>
        <tbody id="ev-rows"></tbody>
      </table>
      <button class="btn small" id="ev-more" style="margin-top:12px;display:none">Load more</button>
    </div>`;

  const evRowsEl = document.getElementById("ev-rows");
  const evMore = document.getElementById("ev-more");
  const evStatus = document.getElementById("ev-status");
  let evCursor = null;
  const evRow = (e) => {
    const cls = e.status_code >= 500 ? "off" : e.status_code >= 400 ? "warn" : "ok";
    return `<tr>
      <td class="muted">${fmt(e.created_at)}</td>
      <td>${esc(emailById[e.user_id] || e.user_id)}</td>
      <td class="mono">${esc(e.method)}</td>
      <td class="mono">${esc(e.route)}</td>
      <td><span class="badge ${cls}">${e.status_code}</span></td>
    </tr>`;
  };
  const loadEvents = async (reset) => {
    if (reset) { evCursor = null; evRowsEl.innerHTML = `<tr><td colspan="5" class="muted">Loading…</td></tr>`; }
    const qs = new URLSearchParams({ limit: "20" });
    if (evStatus.value) qs.set("status_code", evStatus.value);
    if (evCursor) qs.set("cursor", evCursor);
    let page;
    try { page = await API.call("/admin/events?" + qs.toString()); }
    catch (ex) { evRowsEl.innerHTML = `<tr><td colspan="5" class="err-text">${esc(ex.message)}</td></tr>`; return; }
    const html = page.items.map(evRow).join("");
    if (reset) evRowsEl.innerHTML = html || `<tr><td colspan="5" class="muted">No events.</td></tr>`;
    else evRowsEl.insertAdjacentHTML("beforeend", html);
    evCursor = page.next_cursor;
    evMore.style.display = page.has_more ? "" : "none";
  };
  evMore.addEventListener("click", () => loadEvents(false));
  evStatus.addEventListener("change", () => loadEvents(true));
  loadEvents(true);
}

// Admin · Modules · Leaderboards - reset-cadence overrides per board.
async function renderLeaderboards() {
  const body = document.getElementById("tab-body");
  body.innerHTML = `
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px">
        <h2 style="flex:1;margin:0">Leaderboard reset cadences <span id="lb-board-count" class="badge muted" style="font-size:.62em;vertical-align:middle;font-weight:normal"></span></h2>
        <input id="lb-board-search" placeholder="Search board…" style="max-width:240px;flex:0 0 auto">
      </div>
      <p class="hint">
        Master-only. Override how a board's reset cadence is treated by
        cheater detection. <code>none</code> = the board never resets
        (lifetime accumulating stat); detection on these boards skips
        score-outlier + rank-gap and uses ONLY velocity.
        <code>auto</code> falls back to the hardcoded mapping in
        <code>models.py</code>. Changes apply within a few seconds -
        the cheaters cache is invalidated + the warmer is kicked.
      </p>
      <div id="lb-boards-rows"><div class="loading">Loading boards…</div></div>
    </div>

    <div class="card">
      <h2 style="margin:0 0 6px">Recompute / reset derived data</h2>
      <p class="hint" style="margin:0 0 14px">
        Every leaderboards-derived dataset can be reset &amp; recalculated on its own, so a quick fix doesn't
        rerun the slow ones. <strong>Player</strong> (<a href="https://trove.aallyn.net/activity" target="_blank" rel="noopener">/activity</a>)
        and <strong>Class</strong> (<a href="https://trove.aallyn.net/class-activity" target="_blank" rel="noopener">/class-activity</a>)
        activity are the slow rebuilds - they replay every stored capture (a few minutes, background);
        <strong>Cheaters</strong> and <strong>Views</strong> only recompute the latest capture (seconds, inline).
        Everything here is derived from the captures, so nothing irreplaceable is lost.
        <strong>Rebuild</strong> keeps existing rows; <strong>Reset &amp; recalculate</strong> wipes first.
      </p>
      <div class="row" style="align-items:flex-end;gap:12px;flex-wrap:wrap;margin-bottom:14px">
        <label style="flex:0 0 auto">
          <span class="muted" style="font-size:.78rem;display:block;margin-bottom:4px">Days back for the activity rebuilds (0 = all history)</span>
          <input type="number" id="rc-days" value="0" min="0" max="1000" style="width:140px">
        </label>
      </div>
      <div style="display:grid;gap:10px">
        <div class="row" style="align-items:center;gap:10px;flex-wrap:wrap">
          <span style="flex:1 1 200px"><strong>Player activity</strong> <span class="muted" style="font-size:.78rem">— /activity history</span></span>
          <button class="btn small" id="rc-act-rebuild" type="button">Rebuild</button>
          <button class="btn small danger" id="rc-act-reset" type="button">Reset &amp; recalculate</button>
        </div>
        <div class="row" style="align-items:center;gap:10px;flex-wrap:wrap">
          <span style="flex:1 1 200px"><strong>Class activity</strong> <span class="muted" style="font-size:.78rem">— /class-activity history (raw + clean)</span></span>
          <button class="btn small" id="rc-cls-rebuild" type="button">Rebuild</button>
          <button class="btn small danger" id="rc-cls-reset" type="button">Reset &amp; recalculate</button>
        </div>
        <div class="row" style="align-items:center;gap:10px;flex-wrap:wrap">
          <span style="flex:1 1 200px"><strong>Cheaters</strong> <span class="muted" style="font-size:.78rem">— per-player flag detection, latest capture</span></span>
          <button class="btn small danger" id="rc-cheat-reset" type="button">Reset &amp; recalculate</button>
        </div>
        <div class="row" style="align-items:center;gap:10px;flex-wrap:wrap">
          <span style="flex:1 1 200px"><strong>Alt clusters</strong> <span class="muted" style="font-size:.78rem">— similar-name/near-score groups, latest capture (shares the cheaters pass)</span></span>
          <button class="btn small danger" id="rc-cluster-reset" type="button">Reset &amp; recalculate</button>
        </div>
        <div class="row" style="align-items:center;gap:10px;flex-wrap:wrap">
          <span style="flex:1 1 200px"><strong>Leaderboard views</strong> <span class="muted" style="font-size:.78rem">— page snapshot caches</span></span>
          <button class="btn small danger" id="rc-views-reset" type="button">Reset &amp; recalculate</button>
        </div>
        <div class="row" style="align-items:center;gap:10px;flex-wrap:wrap;border-top:1px solid var(--border,#2a2f3a);padding-top:10px">
          <span style="flex:1 1 200px"><strong>Everything</strong> <span class="muted" style="font-size:.78rem">— all four in one go</span></span>
          <button class="btn small danger" id="rc-all-reset" type="button">Reset &amp; recalculate all</button>
        </div>
      </div>
      <p class="hint" id="rc-result" style="margin:14px 0 0"></p>
    </div>`;
  renderLeaderboardsBoardsTable();
  wireRecomputeCard();
}

// ── Admin · Modules · Leaderboards · recompute / reset ──────────────────────
// Each leaderboards-derived dataset gets its OWN reset & recalculate so a quick
// fix doesn't rerun the slow activity backfills. Player + class activity replay
// the stored captures (master /v1/{activity,class-activity}/backfill, 202 +
// background); cheaters + views only recompute the latest capture
// (/v1/leaderboards/{cheaters,views}/recompute, inline). All accept the session
// JWT via require_master_ingest. "Everything" fires all four at once.
function wireRecomputeCard() {
  const daysEl = document.getElementById("rc-days");
  const resultEl = document.getElementById("rc-result");
  if (!daysEl) return;
  // 0 = ALL stored history (no lower bound); else clamp to [1, 1000] days.
  const days = () => {
    const v = parseInt(daysEl.value, 10);
    if (!Number.isFinite(v) || v <= 0) return 0;
    return Math.min(1000, v);
  };
  const rangeLabel = () => (days() === 0 ? "the entire stored history" : `the last <strong>${days()} days</strong>`);
  const qs = (reset) => `total_days=${days()}` + (reset ? "&reset=true" : "&force=true");
  const show = (msg) => { resultEl.textContent = msg; };

  const postActivity = (reset) => API.call(`/v1/activity/backfill?${qs(reset)}`, { method: "POST" });
  const postClass = (reset) => API.call(`/v1/class-activity/backfill?${qs(reset)}`, { method: "POST" });
  const postCheaters = () => API.call("/v1/leaderboards/cheaters/recompute", { method: "POST" });
  const postViews = () => API.call("/v1/leaderboards/views/recompute", { method: "POST" });

  // Non-destructive rebuilds (force): no confirmation needed.
  const on = (id, fn) => { const el = document.getElementById(id); if (el) el.addEventListener("click", fn); };
  const rebuild = (label, post) => async () => {
    try { const r = await post(false); show(r.message || `${label} rebuild started.`); toast(`${label} rebuild started`, "ok"); }
    catch (ex) { toast(ex.message || "Failed to start rebuild", "err"); }
  };
  on("rc-act-rebuild", rebuild("Player activity", postActivity));
  on("rc-cls-rebuild", rebuild("Class activity", postClass));

  // Destructive resets: confirm in a modal (onConfirm throws -> stays open on error).
  const onReset = (id, title, bodyHtml, fn) =>
    on(id, () => modal(title, bodyHtml, fn, "Reset & recalculate"));

  onReset(
    "rc-act-reset", "Reset player activity?",
    `<p>Deletes the stored <code>/activity</code> history and recomputes ${rangeLabel()} from the captures.
     Runs in the background (a few minutes); the chart reads empty until it lands.</p>
     <p class="hint">Fully derived from the captures - nothing irreplaceable is lost.</p>`,
    async () => { const r = await postActivity(true); show(r.message || "Player-activity reset + rebuild started."); toast("Player-activity reset started", "ok"); },
  );
  onReset(
    "rc-cls-reset", "Reset class activity?",
    `<p>Deletes the stored <code>/class-activity</code> history (raw + clean views) and recomputes ${rangeLabel()}
     from the captures. Runs in the background; the chart reads empty until it lands.</p>
     <p class="hint">Fully derived from the captures - nothing irreplaceable is lost.</p>`,
    async () => { const r = await postClass(true); show(r.message || "Class-activity reset + rebuild started."); toast("Class-activity reset started", "ok"); },
  );
  onReset(
    "rc-cheat-reset", "Reset cheater detection?",
    `<p>Clears the cached cheater flags (in-process + Redis) and recomputes the latest capture from scratch.
     Runs in the <strong>background</strong> (returns immediately, so it can't hang the page); the
     Cheaters + Alt-clusters tabs refresh within a minute. This single pass also rebuilds the alt clusters.</p>`,
    async () => {
      const r = await postCheaters();
      show(`Recompute started — ${r.redis_snapshots_cleared ?? 0} cached snapshot(s) cleared. `
         + `The Cheaters + Alt-clusters tabs refresh within a minute.`);
      toast("Recompute started", "ok");
    },
  );
  onReset(
    "rc-cluster-reset", "Reset alt-cluster detection?",
    `<p>Recomputes the latest capture's <strong>alt clusters</strong> (similar-name accounts at near-identical scores).
     Runs in the <strong>background</strong> (returns immediately).</p>
     <p class="hint">Clusters and per-player cheater flags are computed in one pass, so this also refreshes the
     Cheaters tab.</p>`,
    async () => {
      await postCheaters();
      show("Alt-cluster recompute started (shares the cheaters pass). The Alt clusters tab refreshes within a minute.");
      toast("Recompute started", "ok");
    },
  );
  onReset(
    "rc-views-reset", "Reset leaderboard views?",
    `<p>Clears the page snapshot caches (anchor list, board lists, entry pages, board-history charts) and
     re-warms the latest captures. Inline - a few seconds.</p>`,
    async () => {
      const r = await postViews();
      show(`Views recomputed: ${r.keys_cleared ?? "?"} keys cleared, ${r.board_charts_warmed ?? "?"} board charts re-warmed.`);
      toast("Views recomputed", "ok");
    },
  );
  onReset(
    "rc-all-reset", "Reset EVERYTHING?",
    `<p>Resets &amp; recalculates <strong>all four</strong>: player activity, class activity, cheaters, and views,
     covering ${rangeLabel()} for the activity rebuilds. The two activity rebuilds run in the background;
     cheaters + views complete inline.</p>
     <p class="hint">All derived from the captures - nothing irreplaceable is lost.</p>`,
    async () => {
      const results = await Promise.allSettled([postActivity(true), postClass(true), postCheaters(), postViews()]);
      const failed = results.filter((r) => r.status === "rejected").length;
      if (failed === results.length) throw new Error("All four failed - check the api logs.");
      show(failed ? `${results.length - failed}/${results.length} started; ${failed} failed - check the api logs.`
                  : "All four reset + recompute started (activity in the background).");
      toast(failed ? `${failed} of ${results.length} failed` : "All reset + recompute started", failed ? "err" : "ok");
    },
  );
}

// ─── Admin · Modules · Giveaways ────────────────────────────────────────────
// Two sub-tabs: "Giveaways" (create with date pickers + manage / draw / cancel)
// and "Vault" (the prize-code pool). Backed by /admin/giveaways/* + /admin/vault/*.

const GW_SUBTAB_KEY = "kiwi_gw_subtab";
const GW_STATUS_BADGE = {
  scheduled: '<span class="badge muted">scheduled</span>',
  open:      '<span class="badge ok">open</span>',
  drawn:     '<span class="badge ok">drawn</span>',
  closed:    '<span class="badge warn">closed</span>',
  cancelled: '<span class="badge off">cancelled</span>',
};
const CODE_STATUS_BADGE = {
  available: '<span class="badge ok">available</span>',
  reserved:  '<span class="badge warn">reserved</span>',
  awarded:   '<span class="badge muted">awarded</span>',
};

// datetime-local <-> UTC ISO. The input is naive local time; toISOString() gives
// UTC for the API, and we convert back to local to pre-fill the edit form.
function toLocalInput(iso) {
  if (!iso) return "";
  const d = new Date(iso);
  const p = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
}
function fromLocalInput(v) {
  return v ? new Date(v).toISOString() : null;
}

async function renderGiveaways() {
  const body = document.getElementById("tab-body");
  const sub = localStorage.getItem(GW_SUBTAB_KEY) === "vault" ? "vault" : "giveaways";
  body.innerHTML = `
    <div class="config-subtabs">
      <button class="config-subtab ${sub === "giveaways" ? "active" : ""}" data-gsub="giveaways">Giveaways</button>
      <button class="config-subtab ${sub === "vault" ? "active" : ""}" data-gsub="vault">Vault</button>
    </div>
    <div id="gw-pane"><div class="loading">Loading…</div></div>`;
  body.querySelectorAll("[data-gsub]").forEach((b) =>
    b.addEventListener("click", () => { localStorage.setItem(GW_SUBTAB_KEY, b.dataset.gsub); renderGiveaways(); }));
  if (sub === "vault") renderVaultPane();
  else renderGiveawaysPane();
}

async function renderGiveawaysPane() {
  const pane = document.getElementById("gw-pane");
  let items;
  try { items = await API.call("/admin/giveaways"); }
  catch (ex) { pane.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`; return; }

  const row = (g) => `
    <tr>
      <td>${esc(g.title)}<div class="muted" style="font-size:.82rem">${esc(g.prize_name)}</div></td>
      <td>${GW_STATUS_BADGE[g.status] || esc(g.status)}</td>
      <td class="muted" style="font-size:.84rem;white-space:nowrap">${fmt(g.starts_at)} →<br>${fmt(g.ends_at)}</td>
      <td>${g.entry_count}</td>
      <td>${g.winner_username ? esc(g.winner_username) : '<span class="muted">-</span>'}</td>
      <td style="white-space:nowrap">${(g.status === "open" || g.status === "scheduled") ? `
        <button class="btn small" data-edit="${g.id}">Edit</button>
        <button class="btn small" data-draw="${g.id}">Draw</button>
        <button class="btn small danger" data-cancel="${g.id}">Cancel</button>` : ""}</td>
    </tr>`;

  pane.innerHTML = `
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px">
        <h2 style="flex:1;margin:0">Giveaways (${items.length})</h2>
        <button class="btn primary small" id="gw-new">+ New giveaway</button>
      </div>
      <p class="hint">Scheduled giveaways open + auto-draw on their dates (checked every minute). "Draw" forces an immediate draw; the winner is emailed their code automatically.</p>
      ${items.length ? `<table>
        <thead><tr><th>Giveaway</th><th>Status</th><th>Window</th><th>Entries</th><th>Winner</th><th></th></tr></thead>
        <tbody>${items.map(row).join("")}</tbody>
      </table>` : `<p class="muted">No giveaways yet. Add a prize code to the Vault, then create one.</p>`}
    </div>`;

  document.getElementById("gw-new").addEventListener("click", () => openGiveawayForm(null));
  pane.querySelectorAll("[data-edit]").forEach((b) =>
    b.addEventListener("click", () => openGiveawayForm(items.find((g) => g.id === b.dataset.edit))));
  pane.querySelectorAll("[data-draw]").forEach((b) =>
    b.addEventListener("click", () => confirmGiveawayAction(b.dataset.draw, "draw")));
  pane.querySelectorAll("[data-cancel]").forEach((b) =>
    b.addEventListener("click", () => confirmGiveawayAction(b.dataset.cancel, "cancel")));
}

async function openGiveawayForm(existing) {
  let items;
  try { items = await API.call("/admin/vault/items"); }
  catch (ex) { toast(ex.message, "err"); return; }
  // Drawers with an available code, plus this giveaway's current drawer (edit).
  const selectable = items.filter((it) => it.available > 0 || (existing && it.id === existing.vault_item_id));
  if (!selectable.length) {
    toast("Add a drawer with codes to the Vault first.", "err");
    localStorage.setItem(GW_SUBTAB_KEY, "vault"); renderGiveaways();
    return;
  }
  const opts = selectable.map((it) =>
    `<option value="${it.id}" ${existing && it.id === existing.vault_item_id ? "selected" : ""}>${esc(it.name)} (${it.available} available)</option>`).join("");

  modal(existing ? "Edit giveaway" : "New giveaway", `
    <label>Title <span class="muted">(the event, e.g. "Weekend Giveaway")</span></label>
    <input id="gw-title" value="${existing ? esc(existing.title) : ""}" maxlength="160">
    <label>Prize drawer <span class="muted">(one available code is reserved)</span></label>
    <select id="gw-item">${opts}</select>
    <label>Description <span class="muted">(optional - defaults to the drawer's)</span></label>
    <textarea id="gw-desc" rows="3">${existing ? esc(existing.description || "") : ""}</textarea>
    <div class="row" style="gap:12px">
      <div style="flex:1"><label>Starts</label><input type="datetime-local" id="gw-start" style="width:100%" value="${existing ? toLocalInput(existing.starts_at) : ""}"></div>
      <div style="flex:1"><label>Ends</label><input type="datetime-local" id="gw-end" style="width:100%" value="${existing ? toLocalInput(existing.ends_at) : ""}"></div>
    </div>
  `, async () => {
    const body = {
      title: document.getElementById("gw-title").value.trim(),
      vault_item_id: document.getElementById("gw-item").value,
      description: document.getElementById("gw-desc").value.trim() || null,
      starts_at: fromLocalInput(document.getElementById("gw-start").value),
      ends_at: fromLocalInput(document.getElementById("gw-end").value),
    };
    if (!body.title) throw new Error("Title is required.");
    if (!body.starts_at || !body.ends_at) throw new Error("Start and end dates are required.");
    if (existing) await API.call(`/admin/giveaways/${existing.id}`, { method: "PATCH", body });
    else await API.call("/admin/giveaways", { method: "POST", body });
    toast(existing ? "Giveaway updated." : "Giveaway created.", "ok");
    renderGiveawaysPane();
  }, existing ? "Save" : "Create");
}

function confirmGiveawayAction(id, action) {
  const draw = action === "draw";
  modal(draw ? "Draw the winner now?" : "Cancel this giveaway?",
    `<p>${draw
      ? "A random entrant is picked immediately and emailed the code. This can't be undone."
      : "The giveaway is cancelled and its prize code returned to the vault."}</p>`,
    async () => {
      await API.call(`/admin/giveaways/${id}/${action}`, { method: "POST" });
      toast(draw ? "Winner drawn + emailed." : "Giveaway cancelled.", "ok");
      renderGiveawaysPane();
    }, draw ? "Draw now" : "Cancel giveaway");
}

async function renderVaultPane() {
  const pane = document.getElementById("gw-pane");
  let items;
  try { items = await API.call("/admin/vault/items"); }
  catch (ex) { pane.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`; return; }

  const drawer = (it) => `
    <div class="card" data-drawer="${it.id}" style="margin-bottom:12px">
      <div class="row" style="align-items:center;gap:10px;flex-wrap:wrap">
        <div style="flex:1;min-width:180px">
          <h3 style="margin:0">${esc(it.name)}</h3>
          ${it.description ? `<div class="muted" style="font-size:.85rem">${esc(it.description)}</div>` : ""}
        </div>
        <div style="display:flex;gap:6px;align-items:center;flex-wrap:wrap">
          <span class="badge ok">${it.available} available</span>
          ${it.reserved ? `<span class="badge warn">${it.reserved} reserved</span>` : ""}
          ${it.awarded ? `<span class="badge muted">${it.awarded} awarded</span>` : ""}
          <button class="btn small primary" data-addcodes="${it.id}">+ Codes</button>
          <button class="btn small" data-toggle="${it.id}">View codes</button>
          <button class="btn small" data-editdrawer="${it.id}">Edit</button>
          <button class="btn small danger" data-deldrawer="${it.id}">Delete</button>
        </div>
      </div>
      <div id="codes-${it.id}" hidden></div>
    </div>`;

  pane.innerHTML = `
    <div class="row" style="align-items:center;margin-bottom:10px">
      <h2 style="flex:1;margin:0">Vault (${items.length} drawer${items.length === 1 ? "" : "s"})</h2>
      <button class="btn primary small" id="drawer-new">+ New drawer</button>
    </div>
    <p class="hint">A drawer is a named prize (e.g. "Trove Radiant Mount"). Write the name + description <b>once</b>, then bulk-add codes to it - one per line. A giveaway draws one available code from a drawer.</p>
    ${items.length ? items.map(drawer).join("") : `<p class="muted">No drawers yet. Create one, then add codes to it.</p>`}`;

  document.getElementById("drawer-new").addEventListener("click", () => openDrawerForm(null));
  pane.querySelectorAll("[data-editdrawer]").forEach((b) =>
    b.addEventListener("click", () => openDrawerForm(items.find((i) => i.id === b.dataset.editdrawer))));
  pane.querySelectorAll("[data-addcodes]").forEach((b) =>
    b.addEventListener("click", () => openAddCodes(b.dataset.addcodes)));
  pane.querySelectorAll("[data-deldrawer]").forEach((b) =>
    b.addEventListener("click", () => modal("Delete drawer?",
      "<p>Deletes the drawer and its unused codes. Drawers with reserved/awarded codes can't be deleted.</p>",
      async () => {
        await API.call(`/admin/vault/items/${b.dataset.deldrawer}`, { method: "DELETE" });
        toast("Drawer deleted.", "ok"); renderVaultPane();
      }, "Delete")));
  pane.querySelectorAll("[data-toggle]").forEach((b) =>
    b.addEventListener("click", () => toggleCodes(b.dataset.toggle, b)));
}

async function toggleCodes(itemId, btn) {
  const host = document.getElementById("codes-" + itemId);
  if (!host.hidden) { host.hidden = true; btn.textContent = "View codes"; return; }
  host.hidden = false; btn.textContent = "Hide codes";
  host.innerHTML = `<p class="muted" style="margin:10px 0 0">Loading…</p>`;
  let codes;
  try { codes = await API.call(`/admin/vault/items/${itemId}/codes`); }
  catch (ex) { host.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`; return; }
  if (!codes.length) { host.innerHTML = `<p class="muted" style="margin:10px 0 0">No codes in this drawer yet.</p>`; return; }
  host.innerHTML = `
    <table style="margin-top:12px">
      <thead><tr><th>Code</th><th>Status</th><th>Awarded to</th><th></th></tr></thead>
      <tbody>${codes.map((c) => `
        <tr>
          <td class="mono" style="word-break:break-all">${esc(c.code)}</td>
          <td>${CODE_STATUS_BADGE[c.status] || esc(c.status)}</td>
          <td class="muted" style="font-size:.84rem">${c.awarded_to_email ? esc(c.awarded_to_email) : "-"}</td>
          <td>${c.status === "available" ? `<button class="btn small danger" data-delcode="${c.id}">Delete</button>` : ""}</td>
        </tr>`).join("")}</tbody>
    </table>`;
  host.querySelectorAll("[data-delcode]").forEach((b) =>
    b.addEventListener("click", () => modal("Delete code?",
      "<p>Remove this code from the drawer.</p>",
      async () => {
        await API.call(`/admin/vault/codes/${b.dataset.delcode}`, { method: "DELETE" });
        toast("Code deleted.", "ok"); renderVaultPane();
      }, "Delete")));
}

function openDrawerForm(existing) {
  modal(existing ? "Edit drawer" : "New drawer", `
    <label>Name <span class="muted">(the prize, e.g. "Trove Radiant Mount")</span></label>
    <input id="dr-name" value="${existing ? esc(existing.name) : ""}" maxlength="120">
    <label>Description <span class="muted">(optional - shown to entrants + in the win email)</span></label>
    <textarea id="dr-desc" rows="3">${existing ? esc(existing.description || "") : ""}</textarea>
  `, async () => {
    const body = {
      name: document.getElementById("dr-name").value.trim(),
      description: document.getElementById("dr-desc").value.trim() || null,
    };
    if (!body.name) throw new Error("Name is required.");
    if (existing) await API.call(`/admin/vault/items/${existing.id}`, { method: "PATCH", body });
    else await API.call("/admin/vault/items", { method: "POST", body });
    toast(existing ? "Drawer updated." : "Drawer created.", "ok");
    renderVaultPane();
  }, existing ? "Save" : "Create");
}

function openAddCodes(itemId) {
  modal("Add codes", `
    <label>Codes <span class="muted">(one per line - paste as many as you like)</span></label>
    <textarea id="ac-codes" rows="10" class="mono" placeholder="ABCD-1234-EFGH\nWXYZ-5678-IJKL\n…"></textarea>
  `, async () => {
    const codes = document.getElementById("ac-codes").value.split(/\r?\n/);
    if (!codes.some((c) => c.trim())) throw new Error("Paste at least one code.");
    const r = await API.call(`/admin/vault/items/${itemId}/codes`, { method: "POST", body: { codes } });
    toast(`Added ${r.added} code${r.added === 1 ? "" : "s"}${r.skipped ? ` (${r.skipped} duplicate/blank skipped)` : ""}.`, "ok");
    renderVaultPane();
  }, "Add codes");
}


// ─── Leaderboard reset-cadence table (lives inside the Admin tab) ────────
// Lists every captured board with an inline dropdown to pin its reset
// cadence to daily / weekly / none / auto. PATCHes /admin/leaderboards/
// boards/{uuid} on change; on success bumps the row's cached state +
// re-paints just that row's badges. Search filter is client-side over
// the cached list - no round-trip per keystroke.
let _lbBoards = [];

async function renderLeaderboardsBoardsTable() {
  const host = document.getElementById("lb-boards-rows");
  if (!host) return;
  try {
    const data = await API.call("/admin/leaderboards/boards");
    _lbBoards = data.items || [];
  } catch (ex) {
    host.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`;
    return;
  }
  if (!_lbBoards.length) {
    host.innerHTML = `<p class="muted">No leaderboards captured yet.</p>`;
    return;
  }
  paintLeaderboardsBoards("");
  const search = document.getElementById("lb-board-search");
  if (search) {
    search.addEventListener("input", (e) => {
      paintLeaderboardsBoards(e.target.value.trim().toLowerCase());
    });
  }
}

function paintLeaderboardsBoards(q) {
  const host = document.getElementById("lb-boards-rows");
  if (!host) return;
  const list = (q
    ? _lbBoards.filter((b) =>
        b.name.toLowerCase().includes(q) ||
        b.category.toLowerCase().includes(q) ||
        String(b.uuid).includes(q))
    : _lbBoards.slice()
  ).sort((a, b) => a.uuid - b.uuid);   // order by board id (ascending)
  const countEl = document.getElementById("lb-board-count");
  if (countEl) countEl.textContent = q
    ? `${list.length} / ${_lbBoards.length}`   // matched / total while searching
    : `${_lbBoards.length}`;
  if (!list.length) {
    host.innerHTML = `<p class="muted">No matching boards.</p>`;
    return;
  }
  const cadenceBadge = (effective) => {
    if (effective === "daily")  return '<span class="badge ok">daily</span>';
    if (effective === "weekly") return '<span class="badge ok">weekly</span>';
    // "none" + "default" are both lifetime semantically; surface them
    // distinctly so the admin knows whether they pinned it explicitly.
    if (effective === "none") return '<span class="badge warn">none</span>';
    return '<span class="badge muted">default</span>';
  };
  const overrideValue = (b) =>
    b.reset_kind_override === null || b.reset_kind_override === undefined
      ? "" : b.reset_kind_override;
  const row = (b) => `
    <tr data-uuid="${b.uuid}">
      <td><code>${b.uuid}</code></td>
      <td>${esc(b.name)}</td>
      <td class="muted">${esc(b.category)}</td>
      <td>${cadenceBadge(b.effective_reset_kind)}</td>
      <td>
        <select data-act="set-cadence" style="max-width:140px">
          <option value=""${overrideValue(b) === "" ? " selected" : ""}>auto</option>
          <option value="daily"${overrideValue(b) === "daily" ? " selected" : ""}>daily</option>
          <option value="weekly"${overrideValue(b) === "weekly" ? " selected" : ""}>weekly</option>
          <option value="none"${overrideValue(b) === "none" ? " selected" : ""}>none</option>
        </select>
      </td>
    </tr>`;
  host.innerHTML = `
    <table>
      <thead><tr><th>UUID</th><th>Board</th><th>Category</th><th>Effective</th><th>Override</th></tr></thead>
      <tbody>${list.map(row).join("")}</tbody>
    </table>`;

  host.querySelectorAll("[data-uuid]").forEach((tr) => {
    const uuid = Number(tr.dataset.uuid);
    const select = tr.querySelector('[data-act="set-cadence"]');
    select.addEventListener("change", async () => {
      const next = select.value === "" ? null : select.value;
      select.disabled = true;
      try {
        const updated = await API.call(`/admin/leaderboards/boards/${uuid}`, {
          method: "PATCH",
          body: { reset_kind_override: next },
        });
        // Refresh the cached row so subsequent renders / filter changes
        // reflect the new state without a full re-fetch.
        const idx = _lbBoards.findIndex((b) => b.uuid === uuid);
        if (idx >= 0) _lbBoards[idx] = updated;
        // Repaint just this row's effective badge - selecting a value
        // already shows in the dropdown, so we only need to update the
        // visible cadence column.
        const effCell = tr.querySelector("td:nth-child(4)");
        if (effCell) {
          effCell.innerHTML =
            updated.effective_reset_kind === "daily"  ? '<span class="badge ok">daily</span>' :
            updated.effective_reset_kind === "weekly" ? '<span class="badge ok">weekly</span>' :
            updated.effective_reset_kind === "none"   ? '<span class="badge warn">none</span>' :
                                                        '<span class="badge muted">default</span>';
        }
        toast(`Saved ${updated.name} → ${updated.effective_reset_kind}`, "ok");
      } catch (ex) {
        toast(`Failed: ${ex.message}`, "err");
      } finally {
        select.disabled = false;
      }
    });
  });
}

// ─── Configuration tab (master-only) ─────────────────────────────────────
// Dedicated tab - keeps Admin focused on users/activity and Configuration
// focused on every runtime-tunable knob (rate limits, webhooks, alerts).
// Settings are grouped by category server-side so adding a new category is
// just a registry entry on the backend; the UI requires no change.

const CONFIG_CATEGORY_LABELS = {
  features: "Site features",
  feedback: "Feedback endpoint",
  api_rate_limits: "API rate limits",
  auth_rate_limits: "Auth flow rate limits",
  archive_rate_limits: "Archive-query rate limits",
  scope_multipliers: "Per-scope rate-limit multipliers",
  rate_limit_alerts: "Rate-limit alert digest",
  ingest_cooldown: "Ingest cooldown",
  cheater_detection: "Cheater detection",
  class_activity: "Class activity",
  trove_status: "Trove server status",
  community_feeds: "Community feeds",
};

async function renderConfigTab() {
  const body = document.getElementById("tab-body");
  body.innerHTML = `
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px">
        <h2 style="flex:1;margin:0">Runtime configuration</h2>
        <span class="muted" style="font-size:12px">master-only · changes apply within 5s</span>
      </div>
      <p class="hint">
        Tunables that don't require an env-var edit or container restart.
        Edit applies on the next request (5 s cache). Reset clears the override
        so the code default takes effect again. Settings flagged with ⚠
        require an API container restart because they're bound into the
        FastAPI dependency tree at startup.
      </p>
      <div id="config-body"><div class="loading">Loading config…</div></div>
    </div>`;
  renderConfigCard();
}

const CONFIG_SUBTAB_KEY = "kiwi_config_subtab";
function readConfigSubtab() {
  try { return localStorage.getItem(CONFIG_SUBTAB_KEY); } catch (_) { return null; }
}
function writeConfigSubtab(v) {
  try { localStorage.setItem(CONFIG_SUBTAB_KEY, v); } catch (_) {}
}

async function renderConfigCard() {
  const bodyEl = document.getElementById("config-body");
  if (!bodyEl) return;
  let data;
  try {
    data = await API.call("/admin/config");
  } catch (ex) {
    bodyEl.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`;
    return;
  }

  // Group by category - preserve server-side ordering within each.
  const byCategory = {};
  for (const item of data.items) {
    (byCategory[item.category] ||= []).push(item);
  }
  const categories = Object.keys(byCategory);
  if (!categories.length) {
    bodyEl.innerHTML = `<p class="muted">No tunables registered.</p>`;
    return;
  }
  // Pick active sub-tab: stored value if still valid, else first.
  const stored = readConfigSubtab();
  const active = (stored && categories.includes(stored)) ? stored : categories[0];

  const renderValue = (item) => {
    if (item.secret) {
      // Mask: show ●●● + the last 4 of the value if any, blank if empty.
      const v = String(item.value ?? "");
      return v ? `<span class="mono">●●● ${esc(v.slice(-4))}</span>`
               : `<span class="muted">(not set)</span>`;
    }
    if (item.value === null || item.value === undefined || item.value === "") {
      return `<span class="muted">(empty)</span>`;
    }
    return `<span class="mono">${esc(String(item.value))}</span>`;
  };

  const renderRow = (item) => {
    const stateBadge = item.is_default
      ? '<span class="badge muted">default</span>'
      : '<span class="badge ok">overridden</span>';
    return `
      <tr data-key="${esc(item.key)}">
        <td>
          <code>${esc(item.key)}</code> ${stateBadge}
          <div class="muted" style="font-size:12px;margin-top:4px;max-width:560px">${esc(item.description)}</div>
        </td>
        <td>${renderValue(item)}</td>
        <td class="muted" style="white-space:nowrap">${item.updated_at ? fmt(item.updated_at) : "-"}</td>
        <td style="white-space:nowrap">
          <button class="btn small" data-act="edit">Edit</button>
          <button class="btn small" data-act="reset"${item.is_default ? " disabled" : ""}>Reset</button>
        </td>
      </tr>`;
  };

  // Sub-tab strip - one chip per category, badge with count.
  const subtabs = categories.map((cat) => {
    const label = CONFIG_CATEGORY_LABELS[cat] || cat;
    const count = byCategory[cat].length;
    const overridden = byCategory[cat].filter((i) => !i.is_default).length;
    const overrideBadge = overridden
      ? `<span class="badge ok" style="margin-left:6px">${overridden}</span>`
      : "";
    return `
      <button type="button" class="config-subtab${cat === active ? " active" : ""}"
              data-subtab="${esc(cat)}">
        ${esc(label)}
        <span class="muted" style="margin-left:8px;font-size:11px">${count}</span>
        ${overrideBadge}
      </button>`;
  }).join("");

  // Single-table body for the active sub-tab. Switching sub-tabs
  // re-renders ONLY this region, not the whole tab.
  const activeItems = byCategory[active] || [];
  const tableHtml = `
    <table>
      <thead><tr><th>Setting</th><th>Value</th><th>Updated</th><th></th></tr></thead>
      <tbody>${activeItems.map(renderRow).join("")}</tbody>
    </table>`;

  bodyEl.innerHTML = `
    <div class="config-subtabs" role="tablist">${subtabs}</div>
    <div id="config-subtab-body">${tableHtml}</div>`;

  bodyEl.querySelectorAll("[data-subtab]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const next = btn.dataset.subtab;
      if (next === active) return;
      writeConfigSubtab(next);
      renderConfigCard();  // re-render with the new active sub-tab
    });
  });

  // Wire row actions on the visible sub-tab only.
  bodyEl.querySelectorAll("[data-key]").forEach((tr) => {
    const key = tr.dataset.key;
    const item = data.items.find((i) => i.key === key);
    tr.querySelector('[data-act="edit"]').addEventListener("click", () => editSetting(item));
    const rb = tr.querySelector('[data-act="reset"]');
    if (rb && !rb.disabled) rb.addEventListener("click", () => resetSetting(item));
  });
}


function editSetting(item) {
  // Type-appropriate input. For ints/floats we use type=number with
  // step/min/max from the registry; for bools a select; for strings
  // (incl. secrets) a textarea (handles long URLs cleanly).
  let inputHtml;
  if (item.type === "bool") {
    const v = String(!!item.value);
    inputHtml = `
      <select id="cfg-input" style="width:100%">
        <option value="true"${v === "true" ? " selected" : ""}>true</option>
        <option value="false"${v === "false" ? " selected" : ""}>false</option>
      </select>`;
  } else if (item.type === "int" || item.type === "float") {
    const step = item.type === "int" ? "1" : "any";
    const min = item.min_value != null ? ` min="${item.min_value}"` : "";
    const max = item.max_value != null ? ` max="${item.max_value}"` : "";
    inputHtml = `<input id="cfg-input" type="number" step="${step}"${min}${max} value="${esc(String(item.value ?? ""))}" style="width:100%">`;
  } else {
    // String (and secrets): textarea handles long values + word-wrap.
    // For secrets we DON'T pre-fill - master must paste the value in fresh,
    // which is safer than echoing it on every edit.
    const placeholder = item.secret
      ? "Paste new value (current value is hidden for security)"
      : "Value";
    const val = item.secret ? "" : esc(String(item.value ?? ""));
    inputHtml = `<textarea id="cfg-input" rows="3" placeholder="${placeholder}" style="width:100%;font-family:inherit">${val}</textarea>`;
  }
  const rangeHint = (item.min_value != null || item.max_value != null)
    ? `<p class="hint">Range: ${item.min_value ?? "–∞"} to ${item.max_value ?? "+∞"}</p>` : "";
  const defaultHint = `<p class="hint">Code default: <code>${esc(String(item.default))}</code></p>`;

  modal(
    `Edit ${item.key}`,
    `<p>${esc(item.description)}</p>${inputHtml}${rangeHint}${defaultHint}`,
    async () => {
      const raw = document.getElementById("cfg-input").value;
      let value;
      if (item.type === "bool")        value = (raw === "true");
      else if (item.type === "int")    value = parseInt(raw, 10);
      else if (item.type === "float")  value = parseFloat(raw);
      else                              value = raw;  // string
      try {
        await API.call(`/admin/config/${encodeURIComponent(item.key)}`, {
          method: "PUT", body: { value },
        });
        await renderConfigCard();
      } catch (ex) {
        alert("Save failed: " + ex.message);
      }
    },
    "Save",
  );
}


function resetSetting(item) {
  modal(
    `Reset ${item.key}?`,
    `<p>The override will be dropped and the code default (<code>${esc(String(item.default))}</code>) will take effect again within 5 seconds.</p>`,
    async () => {
      try {
        await API.call(`/admin/config/${encodeURIComponent(item.key)}`, { method: "DELETE" });
        await renderConfigCard();
      } catch (ex) {
        alert("Reset failed: " + ex.message);
      }
    },
    "Reset to default",
  );
}


async function renderAdminUser(userId, days = 30) {
  const body = document.getElementById("tab-body");
  body.innerHTML = `<div class="loading">Loading user…</div>`;
  let user, tokens, activity;
  try {
    [user, tokens, activity] = await Promise.all([
      API.call(`/admin/users/${userId}`),
      API.call(`/admin/users/${userId}/tokens`),
      API.call(`/admin/users/${userId}/activity?days=${days}`),
    ]);
  } catch (ex) { body.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`; return; }

  const tokenRows = tokens.length ? tokens.map((t) => `
    <tr>
      <td>${esc(t.name)}</td>
      <td class="mono">${esc(t.prefix)}…</td>
      <td>${adminScope(t)}</td>
      <td>${t.allowed_ip_count > 0 ? `${t.allowed_ip_count} pinned <span class="muted">(hashed)</span>` : '<span class="muted">any</span>'}</td>
      <td>${t.request_count}</td>
      <td>${t.revoked ? '<span class="badge off">revoked</span>' : '<span class="badge ok">active</span>'}</td>
      <td>${t.revoked ? "" : `<button class="btn small danger" data-revoke="${t.id}">Revoke</button>`}</td>
    </tr>`).join("") : `<tr><td colspan="7" class="muted">No tokens.</td></tr>`;

  const eps = activity.by_endpoint.length ? activity.by_endpoint.map((e) => `
    <tr><td class="mono">${esc(e.method)} ${esc(e.route)}</td><td>${e.count}</td>
    <td>${e.error_count}</td><td>${e.avg_duration_ms} ms</td></tr>`).join("")
    : `<tr><td colspan="4" class="muted">No requests in this window.</td></tr>`;

  body.innerHTML = `
    <button class="btn small" id="admin-back" style="margin-bottom:16px">← All users</button>
    <div class="card">
      <h2>${esc(user.email)} ${user.is_superuser ? '<span class="badge ok">admin</span>' : ""}</h2>
      <table>
        <tr><td class="muted">Status</td><td>${user.is_active ? "active" : '<span class="badge off">inactive</span>'} ·
          ${user.is_verified ? "verified" : '<span class="badge warn">unverified</span>'}</td></tr>
        <tr><td class="muted">Member since</td><td>${fmtDay(user.created_at)}</td></tr>
        <tr><td class="muted">Last login</td><td>${fmt(user.last_login_at)}</td></tr>
      </table>
      <div class="stat-grid" style="margin-top:14px">
        <div class="stat"><div class="n">${activity.total_requests}</div><div class="l">Requests (${days}d)</div></div>
        <div class="stat"><div class="n">${activity.error_count}</div><div class="l">Errors</div></div>
        <div class="stat"><div class="n">${activity.rate_limited}</div><div class="l">Rate-limit hits</div></div>
      </div>
    </div>
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px">
        <h2 style="flex:1;margin:0">Tokens</h2>
        <button class="btn small danger" id="revoke-all" style="flex:0 0 auto">Revoke all</button>
      </div>
      <table>
        <thead><tr><th>Name</th><th>Prefix</th><th>Scopes</th><th>IPs</th><th>Requests</th><th>Status</th><th></th></tr></thead>
        <tbody>${tokenRows}</tbody>
      </table>
    </div>
    <div class="card">
      <h2>Activity by endpoint</h2>
      <table>
        <thead><tr><th>Endpoint</th><th>Requests</th><th>Errors</th><th>Avg latency</th></tr></thead>
        <tbody>${eps}</tbody>
      </table>
    </div>`;

  document.getElementById("admin-back").addEventListener("click", () => renderUsers());
  document.getElementById("revoke-all").addEventListener("click", async () => {
    if (!confirm(`Revoke ALL active tokens for ${user.email}?`)) return;
    try {
      const r = await API.call(`/admin/users/${userId}/tokens`, { method: "DELETE" });
      toast(`Revoked ${r.revoked} token(s).`, "ok"); renderAdminUser(userId, days);
    } catch (ex) { toast(ex.message, "err"); }
  });
  body.querySelectorAll("[data-revoke]").forEach((b) =>
    b.addEventListener("click", async () => {
      if (!confirm("Revoke this token?")) return;
      try {
        await API.call(`/admin/tokens/${b.dataset.revoke}/revoke`, { method: "POST" });
        toast("Token revoked.", "ok"); renderAdminUser(userId, days);
      } catch (ex) { toast(ex.message, "err"); }
    }));
}

// --- Ingest tab (master-only) ---------------------------------------------
// Manual cfg replay UI for the four bot-cfg endpoints. Same data shapes the
// bot uses: leaderboards/market take the raw .cfg as a multipart `file`,
// rotations/challenge + rotations/chaos-chest take `{name}` parsed from
// QuestLog.cfg / WelcomeLog.cfg respectively.

const INGEST_KINDS = [
  {
    key: "leaderboards",
    title: "Leaderboards",
    cfg: "LeaderBot.cfg",
    description: "Replay a LeaderBot.cfg dump through /v1/leaderboards/insert. Idempotent on the anchor.",
    mode: "multipart",
    endpoint: "/v1/leaderboards/insert",
    timestampField: true,
  },
  {
    key: "market",
    title: "Market",
    cfg: "GrainusMod.cfg",
    description: "Replay a GrainusMod.cfg dump through /v1/market/insert. Listings upsert by UUID.",
    mode: "multipart",
    endpoint: "/v1/market/insert",
  },
  {
    key: "challenge",
    title: "Challenge",
    cfg: "QuestLog.cfg",
    description: "Parse the `challenge = …` line and POST {name} to /v1/rotations/challenge/insert. The server infers the 20-min window anchor from now.",
    mode: "parse-name",
    endpoint: "/v1/rotations/challenge/insert",
    parseKey: "challenge",
  },
  {
    key: "chaoschest",
    title: "Chaos Chest",
    cfg: "WelcomeLog.cfg",
    description: "Parse the `chaoschest = …` line and POST {name} to /v1/rotations/chaos-chest/insert. The server infers the weekly anchor from now.",
    mode: "parse-name",
    endpoint: "/v1/rotations/chaos-chest/insert",
    parseKey: "chaoschest",
  },
];

// --- Discord (master) ------------------------------------------------------
// Slash commands are defined server-side (app/discord/commands.py) but Discord
// only learns them via a PUT. This tab previews the local set and pushes it on
// demand so there's no CLI step. Master-only (router-level superuser dep).
async function renderDiscord() {
  const body = document.getElementById("tab-body");
  body.innerHTML = `
    <div class="card">
      <h2 style="margin:0 0 6px">Discord commands</h2>
      <p class="hint" style="margin:0">
        Slash commands are defined in the API code, but Discord only learns about
        them when you push them here - editing a command and redeploying does
        nothing on its own. Global pushes can take up to ~1 hour to appear in
        clients; a guild push is instant (handy while testing).
      </p>
    </div>
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px">
        <h2 style="flex:1;margin:0">Defined commands</h2>
        <button type="button" class="btn small" data-act="refresh">Refresh</button>
      </div>
      <div id="discord-cmd-list"><div class="loading">Loading…</div></div>
    </div>
    <div class="card">
      <h2 style="margin:0 0 6px">Push to Discord</h2>
      <p class="hint" style="margin:0 0 14px">
        Bulk-overwrites the app's registered slash commands with the set above,
        using the configured bot token. Leave the guild id blank to push globally.
      </p>
      <div class="row" style="align-items:center">
        <input type="text" id="discord-guild" placeholder="Guild id (optional · instant push)">
        <button class="btn primary" data-act="push">Push to Discord</button>
        <button class="btn" data-act="clear-guild">Clear guild commands</button>
      </div>
      <p class="hint" style="margin:10px 0 0">
        Seeing each command <strong>twice</strong> in one server? That's a leftover
        per-guild test push on top of the global commands. Put that server's id above
        and click <strong>Clear guild commands</strong> - the global set then shows alone.
      </p>
      <div id="discord-push-result" class="ingest-result"></div>
    </div>`;

  const listEl = document.getElementById("discord-cmd-list");
  async function loadCommands() {
    listEl.innerHTML = `<div class="loading">Loading…</div>`;
    try {
      const data = await API.call("/admin/discord/commands");
      listEl.innerHTML = data.count
        ? data.commands.map((c) =>
            `<div class="row" style="align-items:baseline;gap:10px;padding:7px 0;border-bottom:1px solid var(--border)">
               <code style="flex:0 0 auto">/${esc(c.name)}</code>
               <span class="muted" style="flex:1">${esc(c.description || "")}</span>
             </div>`).join("")
        : `<p class="muted" style="margin:0">No commands defined.</p>`;
    } catch (ex) {
      listEl.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`;
    }
  }
  body.querySelector('[data-act="refresh"]').addEventListener("click", loadCommands);
  loadCommands();

  const result = document.getElementById("discord-push-result");
  body.querySelector('[data-act="push"]').addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    const guild = document.getElementById("discord-guild").value.trim();
    btn.disabled = true;
    result.className = "ingest-result";
    result.textContent = "Pushing…";
    try {
      const qs = guild ? `?guild_id=${encodeURIComponent(guild)}` : "";
      const data = await API.call("/admin/discord/register-commands" + qs, { method: "POST" });
      const where = data.scope === "guild" ? `to guild ${data.guild_id}` : "globally";
      const note = data.scope === "global" ? " It can take up to ~1h to appear in clients." : "";
      result.className = "ingest-result ok";
      result.textContent = `Pushed ${data.count} command(s) ${where}.${note}`;
      toast(`Pushed ${data.count} command(s) to Discord`, "ok");
      loadCommands();
    } catch (ex) {
      result.className = "ingest-result err";
      result.textContent = ex.message;
      toast("Discord push failed", "err");
    } finally {
      btn.disabled = false;
    }
  });

  body.querySelector('[data-act="clear-guild"]').addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    const guild = document.getElementById("discord-guild").value.trim();
    if (!guild) {
      result.className = "ingest-result err";
      result.textContent = "Enter the guild id to clear its commands.";
      return;
    }
    btn.disabled = true;
    result.className = "ingest-result";
    result.textContent = "Clearing…";
    try {
      const qs = `?guild_id=${encodeURIComponent(guild)}`;
      await API.call("/admin/discord/clear-guild-commands" + qs, { method: "POST" });
      result.className = "ingest-result ok";
      result.textContent = `Cleared guild-scoped commands for ${guild}. The global commands remain.`;
      toast("Cleared guild commands", "ok");
    } catch (ex) {
      result.className = "ingest-result err";
      result.textContent = ex.message;
      toast("Clear failed", "err");
    } finally {
      btn.disabled = false;
    }
  });
}

// --- Codexes module (master) ----------------------------------------------
// The codex (parsed binfab game data) is materialized in Postgres from the update
// archive and refreshes itself: a parser-code change bumps CODEX_PARSER_VERSION
// and the next sync force-rebuilds the branch; a game update applies as a delta.
// This tab shows the state and offers a manual force-rebuild to apply a parser
// change immediately (every prefab re-parsed, UPSERTed in place - no empty window).
async function renderCodexes() {
  const body = document.getElementById("tab-body");
  body.innerHTML = `
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px">
        <h2 style="flex:1;margin:0">Codexes</h2>
        <select id="cdx-branch" style="max-width:160px">
          <option value="live-us">Live (live-us)</option>
          <option value="pts">PTS (pts)</option>
        </select>
      </div>
      <p class="hint" style="margin:0 0 14px">
        Parsed Trove game data materialized in Postgres. It rebuilds automatically
        when the parser version advances (after a deploy) or a game update lands.
        Force a rebuild here to apply a parser change right away instead of waiting
        for the next sync - every prefab is re-parsed and upserted in place.
      </p>
      <div id="cdx-status"><div class="loading">Loading…</div></div>
      <div class="row" style="align-items:center;margin-top:14px">
        <button class="btn primary" data-act="rebuild">Rebuild now</button>
        <button type="button" class="btn small" data-act="refresh">Refresh</button>
      </div>
      <div id="cdx-result" class="ingest-result"></div>
    </div>`;

  const branchSel = document.getElementById("cdx-branch");
  const statusEl = document.getElementById("cdx-status");
  const result = document.getElementById("cdx-result");
  let pollTimer = null;
  const stop = () => { if (pollTimer) { clearInterval(pollTimer); pollTimer = null; } };
  const branch = () => branchSel.value;

  function renderStatus(s) {
    const rb = s.rebuild || {};
    const stale = s.parser_version < s.current_parser_version;
    const rows = [
      ["Entries", Number(s.entry_count || 0).toLocaleString()],
      ["Parser version", `${s.parser_version}${stale ? ` → ${s.current_parser_version} (rebuild pending on next sync)` : " · current"}`],
    ];
    if (rb.running) rows.push(["Rebuild", "running…"]);
    else if (rb.error) rows.push(["Last rebuild", `failed: ${rb.error}`]);
    else if (rb.counts) rows.push(["Last rebuild", `indexed ${Number(rb.counts.indexed || 0).toLocaleString()} · removed ${rb.counts.removed || 0}`]);
    statusEl.innerHTML = rows.map(([k, v]) =>
      `<div class="row" style="gap:10px;padding:5px 0;border-bottom:1px solid var(--border)">
         <span class="muted" style="flex:0 0 130px">${esc(k)}</span><span style="flex:1">${esc(String(v))}</span></div>`).join("");
    return !!rb.running;
  }

  async function loadStatus() {
    if (!document.body.contains(statusEl)) return stop();  // left the tab
    try {
      const s = await API.call(`/admin/codexes/status?branch=${encodeURIComponent(branch())}`);
      const running = renderStatus(s);
      if (running && !pollTimer) pollTimer = setInterval(loadStatus, 3000);
      if (!running) stop();
    } catch (ex) {
      statusEl.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`;
      stop();
    }
  }

  branchSel.addEventListener("change", () => {
    result.textContent = ""; result.className = "ingest-result"; stop(); loadStatus();
  });
  body.querySelector('[data-act="refresh"]').addEventListener("click", loadStatus);

  body.querySelector('[data-act="rebuild"]').addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    btn.disabled = true;
    result.className = "ingest-result";
    result.textContent = "Starting rebuild…";
    try {
      const data = await API.call(`/admin/codexes/rebuild?branch=${encodeURIComponent(branch())}`, { method: "POST" });
      result.className = "ingest-result ok";
      result.textContent = data.message || (data.started ? "Rebuild started." : "Rebuild already running.");
      toast(data.started ? "Codex rebuild started" : "Rebuild already running", "ok");
      loadStatus();
    } catch (ex) {
      result.className = "ingest-result err";
      result.textContent = ex.message;
      toast("Rebuild failed to start", "err");
    } finally {
      btn.disabled = false;
    }
  });

  loadStatus();
}

// --- Bot stats (master) ----------------------------------------------------
// Read-only view of the gateway bot's reach (servers + users it can see) and
// slash-command usage. Written by the bot (presence) + the interactions endpoint
// (per-command counts); served by GET /admin/bot/stats.
function _statBlock(label, value) {
  return `<div style="min-width:120px">
      <div style="font-size:30px;font-weight:700;line-height:1.1">${value}</div>
      <div class="muted" style="font-size:12px;margin-top:2px">${esc(label)}</div>
    </div>`;
}

async function renderBotStats() {
  const body = document.getElementById("tab-body");
  body.innerHTML = `
    <div class="card">
      <h2 style="margin:0 0 6px">Bot statistics</h2>
      <p class="hint" style="margin:0 0 12px">
        Live reach of the Kiwi gateway bot and how often each slash command runs.
        Presence is refreshed by the bot each minute; command counts persist in the database.
      </p>
      <button class="btn" data-act="refresh">Refresh</button>
    </div>
    <div id="botstats-body"><div class="loading">Loading…</div></div>`;

  const el = document.getElementById("botstats-body");
  const fmt = (n) => (typeof n === "number" ? n.toLocaleString() : "—");

  async function load() {
    el.innerHTML = `<div class="loading">Loading…</div>`;
    try {
      const data = await API.call("/admin/bot/stats");
      const when = data.updated_at ? new Date(data.updated_at).toLocaleString() : "never";
      const rows = (data.commands || []).length
        ? data.commands.map((c) =>
            `<div class="row" style="align-items:baseline;gap:10px;padding:7px 0;border-bottom:1px solid var(--border)">
               <code style="flex:0 0 auto">/${esc(c.name)}</code>
               <span style="flex:1"></span>
               <strong>${fmt(c.count)}</strong>
             </div>`).join("")
        : `<p class="muted" style="margin:0">No commands used yet.</p>`;
      el.innerHTML = `
        <div class="card">
          <div class="row" style="gap:32px;flex-wrap:wrap">
            ${_statBlock("Servers", fmt(data.guild_count))}
            ${_statBlock("Users it can see", fmt(data.member_count))}
            ${_statBlock("Commands used", fmt(data.total_commands))}
          </div>
          <p class="hint" style="margin:14px 0 0">Presence updated: ${esc(when)}</p>
        </div>
        <div class="card">
          <h2 style="margin:0 0 10px">Command usage</h2>
          ${rows}
        </div>`;
    } catch (ex) {
      el.innerHTML = `<div class="card"><p class="err-text">${esc(ex.message)}</p></div>`;
    }
  }
  body.querySelector('[data-act="refresh"]').addEventListener("click", load);
  load();
}


// --- Supporters (master) ---------------------------------------------------
// CRUD over the public credits list shown on /support and exposed tokenless at
// /v1/misc/supporters. Master-only (router-level superuser dep).
async function renderSupporters() {
  const body = document.getElementById("tab-body");
  body.innerHTML = `
    <div class="card">
      <h2 style="margin:0 0 6px">Supporters</h2>
      <p class="hint" style="margin:0">
        The credits list shown on
        <a href="https://trove.aallyn.net/support" target="_blank" rel="noopener">trove.aallyn.net/support</a>
        and exposed tokenless at <code>/v1/misc/supporters</code>. Names appear in the order you add them.
      </p>
    </div>
    <div class="card">
      <div class="row" style="align-items:center">
        <input type="text" id="supporter-name" placeholder="Supporter name" autocomplete="off" spellcheck="false">
        <button class="btn primary" data-act="add">Add</button>
      </div>
      <div id="supporter-result" class="ingest-result"></div>
    </div>
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px">
        <h2 style="flex:1;margin:0">Current list</h2>
        <button type="button" class="btn small" data-act="refresh">Refresh</button>
      </div>
      <div id="supporter-list"><div class="loading">Loading…</div></div>
    </div>`;

  const listEl = document.getElementById("supporter-list");
  const result = document.getElementById("supporter-result");
  const input = document.getElementById("supporter-name");

  async function load() {
    listEl.innerHTML = `<div class="loading">Loading…</div>`;
    try {
      const data = await API.call("/admin/supporters");
      if (!data.count) {
        listEl.innerHTML = `<p class="muted" style="margin:0">No supporters yet — add one above.</p>`;
        return;
      }
      listEl.innerHTML = data.items.map((s) =>
        `<div class="row" style="align-items:center;gap:10px;padding:7px 0;border-bottom:1px solid var(--border)">
           <span style="flex:1;font-weight:600">${esc(s.name)}</span>
           <button class="btn small" data-remove="${esc(s.name)}">Remove</button>
         </div>`).join("");
      listEl.querySelectorAll("[data-remove]").forEach((b) =>
        b.addEventListener("click", () => remove(b.dataset.remove)));
    } catch (ex) {
      listEl.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`;
    }
  }

  async function add() {
    const name = input.value.trim();
    if (!name) return;
    result.className = "ingest-result";
    result.textContent = "Adding…";
    try {
      await API.call("/admin/supporters", { method: "POST", body: { name } });
      result.className = "ingest-result ok";
      result.textContent = `Added ${name}.`;
      input.value = "";
      toast(`Added ${name}`, "ok");
      load();
    } catch (ex) {
      result.className = "ingest-result err";
      result.textContent = ex.message;
    }
  }

  async function remove(name) {
    if (!window.confirm(`Remove "${name}" from supporters?`)) return;
    try {
      await API.call(`/admin/supporters/${encodeURIComponent(name)}`, { method: "DELETE" });
      toast(`Removed ${name}`, "ok");
      load();
    } catch (ex) {
      toast(ex.message, "err");
    }
  }

  document.querySelector('[data-act="add"]').addEventListener("click", add);
  document.querySelector('[data-act="refresh"]').addEventListener("click", load);
  input.addEventListener("keydown", (e) => { if (e.key === "Enter") add(); });
  load();
}

async function renderClaims() {
  const body = document.getElementById("tab-body");
  body.innerHTML = `
    <div class="card">
      <h2 style="margin:0 0 6px">Trove name claims</h2>
      <p class="hint" style="margin:0">
        Players claim an in-game name on the
        <a href="https://trove.aallyn.net/dashboard" target="_blank" rel="noopener">User Dashboard</a>.
        Verification is <strong>manual</strong> — approve a claim here to mark it verified,
        or reject it to release the name. There is no automatic ownership check.
      </p>
    </div>
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px;gap:14px">
        <h2 style="flex:1;margin:0">Pending claims</h2>
        <label class="row" style="gap:6px;align-items:center;font-size:.85rem;margin:0">
          <input type="checkbox" id="claims-show-all"> Show all
        </label>
        <button type="button" class="btn small" data-act="refresh">Refresh</button>
      </div>
      <div id="claims-list"><div class="loading">Loading…</div></div>
    </div>
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px;gap:10px">
        <h2 style="flex:1;margin:0">Username change requests</h2>
        <button type="button" class="btn small" data-act="uname-refresh">Refresh</button>
      </div>
      <p class="hint" style="margin:0 0 8px">Users request to change their frozen <strong>Trove username</strong>
        (their mod handle). <strong>Approve</strong> renames the account + re-homes their mod/modpack URLs;
        <strong>Reject</strong> asks for a reason shown to the user.</p>
      <div id="uname-list"><div class="loading">Loading…</div></div>
    </div>`;

  const listEl = document.getElementById("claims-list");
  const showAll = document.getElementById("claims-show-all");

  async function load() {
    listEl.innerHTML = `<div class="loading">Loading…</div>`;
    try {
      const data = await API.call(`/admin/site-claims?pending_only=${showAll.checked ? "false" : "true"}`);
      if (!data.count) {
        listEl.innerHTML = `<p class="muted" style="margin:0">No ${showAll.checked ? "" : "pending "}claims.</p>`;
        return;
      }
      listEl.innerHTML = data.items.map((c) => {
        const when = c.claimed_at ? new Date(c.claimed_at).toLocaleString() : "—";
        const status = c.claim_verified
          ? `<span style="color:#5dd078;font-size:.74rem;font-weight:700">✓ verified</span>`
          : `<span style="color:#f9c74f;font-size:.74rem;font-weight:700">pending</span>`;
        const actions = c.claim_verified
          ? `<button class="btn small" data-reject="${esc(c.user_id)}">Release</button>`
          : `<button class="btn small primary" data-approve="${esc(c.user_id)}">Approve</button>
             <button class="btn small" data-reject="${esc(c.user_id)}">Reject</button>`;
        return `<div class="row" style="align-items:center;gap:10px;padding:9px 0;border-bottom:1px solid var(--border)">
            <div style="flex:1;min-width:0">
              <div style="font-weight:600">${esc(c.claimed_trove_display || c.claimed_trove_name || "—")} &nbsp;${status}</div>
              <div class="muted" style="font-size:.8rem">${esc(c.display_name || c.username)} · claimed ${esc(when)}</div>
            </div>
            ${actions}
          </div>`;
      }).join("");
      listEl.querySelectorAll("[data-approve]").forEach((b) =>
        b.addEventListener("click", () => act(b.dataset.approve, "approve")));
      listEl.querySelectorAll("[data-reject]").forEach((b) =>
        b.addEventListener("click", () => act(b.dataset.reject, "reject")));
    } catch (ex) {
      listEl.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`;
    }
  }

  async function act(userId, kind) {
    if (kind === "reject" && !window.confirm("Reject this claim and release the name?")) return;
    try {
      await API.call(`/admin/site-claims/${encodeURIComponent(userId)}/${kind}`, { method: "POST" });
      toast(kind === "approve" ? "Claim approved" : "Claim released", "ok");
      load();
    } catch (ex) {
      toast(ex.message, "err");
    }
  }

  // ── Username change requests ──
  async function loadUnames() {
    const el = document.getElementById("uname-list");
    el.innerHTML = `<div class="loading">Loading…</div>`;
    try {
      const data = await API.call("/admin/username-requests?status=pending");
      if (!data.items || !data.items.length) { el.innerHTML = `<p class="muted" style="margin:0">No pending requests.</p>`; return; }
      el.innerHTML = data.items.map((r) => {
        const when = r.created_at ? new Date(r.created_at).toLocaleString() : "—";
        return `<div class="row" style="align-items:center;gap:10px;padding:9px 0;border-bottom:1px solid var(--border)">
            <div style="flex:1;min-width:0">
              <div style="font-weight:600"><code>${esc(r.current_username)}</code> → <code>${esc(r.requested_username)}</code></div>
              <div class="muted" style="font-size:.8rem">requested ${esc(when)}</div>
            </div>
            <button class="btn small primary" data-uname-approve="${esc(r.id)}">Approve</button>
            <button class="btn small" data-uname-reject="${esc(r.id)}">Reject</button>
          </div>`;
      }).join("");
      el.querySelectorAll("[data-uname-approve]").forEach((b) =>
        b.addEventListener("click", () => unameAct(b.dataset.unameApprove, "approve")));
      el.querySelectorAll("[data-uname-reject]").forEach((b) =>
        b.addEventListener("click", () => unameAct(b.dataset.unameReject, "reject")));
    } catch (ex) { el.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`; }
  }
  async function unameAct(id, kind) {
    let reason = "";
    if (kind === "reject") {
      reason = window.prompt("Reason for denying this username change (shown to the user):", "");
      if (reason === null) return;
    }
    try {
      await API.call(`/admin/username-requests/${encodeURIComponent(id)}/${kind}`,
        kind === "reject" ? { method: "POST", body: { reason } } : { method: "POST" });
      toast(kind === "approve" ? "Username changed" : "Request rejected", "ok");
      loadUnames();
    } catch (ex) { toast(ex.message, "err"); }
  }

  document.querySelector('[data-act="refresh"]').addEventListener("click", load);
  document.querySelector('[data-act="uname-refresh"]').addEventListener("click", loadUnames);
  showAll.addEventListener("change", load);
  load();
  loadUnames();
}

// --- Mods hub moderation (master) ------------------------------------------
// Users report a shared mod from its /mods/{slug} page; masters triage the
// reports here. "Take down" drops the project from all public listings + detail
// reads (the owner still sees it, flagged); "Restore" reverses it. Backed by
// /admin/mods/* (see app/admin/router.py).

async function renderModsModeration() {
  const body = document.getElementById("tab-body");
  body.innerHTML = `
    <div class="card">
      <h2 style="margin:0 0 6px">Mods hub moderation</h2>
      <p class="hint" style="margin:0">
        Reports filed against shared mods on the
        <a href="https://trove.aallyn.net/mods" target="_blank" rel="noopener">Mods Hub</a>.
        <strong>Take down</strong> hides a project from all public listings and detail reads
        (the owner still sees it, flagged); <strong>Restore</strong> reverses it.
      </p>
    </div>
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px;gap:14px">
        <h2 style="flex:1;margin:0">Open reports</h2>
        <label class="row" style="gap:6px;align-items:center;font-size:.85rem;margin:0">
          <input type="checkbox" id="mods-show-resolved"> Show resolved
        </label>
        <button type="button" class="btn small" data-act="refresh">Refresh</button>
      </div>
      <div id="mods-reports"><div class="loading">Loading…</div></div>
    </div>
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px;gap:10px">
        <h2 style="flex:1;margin:0">All projects</h2>
        <input type="text" id="mods-q" placeholder="Search title/tags" style="flex:1">
        <input type="text" id="mods-owner" placeholder="Owner" style="width:130px">
        <select id="mods-vis" style="width:120px">
          <option value="">Any visibility</option>
          <option value="public">Public</option>
          <option value="unlisted">Unlisted</option>
          <option value="draft">Draft</option>
        </select>
        <select id="mods-sort" style="width:150px">
          <option value="updated">Recently updated</option>
          <option value="created">Newest</option>
          <option value="popularity">Most popular</option>
          <option value="downloads">Most downloads</option>
          <option value="stars">Most stars</option>
          <option value="size">Largest on disk</option>
        </select>
        <button type="button" class="btn small" data-act="search">Search</button>
      </div>
      <div id="mods-projects"><div class="loading">Loading…</div></div>
    </div>
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px;gap:10px">
        <h2 style="flex:1;margin:0">Stray mod import</h2>
        <button type="button" class="btn small" data-act="stray-refresh">Refresh</button>
      </div>
      <p class="hint" style="margin:0 0 10px">Import <strong>stray</strong> (unclaimed) mods.
        <strong>Bulk import</strong> brings everything in, visible; <strong>Resync</strong> refreshes download
        counts + changed files and queues any newly-found mods for approval below.</p>
      <div id="stray-state" class="muted" style="font-size:.85rem;margin-bottom:8px">Loading…</div>
      <div class="row" style="gap:8px">
        <button type="button" class="btn small" data-act="stray-import">Bulk import</button>
        <button type="button" class="btn small" data-act="stray-resync">Resync</button>
      </div>
    </div>
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px;gap:10px">
        <h2 style="flex:1;margin:0">Stray mods</h2>
        <input type="text" id="stray-q" placeholder="Search title/author" style="flex:1">
        <select id="stray-status" style="width:160px">
          <option value="approved">Approved</option>
          <option value="pending">Pending approval</option>
          <option value="rejected">Rejected</option>
          <option value="">All unclaimed</option>
        </select>
        <button type="button" class="btn small" data-act="stray-pending-refresh">Refresh</button>
      </div>
      <p class="hint" style="margin:0 0 8px">Imported, unclaimed mods. Tick rows to <strong>assign many at once</strong> to a user by their ID, or use the per-row actions. Pending mods also show Approve / Reject.</p>
      <div id="stray-pending"><div class="loading">Loading…</div></div>
    </div>
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px;gap:10px">
        <h2 style="flex:1;margin:0">Mod claims</h2>
        <button type="button" class="btn small" data-act="claims-refresh">Refresh</button>
      </div>
      <p class="hint" style="margin:0 0 8px">Users requesting to claim a stray mod. <strong>Approve</strong> hands the mod over (it becomes their regular mod).</p>
      <div id="mod-claims"><div class="loading">Loading…</div></div>
    </div>
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px;gap:10px">
        <h2 style="flex:1;margin:0">Modpacks</h2>
        <input type="text" id="modpacks-q" placeholder="Search title/tags" style="flex:1">
        <input type="text" id="modpacks-owner" placeholder="Owner" style="width:130px">
        <select id="modpacks-vis" style="width:120px">
          <option value="">Any visibility</option>
          <option value="public">Public</option>
          <option value="unlisted">Unlisted</option>
          <option value="draft">Draft</option>
        </select>
        <button type="button" class="btn small" data-act="modpacks-search">Search</button>
      </div>
      <p class="hint" style="margin:0 0 8px">User-curated bundles of mods. <strong>Take down</strong> hides one from public view; <strong>Delete</strong> removes it.</p>
      <div id="modpacks-list"><div class="loading">Loading…</div></div>
    </div>`;

  const listEl = document.getElementById("mods-reports");
  const showResolved = document.getElementById("mods-show-resolved");
  const projEl = document.getElementById("mods-projects");

  async function load() {
    listEl.innerHTML = `<div class="loading">Loading…</div>`;
    try {
      const data = await API.call(`/admin/mods/reports?resolved=${showResolved.checked ? "true" : "false"}`);
      if (!data.items || !data.items.length) {
        listEl.innerHTML = `<p class="muted" style="margin:0">No ${showResolved.checked ? "resolved " : "open "}reports.</p>`;
        return;
      }
      listEl.innerHTML = data.items.map((r) => {
        const when = r.created_at ? new Date(r.created_at).toLocaleString() : "—";
        return `<div class="row" style="align-items:flex-start;gap:10px;padding:9px 0;border-bottom:1px solid var(--border)">
            <div style="flex:1;min-width:0">
              <div style="font-weight:600">
                <a href="https://trove.aallyn.net/mods/${encodeURIComponent(r.project_handle || "")}/${encodeURIComponent(r.project_slug)}" target="_blank" rel="noopener">${esc(r.project_slug)}</a>
                ${r.resolved ? '<span style="color:#5dd078;font-size:.74rem;font-weight:700">· resolved</span>' : ''}
              </div>
              <div class="muted" style="font-size:.8rem">by ${esc(r.reporter_username)} · ${esc(when)}</div>
              <div style="font-size:.86rem;margin-top:3px">${esc(r.reason)}</div>
            </div>
            <div class="row" style="gap:6px;flex:0 0 auto">
              ${r.resolved ? "" : `<button class="btn small" data-dismiss="${esc(r.id)}" data-label="${esc(r.project_slug)}">Dismiss</button>`}
              <button class="btn small danger" data-takedown="${esc(r.project_id)}" data-label="${esc(r.project_slug)}">Take down</button>
            </div>
          </div>`;
      }).join("");
      listEl.querySelectorAll("[data-takedown]").forEach((b) =>
        b.addEventListener("click", () => takedown(b.dataset.takedown, b.dataset.label)));
      listEl.querySelectorAll("[data-dismiss]").forEach((b) =>
        b.addEventListener("click", () => dismissReport(b.dataset.dismiss, b.dataset.label)));
    } catch (ex) {
      listEl.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`;
    }
  }

  async function dismissReport(reportId, label = "") {
    if (!window.confirm(`Dismiss the report against "${label || reportId}"? The mod stays up; the report is marked resolved.`)) return;
    try {
      await API.call(`/admin/mods/reports/${encodeURIComponent(reportId)}/dismiss`, { method: "POST" });
      toast("Report dismissed", "ok");
      load();
    } catch (ex) { toast(ex.message, "err"); }
  }

  async function takedown(id, label = "", reason = "") {
    if (!window.confirm(`Take down "${label || id}"? It will disappear from public view.`)) return;
    try {
      await API.call(`/admin/mods/projects/${encodeURIComponent(id)}/takedown`,
        { method: "POST", body: { reason } });
      toast("Mod taken down", "ok");
      load();
    } catch (ex) { toast(ex.message, "err"); }
  }

  async function restore(id) {
    try {
      await API.call(`/admin/mods/projects/${encodeURIComponent(id)}/restore`, { method: "POST" });
      toast("Mod restored", "ok");
      load();
    } catch (ex) { toast(ex.message, "err"); }
  }

  async function loadProjects() {
    projEl.innerHTML = `<div class="loading">Loading…</div>`;
    const params = new URLSearchParams();
    const q = document.getElementById("mods-q").value.trim();
    const owner = document.getElementById("mods-owner").value.trim();
    const vis = document.getElementById("mods-vis").value;
    const sort = document.getElementById("mods-sort").value;
    if (q) params.set("q", q);
    if (owner) params.set("owner", owner);
    if (vis) params.set("visibility", vis);
    if (sort) params.set("sort", sort);
    try {
      const data = await API.call(`/admin/mods/projects?${params.toString()}`);
      if (!data.items || !data.items.length) {
        projEl.innerHTML = `<p class="muted" style="margin:0">No projects.</p>`;
        return;
      }
      projEl.innerHTML = `<p class="muted" style="margin:0 0 6px;font-size:.8rem">${data.total} total</p>` +
        data.items.map((p) => {
          const flags = `<span class="badge muted">${esc(p.visibility)}</span>` +
            (p.taken_down ? ' <span class="badge warn">taken down</span>' : '');
          const tdBtn = p.taken_down
            ? `<button class="btn small" data-restore="${esc(p.id)}">Restore</button>`
            : `<button class="btn small" data-takedown="${esc(p.id)}" data-label="${esc(p.slug)}">Take down</button>`;
          const assignBtn = p.is_stray
            ? `<button class="btn small" data-assign="${esc(p.id)}" data-label="${esc(p.title)}">Assign…</button>` : "";
          return `<div class="row" style="align-items:center;gap:10px;padding:9px 0;border-bottom:1px solid var(--border)">
              <div style="flex:1;min-width:0">
                <div style="font-weight:600">
                  <a href="https://trove.aallyn.net/mods/${encodeURIComponent(p.handle || "")}/${encodeURIComponent(p.slug)}" target="_blank" rel="noopener">${esc(p.title)}</a> ${flags}
                </div>
                <div class="muted" style="font-size:.8rem">${esc(p.handle || "?")}/${esc(p.slug)} · by ${esc(p.owner_username)} · ${Number(p.download_count || 0)} downloads · ${Number(p.star_count || 0)}★ · ${formatBytes(Number(p.size_bytes || 0))} · updated ${fmtDay(p.updated_at)}</div>
              </div>
              ${assignBtn}
              ${tdBtn}
              <button class="btn small danger" data-delete="${esc(p.id)}" data-label="${esc(p.slug)}">Delete</button>
            </div>`;
        }).join("");
      projEl.querySelectorAll("[data-assign]").forEach((b) =>
        b.addEventListener("click", () => assignStray([b.dataset.assign], b.dataset.label)));
      projEl.querySelectorAll("[data-takedown]").forEach((b) =>
        b.addEventListener("click", () => takedown(b.dataset.takedown, b.dataset.label).then(loadProjects)));
      projEl.querySelectorAll("[data-restore]").forEach((b) =>
        b.addEventListener("click", () => restore(b.dataset.restore).then(loadProjects)));
      projEl.querySelectorAll("[data-delete]").forEach((b) =>
        b.addEventListener("click", () => deleteProject(b.dataset.delete, b.dataset.label)));
    } catch (ex) {
      projEl.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`;
    }
  }

  async function deleteProject(id, label = "") {
    if (!window.confirm(`Permanently delete "${label || id}"? This removes its branches, commits and releases.`)) return;
    try {
      await API.call(`/admin/mods/projects/${encodeURIComponent(id)}`, { method: "DELETE" });
      toast("Project deleted", "ok");
      loadProjects();
    } catch (ex) { toast(ex.message, "err"); }
  }

  // ── Stray (imported) mods: catalog import + approval queue + claims ──
  async function loadStrayState() {
    const el = document.getElementById("stray-state");
    try {
      const s = await API.call("/admin/mods/stray/import");
      const counts = `${s.processed}/${s.total} (imported ${s.imported}, updated ${s.updated}, pending ${s.pending_added}, failed ${s.failed})`;
      if (s.running) el.innerHTML = `<strong>Running…</strong> ${esc(s.phase)} · ${counts}`;
      else if (s.phase === "done") el.innerHTML = `Last run: done · ${counts}`;
      else if (s.phase === "error") el.innerHTML = `<span class="err-text">Last run errored: ${esc(s.last_error || "")}</span>`;
      else el.textContent = "Idle — no import run yet.";
    } catch (ex) { el.innerHTML = `<span class="err-text">${esc(ex.message)}</span>`; }
  }
  async function startImport(resync) {
    const msg = resync ? "Resync stray mods now?"
      : "Bulk-import all stray mods now? This mirrors every mod file and may take a while.";
    if (!window.confirm(msg)) return;
    try {
      const r = await API.call(`/admin/mods/stray/import?resync=${resync ? "true" : "false"}`, { method: "POST" });
      toast(r.started ? "Import started" : (r.reason || "Already running"), r.started ? "ok" : "err");
      loadStrayState();
    } catch (ex) { toast(ex.message, "err"); }
  }
  async function loadPending() {
    const el = document.getElementById("stray-pending");
    const statusEl = document.getElementById("stray-status");
    const status = statusEl ? statusEl.value : "approved";   // "" = all unclaimed
    const q = (document.getElementById("stray-q") || {}).value.trim();
    const label = status || "unclaimed";
    el.innerHTML = `<div class="loading">Loading…</div>`;
    try {
      const params = new URLSearchParams({ status, limit: "200" });
      if (q) params.set("q", q);
      const data = await API.call(`/admin/mods/stray?${params.toString()}`);
      if (!data.items || !data.items.length) {
        el.innerHTML = `<p class="muted" style="margin:0">No ${esc(label)} stray mods${q ? " matching your search" : ""}.</p>`;
        return;
      }
      el.innerHTML = `
        <div style="display:flex;align-items:center;gap:12px;margin-bottom:8px;flex-wrap:wrap">
          <label class="chk" style="margin:0;font-size:.82rem;flex:0 0 auto"><input type="checkbox" id="stray-all"> Select all</label>
          <span class="muted" style="font-size:.8rem;flex:1">${data.total} ${esc(label)}${data.total > data.items.length ? ` · showing ${data.items.length}` : ""}</span>
          <button type="button" class="btn small" id="stray-bulk-assign" disabled>Assign selected…</button>
        </div>` + data.items.map((p) => `
        <div style="display:flex;align-items:center;gap:10px;padding:9px 0;border-bottom:1px solid var(--border)">
          <input type="checkbox" class="stray-pick" style="width:auto;flex:0 0 auto;margin:0" data-id="${esc(p.id)}" data-label="${esc(p.title)}">
          <div style="flex:1;min-width:0">
            <div style="font-weight:600"><a href="https://trove.aallyn.net/mods/stray/${encodeURIComponent(p.slug)}" target="_blank" rel="noopener">${esc(p.title)}</a>
              ${p.stray_status && p.stray_status !== status ? `<span class="badge muted" style="font-size:.7rem">${esc(p.stray_status)}</span>` : ""}</div>
            <div class="muted" style="font-size:.8rem">by ${esc(p.author || "?")} · ${Number(p.download_count || 0)} downloads</div>
          </div>
          <button class="btn small" data-assign="${esc(p.id)}" data-label="${esc(p.title)}">Assign…</button>
          ${p.stray_status === "pending" ? `<button class="btn small" data-approve="${esc(p.id)}">Approve</button>
          <button class="btn small danger" data-reject="${esc(p.id)}">Reject</button>` : ""}
        </div>`).join("");
      const picks = () => Array.from(el.querySelectorAll(".stray-pick:checked")).map((c) => c.dataset.id);
      const bulkBtn = document.getElementById("stray-bulk-assign");
      const syncBulk = () => { const n = picks().length; bulkBtn.disabled = !n; bulkBtn.textContent = n ? `Assign ${n} selected…` : "Assign selected…"; };
      el.querySelectorAll(".stray-pick").forEach((c) => c.addEventListener("change", syncBulk));
      document.getElementById("stray-all").addEventListener("change", (e) => {
        el.querySelectorAll(".stray-pick").forEach((c) => { c.checked = e.target.checked; });
        syncBulk();
      });
      bulkBtn.addEventListener("click", () => { const ids = picks(); if (ids.length) assignStray(ids); });
      el.querySelectorAll("[data-approve]").forEach((b) => b.addEventListener("click", () => strayAction(b.dataset.approve, "approve")));
      el.querySelectorAll("[data-reject]").forEach((b) => b.addEventListener("click", () => strayAction(b.dataset.reject, "reject")));
      el.querySelectorAll("[data-assign]").forEach((b) => b.addEventListener("click", () => assignStray([b.dataset.assign], b.dataset.label)));
    } catch (ex) { el.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`; }
  }
  async function strayAction(id, action) {
    try {
      await API.call(`/admin/mods/stray/${encodeURIComponent(id)}/${action}`, { method: "POST" });
      toast(action === "approve" ? "Approved" : "Rejected", "ok");
      loadPending();
    } catch (ex) { toast(ex.message, "err"); }
  }
  // Proactively hand one or more stray mods to a known modder by User ID - no claim
  // request. `ids` is an array of project ids; one entry = single assign.
  function assignStray(ids, label = "") {
    const many = ids.length > 1;
    const what = many ? `${ids.length} selected mods` : esc(label || ids[0]);
    modal(many ? "Assign mods to a user" : "Assign mod to a user", `
      <p class="hint">Hand <b>${what}</b> directly to a user by their database <b>User ID</b>. They stop being stray
        and become that user's mods (re-homed to <span class="mono">/mods/&lt;username&gt;/…</span>). No claim request.</p>
      <label>User ID</label>
      <input id="assign-uid" maxlength="32" autocomplete="off" placeholder="e.g. 6612ab9f…">
      <p class="field-help">The SiteUser database id — copy it from the <b>Dashboard users</b> tab (open a user → User ID).</p>
    `, async () => {
      const userId = document.getElementById("assign-uid").value.trim();
      if (!userId) throw new Error("Enter a User ID.");
      const res = await API.call(`/admin/mods/stray/assign`,
        { method: "POST", body: { user_id: userId, project_ids: ids } });
      const n = (res.assigned || []).length;
      const failed = (res.errors || []).length;
      const suffix = failed ? ` (${failed} skipped)` : "";
      toast(n ? `Assigned ${n} mod${n === 1 ? "" : "s"} to @${res.owner_handle}${suffix}`
              : `Nothing assigned${suffix}`, n ? "ok" : "err");
      loadPending(); loadProjects();
    }, "Assign");
  }
  async function loadClaims() {
    const el = document.getElementById("mod-claims");
    el.innerHTML = `<div class="loading">Loading…</div>`;
    try {
      const data = await API.call("/admin/mods/claims?status=pending");
      if (!data.items || !data.items.length) { el.innerHTML = `<p class="muted" style="margin:0">No pending claims.</p>`; return; }
      el.innerHTML = data.items.map((c) => {
        const when = c.created_at ? new Date(c.created_at).toLocaleString() : "—";
        return `<div class="row" style="align-items:flex-start;gap:10px;padding:9px 0;border-bottom:1px solid var(--border)">
            <div style="flex:1;min-width:0">
              <div style="font-weight:600">${esc(c.project_title || c.project_slug)}</div>
              <div class="muted" style="font-size:.8rem">claimed by ${esc(c.claimant_username)} · ${esc(when)}</div>
              ${c.message ? `<div style="font-size:.85rem;margin-top:3px">${esc(c.message)}</div>` : ""}
            </div>
            <button class="btn small" data-claim-approve="${esc(c.id)}">Approve</button>
            <button class="btn small danger" data-claim-reject="${esc(c.id)}">Reject</button>
          </div>`;
      }).join("");
      el.querySelectorAll("[data-claim-approve]").forEach((b) => b.addEventListener("click", () => claimAction(b.dataset.claimApprove, "approve")));
      el.querySelectorAll("[data-claim-reject]").forEach((b) => b.addEventListener("click", () => claimAction(b.dataset.claimReject, "reject")));
    } catch (ex) { el.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`; }
  }
  async function claimAction(id, action) {
    if (action === "approve" && !window.confirm("Approve this claim and hand the mod over to the user?")) return;
    try {
      await API.call(`/admin/mods/claims/${encodeURIComponent(id)}/${action}`, { method: "POST" });
      toast(action === "approve" ? "Handed over" : "Claim rejected", "ok");
      loadClaims();
      loadProjects();
    } catch (ex) { toast(ex.message, "err"); }
  }

  // ── Modpacks moderation ──
  async function loadModpacks() {
    const el = document.getElementById("modpacks-list");
    el.innerHTML = `<div class="loading">Loading…</div>`;
    const params = new URLSearchParams();
    const q = document.getElementById("modpacks-q").value.trim();
    const owner = document.getElementById("modpacks-owner").value.trim();
    const vis = document.getElementById("modpacks-vis").value;
    if (q) params.set("q", q);
    if (owner) params.set("owner", owner);
    if (vis) params.set("visibility", vis);
    try {
      const data = await API.call(`/admin/modpacks?${params.toString()}`);
      if (!data.items || !data.items.length) { el.innerHTML = `<p class="muted" style="margin:0">No modpacks.</p>`; return; }
      el.innerHTML = `<p class="muted" style="margin:0 0 6px;font-size:.8rem">${data.total} total</p>` + data.items.map((p) => {
        const flags = `<span class="badge muted">${esc(p.visibility)}</span>` + (p.taken_down ? ' <span class="badge warn">taken down</span>' : '');
        const tdBtn = p.taken_down
          ? `<button class="btn small" data-mp-restore="${esc(p.id)}">Restore</button>`
          : `<button class="btn small" data-mp-takedown="${esc(p.id)}" data-label="${esc(p.slug)}">Take down</button>`;
        return `<div class="row" style="align-items:center;gap:10px;padding:9px 0;border-bottom:1px solid var(--border)">
            <div style="flex:1;min-width:0">
              <div style="font-weight:600"><a href="https://trove.aallyn.net/modpacks/${encodeURIComponent(p.handle || "")}/${encodeURIComponent(p.slug)}" target="_blank" rel="noopener">${esc(p.title)}</a> ${flags}</div>
              <div class="muted" style="font-size:.8rem">${esc(p.handle || "?")}/${esc(p.slug)} · by ${esc(p.owner_username)} · ${Number(p.mod_count || 0)} mods · ${Number(p.download_count || 0)} downloads</div>
            </div>
            ${tdBtn}
            <button class="btn small danger" data-mp-delete="${esc(p.id)}" data-label="${esc(p.slug)}">Delete</button>
          </div>`;
      }).join("");
      el.querySelectorAll("[data-mp-takedown]").forEach((b) => b.addEventListener("click", () => modpackAction(b.dataset.mpTakedown, "takedown", b.dataset.label)));
      el.querySelectorAll("[data-mp-restore]").forEach((b) => b.addEventListener("click", () => modpackAction(b.dataset.mpRestore, "restore")));
      el.querySelectorAll("[data-mp-delete]").forEach((b) => b.addEventListener("click", () => deleteModpack(b.dataset.mpDelete, b.dataset.label)));
    } catch (ex) { el.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`; }
  }
  async function modpackAction(id, action, label = "") {
    if (action === "takedown" && !window.confirm(`Take down "${label || id}"? It will disappear from public view.`)) return;
    try {
      await API.call(`/admin/modpacks/${encodeURIComponent(id)}/${action}`, { method: "POST" });
      toast(action === "takedown" ? "Modpack taken down" : "Modpack restored", "ok");
      loadModpacks();
    } catch (ex) { toast(ex.message, "err"); }
  }
  async function deleteModpack(id, label = "") {
    if (!window.confirm(`Permanently delete modpack "${label || id}"? This cannot be undone.`)) return;
    try {
      await API.call(`/admin/modpacks/${encodeURIComponent(id)}`, { method: "DELETE" });
      toast("Modpack deleted", "ok");
      loadModpacks();
    } catch (ex) { toast(ex.message, "err"); }
  }

  document.querySelector('[data-act="refresh"]').addEventListener("click", load);
  document.querySelector('[data-act="search"]').addEventListener("click", loadProjects);
  document.getElementById("mods-sort").addEventListener("change", loadProjects);
  document.querySelector('[data-act="stray-refresh"]').addEventListener("click", loadStrayState);
  document.querySelector('[data-act="stray-import"]').addEventListener("click", () => startImport(false));
  document.querySelector('[data-act="stray-resync"]').addEventListener("click", () => startImport(true));
  document.querySelector('[data-act="stray-pending-refresh"]').addEventListener("click", loadPending);
  document.getElementById("stray-status").addEventListener("change", loadPending);
  let _strayQT;
  document.getElementById("stray-q").addEventListener("input", () => { clearTimeout(_strayQT); _strayQT = setTimeout(loadPending, 300); });
  document.querySelector('[data-act="claims-refresh"]').addEventListener("click", loadClaims);
  document.querySelector('[data-act="modpacks-search"]').addEventListener("click", loadModpacks);
  showResolved.addEventListener("change", load);
  load();
  loadProjects();
  loadStrayState();
  loadPending();
  loadClaims();
  loadModpacks();
}

async function renderIngest() {
  const body = document.getElementById("tab-body");
  const cardHtml = INGEST_KINDS.map((k) => `
    <div class="card ingest-card" data-kind="${esc(k.key)}">
      <h3 style="margin:0 0 4px">${esc(k.title)}</h3>
      <p class="hint" style="margin:0 0 10px">${esc(k.description)}</p>
      <p class="hint" style="margin:0 0 14px"><strong>Source:</strong> <code>${esc(k.cfg)}</code></p>
      <div class="row" style="align-items:center">
        <label class="btn small" style="flex:0 0 auto;margin:0;cursor:pointer">
          Choose .cfg…
          <input type="file" accept=".cfg,.txt,text/plain" style="display:none" data-act="file">
        </label>
        <span class="muted ingest-filename" data-act="filename">No file chosen</span>
      </div>
      ${k.timestampField ? `
        <label style="display:block;margin-top:12px">
          <span class="muted" style="font-size:.78rem;display:block;margin-bottom:4px">
            Anchor override (unix seconds, optional - for back-fills)
          </span>
          <input type="number" data-act="timestamp" placeholder="e.g. 1780830000" style="width:100%">
        </label>` : ""}
      <div class="row" style="margin-top:14px;justify-content:flex-end">
        <button class="btn primary small" data-act="submit" disabled>Insert</button>
      </div>
      <div data-act="result" class="ingest-result"></div>
    </div>
  `).join("");

  body.innerHTML = `
    <div class="card">
      <h2 style="margin:0 0 6px">Manual cfg ingest</h2>
      <p class="hint" style="margin:0">
        Replay a captured cfg through the same ingest path the bot uses.
        Useful for back-fills, recovering missed captures, or testing the
        server side without running the bot. Every endpoint here is
        master-only and your session token is used directly.
      </p>
    </div>
    <div class="ingest-grid">${cardHtml}</div>
    <div class="card" id="backlog-card">
      <h2 style="margin:0 0 6px">Backlog re-ingest <span class="badge muted" style="font-weight:normal">no upload</span></h2>
      <p class="hint" style="margin:0 0 12px">
        Every dump the API receives is saved server-side (gzipped, keyed by anchor). Re-ingest the
        whole backlog here — the <strong>server</strong> reads from disk and paces itself, so there's
        no upload and no memory pile-up, and the heavy cheaters/activity compute runs <strong>once at
        the end</strong>. You can also drop <code>&lt;unix&gt;.cfg</code> files straight into the
        backlog folder on the host.
      </p>
      <div class="row" style="align-items:center;gap:12px;flex-wrap:wrap">
        <span class="muted" id="backlog-count">…</span>
        <label style="display:inline-flex;align-items:center;gap:6px;font-size:.85rem">
          <input type="checkbox" id="backlog-clear"> reset everything first
        </label>
        <button class="btn small" id="backlog-refresh" type="button" style="margin-left:auto">Refresh</button>
        <button class="btn primary small" id="backlog-go" type="button">Re-ingest backlog</button>
      </div>
      <div id="backlog-progress" style="margin-top:12px"></div>
    </div>
    <div class="card" id="bulk-lb-card">
      <h2 style="margin:0 0 6px">Bulk leaderboard back-fill</h2>
      <p class="hint" style="margin:0 0 12px">
        Select many LeaderBot cfg files at once — the anchor is read from each
        <strong>filename</strong> (the bot saves backlog files as <code>&lt;unix&gt;.cfg</code>).
        They're ingested oldest-first, one at a time, with the 14-day limit lifted so
        historical captures land at their real anchor.
      </p>
      <div class="row" style="align-items:center;gap:10px">
        <label class="btn small" style="flex:0 0 auto;margin:0;cursor:pointer">
          Choose .cfg files…
          <input type="file" accept=".cfg,.txt,text/plain" multiple style="display:none" id="bulk-lb-files">
        </label>
        <span class="muted" id="bulk-lb-count">No files chosen</span>
        <button class="btn primary small" id="bulk-lb-go" disabled style="margin-left:auto">Ingest</button>
      </div>
      <div id="bulk-lb-list" style="margin-top:12px"></div>
    </div>
    <div class="card" id="lb-reset-card" style="border:1px solid rgba(248,113,113,.45)">
      <h2 style="margin:0 0 6px;color:#f87171">Danger zone — reset leaderboards</h2>
      <p class="hint" style="margin:0 0 12px">
        Wipes <strong>all</strong> leaderboard entries (hot + archive), the activity history,
        and every cheater/activity cache. Board reset-cadence overrides are kept. This is
        irreversible — re-ingest from backlog to rebuild. Type <code>RESET</code> to enable.
      </p>
      <div class="row" style="align-items:center;gap:12px;flex-wrap:wrap">
        <input type="text" id="lb-reset-confirm" placeholder="Type RESET" autocomplete="off" spellcheck="false" style="flex:0 0 auto">
        <label style="display:inline-flex;align-items:center;gap:6px;font-size:.85rem">
          <input type="checkbox" id="lb-reset-drop-boards"> also drop board metadata
        </label>
        <button class="btn small danger" id="lb-reset-go" disabled style="margin-left:auto">Reset everything</button>
      </div>
      <div id="lb-reset-result" class="ingest-result"></div>
    </div>
    <div class="card" id="ingest-log-card">
      <div class="row" style="align-items:center;margin-bottom:6px">
        <h2 style="flex:1;margin:0">Recent submissions</h2>
        <button type="button" class="btn small" data-act="refresh-ingest-log">Refresh</button>
      </div>
      <p class="hint" style="margin:0 0 14px">
        Last 20 ingest calls (both bot and master submissions) - endpoint,
        when, who, and a summary of what landed. 30-day rolling history.
      </p>
      <div id="ingest-log-body"><div class="loading">Loading…</div></div>
    </div>`;

  for (const card of body.querySelectorAll("[data-kind]")) {
    const kind = INGEST_KINDS.find((k) => k.key === card.dataset.kind);
    wireIngestCard(card, kind);
  }
  wireBulkLeaderboards();
  wireBacklogReingest();
  wireLeaderboardReset();
  document.querySelector('[data-act="refresh-ingest-log"]')
    .addEventListener("click", renderIngestLog);
  renderIngestLog();
}

// Bulk leaderboard back-fill: many cfg files at once, each anchored by the unix
// timestamp in its filename (the bot's `backlog/<name>/<unix>.cfg`). Ingested
// oldest-first, sequentially, with ?backfill=true so the 14-day anchor limit
// is lifted for historical re-seeds. Each insert is 202-accepted; the actual
// boards/entries land in the ingest log below.
const _BULK_COL = { pending: "#9aa4b2", run: "#9aa4b2", ok: "#5dd078", err: "#f87171", skip: "#9aa4b2" };
const _BULK_ICON = { pending: "·", run: "…", ok: "✓", err: "✗", skip: "⊘" };

function parseAnchorFromName(name) {
  // Take the first plausible 10-digit unix-seconds value (2020 .. 2035) in the
  // filename - tolerates a prefix like `LeaderBot20k_1780830000.cfg`.
  for (const m of (name.match(/\d{9,11}/g) || [])) {
    const n = parseInt(m, 10);
    if (n >= 1577836800 && n <= 2051222400) return n;
  }
  return null;
}

function fmtAnchor(unix) {
  const d = new Date(unix * 1000);
  if (isNaN(d.getTime())) return String(unix);
  const p = (n) => String(n).padStart(2, "0");
  return `${d.getUTCFullYear()}-${p(d.getUTCMonth() + 1)}-${p(d.getUTCDate())} ${p(d.getUTCHours())}:${p(d.getUTCMinutes())} UTC`;
}

function renderBulkList(listEl, entries) {
  if (!entries.length) { listEl.innerHTML = ""; return; }
  listEl.innerHTML = entries.map((e) => `
    <div class="row" style="align-items:baseline;gap:10px;padding:4px 0;border-bottom:1px solid var(--border)">
      <span style="flex:0 0 14px;color:${_BULK_COL[e.status]}">${_BULK_ICON[e.status]}</span>
      <code style="flex:0 0 auto">${esc(e.file.name)}</code>
      <span class="muted" style="flex:1">${e.anchor != null ? esc(fmtAnchor(e.anchor)) : "— no anchor"}</span>
      <span style="flex:0 0 auto;color:${_BULK_COL[e.status]}">${esc(e.msg)}</span>
    </div>`).join("");
}

function wireBulkLeaderboards() {
  const filesInput = document.getElementById("bulk-lb-files");
  const countEl = document.getElementById("bulk-lb-count");
  const goBtn = document.getElementById("bulk-lb-go");
  const listEl = document.getElementById("bulk-lb-list");
  if (!filesInput) return;
  let entries = [];

  filesInput.addEventListener("change", () => {
    entries = [...filesInput.files].map((file) => ({
      file, anchor: parseAnchorFromName(file.name), status: "pending", msg: "",
    }));
    // Oldest-first so the warmer settles on the newest anchor at the end.
    entries.sort((a, b) => (a.anchor || 0) - (b.anchor || 0));
    const withAnchor = entries.filter((e) => e.anchor != null).length;
    const skipped = entries.length - withAnchor;
    countEl.textContent = entries.length
      ? `${entries.length} file(s) · ${withAnchor} with anchor${skipped ? ` · ${skipped} skipped` : ""}`
      : "No files chosen";
    goBtn.disabled = withAnchor === 0;
    goBtn.textContent = `Ingest ${withAnchor} file(s)`;
    renderBulkList(listEl, entries);
  });

  goBtn.addEventListener("click", async () => {
    filesInput.disabled = true;
    goBtn.disabled = true;
    for (const e of entries) {
      if (e.anchor == null) { e.status = "skip"; e.msg = "no anchor in filename"; renderBulkList(listEl, entries); continue; }
      e.status = "run"; e.msg = "ingesting…"; renderBulkList(listEl, entries);
      try {
        const fd = new FormData();
        fd.append("file", e.file);
        // sync=true → the request WAITS for the dump to fully persist, so the
        // client paces itself (one dump in memory at a time - no OOM pile-up).
        // warm=false → skip per-file warming; we warm once at the end.
        const r = await API.multipart("/v1/leaderboards/insert", fd, {
          query: { timestamp: String(e.anchor), backfill: "true", sync: "true", warm: "false" },
        });
        if (r && r.accepted === false) { e.status = "err"; e.msg = r.message || "failed"; }
        else { e.status = "ok"; e.msg = (r && r.message) || "ingested"; }
      } catch (ex) {
        e.status = "err"; e.msg = (ex && ex.message) || String(ex);
      }
      renderBulkList(listEl, entries);
    }
    const ok = entries.filter((e) => e.status === "ok").length;
    const failed = entries.filter((e) => e.status === "err").length;
    // Every file was a PURE insert (no per-file calc). Run the deferred
    // calculations ONCE now: warm the latest-anchor caches (cheaters + live
    // activity + page snapshots) and recompute the player- AND class-activity
    // histories from the re-seeded captures. All best-effort + background; don't
    // block the ack.
    if (ok) {
      try { await API.call("/v1/leaderboards/warm", { method: "POST" }); } catch (_) {}
      await Promise.allSettled([
        API.call("/v1/activity/backfill?total_days=730&force=true", { method: "POST" }),
        API.call("/v1/class-activity/backfill?total_days=730&force=true", { method: "POST" }),
      ]);
    }
    filesInput.disabled = false;
    goBtn.disabled = false;
    toast(`Bulk back-fill: ${ok} ingested${failed ? `, ${failed} failed` : ""} · recomputing`, failed ? "err" : "ok");
    renderIngestLog();
  });
}

// Backlog re-ingest: server-side replay of the saved dumps (no upload). Poll the
// status endpoint while it runs to show live progress; the poll auto-stops when
// the tab changes (the progress element is gone) or the run finishes.
let _backlogPoll = null;

function stopBacklogPoll() {
  if (_backlogPoll) { clearInterval(_backlogPoll); _backlogPoll = null; }
}

function renderBacklogProgress(s) {
  const cnt = document.getElementById("backlog-count");
  const el = document.getElementById("backlog-progress");
  if (!el) { stopBacklogPoll(); return; }
  if (cnt) cnt.textContent = `${s && s.backlog_files != null ? s.backlog_files : "?"} file(s) in backlog`;
  if (!s || (!s.running && !s.total)) {
    // Idle: surface WHERE the server is scanning so a missing bind-mount / wrong
    // path is obvious instead of a silent empty list.
    el.innerHTML = s && s.backlog_dir
      ? `<p class="hint" style="margin:0;font-size:.78rem">Scanning <code>${esc(s.backlog_dir)}/leaderboards/</code>${
          s.backlog_dir_exists ? "" : ` — <span class="err-text">folder not found in the container. Recreate the api container so the bind-mount + new code take effect (<code>./deploy.sh</code>).</span>`
        }</p>`
      : "";
    return;
  }
  const pct = s.total ? Math.round((s.done / s.total) * 100) : 0;
  const phase = s.running
    ? (s.phase === "recomputing" ? "recomputing (warmer + activity)…" : "ingesting…")
    : "done";
  const errs = (s.errors || []).slice(0, 6).map((e) =>
    `<div class="err-text" style="font-size:.78rem">anchor ${esc(String(e.anchor))}: ${esc(e.error)}</div>`).join("");
  el.innerHTML = `
    <div class="row" style="align-items:center;gap:10px">
      <div style="flex:1;height:8px;background:var(--panel-2,#1b212b);border-radius:99px;overflow:hidden">
        <div style="height:100%;width:${pct}%;background:var(--accent,#4cc9f0);transition:width .3s"></div>
      </div>
      <span class="muted" style="flex:0 0 auto">${s.done || 0}/${s.total || 0}</span>
    </div>
    <div class="muted" style="margin-top:6px;font-size:.82rem">
      ${esc(phase)} · ${s.ok || 0} ok · ${s.failed || 0} failed${s.last_anchor ? ` · last ${esc(fmtAnchor(s.last_anchor))}` : ""}
    </div>
    ${errs}`;
}

async function pollBacklog() {
  if (!document.getElementById("backlog-progress")) { stopBacklogPoll(); return; }
  let s;
  try { s = await API.call("/v1/leaderboards/reingest-status"); } catch (_) { return; }
  renderBacklogProgress(s);
  if (!s.running) {
    stopBacklogPoll();
    const go = document.getElementById("backlog-go");
    if (go) go.disabled = false;
  }
}

function startBacklogPoll() {
  stopBacklogPoll();
  _backlogPoll = setInterval(pollBacklog, 1500);
  pollBacklog();
}

function wireBacklogReingest() {
  const go = document.getElementById("backlog-go");
  const refresh = document.getElementById("backlog-refresh");
  const clearEl = document.getElementById("backlog-clear");
  if (!go) return;

  const loadOnce = async () => {
    try {
      const s = await API.call("/v1/leaderboards/reingest-status");
      renderBacklogProgress(s);
      if (s.running) { go.disabled = true; startBacklogPoll(); }
    } catch (ex) {
      const cnt = document.getElementById("backlog-count");
      const el = document.getElementById("backlog-progress");
      if (cnt) cnt.textContent = "status unavailable";
      if (el) el.innerHTML = `<p class="err-text" style="font-size:.8rem">${esc((ex && ex.message) || "request failed")} — the endpoint may not be live yet; recreate the api container (<code>./deploy.sh</code>).</p>`;
    }
  };

  go.addEventListener("click", async () => {
    const clear = clearEl && clearEl.checked;
    const msg = clear
      ? "RESET all leaderboard data, then re-ingest the entire backlog from scratch?"
      : "Re-ingest the entire backlog?";
    if (!window.confirm(msg)) return;
    go.disabled = true;
    try {
      const qs = clear ? "?clear_first=true" : "";
      const r = await API.call("/v1/leaderboards/reingest-backlog" + qs, { method: "POST" });
      if (!r.started) { toast(r.message || "Backlog is empty", "err"); go.disabled = false; return; }
      toast(`Re-ingesting ${r.files} backlog file(s)…`, "ok");
      startBacklogPoll();
    } catch (ex) {
      toast((ex && ex.message) || "Re-ingest failed to start", "err");
      go.disabled = false;
    }
  });
  refresh.addEventListener("click", loadOnce);
  loadOnce();
}

function wireLeaderboardReset() {
  const confirmEl = document.getElementById("lb-reset-confirm");
  const dropEl = document.getElementById("lb-reset-drop-boards");
  const goBtn = document.getElementById("lb-reset-go");
  const resultEl = document.getElementById("lb-reset-result");
  if (!confirmEl) return;
  const armed = () => confirmEl.value.trim().toUpperCase() === "RESET";
  confirmEl.addEventListener("input", () => { goBtn.disabled = !armed(); });
  goBtn.addEventListener("click", async () => {
    if (!window.confirm("Wipe ALL leaderboard data, activity history, and cheater caches? This cannot be undone.")) return;
    goBtn.disabled = true;
    resultEl.className = "ingest-result";
    resultEl.textContent = "Resetting…";
    try {
      const qs = dropEl && dropEl.checked ? "?drop_boards=true" : "";
      const r = await API.call("/v1/leaderboards/reset" + qs, { method: "POST" });
      resultEl.className = "ingest-result ok";
      resultEl.innerHTML = `<strong>✓ Reset.</strong> <code>${esc(JSON.stringify(r))}</code>`;
      confirmEl.value = "";
      toast("Leaderboards reset", "ok");
      renderIngestLog();
    } catch (ex) {
      resultEl.className = "ingest-result err";
      resultEl.textContent = (ex && ex.message) || String(ex);
    } finally {
      goBtn.disabled = !armed();
    }
  });
}

const INGEST_ENDPOINT_LABELS = {
  "/v1/leaderboards/insert": { label: "Leaderboards", cls: "ingest-log-lb" },
  "/v1/market/insert":       { label: "Market",       cls: "ingest-log-mkt" },
  "/v1/rotations/challenge/insert":   { label: "Challenge",   cls: "ingest-log-chl" },
  "/v1/rotations/chaos-chest/insert": { label: "Chaos Chest", cls: "ingest-log-cc" },
};

function fmtIngestSummary(endpoint, summary) {
  // Endpoint-specific short rendering. Falls through to a JSON dump for
  // unknown shapes so a future ingest type doesn't disappear silently.
  if (!summary) return "";
  switch (endpoint) {
    case "/v1/leaderboards/insert": {
      const parts = [];
      if (summary.boards != null)  parts.push(`${summary.boards} board(s)`);
      if (summary.entries != null) parts.push(`${summary.entries.toLocaleString()} entries`);
      if (summary.anchor)          parts.push(`anchor ${fmt(new Date(summary.anchor * 1000).toISOString())}`);
      if (summary.bytes != null)   parts.push(`${(summary.bytes / 1024).toFixed(0)} KB`);
      return parts.join(" · ");
    }
    case "/v1/market/insert": {
      const parts = [];
      if (summary.parsed != null)              parts.push(`parsed ${summary.parsed}`);
      if (summary.imported != null)            parts.push(`imported ${summary.imported}`);
      if (summary.ignored_not_in_list != null) parts.push(`skipped ${summary.ignored_not_in_list}`);
      if (summary.bytes != null)               parts.push(`${(summary.bytes / 1024).toFixed(0)} KB`);
      return parts.join(" · ");
    }
    case "/v1/rotations/challenge/insert":
    case "/v1/rotations/chaos-chest/insert": {
      const parts = [];
      if (summary.name) parts.push(`name = ${summary.name}`);
      if (summary.refreshed != null) parts.push(summary.refreshed ? "refreshed" : "new");
      return parts.join(" · ");
    }
    default:
      return `<code class="mono">${esc(JSON.stringify(summary))}</code>`;
  }
}

async function renderIngestLog() {
  const bodyEl = document.getElementById("ingest-log-body");
  if (!bodyEl) return;
  bodyEl.innerHTML = `<div class="loading">Loading…</div>`;
  let rows;
  try {
    rows = await API.call("/admin/ingest/log?limit=20");
  } catch (ex) {
    bodyEl.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`;
    return;
  }
  if (!rows.length) {
    bodyEl.innerHTML = `<p class="muted">No submissions yet - try uploading a cfg above.</p>`;
    return;
  }
  bodyEl.innerHTML = `
    <table>
      <thead><tr>
        <th>When</th><th>Endpoint</th><th>By</th><th>Summary</th><th>Status</th>
      </tr></thead>
      <tbody>
        ${rows.map((r) => {
          const ep = INGEST_ENDPOINT_LABELS[r.endpoint] || { label: r.endpoint, cls: "" };
          const status = r.success
            ? `<span class="badge ok">OK</span>`
            : `<span class="badge off" title="${esc(r.error || "")}">FAIL</span>`;
          const summaryRendered = fmtIngestSummary(r.endpoint, r.summary);
          // Auth-method indicator: small pill next to the email so an
          // operator can tell at a glance whether the bot pushed this
          // row or whether the master submitted through the portal.
          // Token submissions also include the token name (e.g. the
          // bot's "trove-bot") so multiple tokens can be distinguished.
          let authPill = "";
          if (r.auth_via === "token") {
            const label = r.token_name ? `bot: ${r.token_name}` : "bot";
            authPill = `<span class="badge ingest-log-via-token" title="Submitted via API token">${esc(label)}</span>`;
          } else {
            authPill = `<span class="badge muted" title="Submitted via portal session">portal</span>`;
          }
          return `
            <tr>
              <td class="muted" style="white-space:nowrap">${fmt(r.timestamp)}</td>
              <td><span class="badge ${ep.cls}">${esc(ep.label)}</span></td>
              <td style="font-size:.85rem">
                <div class="muted">${esc(r.user_email)}</div>
                <div style="margin-top:3px">${authPill}</div>
              </td>
              <td style="font-size:.85rem">${summaryRendered}</td>
              <td>${status}</td>
            </tr>`;
        }).join("")}
      </tbody>
    </table>`;
}

function wireIngestCard(card, kind) {
  const fileInput = card.querySelector('[data-act="file"]');
  const filenameEl = card.querySelector('[data-act="filename"]');
  const submitBtn = card.querySelector('[data-act="submit"]');
  const resultEl = card.querySelector('[data-act="result"]');
  const tsInput = card.querySelector('[data-act="timestamp"]');

  let file = null;
  fileInput.addEventListener("change", () => {
    file = fileInput.files[0] || null;
    filenameEl.textContent = file ? `${file.name} (${formatBytes(file.size)})` : "No file chosen";
    submitBtn.disabled = !file;
    resultEl.innerHTML = "";
    resultEl.className = "ingest-result";
  });

  submitBtn.addEventListener("click", async () => {
    if (!file) return;
    submitBtn.disabled = true;
    resultEl.className = "ingest-result";
    resultEl.innerHTML = `<span class="muted">Uploading…</span>`;
    try {
      let result;
      if (kind.mode === "multipart") {
        const fd = new FormData();
        fd.append("file", file);
        const query = tsInput && tsInput.value ? { timestamp: tsInput.value } : null;
        result = await API.multipart(kind.endpoint, fd, { query });
      } else {
        const name = await parseCfgValue(file, kind.parseKey);
        if (!name) {
          resultEl.className = "ingest-result err";
          resultEl.innerHTML = `Cfg did not contain a usable <code>${esc(kind.parseKey)} = …</code> line (saw nothing or <code>none</code>).`;
          submitBtn.disabled = false;
          return;
        }
        result = await API.call(kind.endpoint, { method: "POST", body: { name } });
      }
      resultEl.className = "ingest-result ok";
      resultEl.innerHTML = `<strong>✓ Inserted.</strong> <code>${esc(JSON.stringify(result))}</code>`;
      // Reflect the new row in the Recent submissions table without a
      // page reload - fire-and-forget so a render hiccup doesn't mask
      // the success ack the user just got.
      renderIngestLog();
    } catch (ex) {
      resultEl.className = "ingest-result err";
      resultEl.innerHTML = esc(ex.message || String(ex));
    } finally {
      submitBtn.disabled = false;
    }
  });
}

async function parseCfgValue(file, key) {
  const text = await file.text();
  const prefix = key + " = ";
  for (const line of text.split(/\r?\n/)) {
    if (line.startsWith(prefix)) {
      const v = line.slice(prefix.length).trim();
      if (!v || v === "none") return null;
      return v;
    }
  }
  return null;
}

function formatBytes(n) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(2)} MB`;
}


// --- Boot ------------------------------------------------------------------

const FALLBACK_CONFIG = {
  app_name: "Kiwi API", captcha_provider: "turnstile", captcha_sitekey: null,
  require_verified_for_tokens: true, scopes: [], token_creation_daily_limit: 3, revoke_reasons: [],
  github_oauth_enabled: false,
};

function downloadBlob(content, type, filename) {
  const url = URL.createObjectURL(new Blob([content], { type }));
  const a = document.createElement("a");
  a.href = url; a.download = filename; a.click();
  URL.revokeObjectURL(url);
}

function downloadJSON(obj, filename) {
  downloadBlob(JSON.stringify(obj, null, 2), "application/json", filename);
}

function downloadCSV(filename, headers, rows) {
  const cell = (v) => {
    const s = String(v ?? "");
    return /[",\n]/.test(s) ? '"' + s.replace(/"/g, '""') + '"' : s;
  };
  const csv = [headers.join(","), ...rows.map((r) => r.map(cell).join(","))].join("\n");
  downloadBlob(csv, "text/csv;charset=utf-8", filename);
}

// Handle the GitHub OAuth redirect landing on dev.aallyn.net/#oauth=<code>.
async function handleOAuthRedirect() {
  const hash = location.hash.slice(1);
  if (hash.startsWith("oauth=")) {
    const code = hash.slice(6);
    history.replaceState(null, "", location.pathname);
    try {
      const r = await API.call("/auth/oauth/exchange", { auth: false, method: "POST", body: { code } });
      API.setTokens(r.access_token, r.refresh_token);
      await loadDashboard();
      return true;
    } catch (_) { toast("GitHub sign-in failed.", "err"); }
  } else if (hash.startsWith("oauth_error=")) {
    history.replaceState(null, "", location.pathname);
    toast("GitHub sign-in failed (" + hash.slice(12) + ").", "err");
  }
  return false;
}

(async function init() {
  // Never let a bad/blocked /config response leave state.config null - the whole
  // SPA would crash. Always end up with a usable object.
  try {
    const cfg = await API.call("/config", { auth: false });
    state.config = (cfg && typeof cfg === "object") ? cfg : FALLBACK_CONFIG;
  } catch (_) {
    state.config = FALLBACK_CONFIG;
  }

  if (await handleOAuthRedirect()) return;

  if (API.token) {
    try { await loadDashboard(); return; }
    catch (_) { API.clear(); }
  }
  renderAuth("login");
})();
