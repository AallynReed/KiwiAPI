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

  async _call(path, { method = "GET", body = null, auth = true } = {}, allowRefresh = true) {
    const headers = {};
    if (body) headers["Content-Type"] = "application/json";
    if (auth && this.token) headers["Authorization"] = "Bearer " + this.token;
    const res = await fetch(API_BASE + path, { method, headers, body: body ? JSON.stringify(body) : undefined });
    let data = null;
    try { data = await res.json(); } catch (_) { /* no body */ }

    if (res.status === 401 && auth && allowRefresh) {
      if (await this._tryRefresh()) return this._call(path, { method, body, auth }, false);
      // Refresh failed — the session is truly gone. Bounce to login gracefully.
      this.clear();
      const wasLoggedIn = !!state.user;
      state.user = null;
      location.hash = "";
      renderAuth("login");
      if (wasLoggedIn) toast("Your session expired — please log in again.", "err");
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
function fmt(iso) { return iso ? new Date(iso).toLocaleString() : "—"; }
function fmtDay(iso) { return iso ? new Date(iso).toLocaleDateString() : "—"; }

// Local password-strength heuristic (no network — the server still does the
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
      // Mirrors EMAIL_SPAM_NOTICE in app/auth/router.py — signup returns the user
      // object (no message), so the spam guidance is added here on the portal side.
      toast("Account created — verify your email to finish signing up. Don't see it? "
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

const TABS = ["tokens", "activity", "account", "admin"];

function tabFromHash() {
  const h = location.hash.replace(/^#/, "");
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
  const u = state.user;
  const verified = u.is_verified
    ? '<span class="badge ok">verified</span>'
    : '<span class="badge warn">unverified</span>';
  // A non-admin landing on a stale "admin" tab falls back to tokens.
  if (state.tab === "admin" && !u.is_superuser) state.tab = "tokens";
  if (!TABS.includes(state.tab)) state.tab = "tokens";

  app.innerHTML = `
    <div class="topbar">
      <div class="brand"><span class="mark">◆</span> ${esc(state.config?.app_name || "Kiwi API")}</div>
      <div class="who">${esc(u.email)} ${verified} ${themeBtn()}
        <button class="btn small" id="logout">Log out</button></div>
    </div>
    <div class="container">
      <div class="nav-tabs" role="tablist">
        <button data-tab="tokens" role="tab">API tokens</button>
        <button data-tab="activity" role="tab">Activity</button>
        <button data-tab="account" role="tab">Account</button>
        ${u.is_superuser ? '<button data-tab="admin" role="tab">Admin</button>' : ""}
      </div>
      <div id="tab-body"></div>
    </div>`;
  document.getElementById("logout").addEventListener("click", async () => {
    try { await API.call("/auth/logout", { method: "POST", auth: false, body: { refresh_token: API.refresh } }); } catch (_) {}
    API.clear(); location.hash = ""; renderAuth("login");
  });
  // Tab clicks navigate via the URL hash, so the back button moves between tabs.
  app.querySelectorAll(".nav-tabs button").forEach((b) =>
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
  app.querySelectorAll(".nav-tabs button").forEach((b) => {
    const on = b.dataset.tab === state.tab;
    b.classList.toggle("active", on);
    b.setAttribute("aria-selected", on ? "true" : "false");
  });
  if (state.tab === "tokens") renderTokens();
  else if (state.tab === "activity") renderActivity();
  else if (state.tab === "admin") renderAdmin();
  else renderAccount();
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
  // Pinned IPs are stored HASHED — we can't show what's currently set, only
  // the count. Submitting REPLACES the whole list; submitting an empty box
  // drops every IP restriction on the token.
  const pinHint = token.allowed_ip_count
    ? `<span class="muted">${token.allowed_ip_count} pinned (hidden — IPs are hashed server-side)</span>`
    : `<span class="muted">none pinned</span>`;
  modal("Edit token", `
    <label>Name</label>
    <input id="edit-name" value="${esc(token.name)}" maxlength="80">
    <label>Allowed IPs <span class="muted">(optional — one per line. Replaces the whole list; leave empty to drop all pinning.)</span></label>
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
    toast("Token rotated — copy the new secret.", "ok");
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
      <td class="muted">${esc(t.revoke_reason || "—")}</td>
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

        <label>Allowed IPs <span class="muted">(optional — one exact IP per line. Stored hashed; you won't be able to see them again.)</span></label>
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
      <p class="hint">Send the token as a Bearer credential — there's no login call, the token <em>is</em> the credential.</p>
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
    if (slot) revealSecret(slot, token, "Rotated secret — copy it now, it won't be shown again:");
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
      preview.textContent = m === 0 ? "Mask: 0 — pick a scope, or enable All scopes" : `Mask: ${m}`;
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
          "New token — copy it now, it won't be shown again:");
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
        <h2 style="flex:1;margin:0">Usage — last ${days} days</h2>
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
        <td class="mono">${esc(s.ip || "—")}</td>
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
        <tr><td class="muted">Display name</td><td>${esc(u.display_name || "—")}
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
      // Returns fresh tokens — every other session was just logged out.
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

async function renderAdmin(days = 30) {
  const body = document.getElementById("tab-body");
  body.innerHTML = `<div class="loading">Loading admin…</div>`;
  let overview, users;
  try {
    [overview, users] = await Promise.all([
      API.call(`/admin/activity?days=${days}`),
      API.call(`/admin/users?days=${days}`),
    ]);
  } catch (ex) { body.innerHTML = `<p class="err-text">${esc(ex.message)}</p>`; return; }

  const emailById = Object.fromEntries(users.map((u) => [u.id, u.email]));
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
        <h2 style="flex:1;margin:0">Overview — last ${days} days</h2>
        <select id="admin-days" style="max-width:140px;flex:0 0 auto">
          <option value="1">24 hours</option><option value="7">7 days</option>
          <option value="30">30 days</option><option value="90">90 days</option>
        </select>
      </div>
      <div class="stat-grid">
        <div class="stat"><div class="n">${overview.total_requests}</div><div class="l">Requests</div></div>
        <div class="stat"><div class="n">${overview.error_count}</div><div class="l">Errors</div></div>
        <div class="stat"><div class="n">${overview.rate_limited}</div><div class="l">Rate-limit hits</div></div>
      </div>
    </div>
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px">
        <h2 style="flex:1;margin:0">Users (${users.length})</h2>
        <input id="user-search" placeholder="Search email…" style="max-width:220px;flex:0 0 auto">
      </div>
      <p class="hint">Click a user to see their tokens and activity.</p>
      <table>
        <thead><tr><th>User</th><th>Tokens</th><th>Requests</th><th>RL hits</th><th>Last used</th></tr></thead>
        <tbody id="user-rows"></tbody>
      </table>
    </div>
    <div class="card">
      <div class="row" style="align-items:center;margin-bottom:6px">
        <h2 style="flex:1;margin:0">Recent events</h2>
        <select id="ev-status" style="max-width:170px;flex:0 0 auto">
          <option value="">All statuses</option>
          <option value="429">429 — rate-limited</option>
          <option value="401">401</option><option value="403">403</option><option value="500">500</option>
        </select>
      </div>
      <table>
        <thead><tr><th>When</th><th>User</th><th>Method</th><th>Route</th><th>Status</th></tr></thead>
        <tbody id="ev-rows"></tbody>
      </table>
      <button class="btn small" id="ev-more" style="margin-top:12px;display:none">Load more</button>
    </div>`;

  const sel = document.getElementById("admin-days"); sel.value = String(days);
  sel.addEventListener("change", () => renderAdmin(Number(sel.value)));

  // Users: client-side email search + row click-through.
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

  // Events feed: cursor-paginated, optional status filter.
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

  document.getElementById("admin-back").addEventListener("click", () => renderAdmin());
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
  // Never let a bad/blocked /config response leave state.config null — the whole
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
