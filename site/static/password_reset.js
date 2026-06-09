/* ═══════════════════════════════════════════════════════════════════════
   password_reset.js - wires /forgot-password and /reset-password
   ───────────────────────────────────────────────────────────────────────
   Loaded by both pages so the bundle is shared. Each form is wired
   only when its DOM is present, so the script is a no-op on the page
   that doesn't own it. Reads window.BTTAuth.API for the API base
   (set by site_auth.js).
   ═══════════════════════════════════════════════════════════════════════ */

(function () {
  'use strict';

  // ─── Helpers ───────────────────────────────────────────────────────
  const API = (window.BTTAuth && window.BTTAuth.API) || 'https://api.aallyn.net';

  async function postJSON(path, body) {
    const res = await fetch(API + path, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
    let data = null;
    try { data = await res.json(); } catch (_) { /* no body */ }
    return { ok: res.ok, status: res.status, data };
  }

  function t(s) {
    return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s;
  }

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

  // ─── /forgot-password ──────────────────────────────────────────────
  function wireForgotForm() {
    const $form = document.getElementById('forgot-form');
    if (!$form) return;
    const $email = document.getElementById('forgot-email');
    const $err = document.getElementById('forgot-error');
    const $ok = document.getElementById('forgot-success');
    const $submit = document.getElementById('forgot-submit');
    const $captcha = document.getElementById('forgot-captcha');

    // site_auth.js exposes mountCaptcha via window.BTTAuth when loaded
    // first. We let the JS run independently if site_auth.js hasn't
    // loaded yet (no-op getter).
    let getCaptchaToken = () => null;
    if ($captcha && window.BTTAuth && window.BTTAuth.mountCaptcha) {
      window.BTTAuth.mountCaptcha($captcha).then((fn) => { getCaptchaToken = fn; });
    }

    $form.addEventListener('submit', async (e) => {
      e.preventDefault();
      $err.hidden = true;
      $ok.hidden = true;
      $submit.disabled = true;
      try {
        const r = await postJSON('/v1/site-auth/forgot-password', {
          email: $email.value.trim(),
          captcha_token: getCaptchaToken(),
        });
        if (!r.ok) {
          // The endpoint is enumeration-safe, so most errors here are
          // either captcha-failed (later) or rate-limit. Surface what
          // the server says.
          $err.textContent = errorMessage(r.data) || t('Something went wrong. Try again in a minute.');
          $err.hidden = false;
          return;
        }
        $ok.textContent = (r.data && r.data.message) ||
          t("Check your inbox - if that email's registered, a reset link is on its way.");
        $ok.hidden = false;
        // Don't auto-redirect - the user needs to go to their email
        // client next, not back to /login.
      } finally {
        $submit.disabled = false;
      }
    });
  }

  // ─── /reset-password ───────────────────────────────────────────────
  function wireResetForm() {
    const $form = document.getElementById('reset-form');
    if (!$form) return;
    const $noToken = document.getElementById('reset-no-token');
    const $pw = document.getElementById('reset-password');
    const $confirm = document.getElementById('reset-password-confirm');
    const $err = document.getElementById('reset-error');
    const $ok = document.getElementById('reset-success');
    const $submit = document.getElementById('reset-submit');

    // Token comes from the email link's ``?token=...`` query param.
    const params = new URLSearchParams(location.search);
    const token = params.get('token');
    if (!token) {
      $form.hidden = true;
      if ($noToken) $noToken.hidden = false;
      return;
    }

    $form.addEventListener('submit', async (e) => {
      e.preventDefault();
      $err.hidden = true;
      $ok.hidden = true;
      if ($pw.value !== $confirm.value) {
        $err.textContent = t("The two passwords don't match.");
        $err.hidden = false;
        return;
      }
      $submit.disabled = true;
      try {
        const r = await postJSON('/v1/site-auth/reset-password', {
          token,
          new_password: $pw.value,
        });
        if (!r.ok) {
          $err.textContent = errorMessage(r.data) || t('Reset failed.');
          $err.hidden = false;
          return;
        }
        $ok.textContent = (r.data && r.data.message) ||
          t('Your password has been reset. Redirecting to sign in…');
        $ok.hidden = false;
        // Short pause so the user reads the success message, then
        // bounce to /login.
        setTimeout(() => { location.href = '/login'; }, 1800);
      } finally {
        $submit.disabled = false;
      }
    });
  }

  function boot() {
    wireForgotForm();
    wireResetForm();
  }
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', boot);
  } else {
    boot();
  }
})();
