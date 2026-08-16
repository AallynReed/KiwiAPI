/* /dashboard page logic. Depends on site_auth.js (window.BTTAuth). Renders the
   profile + Trove name claim card, plus the mods/modpacks/discord/webhooks/dm
   sections. Client-side login gate only (no session / null /me → /login?next=…),
   so the page can serve from the static cache. */

(function () {
  'use strict';

  const { esc } = window.BTTUtil;

  const Auth = window.BTTAuth;
  if (!Auth) { console.error('[dashboard] site_auth.js missing'); return; }

  const $ = (id) => document.getElementById(id);
  const $loading = $('dash-loading');
  const $body = $('dash-body');
  const $avatar = $('dash-avatar');
  const $sideName = $('dash-side-name');
  const $wonBadge = $('dash-won-badge');
  const $gwList = $('dash-gw-list');
  const $profUsername = $('prof-username');
  const $profDisplayName = $('prof-display-name');
  const $profEditName = $('prof-edit-name');
  const $profNotify = $('prof-notify-email');
  const $profEditNotify = $('prof-edit-notify');
  const $profCreated = $('prof-created');
  const $logout = $('dash-logout');
  const $export = $('dash-export');
  const $deleteAcct = $('dash-delete');
  const $acctError = $('dash-account-error');
  const $troveTagUnverified = $('trove-tag-unverified');
  const $troveTagVerified = $('trove-tag-verified');
  const $claimState = $('trove-claim-state');
  const $claimForm = $('trove-claim-form');
  const $claimInput = $('trove-name-input');
  const $claimError = $('trove-claim-error');
  const $claimedState = $('trove-claimed-state');
  const $claimedName = $('trove-claimed-name');
  const $claimedWhen = $('trove-claimed-when');
  const $unclaim = $('trove-unclaim');
  const $verifyBlock = $('trove-verify-block');
  const $verifyBtn = $('trove-verify-btn');
  const $verifyBaseline = $('trove-verify-baseline');
  const $verifyWhen = $('trove-verify-when');
  const $verifyResult = $('trove-verify-result');
  const $stats = $('dash-stats');
  const $statsMeta = $('dash-stats-meta');
  const $statsBody = $('dash-stats-body');

  boot().catch((err) => {
    console.error('[dashboard] boot failed', err);
    showError(err);
  });

  async function boot() {
    // Client-side gate. The session is HttpOnly cookies, so hasSession() (the
    // JS-readable hint cookie) is the only thing a script can check.
    if (!Auth.hasSession()) { redirectToLogin(); return; }
    const user = await Auth.getMe({ force: true });
    if (!user) { redirectToLogin(); return; }
    renderUser(user);
    setupSections();
    loadMyGiveaways();
    if (user.claimed_trove_name) await loadTroveStats();
  }

  function redirectToLogin() {
    location.href = '/login?next=/dashboard';
  }

  function showError(err) {
    if ($loading) $loading.innerHTML = `<p class="dash-error">${esc(t('Failed to load'))}: ${esc((err && err.message) || String(err))}</p>`;
  }

  function renderUser(user) {
    if ($loading) $loading.hidden = true;
    if ($body) $body.hidden = false;

    if ($sideName) $sideName.textContent = user.display_name || user.username;
    if ($avatar && user.avatar_url) { $avatar.src = user.avatar_url; $avatar.hidden = false; }
    const $myProfile = document.getElementById('dash-my-profile');
    if ($myProfile && user.username) $myProfile.href = '/mods/' + encodeURIComponent(user.username);

    if ($profUsername)    $profUsername.textContent = user.username;
    const $profDiscord = $('prof-discord-handle');
    if ($profDiscord)     $profDiscord.textContent = '@' + (user.discord_handle || user.username);
    if ($profDisplayName) $profDisplayName.textContent = user.display_name || t('(none set)');
    if ($profNotify)      $profNotify.textContent = user.notify_email || t('Not set');
    if ($profCreated)     $profCreated.textContent = formatDate(user.created_at);
    setupUsernameRequest();

    // Trove claim state - two visual modes.
    if (user.claimed_trove_name) {
      $claimState.hidden = true;
      $claimedState.hidden = false;
      $claimedName.textContent = user.claimed_trove_display || user.claimed_trove_name;
      $claimedWhen.textContent = user.claimed_at
        ? t('Claimed {when}').replace('{when}', formatDate(user.claimed_at))
        : '';
      renderVerifyBlock(user);
      if ($stats) $stats.hidden = false;
    } else {
      $claimState.hidden = false;
      $claimedState.hidden = true;
      if ($troveTagUnverified) $troveTagUnverified.hidden = true;
      if ($troveTagVerified)   $troveTagVerified.hidden = true;
      if ($verifyBlock)        $verifyBlock.hidden = true;
      if ($stats)              $stats.hidden = true;
    }

    wireActions(user);
  }

  function renderVerifyBlock(user) {
    if (!$verifyBlock) return;
    $verifyBlock.hidden = false;
    const verified = !!user.claim_verified;
    $verifyBlock.dataset.state = verified ? 'verified' : 'unverified';

    // Swap the heading tag (unverified amber pill vs verified green pill).
    if ($troveTagUnverified) $troveTagUnverified.hidden = verified;
    if ($troveTagVerified)   $troveTagVerified.hidden = !verified;

    // Body content: one of two inline blocks. Use class-targeting
    // children instead of separate IDs since both live in the same
    // wrapper and only one renders at a time.
    const $unv = $verifyBlock.querySelector('.dash-verify-unverified');
    const $ver = $verifyBlock.querySelector('.dash-verify-verified');
    if ($unv) $unv.hidden = verified;
    if ($ver) $ver.hidden = !verified;

    if (verified) {
      if ($verifyWhen) {
        $verifyWhen.textContent = user.claim_verified_at
          ? t('Verified {when}').replace('{when}', formatDate(user.claim_verified_at))
          : '';
      }
    } else {
      if ($verifyBaseline) {
        const n = user.claim_baseline_board_count || 0;
        $verifyBaseline.textContent = n > 0
          ? t('We have a baseline on {n} board(s) - score on any of them to verify.').replace('{n}', String(n))
          : t("We didn't capture any leaderboard data for that name at claim time. Play a bit, then click Verify to re-check.");
      }
    }
    // Result toast (success or failure) only shows after a manual check.
    if ($verifyResult) $verifyResult.hidden = true;
  }

  function wireActions(user) {
    if ($profEditName) {
      $profEditName.onclick = async () => {
        const next = prompt(t('New display name (blank to clear):'), user.display_name || '');
        if (next === null) return;
        const r = await Auth.callJSON('/v1/site-auth/me', {
          method: 'PATCH', json: { display_name: next.trim() || null },
        });
        if (r.ok && r.data) renderUser(r.data);
      };
    }

    if ($profEditNotify) {
      $profEditNotify.onclick = async () => {
        const next = prompt(
          t('Email for notifications (optional, blank to remove):'),
          user.notify_email || '',
        );
        if (next === null) return;
        const r = await Auth.callJSON('/v1/site-auth/me', {
          method: 'PATCH', json: { notify_email: next.trim() },
        });
        if (r.ok && r.data) { user = r.data; renderUser(r.data); }
        else alert(Auth.errorMessage(r.data) || t('That email address looks invalid.'));
      };
    }

    if ($logout) {
      $logout.onclick = async () => {
        await Auth.logout();
        location.href = '/';
      };
    }

    if ($export) {
      $export.onclick = async () => {
        if ($acctError) $acctError.hidden = true;
        $export.disabled = true;
        try {
          const res = await Auth.call('/v1/site-auth/me/export');
          if (!res.ok) throw new Error('export failed');
          const blob = await res.blob();
          const url = URL.createObjectURL(blob);
          const a = document.createElement('a');
          a.href = url;
          a.download = 'better-trove-tools-export.json';
          document.body.appendChild(a);
          a.click();
          a.remove();
          URL.revokeObjectURL(url);
        } catch (_) {
          if ($acctError) {
            $acctError.textContent = t('Could not export your data. Please try again.');
            $acctError.hidden = false;
          }
        } finally {
          $export.disabled = false;
        }
      };
    }

    if ($deleteAcct) {
      $deleteAcct.onclick = async () => {
        if ($acctError) $acctError.hidden = true;
        const typed = prompt(
          t('This permanently deletes your account and personal data and cannot be undone. Type your username to confirm:'),
        );
        if (typed === null) return;
        const r = await Auth.callJSON('/v1/site-auth/me/delete', {
          method: 'POST', json: { confirm_username: typed.trim() },
        });
        if (!r.ok) {
          if ($acctError) {
            $acctError.textContent =
              Auth.errorMessage(r.data) || t('Could not delete your account. Please try again.');
            $acctError.hidden = false;
          }
          return;
        }
        await Auth.logout();
        location.href = '/';
      };
    }

    if ($claimForm) {
      $claimForm.onsubmit = async (e) => {
        e.preventDefault();
        $claimError.hidden = true;
        const name = ($claimInput.value || '').trim();
        if (!name) return;
        const r = await Auth.callJSON('/v1/site-auth/me/claim-trove-name', {
          method: 'POST', json: { trove_name: name },
        });
        if (!r.ok) {
          $claimError.textContent = Auth.errorMessage(r.data) || t('Failed to claim that name.');
          $claimError.hidden = false;
          return;
        }
        renderUser(r.data);
        await loadTroveStats();
      };
    }

    if ($unclaim) {
      $unclaim.onclick = async () => {
        if (!confirm(t('Release this Trove name? You can re-claim later.'))) return;
        const r = await Auth.callJSON('/v1/site-auth/me/claim-trove-name', {
          method: 'DELETE',
        });
        if (r.ok && r.data) renderUser(r.data);
      };
    }

    if ($verifyBtn) {
      $verifyBtn.onclick = async () => {
        $verifyBtn.disabled = true;
        const prevLabel = $verifyBtn.textContent;
        $verifyBtn.textContent = t('Checking…');
        try {
          const r = await Auth.callJSON('/v1/site-auth/me/verify-trove-claim', {
            method: 'POST',
          });
          if (!r.ok) {
            if ($verifyResult) {
              $verifyResult.textContent =
                Auth.errorMessage(r.data) || t('Verification failed.');
              $verifyResult.dataset.tone = 'error';
              $verifyResult.hidden = false;
            }
            return;
          }
          // Refresh the user payload so the badge + stats flip in one
          // render pass. The endpoint returns {verified, detail, user}.
          if (r.data.user) renderUser(r.data.user);
          if ($verifyResult) {
            $verifyResult.textContent = r.data.detail || '';
            $verifyResult.dataset.tone = r.data.verified ? 'success' : 'info';
            $verifyResult.hidden = false;
          }
        } finally {
          $verifyBtn.disabled = false;
          // Restore the label even if the render didn't reach this btn
          // (e.g. claim verified → block hides the unverified body).
          $verifyBtn.textContent = prevLabel;
        }
      };
    }
  }

  // Trove username change request.
  let _usernameReqWired = false;
  let _latestUnameReq = null;
  function setupUsernameRequest() {
    const btn = $('prof-edit-username');
    if (!btn) return;
    if (!_usernameReqWired) {
      _usernameReqWired = true;
      // Just a button inline with the username; the form lives in a modal so it
      // never shows until clicked (a stray inline form was bleeding through CSS).
      btn.addEventListener('click', () => openUsernameModal(
        _latestUnameReq && _latestUnameReq.status === 'pending' ? _latestUnameReq.requested_username : ''));
    }
    // Load the latest request to show pending / denial state.
    Auth.callJSON('/v1/site-auth/me/username-request').then((r) => {
      if (r.ok && r.data) showUsernameStatus(r.data.request);
    }).catch(() => {});
  }
  function openUsernameModal(prefill) {
    const ov = document.createElement('div');
    ov.className = 'dash-modal-overlay';
    ov.innerHTML =
      '<div class="dash-modal" role="dialog" aria-modal="true" aria-labelledby="uname-modal-title">' +
        '<form id="uname-modal-form">' +
          '<h3 class="dash-modal-title" id="uname-modal-title">' + esc(t('Request username change')) + '</h3>' +
          '<p class="dash-modal-message">' +
            esc(t('This is your handle for mods & modpacks (/mods/<you>/…). Changes are reviewed by a moderator.')) + '</p>' +
          '<div class="dash-mod-form" style="margin-top:4px">' +
            '<input type="text" name="username" maxlength="24" autocomplete="off" placeholder="new_username" value="' +
              esc(prefill || '') + '" style="flex:1 1 100%">' +
          '</div>' +
          '<p class="dash-error" id="uname-modal-error" hidden style="margin-top:10px"></p>' +
          '<div class="dash-modal-actions" style="margin-top:14px">' +
            '<button type="button" class="dash-btn dash-btn-mini dash-btn-ghost" data-act="cancel">' + esc(t('Cancel')) + '</button>' +
            '<button type="submit" class="dash-btn dash-btn-mini">' + esc(t('Request')) + '</button>' +
          '</div>' +
        '</form>' +
      '</div>';
    const form = ov.querySelector('#uname-modal-form');
    const input = form.username;
    const err = ov.querySelector('#uname-modal-error');
    const submitBtn = form.querySelector('[type="submit"]');
    let release = null;
    const close = () => { if (release) release(); document.removeEventListener('keydown', onKey); ov.remove(); };
    const onKey = (e) => { if (e.key === 'Escape') close(); };
    ov.addEventListener('click', (e) => { if (e.target === ov) close(); });
    ov.querySelector('[data-act="cancel"]').addEventListener('click', close);
    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      err.hidden = true;
      const name = input.value.trim();
      if (!name) { err.textContent = t('Enter a username.'); err.hidden = false; return; }
      submitBtn.disabled = true;
      const r = await Auth.callJSON('/v1/site-auth/me/username-request', { json: { username: name } });
      if (r.ok && r.data && r.data.request) {
        close();
        showUsernameStatus(r.data.request);
      } else {
        err.textContent = Auth.errorMessage(r.data) || t('Could not request that username.');
        err.hidden = false;
        submitBtn.disabled = false;
      }
    });
    document.body.appendChild(ov);
    // Trap focus in the dialog (Escape handled by the helper), first focus the input.
    const panel = ov.querySelector('.dash-modal');
    if (window.BTTUtil && window.BTTUtil.trapFocus) {
      release = window.BTTUtil.trapFocus(panel, { onEscape: close, initialFocus: input });
    } else {
      document.addEventListener('keydown', onKey);
      input.focus();
    }
  }
  function showUsernameStatus(req) {
    _latestUnameReq = req || null;
    const status = $('prof-username-status');
    if (!status) return;
    if (!req || req.status === 'approved') { status.hidden = true; return; }
    if (req.status === 'pending') {
      status.innerHTML = '<i class="fa-solid fa-clock"></i> ' +
        esc(t('Requested')) + ' <strong>' + esc(req.requested_username) + '</strong> — ' + esc(t('awaiting moderator approval.'));
    } else if (req.status === 'rejected') {
      status.innerHTML = '<i class="fa-solid fa-circle-xmark"></i> ' +
        esc(t('Your request for')) + ' <strong>' + esc(req.requested_username) + '</strong> ' + esc(t('was declined.')) +
        (req.reason ? ' ' + esc(req.reason) : '');
    }
    status.hidden = false;
  }

  // Sidebar section switching.
  const SECTIONS = ['profile', 'giveaways', 'mods', 'modpacks', 'leaderboard', 'discord', 'webhooks', 'dmsubs', 'images'];
  function setupSections() {
    const navItems = Array.prototype.slice.call(document.querySelectorAll('.dash-nav-item'));
    const nav = navItems[0] ? navItems[0].closest('.dash-nav') : null;
    // Wire the role="tablist"/role="tab" relationships to the panels.
    navItems.forEach((b) => {
      const sec = b.dataset.section;
      b.id = 'dash-tab-' + sec;
      b.setAttribute('aria-controls', 'dash-panel-' + sec);
      b.setAttribute('aria-selected', 'false');
      b.tabIndex = -1;
      b.addEventListener('click', () => showSection(sec));
    });
    document.querySelectorAll('.dash-section').forEach((s) => {
      const sec = s.dataset.pane;
      s.id = 'dash-panel-' + sec;
      s.setAttribute('role', 'tabpanel');
      s.setAttribute('aria-labelledby', 'dash-tab-' + sec);
      s.setAttribute('tabindex', '0');
    });
    // Arrow / Home / End move between tabs (WAI-ARIA tablist pattern).
    if (nav) nav.addEventListener('keydown', (e) => {
      const i = navItems.indexOf(document.activeElement);
      if (i < 0) return;
      let next = null;
      if (e.key === 'ArrowDown' || e.key === 'ArrowRight') next = navItems[(i + 1) % navItems.length];
      else if (e.key === 'ArrowUp' || e.key === 'ArrowLeft') next = navItems[(i - 1 + navItems.length) % navItems.length];
      else if (e.key === 'Home') next = navItems[0];
      else if (e.key === 'End') next = navItems[navItems.length - 1];
      if (next) { e.preventDefault(); showSection(next.dataset.section); next.focus(); }
    });
    const hash = location.hash.replace(/^#/, '');
    showSection(SECTIONS.includes(hash) ? hash : 'profile');
    window.addEventListener('hashchange', () => {
      const h = location.hash.replace(/^#/, '');
      if (SECTIONS.includes(h)) showSection(h);
    });
  }
  function showSection(name) {
    document.querySelectorAll('.dash-nav-item').forEach((b) => {
      const on = b.dataset.section === name;
      b.classList.toggle('active', on);
      b.setAttribute('aria-selected', on ? 'true' : 'false');
      b.tabIndex = on ? 0 : -1;   // roving tabindex: only the selected tab is tabbable
    });
    document.querySelectorAll('.dash-section').forEach((s) => { s.hidden = s.dataset.pane !== name; });
    if (location.hash.replace(/^#/, '') !== name) history.replaceState(null, '', '#' + name);
    // Lazy-load the Discord Bot section the first time it's opened so users
    // who never visit it don't pay the guild-list round-trip.
    if (name === 'discord' && !_discordLoaded) { _discordLoaded = true; loadDiscordBot(); }
    if (name === 'mods' && !_modsLoaded) { _modsLoaded = true; loadMyMods(); }
    if (name === 'modpacks' && !_modpacksLoaded) { _modpacksLoaded = true; loadMyModpacks(); }
    if (name === 'webhooks' && !_webhooksLoaded) { _webhooksLoaded = true; loadWebhooks(); }
    if (name === 'dmsubs' && !_dmsubsLoaded) { _dmsubsLoaded = true; loadDmSubs(); }
    if (name === 'images' && !_imagesLoaded) {
      _imagesLoaded = true;
      const box = document.getElementById('dash-images-body');
      if (box && window.ImageStudio) window.ImageStudio.mount(box);
    }
  }
  let _imagesLoaded = false;

  // My Modpacks section.
  let _modpacksLoaded = false;

  async function loadMyModpacks() {
    const form = $('dash-modpack-create');
    const err = $('dash-modpack-error');
    if (form && !form.dataset.wired) {
      form.dataset.wired = '1';
      form.addEventListener('submit', async (e) => {
        e.preventDefault();
        if (err) err.hidden = true;
        const btn = form.querySelector('button[type=submit]');
        btn.disabled = true;
        const r = await Auth.callJSON('/v1/modpacks/hub/projects', {
          json: { title: form.title.value.trim(), visibility: form.visibility.value },
        });
        btn.disabled = false;
        if (r.ok && r.data && r.data.slug) {
          location.href = '/modpacks/' + encodeURIComponent(r.data.handle) + '/' + encodeURIComponent(r.data.slug);
        } else if (err) {
          err.textContent = Auth.errorMessage(r.data) || t('Could not create the modpack.');
          err.hidden = false;
        }
      });
    }
    await renderOwnedModpacks();
  }

  async function renderOwnedModpacks() {
    const list = $('dash-modpacks-list');
    if (!list) return;
    const r = await Auth.callJSON('/v1/modpacks/hub/me/projects');
    const items = (r.ok && r.data && Array.isArray(r.data.items)) ? r.data.items : null;
    if (items === null) {
      list.innerHTML = `<p class="dash-empty">${esc(t("Couldn't load your modpacks right now."))}</p>`;
    } else if (!items.length) {
      list.innerHTML = `<p class="dash-empty">${esc(t("You haven't created any modpacks yet. Create one above, or browse"))} <a href="/modpacks">${esc(t('Modpacks'))}</a>.</p>`;
    } else {
      list.innerHTML = items.map(modpackCard).join('');
    }
  }

  function modpackCard(p) {
    const vis = p.visibility === 'public'
      ? `<span class="dash-tag dash-tag-verified">${esc(t('public'))}</span>`
      : p.visibility === 'unlisted'
        ? `<span class="dash-tag">${esc(t('unlisted'))}</span>`
        : `<span class="dash-tag dash-tag-unverified">${esc(t('draft'))}</span>`;
    const collab = p.is_collaborator ? `<span class="dash-tag">${esc(t('collaborator'))}</span>` : '';
    return `
      <a class="dash-mod-card" href="/modpacks/${encodeURIComponent(p.handle)}/${encodeURIComponent(p.slug)}">
        <span class="dash-mod-title">${esc(p.title)} ${vis} ${collab}</span>
        <span class="dash-mod-meta">
          <i class="fa-solid fa-cube" aria-hidden="true"></i> ${Number(p.mod_count || 0)}
          · <i class="fa-solid fa-download" aria-hidden="true"></i> ${Number(p.download_count || 0).toLocaleString()}
          · <i class="fa-solid fa-heart" aria-hidden="true"></i> ${Number(p.star_count || 0).toLocaleString()}
        </span>
      </a>`;
  }

  // My Mods section.
  let _modsLoaded = false;

  async function loadMyMods() {
    const list = $('dash-mods-list');
    const form = $('dash-mod-create');
    const err = $('dash-mod-error');
    if (form && !form.dataset.wired) {
      form.dataset.wired = '1';
      // "Made by someone else" reveals the creator field + forces releases-only mode
      // (you can't own the source of a mod you're just sharing on the author's behalf).
      const onBehalfBox = $('dash-mod-onbehalf');
      const modeHintEl = $('dash-mod-mode-hint');
      if (onBehalfBox) onBehalfBox.addEventListener('change', () => {
        const on = onBehalfBox.checked;
        const credited = $('dash-mod-credited');
        const modeSel = $('dash-mod-mode');
        if (credited) { credited.hidden = !on; credited.required = on; }
        if (modeSel) { modeSel.hidden = on; if (on) modeSel.value = 'releases'; }
        if (modeHintEl) modeHintEl.hidden = on;
      });
      window.BTTUtil.modeHint($('dash-mod-mode'), modeHintEl);
      form.addEventListener('submit', async (e) => {
        e.preventDefault();
        if (err) err.hidden = true;
        const onBehalf = !!(form.on_behalf && form.on_behalf.checked);
        const credited = onBehalf ? (form.credited_author.value || '').trim() : null;
        if (onBehalf && !credited) {
          if (err) { err.textContent = t('Name the creator this mod was made by.'); err.hidden = false; }
          return;
        }
        const btn = form.querySelector('button[type=submit]');
        btn.disabled = true;
        const r = await Auth.callJSON('/v1/mods/hub/projects', {
          json: {
            title: form.title.value.trim(),
            mode: onBehalf ? 'releases' : form.mode.value,
            visibility: form.visibility.value,
            on_behalf: onBehalf,
            credited_author: credited,
          },
        });
        btn.disabled = false;
        if (r.ok && r.data && r.data.slug) {
          location.href = '/mods/' + encodeURIComponent(r.data.handle) + '/' + encodeURIComponent(r.data.slug);
        } else if (err) {
          err.textContent = Auth.errorMessage(r.data) || t('Could not create the mod.');
          err.hidden = false;
        }
      });
    }
    await Promise.all([renderOwnedMods(), renderStarredMods(), renderCreatorAccess()]);
  }

  // ── API access: the creator token + the API accounts using it ──
  // The token is a connect code, not a password: someone pastes it once in the
  // developer portal and their account shows up below. The permissions live on
  // the connection, so the creator keeps control after handing the token over.

  function creatorCoverText(link) {
    if (link.all_projects) return t('All my mods, including new ones');
    const picked = link.projects || [];
    if (!picked.length) return t('No mods');
    return picked.map((p) => p.title || p.slug).join(', ');
  }

  function creatorTokenReveal(token) {
    return '<div class="dash-token-reveal">' +
      '<p class="dash-hint">' + esc(t('Copy it now — this is the only time it is shown.')) + '</p>' +
      '<code class="dash-token-value">' + esc(token) + '</code>' +
      '<button type="button" class="dash-btn dash-btn-mini" data-act="copy-creator-token">' +
      '<i class="fa-regular fa-copy" aria-hidden="true"></i> <span>' + esc(t('Copy')) + '</span>' +
      '</button></div>';
  }

  // `reveal` is a freshly minted token, passed straight back into the re-render
  // so it survives the refresh that follows creating or replacing it.
  async function renderCreatorAccess(reveal) {
    const box = $('dash-creator-body');
    if (!box) return;
    const r = await Auth.callJSON('/site/mods/creator-token');
    if (!r.ok || !r.data) {
      box.innerHTML = '<p class="dash-error">' + esc(t('Could not load your API access.')) + '</p>';
      return;
    }
    const links = r.data.connections || [];
    const mods = r.data.mods || [];

    const tokenBlock = r.data.has_token
      ? '<p class="dash-hint">' + esc(t('Your creator token')) + ': <code>' + esc(r.data.prefix || '') + '…</code></p>' +
        '<button type="button" class="dash-btn dash-btn-mini dash-btn-ghost" data-act="rotate-creator-token">' +
        '<i class="fa-solid fa-rotate" aria-hidden="true"></i> <span>' + esc(t('Replace token')) + '</span></button>' +
        '<p class="dash-hint">' + esc(t('Replacing it disconnects everyone using the old one.')) + '</p>'
      : '<button type="button" class="dash-btn dash-btn-mini" data-act="create-creator-token">' +
        '<i class="fa-solid fa-key" aria-hidden="true"></i> <span>' + esc(t('Create my creator token')) + '</span></button>';

    const rows = links.length ? links.map((l) =>
      '<li class="dash-creator-row">' +
        '<div class="dash-creator-main">' +
          '<strong>' + esc(l.label || t('Unnamed connection')) + '</strong> ' +
          '<span class="dash-hint">' + esc(creatorCoverText(l)) + '</span>' +
        '</div>' +
        '<div class="dash-creator-actions">' +
          '<button type="button" class="dash-btn dash-btn-mini dash-btn-ghost" data-act="scope" data-link="' + esc(l.id) + '">' +
            esc(t('Choose mods')) + '</button> ' +
          '<button type="button" class="dash-btn dash-btn-mini dash-btn-danger" data-act="revoke" data-link="' + esc(l.id) + '">' +
            esc(t('Remove')) + '</button>' +
        '</div>' +
        '<div class="dash-creator-scope" hidden></div>' +
      '</li>').join('')
      : '<li class="dash-hint">' + esc(t('Nobody is connected yet.')) + '</li>';

    box.innerHTML = (reveal ? creatorTokenReveal(reveal) : '') + tokenBlock +
      '<h3 class="dash-card-title" style="margin:18px 0 6px">' + esc(t('Connected apps')) + '</h3>' +
      '<ul class="dash-creator-list">' + rows + '</ul>';

    const copyBtn = box.querySelector('[data-act="copy-creator-token"]');
    if (copyBtn) copyBtn.addEventListener('click', () => {
      window.BTTUtil.copy(reveal).then((ok) => {
        if (ok) copyBtn.querySelector('span').textContent = t('Copied');
      });
    });

    const createBtn = box.querySelector('[data-act="create-creator-token"]');
    if (createBtn) createBtn.addEventListener('click', async () => {
      createBtn.disabled = true;
      const res = await Auth.callJSON('/site/mods/creator-token', { method: 'POST', json: {} });
      createBtn.disabled = false;
      if (res.ok && res.data && res.data.token) renderCreatorAccess(res.data.token);
    });

    const rotateBtn = box.querySelector('[data-act="rotate-creator-token"]');
    if (rotateBtn) rotateBtn.addEventListener('click', async () => {
      const ok = await confirmModal({
        title: t('Replace your creator token?'),
        message: t('Everyone connected with the old token loses access right away, and you will need to share the new one again.'),
        confirm: t('Replace'),
        danger: true,
      });
      if (!ok) return;
      const res = await Auth.callJSON('/site/mods/creator-token/rotate', { method: 'POST', json: {} });
      if (res.ok && res.data && res.data.token) renderCreatorAccess(res.data.token);
    });

    box.querySelectorAll('[data-act="revoke"]').forEach((b) => b.addEventListener('click', async () => {
      const ok = await confirmModal({
        title: t('Remove this connection?'),
        message: t('That app can no longer touch your mods. Anything it already published stays.'),
        confirm: t('Remove'),
        danger: true,
      });
      if (!ok) return;
      const res = await Auth.callJSON(
        '/site/mods/creator-connections/' + encodeURIComponent(b.dataset.link), { method: 'DELETE' });
      if (res.ok) renderCreatorAccess();
    }));

    box.querySelectorAll('[data-act="scope"]').forEach((b) => b.addEventListener('click', () => {
      const pane = b.closest('.dash-creator-row').querySelector('.dash-creator-scope');
      if (!pane.hidden) { pane.hidden = true; return; }
      const link = links.filter((l) => l.id === b.dataset.link)[0];
      if (!link) return;
      const chosen = {};
      (link.project_ids || []).forEach((id) => { chosen[id] = true; });
      const group = 'scope-' + link.id;
      const picks = mods.length
        ? mods.map((m) =>
            '<label class="dash-mod-check"><input type="checkbox" value="' + esc(m.id) + '"' +
            (chosen[m.id] ? ' checked' : '') + '> <span>' + esc(m.title || m.slug) + '</span></label>').join('')
        : '<p class="dash-hint">' + esc(t('You have no mods yet.')) + '</p>';
      pane.innerHTML =
        '<label class="dash-mod-check"><input type="radio" name="' + esc(group) + '" value="all"' +
          (link.all_projects ? ' checked' : '') + '> <span>' + esc(t('All my mods, including new ones')) + '</span></label>' +
        '<label class="dash-mod-check"><input type="radio" name="' + esc(group) + '" value="some"' +
          (link.all_projects ? '' : ' checked') + '> <span>' + esc(t('Only the mods I pick')) + '</span></label>' +
        '<div class="dash-creator-picks"' + (link.all_projects ? ' hidden' : '') + '>' + picks + '</div>' +
        '<button type="button" class="dash-btn dash-btn-mini" data-act="save-scope">' + esc(t('Save')) + '</button>' +
        '<p class="dash-error" data-act="scope-error" hidden></p>';
      pane.hidden = false;
      const pickBox = pane.querySelector('.dash-creator-picks');
      const allRadio = pane.querySelector('input[type=radio][value="all"]');
      pane.querySelectorAll('input[type=radio]').forEach((rd) =>
        rd.addEventListener('change', () => { pickBox.hidden = allRadio.checked; }));
      pane.querySelector('[data-act="save-scope"]').addEventListener('click', async () => {
        const errEl = pane.querySelector('[data-act="scope-error"]');
        errEl.hidden = true;
        const ids = Array.prototype.slice.call(
          pane.querySelectorAll('input[type=checkbox]:checked')).map((c) => c.value);
        const res = await Auth.callJSON(
          '/site/mods/creator-connections/' + encodeURIComponent(link.id),
          { method: 'PATCH', json: { all_projects: allRadio.checked, project_ids: ids } });
        if (res.ok) { renderCreatorAccess(); return; }
        errEl.textContent = Auth.errorMessage(res.data) || t('Could not save.');
        errEl.hidden = false;
      });
    }));
  }

  async function renderOwnedMods() {
    const list = $('dash-mods-list');
    if (!list) return;
    const r = await Auth.callJSON('/v1/mods/hub/me/projects');
    const items = (r.ok && r.data && Array.isArray(r.data.items)) ? r.data.items : null;
    if (items === null) {
      list.innerHTML = `<p class="dash-empty">${esc(t("Couldn't load your mods right now."))}</p>`;
    } else if (!items.length) {
      list.innerHTML = `<p class="dash-empty">${esc(t("You haven't created any mods yet. Create one above, or browse the"))} <a href="/mods">${esc(t('Mods Hub'))}</a>.</p>`;
    } else {
      list.innerHTML = items.map(modCard).join('');
    }
  }

  async function renderStarredMods() {
    const el = $('dash-starred-list');
    if (!el) return;
    const r = await Auth.callJSON('/v1/mods/hub/me/starred');
    const items = (r.ok && r.data && Array.isArray(r.data.items)) ? r.data.items : [];
    el.innerHTML = items.length
      ? items.map(modCard).join('')
      : `<p class="dash-empty">${esc(t("You haven't starred any mods yet."))} <a href="/mods">${esc(t('Browse the Mods Hub'))}</a>.</p>`;
  }

  function modCard(p) {
    const vis = p.visibility === 'public'
      ? `<span class="dash-tag dash-tag-verified">${esc(t('public'))}</span>`
      : p.visibility === 'unlisted'
        ? `<span class="dash-tag">${esc(t('unlisted'))}</span>`
        : `<span class="dash-tag dash-tag-unverified">${esc(t('draft'))}</span>`;
    const modeTag = p.mode === 'releases' ? `<span class="dash-tag">${esc(t('releases-only'))}</span>` : '';
    const collab = p.is_collaborator ? `<span class="dash-tag">${esc(t('collaborator'))}</span>` : '';
    return `
      <a class="dash-mod-card" href="/mods/${encodeURIComponent(p.handle)}/${encodeURIComponent(p.slug)}">
        <span class="dash-mod-title">${esc(p.title)} ${vis} ${modeTag} ${collab}</span>
        <span class="dash-mod-meta">
          <i class="fa-solid fa-download" aria-hidden="true"></i> ${Number(p.download_count || 0).toLocaleString()}
          · <i class="fa-solid fa-star" aria-hidden="true"></i> ${Number(p.star_count || 0).toLocaleString()}
          · <i class="fa-solid fa-code-fork" aria-hidden="true"></i> ${Number(p.fork_count || 0)}
        </span>
      </a>`;
  }

  // Giveaways section.
  async function loadMyGiveaways() {
    if (!$gwList) return;
    const r = await Auth.callJSON('/v1/giveaways/me');
    if (!r.ok || !Array.isArray(r.data)) {
      $gwList.innerHTML = `<p class="dash-empty">${esc(t("Couldn't load your giveaways right now."))}</p>`;
      return;
    }
    const items = r.data;
    const won = items.filter((g) => g.won).length;
    if ($wonBadge) {
      $wonBadge.hidden = won === 0;
      $wonBadge.textContent = String(won);
    }
    if (!items.length) {
      $gwList.innerHTML = `<p class="dash-empty">${esc(t("You haven't entered any giveaways yet."))} <a href="/giveaways">${esc(t('Browse giveaways'))}</a>.</p>`;
      return;
    }
    $gwList.innerHTML = items.map(gwCard).join('');
    $gwList.querySelectorAll('[data-copy]').forEach((b) =>
      b.addEventListener('click', () => {
        window.BTTUtil.copy(b.dataset.copy);
        const prev = b.textContent;
        b.textContent = t('Copied!');
        setTimeout(() => { b.textContent = prev; }, 1500);
      }));
  }

  function gwCard(g) {
    let status = g.status, cls = '';
    if (g.won) { status = t('🎉 You won!'); cls = 'won'; }
    else if (g.status === 'drawn') { status = t('Not selected'); cls = 'lost'; }
    else if (g.status === 'open') { status = t('Entered · draw pending'); cls = 'pending'; }
    const code = (g.won && g.code) ? `
      <div class="gw-my-code">
        <code>${esc(g.code)}</code>
        <button type="button" class="dash-btn dash-btn-mini" data-copy="${esc(g.code)}">${esc(t('Copy'))}</button>
      </div>` : '';
    return `
      <article class="gw-my-card ${g.won ? 'won' : ''}">
        <div class="gw-my-head">
          <div class="gw-my-info">
            <p class="gw-my-prize">${esc(g.prize_name)}</p>
            <p class="gw-my-title">${esc(g.title)}</p>
          </div>
          <span class="gw-my-status ${cls}">${esc(status)}</span>
        </div>
        ${code}
      </article>`;
  }

  // Trove stats (leaderboard appearances).
  async function loadTroveStats() {
    $statsBody.innerHTML = `<p class="dash-loading" data-i18n>${t('Loading your stats - this can take a moment.')}</p>`;
    const r = await Auth.callJSON('/v1/site-auth/me/trove-stats');
    if (!r.ok || !r.data) {
      $statsBody.innerHTML = `<p class="dash-error">${esc(t('Could not load your stats right now.'))}</p>`;
      return;
    }
    const items = r.data.items || [];
    const series = r.data.series || null;

    if (!items.length) {
      $statsBody.innerHTML = `
        <p class="dash-empty">${esc(t('No recent leaderboard appearances for this name yet. Check back in an hour, or try a different name.'))}</p>`;
      $statsMeta.textContent = '';
      return;
    }

    if ($statsMeta) {
      $statsMeta.textContent = t('{n} recent appearance(s)').replace('{n}', items.length);
    }

    // Compact table - board name, rank, score, when. Keep it readable
    // on mobile by stacking via CSS grid below the 600px breakpoint.
    const rows = items.slice(0, 25).map((it) => `
      <div class="dash-stat-row">
        <span class="dash-stat-board">${esc(t('Board') + ' #' + it.leaderboard)}</span>
        <span class="dash-stat-rank">#${it.rank}</span>
        <span class="dash-stat-score">${esc(formatScore(it.score))}</span>
        <span class="dash-stat-when">${esc(formatWhen(it.created_at))}</span>
      </div>`).join('');

    let chartHTML = '';
    if (series && (series.series || []).length) {
      // Cap the chart at the top 6 boards (the API returned them sorted
      // by best-rank; the rest tend to be noise on this size of card).
      const lines = series.series.slice(0, 6).map((s) => ({
        label: s.name,
        bestRank: s.best_rank,
        points: s.points,
      }));
      chartHTML = `
        <div class="dash-chart" id="dash-chart-host">${renderInlineChart(series.anchors, lines)}</div>
        <ul class="dash-chart-legend">
          ${lines.map((l, i) => `
            <li>
              <span class="dash-chart-swatch" style="background:${chartColor(i)}"></span>
              <span class="dash-chart-name" title="${esc(l.label)}">${esc(l.label)}</span>
              <span class="dash-chart-best">${esc(t('best #' + l.bestRank))}</span>
            </li>`).join('')}
        </ul>`;
    }

    $statsBody.innerHTML = `
      ${chartHTML}
      <div class="dash-stat-table" role="table">${rows}</div>`;
  }

  // Tiny dependency-free SVG line chart. Same vocabulary as leaderboards.js but
  // stripped to essentials: no tooltip, fixed height, labels as a separate <ul>.
  function renderInlineChart(anchors, series) {
    if (!anchors || anchors.length < 2 || !series.length) return '';
    const W = 720, H = 180, PAD = 28;
    const xMin = anchors[0], xMax = anchors[anchors.length - 1];
    const xRange = Math.max(1, xMax - xMin);
    let yMin = Infinity, yMax = -Infinity;
    for (const s of series) for (const p of s.points) {
      if (p.score < yMin) yMin = p.score;
      if (p.score > yMax) yMax = p.score;
    }
    if (!isFinite(yMin) || !isFinite(yMax)) return '';
    if (yMin === yMax) { yMin -= 1; yMax += 1; }
    const pad = (yMax - yMin) * 0.08;
    yMin -= pad; yMax += pad;
    const yRange = yMax - yMin;
    const x = (t) => PAD + ((t - xMin) / xRange) * (W - PAD * 2);
    const y = (v) => PAD + (1 - (v - yMin) / yRange) * (H - PAD * 2);

    const polylines = series.map((s, i) => {
      const pts = s.points.map((p) => `${x(p.created_at).toFixed(1)},${y(p.score).toFixed(1)}`).join(' ');
      return `<polyline fill="none" stroke="${chartColor(i)}" stroke-width="2" stroke-linejoin="round" points="${pts}"></polyline>`;
    }).join('');

    return `
      <svg viewBox="0 0 ${W} ${H}" preserveAspectRatio="none" role="img">
        ${polylines}
      </svg>`;
  }

  const CHART_COLORS = [
    '#4cc9f0', '#f72585', '#43aa8b', '#f8961e', '#b5179e',
    '#4361ee', '#f9c74f', '#7209b7',
  ];
  function chartColor(i) { return CHART_COLORS[i % CHART_COLORS.length]; }

  // Discord Bot section. Master-detail: side-menu of servers the bot is in +
  // a detail panel whose per-server config is split into tabs (Announcements,
  // Clubs, Settings). Servers the bot isn't in yet sit in an "Add the bot"
  // group. Backend: app/bot/router.py (site-JWT).
  let _discordLoaded = false;
  let _discordSelected = null;   // currently-selected guild id (persists across re-renders)
  let _annMeta = {};             // announcement key -> embed-editor meta (variables/default/sample)

  async function loadDiscordBot() {
    const el = $('dash-discord-body');
    if (!el) return;
    el.innerHTML = `<p class="dash-loading">${esc(t('Loading…'))}</p>`;
    const r = await Auth.callJSON('/v1/site-auth/discord/guilds');
    if (!r.ok) {
      el.innerHTML = `<article class="dash-card"><p class="dash-error">${esc(Auth.errorMessage(r.data) || t('Failed to load'))}</p></article>`;
      return;
    }
    const data = r.data || {};
    // Older grant from before we requested the `guilds` scope - reprompt.
    if (!data.guilds_synced) {
      const btn = data.reconnect_url
        ? `<a class="dash-btn" href="${esc(data.reconnect_url)}">${esc(t('Reconnect Discord'))}</a>` : '';
      el.innerHTML = `<article class="dash-card">
        <p class="dash-card-sub">${esc(data.linked
          ? t('We need permission to see your Discord servers. Reconnect Discord to continue.')
          : t('Connect your Discord account to manage the bot in your servers.'))}</p>
        ${btn}
      </article>`;
      return;
    }
    if (data.error) {
      el.innerHTML = `<article class="dash-card"><p class="dash-error">${esc(data.error)}</p></article>`;
      return;
    }
    renderDiscordLayout(el, data);
  }

  function renderDiscordLayout(el, data) {
    const guilds = data.guilds || [];
    const present = guilds.filter((g) => g.bot_present);
    const addable = guilds.filter((g) => !g.bot_present);
    const inviteBtn = data.invite_url
      ? `<a class="dash-btn dash-btn-mini" href="${esc(data.invite_url)}" target="_blank" rel="noopener">${esc(t('Invite the bot to a server'))}</a>` : '';

    if (!guilds.length) {
      el.innerHTML = `<article class="dash-card">
        <p class="dash-empty">${esc(t('No servers found where you can add or manage the bot.'))}</p>
        ${inviteBtn}
      </article>`;
      return;
    }

    // Keep the previous selection if it's still a bot-present server; else
    // default to the first one.
    if (!present.some((g) => g.id === _discordSelected)) {
      _discordSelected = present.length ? present[0].id : null;
    }

    const addGroup = `
      <div class="dash-discord-add">
        <p class="dash-discord-menu-head">${esc(t('Add the bot'))}</p>
        ${addable.length
          ? `<div class="dash-discord-menu">${addable.map(addServerItem).join('')}</div>`
          : `<p class="dash-card-sub-mini">${esc(t('No other servers to add the bot to.'))}</p>`}
        ${inviteBtn}
      </div>`;

    el.innerHTML = `
      <div class="dash-discord-layout">
        <aside class="dash-discord-servers">
          ${present.length ? `
            <p class="dash-discord-menu-head">${esc(t('Your servers'))}</p>
            <div class="dash-discord-menu">${present.map(serverMenuItem).join('')}</div>` : ''}
          ${addGroup}
        </aside>
        <div class="dash-discord-detail" id="dash-discord-detail">
          ${present.length
            ? `<p class="dash-loading">${esc(t('Loading…'))}</p>`
            : `<article class="dash-card"><p class="dash-empty">${esc(t('Add the bot to a server, then select it here to configure announcements and permissions.'))}</p></article>`}
        </div>
      </div>`;

    el.querySelectorAll('.dash-discord-menu-item').forEach((b) =>
      b.addEventListener('click', () => selectServer(b.dataset.guild, present)));
    if (_discordSelected) selectServer(_discordSelected, present);
  }

  function guildIcon(g, size) {
    const dim = `width:${size}px;height:${size}px`;
    return g.icon
      ? `<img class="dash-discord-icon" style="${dim}" src="https://cdn.discordapp.com/icons/${esc(g.id)}/${esc(g.icon)}.png?size=64" alt="" referrerpolicy="no-referrer">`
      : `<span class="dash-discord-icon dash-discord-icon-fallback" style="${dim}">${esc((g.name || '?')[0])}</span>`;
  }

  function serverMenuItem(g) {
    const active = g.id === _discordSelected ? ' active' : '';
    const live = g.announcing
      ? `<span class="dash-discord-live" title="${esc(t('Announcing'))}"></span>` : '';
    return `<button type="button" class="dash-discord-menu-item${active}" data-guild="${esc(g.id)}">
        ${guildIcon(g, 26)}
        <span class="dash-discord-menu-name">${esc(g.name)}</span>
        ${live}
      </button>`;
  }

  function addServerItem(g) {
    const inv = g.invite_url
      ? `<a class="dash-btn dash-btn-mini" href="${esc(g.invite_url)}" target="_blank" rel="noopener">${esc(t('Invite'))}</a>` : '';
    return `<div class="dash-discord-add-item">
        ${guildIcon(g, 24)}
        <span class="dash-discord-menu-name">${esc(g.name)}</span>
        ${inv}
      </div>`;
  }

  async function selectServer(guildId, present) {
    _discordSelected = guildId;
    document.querySelectorAll('.dash-discord-menu-item').forEach((b) =>
      b.classList.toggle('active', b.dataset.guild === guildId));
    const panel = $('dash-discord-detail');
    if (!panel) return;
    panel.innerHTML = `<p class="dash-loading">${esc(t('Loading…'))}</p>`;
    const [r, metaR] = await Promise.all([
      Auth.callJSON(`/v1/site-auth/discord/guilds/${encodeURIComponent(guildId)}`),
      Object.keys(_annMeta).length ? Promise.resolve(null) : Auth.callJSON('/v1/site-auth/discord/announcement-meta'),
    ]);
    if (metaR && metaR.ok && metaR.data) {
      (metaR.data.meta || []).forEach((m) => { _annMeta[m.key] = m; });
    }
    await ensureDesigns();
    if (!r.ok) {
      panel.innerHTML = `<article class="dash-card"><p class="dash-error">${esc(Auth.errorMessage(r.data) || t('Failed to load'))}</p></article>`;
      return;
    }
    const meta = (present || []).find((g) => g.id === guildId) || { id: guildId, name: '', icon: null };
    renderServerDetail(panel, guildId, r.data, meta);
  }

  // Per-server config panel: header + tab bar + the active tab's body. New
  // config categories slot in as additional tabs before Settings.
  // The bot's per-server output language (announcements, board, slash replies).
  function botLangPicker(detail) {
    const opts = (detail.languages || []).map((l) =>
      `<option value="${esc(l.code)}" ${l.code === detail.language ? 'selected' : ''}>${esc(l.label)}</option>`).join('');
    return `<label class="dash-bot-lang" data-i18n-title title="${esc(t('Language the bot speaks in this server'))}">
        <i class="fa-solid fa-language" aria-hidden="true"></i>
        <select class="dash-bot-lang-select" data-act="bot-lang" aria-label="${esc(t('Bot language'))}">${opts}</select>
      </label>`;
  }
  function wireBotLangPicker(panel, guildId, detail) {
    const sel = panel.querySelector('[data-act="bot-lang"]');
    if (!sel) return;
    sel.addEventListener('change', async () => {
      const prev = detail.language;
      sel.disabled = true;
      const r = await Auth.callJSON(
        `/v1/site-auth/discord/guilds/${encodeURIComponent(guildId)}/language`,
        { method: 'PUT', json: { language: sel.value } });
      sel.disabled = false;
      if (r.ok) detail.language = sel.value;
      else { sel.value = prev; }
    });
  }

  function renderServerDetail(panel, guildId, detail, meta) {
    const on = !!detail.announcing;
    panel.innerHTML = `
      <div class="dash-discord-detail-head">
        ${guildIcon(meta, 40)}
        <strong class="dash-discord-detail-name">${esc(meta.name)}</strong>
        <span class="dash-tag${on ? ' dash-tag-verified' : ''}" data-act="enabled-badge">${esc(on ? t('announcing') : t('off'))}</span>
        ${detail.can_manage_announcements ? botLangPicker(detail) : ''}
      </div>
      <div class="dash-discord-tabs" role="tablist">
        <button type="button" class="dash-discord-tab active" data-tab="announcements" role="tab">${esc(t('Announcements'))}</button>
        ${detail.can_manage_clubs ? `<button type="button" class="dash-discord-tab" data-tab="clubs" role="tab">${esc(t('Clubs'))}</button>` : ''}
        <button type="button" class="dash-discord-tab" data-tab="settings" role="tab">${esc(t('Settings'))}</button>
      </div>
      <div class="dash-discord-tabbody" id="dash-discord-tabbody"></div>`;
    wireBotLangPicker(panel, guildId, detail);
    const body = panel.querySelector('#dash-discord-tabbody');
    const tabs = panel.querySelectorAll('.dash-discord-tab');
    const show = (name) => {
      tabs.forEach((b) => b.classList.toggle('active', b.dataset.tab === name));
      if (name === 'settings') renderSettingsTab(body, guildId, detail);
      else if (name === 'clubs') renderClubsTab(body, guildId);
      else renderAnnouncementsTab(body, guildId, detail);
    };
    tabs.forEach((b) => b.addEventListener('click', () => show(b.dataset.tab)));
    show('announcements');
  }

  // ── Announcements tab ──
  // One row per announcement type (from the backend registry): enable + channel +
  // optional ping role. The ping picker is gated by `manage_ping_roles` separately
  // from the channel/toggle (`manage_announcements`). A deleted channel is flagged
  // loudly; deleted roles are already cleaned out server-side before we render.
  function renderAnnouncementsTab(body, guildId, detail) {
    const canManage = !!detail.can_manage_announcements;
    const canPing = !!detail.can_manage_ping_roles;
    if (!canManage && !canPing) {
      body.innerHTML = `<article class="dash-card">
        <h3 class="dash-discord-card-title">${esc(t('Announcements'))}</h3>
        <p class="dash-card-sub">${esc(t('You don’t have permission to configure announcements in this server.'))}</p>
      </article>`;
      return;
    }
    // Preserve registry order, group consecutive types by category.
    const groups = [];
    (detail.announcements || []).forEach((a) => {
      let g = groups.find((x) => x.name === a.category);
      if (!g) { g = { name: a.category, items: [] }; groups.push(g); }
      g.items.push(a);
    });
    const rows = groups.map((g) => `
      <div class="dash-ann-group">
        <p class="dash-discord-menu-head">${esc(t(g.name))}</p>
        ${g.items.map((a) => annRow(a, detail, canManage, canPing)).join('')}
      </div>`).join('');

    body.innerHTML = `
      ${canManage ? liveBoardCard(detail) : ''}
      ${canManage ? marketWatchCard(detail) : ''}
      <article class="dash-card" data-card="announcements">
        <div class="dash-ann-head">
          <h3 class="dash-discord-card-title">${esc(t('Announcements'))}</h3>
          <span class="dash-autosave" data-act="ann-status">${esc(t('Auto-saves'))}</span>
        </div>
        <p class="dash-card-sub-mini">${esc(t('Choose what the bot posts, where, and which roles to ping. Changes save automatically.'))}</p>
        ${rows}
      </article>`;

    const card = body.querySelector('[data-card="announcements"]');
    card.querySelectorAll('.dash-ann-row').forEach((row) => {
      renderRowPreflight(row, (detail.announcements || []).find((x) => x.key === row.dataset.key));
    });
    wireMultiSelects(card);
    card.querySelectorAll('.dash-ann-customize').forEach((btn) =>
      btn.addEventListener('click', () => toggleAnnEmbed(card, guildId, btn.dataset.key, detail)));
    const status = card.querySelector('[data-act="ann-status"]');
    const schedule = makeAutoSaver(() => doSaveAnnouncements(body, guildId, detail, status), status);
    card.addEventListener('change', schedule);     // enable checkboxes + channel selects
    card.addEventListener('ms-change', schedule);   // chip multi-select toggles
    wireBoardCard(body, guildId, detail);
    wireMarketWatchCard(body, guildId, detail);
    _msEnsureOutsideClose();
  }

  // The self-updating "Trove Now" board (one message the bot keeps current).
  function liveBoardCard(detail) {
    const b = detail.live_board || {};
    const chOpts = `<option value="">${esc(t('— none —'))}</option>` +
      (detail.channels || []).map((c) =>
        `<option value="${esc(c.id)}" ${c.id === b.channel_id ? 'selected' : ''}>#${esc(c.name)}</option>`).join('');
    const warn = b.channel_missing
      ? `<span class="dash-ann-warn">⚠ ${esc(t('channel deleted'))}</span>` : '';
    return `
      <article class="dash-card" data-card="board">
        <div class="dash-ann-head">
          <h3 class="dash-discord-card-title">${esc(t('Live “Trove Now” board'))}</h3>
          ${warn}
          <span class="dash-autosave" data-act="board-status">${esc(t('Auto-saves'))}</span>
        </div>
        <p class="dash-card-sub-mini">${esc(t('Keep one always-current message in a channel — challenge, chaos, merchants, biomes and resets, updated automatically.'))}</p>
        <div class="dash-discord-fields">
          <label class="dash-discord-field">
            <span class="dash-discord-label">${esc(t('Board channel'))}</span>
            <select data-field="board-channel">${chOpts}</select>
          </label>
          <label class="dash-discord-check">
            <input type="checkbox" data-field="board-enabled" ${b.enabled ? 'checked' : ''}>
            <span>${esc(t('Show the live board'))}</span>
          </label>
        </div>
        <p class="dash-discord-result" data-act="board-preflight" hidden></p>
      </article>`;
  }

  function wireBoardCard(body, guildId, detail) {
    const card = body.querySelector('[data-card="board"]');
    if (!card) return;
    renderPreflightInto(card.querySelector('[data-act="board-preflight"]'), (detail.live_board || {}).preflight);
    const status = card.querySelector('[data-act="board-status"]');
    const schedule = makeAutoSaver(() => doSaveLiveBoard(card, guildId, detail, status), status);
    card.addEventListener('change', schedule);
  }

  async function doSaveLiveBoard(card, guildId, detail, status) {
    const channel = card.querySelector('[data-field="board-channel"]').value || null;
    const enabled = card.querySelector('[data-field="board-enabled"]').checked;
    const r = await Auth.callJSON(`/v1/site-auth/discord/guilds/${encodeURIComponent(guildId)}/live-board`, {
      method: 'PUT', json: { enabled, channel_id: channel },
    });
    if (!r.ok) { setStatus(status, t('Save failed'), 'error'); return; }
    Object.assign(detail, r.data);
    renderPreflightInto(card.querySelector('[data-act="board-preflight"]'), (detail.live_board || {}).preflight);
    setStatus(status, t('Saved'), 'saved');
  }

  // ── Marketplace watch: one channel + ping roles + a watchlist of items ──
  function mwItemRow(name, maxPrice) {
    return `
      <div class="dash-mw-item">
        <input type="text" class="dash-mw-name" placeholder="${esc(t('Item name'))}"
               value="${esc(name || '')}" maxlength="120" autocomplete="off" spellcheck="false">
        <input type="number" class="dash-mw-price" placeholder="${esc(t('Max flux/ea'))}"
               value="${maxPrice != null ? esc(maxPrice) : ''}" min="0" step="1">
        <button type="button" class="dash-mw-remove" data-act="mw-remove" aria-label="${esc(t('Remove'))}">×</button>
      </div>`;
  }

  function marketWatchCard(detail) {
    const m = detail.market_watch || {};
    const chOpts = `<option value="">${esc(t('— none —'))}</option>` +
      (detail.channels || []).map((c) =>
        `<option value="${esc(c.id)}" ${c.id === m.channel_id ? 'selected' : ''}>#${esc(c.name)}</option>`).join('');
    const warn = m.channel_missing
      ? `<span class="dash-ann-warn">⚠ ${esc(t('channel deleted'))}</span>` : '';
    const itemRows = (m.items || []).map((it) => mwItemRow(it.name, it.max_price_each)).join('');
    return `
      <article class="dash-card" data-card="market-watch">
        <div class="dash-ann-head">
          <h3 class="dash-discord-card-title">${esc(t('Market watch'))}</h3>
          ${warn}
          <span class="dash-autosave" data-act="mw-status">${esc(t('Auto-saves'))}</span>
        </div>
        <p class="dash-card-sub-mini">${esc(t('Get pinged when a watched marketplace item drops to your target price. Checked each hour as new market data lands. Leave the price blank to alert on any listing.'))}</p>
        <div class="dash-discord-fields">
          <label class="dash-discord-field">
            <span class="dash-discord-label">${esc(t('Alert channel'))}</span>
            <select data-field="mw-channel">${chOpts}</select>
          </label>
          <label class="dash-discord-check">
            <input type="checkbox" data-field="mw-enabled" ${m.enabled ? 'checked' : ''}>
            <span>${esc(t('Enable market watch'))}</span>
          </label>
          <div class="dash-discord-field">
            <span class="dash-discord-label">${esc(t('Ping roles'))}</span>
            ${roleMultiSelect(detail.roles, m.ping_role_ids, false)}
          </div>
        </div>
        <p class="dash-discord-label dash-mw-items-head">${esc(t('Watched items'))}</p>
        <div class="dash-mw-items" data-act="mw-items">${itemRows}</div>
        <button type="button" class="dash-btn dash-btn-mini" data-act="mw-add">
          <i class="fa-solid fa-plus" aria-hidden="true"></i> <span>${esc(t('Add item'))}</span>
        </button>
      </article>`;
  }

  function wireMarketWatchCard(body, guildId, detail) {
    const card = body.querySelector('[data-card="market-watch"]');
    if (!card) return;
    wireMultiSelects(card);
    const status = card.querySelector('[data-act="mw-status"]');
    const schedule = makeAutoSaver(() => doSaveMarketWatch(card, guildId, detail, status), status);
    card.addEventListener('change', schedule);     // channel + enable
    card.addEventListener('ms-change', schedule);   // ping-role chips
    card.addEventListener('input', (e) => { if (e.target.closest('.dash-mw-item')) schedule(); });
    card.addEventListener('click', (e) => {
      if (e.target.closest('[data-act="mw-add"]')) {
        card.querySelector('[data-act="mw-items"]').insertAdjacentHTML('beforeend', mwItemRow('', null));
        return;
      }
      const rm = e.target.closest('[data-act="mw-remove"]');
      if (rm) { rm.closest('.dash-mw-item').remove(); schedule(); }
    });
  }

  async function doSaveMarketWatch(card, guildId, detail, status) {
    const channel = card.querySelector('[data-field="mw-channel"]').value || null;
    const enabled = card.querySelector('[data-field="mw-enabled"]').checked;
    const ms = card.querySelector('.dash-ms');
    const ping_role_ids = ms
      ? [...ms.querySelectorAll('.dash-ms-opt.is-selected')].map((o) => o.dataset.id) : [];
    const items = [...card.querySelectorAll('.dash-mw-item')].map((row) => {
      const name = row.querySelector('.dash-mw-name').value.trim();
      const raw = row.querySelector('.dash-mw-price').value.trim();
      const price = raw === '' ? null : Number(raw);
      return { name, max_price_each: (price != null && isFinite(price) && price > 0) ? price : null };
    }).filter((it) => it.name);
    const r = await Auth.callJSON(`/v1/site-auth/discord/guilds/${encodeURIComponent(guildId)}/market-watch`, {
      method: 'PUT', json: { enabled, channel_id: channel, ping_role_ids, items },
    });
    if (!r.ok) { setStatus(status, t('Save failed'), 'error'); return; }
    Object.assign(detail, r.data);
    setStatus(status, t('Saved'), 'saved');
  }

  function annRow(a, detail, canManage, canPing) {
    const chOpts = `<option value="">${esc(t('— none —'))}</option>` +
      (detail.channels || []).map((c) =>
        `<option value="${esc(c.id)}" ${c.id === a.channel_id ? 'selected' : ''}>#${esc(c.name)}</option>`).join('');
    const manageDis = canManage ? '' : 'disabled';
    const warn = a.channel_missing
      ? `<span class="dash-ann-warn">⚠ ${esc(t('channel deleted'))}</span>` : '';
    return `
      <div class="dash-ann-row" data-key="${esc(a.key)}">
        <div class="dash-ann-head">
          <label class="dash-discord-check">
            <input type="checkbox" data-field="enabled" ${a.enabled ? 'checked' : ''} ${manageDis}>
            <span class="dash-ann-label">${esc(t(a.label))}</span>
          </label>
          ${warn}
        </div>
        <p class="dash-ann-desc">${esc(t(a.description))}</p>
        <div class="dash-ann-controls">
          <label class="dash-discord-field">
            <span class="dash-discord-label">${esc(t('Channel'))}</span>
            <select data-field="channel" ${manageDis}>${chOpts}</select>
          </label>
          <div class="dash-discord-field">
            <span class="dash-discord-label">${esc(t('Ping roles'))}</span>
            ${roleMultiSelect(detail.roles, a.ping_role_ids, !canPing)}
          </div>
        </div>
        <p class="dash-discord-result" data-act="row-preflight" hidden></p>
        ${a.customizable && canManage ? `<button type="button" class="dash-btn dash-btn-mini dash-btn-ghost dash-ann-customize" data-key="${esc(a.key)}">${esc(t('Customize embed'))}</button>
        <div class="dash-ann-embed" data-embed-key="${esc(a.key)}" hidden></div>` : ''}
      </div>`;
  }

  // Open/close + lazily mount the shared embed editor for one announcement type.
  // Edits are isolated from the announcements card's auto-saver (stopPropagation),
  // and saved via the dedicated per-type template endpoint.
  function toggleAnnEmbed(card, guildId, key, detail) {
    const sel = (window.CSS && CSS.escape) ? CSS.escape(key) : key;
    const box = card.querySelector('[data-embed-key="' + sel + '"]');
    if (!box) return;
    if (!box.hidden) { box.hidden = true; return; }
    box.hidden = false;
    if (box._mounted) return;
    box._mounted = true;
    const meta = _annMeta[key];
    if (!meta) { box.textContent = t('Editor unavailable.'); return; }
    const a = (detail.announcements || []).find((x) => x.key === key) || {};
    const edEl = document.createElement('div');
    box.appendChild(edEl);
    ['change', 'input', 'ms-change'].forEach((ev) =>
      edEl.addEventListener(ev, (e) => e.stopPropagation()));
    const ctrl = window.EmbedEditor.mount(edEl, {
      meta, template: a.template || null, hasImage: true, designs: _designs,
    });
    const save = document.createElement('button');
    save.type = 'button'; save.className = 'dash-btn dash-btn-mini'; save.textContent = t('Save embed');
    const msg = document.createElement('span'); msg.className = 'wh-cust-msg';
    save.addEventListener('click', async () => {
      save.disabled = true; msg.textContent = '';
      const r = await Auth.callJSON(
        `/v1/site-auth/discord/guilds/${encodeURIComponent(guildId)}/announcements/${encodeURIComponent(key)}/template`,
        { method: 'PUT', json: { template: ctrl.getTemplate() } });
      save.disabled = false;
      msg.textContent = r.ok ? t('Saved.') : (Auth.errorMessage(r.data) || t('Failed to save.'));
      if (r.ok && r.data) {
        const row = (r.data.announcements || []).find((x) => x.key === key);
        a.template = row ? row.template : null;
      }
    });
    const footer = document.createElement('div');
    footer.className = 'wh-cust-footer';
    footer.appendChild(save); footer.appendChild(msg);
    box.appendChild(footer);
  }

  // ── chip / token multi-select (vanilla; no jQuery) ──
  function roleColor(r) {
    return r.color ? '#' + (r.color >>> 0).toString(16).padStart(6, '0') : 'var(--text-mute, #8a93a3)';
  }

  function roleMultiSelect(roles, selectedIds, disabled) {
    const sel = new Set(selectedIds || []);
    const opts = (roles || []).length
      ? roles.map((r) =>
          `<div class="dash-ms-opt${sel.has(r.id) ? ' is-selected' : ''}" data-id="${esc(r.id)}" data-name="@${esc(r.name)}" data-color="${esc(roleColor(r))}">
             <span class="dash-ms-check">✓</span>
             <span class="dash-ms-dot" style="background:${esc(roleColor(r))}"></span>
             <span>@${esc(r.name)}</span>
           </div>`).join('')
      : `<div class="dash-ms-empty">${esc(t('No assignable roles in this server.'))}</div>`;
    return `<div class="dash-ms${disabled ? ' is-disabled' : ''}">
        <div class="dash-ms-box" tabindex="0" role="button"></div>
        <div class="dash-ms-menu" hidden>${opts}</div>
      </div>`;
  }

  function msRenderChips(ms) {
    const box = ms.querySelector('.dash-ms-box');
    const selected = [...ms.querySelectorAll('.dash-ms-opt.is-selected')];
    const chips = selected.map((o) =>
      `<span class="dash-ms-chip">
         <span class="dash-ms-dot" style="background:${esc(o.dataset.color)}"></span>${esc(o.dataset.name)}
         <button type="button" class="dash-ms-x" data-id="${esc(o.dataset.id)}" aria-label="${esc(t('Remove'))}">×</button>
       </span>`).join('');
    box.innerHTML = chips + (selected.length ? '' : `<span class="dash-ms-ph">${esc(t('Add roles…'))}</span>`);
  }

  // One delegated handler per card; the card is recreated each render (no leak).
  function wireMultiSelects(scope) {
    scope.querySelectorAll('.dash-ms').forEach(msRenderChips);
    scope.addEventListener('click', (e) => {
      const x = e.target.closest('.dash-ms-x');
      if (x) {
        const ms = x.closest('.dash-ms');
        const opt = ms.querySelector(`.dash-ms-opt[data-id="${CSS.escape(x.dataset.id)}"]`);
        if (opt) opt.classList.remove('is-selected');
        msRenderChips(ms);
        ms.dispatchEvent(new Event('ms-change', { bubbles: true }));
        e.stopPropagation();
        return;
      }
      const opt = e.target.closest('.dash-ms-opt');
      if (opt && opt.dataset.id) {
        if (opt.closest('.dash-ms').classList.contains('is-disabled')) return;
        opt.classList.toggle('is-selected');
        const ms = opt.closest('.dash-ms');
        msRenderChips(ms);
        ms.dispatchEvent(new Event('ms-change', { bubbles: true }));
        return;
      }
      const box = e.target.closest('.dash-ms-box');
      if (box) {
        const ms = box.closest('.dash-ms');
        if (ms.classList.contains('is-disabled')) return;
        const menu = ms.querySelector('.dash-ms-menu');
        const open = menu.hidden;
        scope.querySelectorAll('.dash-ms-menu').forEach((m) => { if (m !== menu) m.hidden = true; });
        menu.hidden = !open;
      }
    });
  }

  // Close any open menu when clicking outside it (wired once for the document).
  function _msEnsureOutsideClose() {
    if (window.__dashMsDocWired) return;
    window.__dashMsDocWired = true;
    document.addEventListener('click', (e) => {
      document.querySelectorAll('.dash-ms-menu:not([hidden])').forEach((menu) => {
        const ms = menu.closest('.dash-ms');
        if (!ms || !ms.contains(e.target)) menu.hidden = true;
      });
    });
  }

  // ── debounced auto-save (no Save buttons; saves ~2s after the last change) ──
  function makeAutoSaver(fn, statusEl, delay = 2000) {
    let timer = null;
    return function schedule() {
      clearTimeout(timer);
      setStatus(statusEl, t('Saving…'), 'saving');
      timer = setTimeout(fn, delay);
    };
  }

  function setStatus(el, text, state) {
    if (!el) return;
    el.textContent = text;
    el.dataset.state = state || '';
    if (state === 'saved') {
      setTimeout(() => {
        if (el.dataset.state === 'saved') { el.textContent = t('Auto-saves'); el.dataset.state = ''; }
      }, 2500);
    }
  }

  function renderPreflightInto(el, p) {
    if (!el) return;
    if (!p) { el.hidden = true; return; }
    if (p.error) { showDiscordResult(el, p.error, 'error'); return; }
    if (p.ok) { showDiscordResult(el, t('✓ The bot can post here.'), 'success'); return; }
    showDiscordResult(el, t('⚠ The bot is missing permissions here: {x}.')
      .replace('{x}', (p.missing || []).join(', ')), 'error');
  }

  function renderRowPreflight(row, a) {
    renderPreflightInto(row.querySelector('[data-act="row-preflight"]'), a && a.preflight);
    const head = row.querySelector('.dash-ann-head');
    let warn = head.querySelector('.dash-ann-warn');
    if (a && a.channel_missing && !warn) {
      warn = document.createElement('span');
      warn.className = 'dash-ann-warn';
      warn.textContent = '⚠ ' + t('channel deleted');
      head.appendChild(warn);
    } else if ((!a || !a.channel_missing) && warn) {
      warn.remove();
    }
  }

  async function doSaveAnnouncements(body, guildId, detail, status) {
    const announcements = {};
    body.querySelectorAll('.dash-ann-row').forEach((row) => {
      const ms = row.querySelector('.dash-ms');
      announcements[row.dataset.key] = {
        enabled: row.querySelector('[data-field="enabled"]').checked,
        channel_id: row.querySelector('[data-field="channel"]').value || null,
        ping_role_ids: ms
          ? [...ms.querySelectorAll('.dash-ms-opt.is-selected')].map((o) => o.dataset.id) : [],
      };
    });
    const r = await Auth.callJSON(`/v1/site-auth/discord/guilds/${encodeURIComponent(guildId)}/announcements`, {
      method: 'PUT', json: { announcements },
    });
    if (!r.ok) { setStatus(status, Auth.errorMessage(r.data) || t('Save failed'), 'error'); return; }
    Object.assign(detail, r.data);
    // Update preflight + deleted-channel badges in place - no full re-render, so an
    // open chip menu / focus survives the auto-save.
    body.querySelectorAll('.dash-ann-row').forEach((row) => {
      renderRowPreflight(row, (detail.announcements || []).find((x) => x.key === row.dataset.key));
    });
    updateAnnouncingUI(guildId, !!detail.announcing);
    setStatus(status, t('Saved'), 'saved');
  }

  // Reflect the "announcing" state in the detail header badge + the side-menu dot.
  function updateAnnouncingUI(guildId, on) {
    const badge = document.querySelector('#dash-discord-detail [data-act="enabled-badge"]');
    if (badge) {
      badge.className = 'dash-tag' + (on ? ' dash-tag-verified' : '');
      badge.textContent = on ? t('announcing') : t('off');
    }
    const menuItem = [...document.querySelectorAll('.dash-discord-menu-item')].find((b) => b.dataset.guild === guildId);
    if (menuItem) {
      let live = menuItem.querySelector('.dash-discord-live');
      if (on && !live) {
        live = document.createElement('span');
        live.className = 'dash-discord-live';
        live.title = t('Announcing');
        menuItem.appendChild(live);
      } else if (!on && live) {
        live.remove();
      }
    }
  }

  // ── Clubs tab (Discord-side Trove club proxies; manage_clubs) ──
  // Up to N clubs per server. Each: metadata + a Discord role per in-game rank.
  // Loaded fresh (the GET re-checks live roles + drops links to deleted ones).
  async function renderClubsTab(body, guildId) {
    body.innerHTML = `<p class="dash-loading">${esc(t('Loading…'))}</p>`;
    const r = await Auth.callJSON(`/v1/site-auth/discord/guilds/${encodeURIComponent(guildId)}/clubs`);
    if (!r.ok) {
      body.innerHTML = `<article class="dash-card"><p class="dash-error">${esc(Auth.errorMessage(r.data) || t('Failed to load'))}</p></article>`;
      return;
    }
    renderClubs(body, guildId, r.data);
  }

  // A row of club sub-tabs (one per club) with "Add club" on the right; selecting
  // a tab opens that club's menu (config + roster) below.
  function renderClubs(body, guildId, data) {
    const clubs = data.clubs || [];
    const max = data.max_clubs || 3;
    const canAdd = clubs.length < max;
    body.innerHTML = `
      <div class="dash-club-bar">
        <div class="dash-club-tablist" role="tablist">
          ${clubs.map((c, i) => `<button type="button" class="dash-club-tab${i === 0 ? ' active' : ''}" data-club-tab="${esc(c.id)}">${esc(c.name)}</button>`).join('')
            || `<span class="dash-card-sub-mini">${esc(t('No clubs yet.'))}</span>`}
        </div>
        <button type="button" class="dash-btn dash-btn-mini" data-act="add-club" ${canAdd ? '' : 'disabled'}
          title="${canAdd ? '' : esc(t('Maximum of {n} clubs reached.').replace('{n}', max))}">${esc(t('+ Add club'))}</button>
      </div>
      <div id="dash-club-menu"></div>
      <p class="dash-discord-result" data-act="club-add-result" hidden></p>`;

    const addBtn = body.querySelector('[data-act="add-club"]');
    if (addBtn) addBtn.addEventListener('click', () => addClub(body, guildId, addBtn));

    const menu = body.querySelector('#dash-club-menu');
    const tabs = [...body.querySelectorAll('[data-club-tab]')];
    const showClub = (id) => {
      tabs.forEach((b) => b.classList.toggle('active', b.dataset.clubTab === id));
      const club = clubs.find((c) => c.id === id);
      if (club) renderClubMenu(menu, body, guildId, club, data);
    };
    tabs.forEach((b) => b.addEventListener('click', () => showClub(b.dataset.clubTab)));
    if (clubs.length) showClub(clubs[0].id);
    else menu.innerHTML = `<article class="dash-card"><p class="dash-empty">${esc(t('Create a club to set its details, rank roles, and roster.'))}</p></article>`;
  }

  // The selected club's menu: its config card + the roster table.
  function renderClubMenu(menu, tabBody, guildId, club, data) {
    menu.innerHTML = clubConfigCard(club, data) +
      `<article class="dash-card" id="dash-club-roster"><div class="dash-loading">${esc(t('Loading roster…'))}</div></article>`;
    wireClubCard(tabBody, guildId, menu.querySelector('.dash-club'), data);
    loadRoster(menu.querySelector('#dash-club-roster'), guildId, club.id);
  }

  function clubConfigCard(c, data) {
    const roles = data.roles || [];
    const roleOpts = (selId) => `<option value="">${esc(t('— none —'))}</option>` +
      roles.map((r) => `<option value="${esc(r.id)}" ${r.id === selId ? 'selected' : ''}>@${esc(r.name)}</option>`).join('');
    const links = c.role_links || {};
    const ranks = (data.ranks || []).map((rk) => `
      <label class="dash-discord-field">
        <span class="dash-discord-label">${esc(rk.label)}</span>
        <select data-rank="${esc(rk.key)}">${roleOpts(links[rk.key] || '')}</select>
      </label>`).join('');
    const fld = (field, label, ph) => `
      <label class="dash-discord-field">
        <span class="dash-discord-label">${esc(label)}</span>
        <input data-field="${field}" value="${esc(c[field] || '')}" placeholder="${esc(ph)}" maxlength="500">
      </label>`;
    return `
      <article class="dash-card dash-club" data-club="${esc(c.id)}">
        <div class="dash-ann-head">
          <input class="dash-club-name" data-field="name" value="${esc(c.name)}" placeholder="${esc(t('Club name'))}" maxlength="100">
          <span class="dash-autosave" data-act="club-status">${esc(t('Auto-saves'))}</span>
          <button type="button" class="dash-club-del" data-act="del-club">${esc(t('Delete'))}</button>
        </div>
        <label class="dash-discord-check">
          <input type="checkbox" data-field="public" ${c.public ? 'checked' : ''}>
          <span>${esc(t('Show publicly'))}</span>
        </label>
        <label class="dash-discord-field dash-club-wide">
          <span class="dash-discord-label">${esc(t('Description'))}</span>
          <textarea data-field="description" rows="2" maxlength="1000">${esc(c.description || '')}</textarea>
        </label>
        <div class="dash-discord-fields">
          ${fld('banner_url', t('Banner URL'), 'https://…')}
          ${fld('avatar_url', t('Profile picture URL'), 'https://…')}
          ${fld('discord_url', t('Discord link'), 'https://discord.gg/…')}
          ${fld('website_url', t('Website link'), 'https://…')}
        </div>
        <p class="dash-discord-label" style="margin-top:10px">${esc(t('Link in-game ranks to Discord roles'))}</p>
        <div class="dash-discord-fields">${ranks}</div>
        <p class="dash-discord-result" data-act="club-result" hidden></p>
      </article>`;
  }

  function wireClubCard(tabBody, guildId, card, data) {
    const status = card.querySelector('[data-act="club-status"]');
    const schedule = makeAutoSaver(() => doSaveClub(card, guildId, status), status);
    card.addEventListener('input', schedule);    // text inputs + textarea
    card.addEventListener('change', schedule);    // checkbox + rank selects
    card.querySelector('[data-act="del-club"]')
      .addEventListener('click', () => deleteClub(tabBody, guildId, card.dataset.club));
  }

  async function doSaveClub(card, guildId, status) {
    const get = (f) => card.querySelector(`[data-field="${f}"]`);
    const role_links = {};
    card.querySelectorAll('[data-rank]').forEach((sel) => { if (sel.value) role_links[sel.dataset.rank] = sel.value; });
    const payload = {
      name: get('name').value.trim(),
      public: get('public').checked,
      description: get('description').value,
      banner_url: get('banner_url').value.trim() || null,
      avatar_url: get('avatar_url').value.trim() || null,
      discord_url: get('discord_url').value.trim() || null,
      website_url: get('website_url').value.trim() || null,
      role_links,
    };
    if (!payload.name) { setStatus(status, t('Name required'), 'error'); return; }
    const r = await Auth.callJSON(
      `/v1/site-auth/discord/guilds/${encodeURIComponent(guildId)}/clubs/${encodeURIComponent(card.dataset.club)}`,
      { method: 'PUT', json: payload });
    setStatus(status, r.ok ? t('Saved') : (Auth.errorMessage(r.data) || t('Save failed')), r.ok ? 'saved' : 'error');
  }

  async function addClub(tabBody, guildId, addBtn) {
    addBtn.disabled = true;
    const r = await Auth.callJSON(`/v1/site-auth/discord/guilds/${encodeURIComponent(guildId)}/clubs`, { method: 'POST' });
    if (!r.ok) {
      showDiscordResult(tabBody.querySelector('[data-act="club-add-result"]'), Auth.errorMessage(r.data) || t('Failed to add club.'), 'error');
      addBtn.disabled = false;
      return;
    }
    renderClubsTab(tabBody, guildId);   // reload (re-checks roles + shows the new club)
  }

  async function deleteClub(tabBody, guildId, clubId) {
    const ok = await confirmModal({
      title: t('Delete club'),
      message: t('Delete this club? This can’t be undone.'),
      confirm: t('Delete'),
      danger: true,
    });
    if (!ok) return;
    const r = await Auth.callJSON(`/v1/site-auth/discord/guilds/${encodeURIComponent(guildId)}/clubs/${encodeURIComponent(clubId)}`, { method: 'DELETE' });
    if (r.ok) renderClubsTab(tabBody, guildId);
  }

  // ── Club roster (members of each linked rank role, grouped + collapsible) ──
  async function loadRoster(el, guildId, clubId) {
    const r = await Auth.callJSON(`/v1/site-auth/discord/guilds/${encodeURIComponent(guildId)}/clubs/${encodeURIComponent(clubId)}/roster`);
    if (!r.ok) { el.innerHTML = `<p class="dash-error">${esc(Auth.errorMessage(r.data) || t('Failed to load roster'))}</p>`; return; }
    renderRoster(el, r.data, guildId, clubId);
  }

  function renderRoster(el, data, guildId, clubId) {
    if (!data.available) {
      el.innerHTML = `<h3 class="dash-discord-card-title">${esc(t('Club roster'))}</h3>
        <p class="dash-card-sub">${esc(data.error || t('The roster needs the Server Members intent enabled for the bot.'))}</p>`;
      return;
    }
    const ranks = data.ranks || [];
    const groups = ranks.map((rk) => rosterGroup(rk, (data.roster || {})[rk.key] || [])).join('');
    el.innerHTML = `
      <h3 class="dash-discord-card-title">${esc(t('Club roster'))}</h3>
      <p class="dash-card-sub-mini">${esc(t('Members holding each linked rank role. Use the arrows to promote or demote; click a rank to collapse.'))}</p>
      ${groups || `<p class="dash-card-sub-mini">${esc(t('Link some ranks to Discord roles above to populate the roster.'))}</p>`}
      <p class="dash-discord-result" data-act="roster-result" hidden></p>`;
    el.querySelectorAll('.dash-roster-cat').forEach((cat) =>
      cat.addEventListener('click', () => cat.closest('.dash-roster-group').classList.toggle('collapsed')));
    el.querySelectorAll('.dash-roster-act:not([disabled])').forEach((btn) =>
      btn.addEventListener('click', () => onRosterAction(btn, el, guildId, clubId)));
  }

  function rosterGroup(rk, members) {
    const rows = members.length
      ? members.map((m) => `
        <div class="dash-roster-row" data-member="${esc(m.id)}" data-name="${esc(m.name)}" data-rank-label="${esc(rk.label)}">
          ${memberAvatar(m)}
          <span class="dash-roster-name">${esc(m.name)}</span>
          <span class="dash-roster-acts">
            ${rosterActBtn('promote', '↑', rk.promote_to, rk.promote_label)}
            ${rosterActBtn('demote', '↓', rk.demote_to, rk.demote_label)}
          </span>
        </div>`).join('')
      : `<div class="dash-roster-empty">${esc(t('No one with this rank.'))}</div>`;
    return `
      <div class="dash-roster-group">
        <button type="button" class="dash-roster-cat">
          <span class="dash-roster-caret">▸</span>
          <span class="dash-roster-rank">${esc(rk.label)}</span>
          <span class="dash-roster-count">${members.length}</span>
        </button>
        <div class="dash-roster-members">${rows}</div>
      </div>`;
  }

  // One promote/demote arrow. Disabled (with a reason) when there's no target rank.
  function rosterActBtn(action, glyph, toKey, toLabel) {
    if (!toKey) {
      const why = action === 'promote' ? t('Already at the top rank') : t('Already at the lowest rank');
      return `<button type="button" class="dash-roster-act" data-act="${action}" disabled title="${esc(why)}">${glyph}</button>`;
    }
    const tip = (action === 'promote' ? t('Promote to {r}') : t('Demote to {r}')).replace('{r}', toLabel);
    return `<button type="button" class="dash-roster-act dash-roster-act-${action}" data-act="${action}" data-to-label="${esc(toLabel)}" title="${esc(tip)}">${glyph}</button>`;
  }

  async function onRosterAction(btn, el, guildId, clubId) {
    const row = btn.closest('.dash-roster-row');
    const action = btn.dataset.act;                       // 'promote' | 'demote'
    const memberId = row.dataset.member;
    const name = row.dataset.name;
    const fromLabel = row.dataset.rankLabel;
    const toLabel = btn.dataset.toLabel;
    const verb = action === 'promote' ? t('promote') : t('demote');
    const ok = await confirmModal({
      title: action === 'promote' ? t('Promote member') : t('Demote member'),
      message: t('Are you sure you want to {verb} {name} from {from} to {to}?')
        .replace('{verb}', verb).replace('{name}', name).replace('{from}', fromLabel).replace('{to}', toLabel),
      confirm: action === 'promote' ? t('Promote') : t('Demote'),
      danger: action === 'demote',
    });
    if (!ok) return;
    el.querySelectorAll('.dash-roster-act').forEach((b) => { b.disabled = true; });
    const r = await Auth.callJSON(
      `/v1/site-auth/discord/guilds/${encodeURIComponent(guildId)}/clubs/${encodeURIComponent(clubId)}/roster/${encodeURIComponent(memberId)}/${action}`,
      { method: 'POST' });
    if (r.ok) {
      loadRoster(el, guildId, clubId);                   // re-fetch -> the member visibly moves group
    } else {
      showDiscordResult(el.querySelector('[data-act="roster-result"]'),
        Auth.errorMessage(r.data) || t('Couldn’t change the rank.'), 'error');
      el.querySelectorAll('.dash-roster-act').forEach((b) => {
        if (b.dataset.toLabel) b.disabled = false;       // re-enable only the ones that had a target
      });
    }
  }

  function memberAvatar(m) {
    return m.avatar
      ? `<img class="dash-roster-av" src="${esc(m.avatar)}" alt="" referrerpolicy="no-referrer">`
      : `<span class="dash-roster-av dash-roster-av-fb">${esc((m.name || '?')[0].toUpperCase())}</span>`;
  }

  // Reusable confirmation modal -> resolves true (confirm) / false (cancel|esc|backdrop).
  // Replaces window.confirm so the dashboard's destructive actions get a real,
  // styled "Are you sure?" step (promote / demote / delete club).
  function confirmModal({ title, message, confirm = t('Confirm'), cancel = t('Cancel'), danger = false }) {
    return new Promise((resolve) => {
      const ov = document.createElement('div');
      ov.className = 'dash-modal-overlay';
      ov.innerHTML = `
        <div class="dash-modal" role="dialog" aria-modal="true" aria-labelledby="dash-modal-title">
          <h3 class="dash-modal-title" id="dash-modal-title">${esc(title)}</h3>
          <p class="dash-modal-message">${esc(message)}</p>
          <div class="dash-modal-actions">
            <button type="button" class="dash-btn dash-btn-mini dash-btn-ghost" data-act="cancel">${esc(cancel)}</button>
            <button type="button" class="dash-btn dash-btn-mini${danger ? ' dash-btn-danger' : ''}" data-act="ok">${esc(confirm)}</button>
          </div>
        </div>`;
      let release = null;
      const close = (val) => {
        if (release) release();
        document.removeEventListener('keydown', onKey);
        ov.remove();
        resolve(val);
      };
      // Escape cancels; Enter is left to the focused button (OK is focused on open),
      // so it never confirms a destructive action while Cancel has focus.
      const onKey = (e) => { if (e.key === 'Escape') close(false); };
      ov.addEventListener('click', (e) => { if (e.target === ov) close(false); });
      ov.querySelector('[data-act="cancel"]').addEventListener('click', () => close(false));
      ov.querySelector('[data-act="ok"]').addEventListener('click', () => close(true));
      document.body.appendChild(ov);
      // Move/trap focus in the dialog + restore to the opener on close; trapFocus
      // owns Escape. Fall back to a manual listener if the helper is unavailable.
      const panel = ov.querySelector('.dash-modal');
      const okBtn = ov.querySelector('[data-act="ok"]');
      if (window.BTTUtil && window.BTTUtil.trapFocus) {
        release = window.BTTUtil.trapFocus(panel, { onEscape: () => close(false), initialFocus: okBtn });
      } else {
        document.addEventListener('keydown', onKey);
        okBtn.focus();
      }
    });
  }

  // ── Settings tab (role-delegated config permissions; admin-only) ──
  function renderSettingsTab(body, guildId, detail) {
    if (!detail.is_admin) {
      body.innerHTML = `<article class="dash-card">
        <h3 class="dash-discord-card-title">${esc(t('Settings'))}</h3>
        <p class="dash-card-sub">${esc(t('Only the server owner or Manage-Server admins can change who can configure the bot.'))}</p>
      </article>`;
      return;
    }
    body.innerHTML = `<article class="dash-card" data-card="perms">${discordPermsHtml(detail)}</article>`;
    const card = body.querySelector('[data-card="perms"]');
    const status = card.querySelector('[data-act="perms-status"]');
    const schedule = makeAutoSaver(() => doSavePermissions(card, guildId, status), status);
    card.addEventListener('change', schedule);
  }

  // Owner/admin-only: delegate each config capability to Discord roles.
  function discordPermsHtml(detail) {
    const roles = detail.roles || [];
    const perms = detail.permissions || {};
    const caps = (detail.capabilities || []).map((cap) => {
      const selected = new Set(perms[cap.key] || []);
      const roleChecks = roles.length
        ? roles.map((rl) => {
            const color = rl.color ? '#' + (rl.color >>> 0).toString(16).padStart(6, '0') : 'var(--text-mute, #8a93a3)';
            return `<label class="dash-discord-role">
                <input type="checkbox" data-cap="${esc(cap.key)}" value="${esc(rl.id)}" ${selected.has(rl.id) ? 'checked' : ''}>
                <span class="dash-discord-dot" style="background:${color}"></span>
                ${esc(rl.name)}
              </label>`;
          }).join('')
        : `<span class="dash-card-sub-mini">${esc(t('No assignable roles in this server.'))}</span>`;
      return `<div class="dash-discord-cap">
          <div class="dash-discord-cap-label">${esc(cap.label)}</div>
          <div>${roleChecks}</div>
        </div>`;
    }).join('');
    return `
      <div class="dash-ann-head">
        <h3 class="dash-discord-card-title">${esc(t('Who can configure'))}</h3>
        <span class="dash-autosave" data-act="perms-status">${esc(t('Auto-saves'))}</span>
      </div>
      <p class="dash-card-sub-mini">${esc(t('Delegate to Discord roles. You (server owner / Manage Server) always have access; a deleted role loses its grant automatically.'))}</p>
      ${caps}`;
  }

  async function doSavePermissions(card, guildId, status) {
    const permissions = {};
    card.querySelectorAll('input[data-cap]:checked').forEach((cb) => {
      (permissions[cb.dataset.cap] = permissions[cb.dataset.cap] || []).push(cb.value);
    });
    const r = await Auth.callJSON(`/v1/site-auth/discord/guilds/${encodeURIComponent(guildId)}/permissions`, {
      method: 'PUT', json: { permissions },
    });
    setStatus(status, r.ok ? t('Saved') : (Auth.errorMessage(r.data) || t('Save failed')), r.ok ? 'saved' : 'error');
  }

  function showDiscordResult(el, msg, tone) {
    if (!el) return;
    el.textContent = msg;
    el.dataset.tone = tone || 'info';
    el.hidden = false;
  }

  function formatScore(score) {
    return Number.isInteger(score)
      ? score.toLocaleString()
      : Number(score).toLocaleString(undefined, { maximumFractionDigits: 2 });
  }

  function formatDate(iso) {
    if (!iso) return '';
    const d = new Date(iso);
    if (isNaN(d.getTime())) return '';
    const pad = (n) => String(n).padStart(2, '0');
    return `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())}`;
  }

  function formatWhen(unix) {
    const d = new Date(unix * 1000);
    if (isNaN(d.getTime())) return '';
    const pad = (n) => String(n).padStart(2, '0');
    return `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())} ${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())} UTC`;
  }

  // Webhooks section. Outbound Discord webhooks: paste a channel webhook URL,
  // pick events, and Kiwi POSTs a rendered embed when each fires. Backend:
  // app/webhooks/router.py (site-JWT, /v1/webhooks/*).
  let _webhooksLoaded = false;
  let _whMeta = {};   // event type -> {variables, default_template, sample}

  // Saved Image Studio designs, shared by the webhook + bot embed editors' image picker.
  let _designs = [];
  let _designsLoaded = false;
  async function ensureDesigns() {
    if (_designsLoaded) return;
    _designsLoaded = true;
    try {
      const r = await Auth.callJSON('/v1/images');
      if (r.ok && r.data) _designs = (r.data.items || []).map((d) => ({ id: d.id, name: d.name, render_url: d.render_url }));
    } catch (e) { /* Image Studio may be disabled */ }
  }

  const WH_EVENTS = [
    { type: 'challenge', label: 'Hourly challenge', desc: 'Each new hourly challenge as it starts' },
    { type: 'mod_release', label: 'New mod release', desc: 'When a mod publishes a new release on the Mods Hub' },
    { type: 'game_update', label: 'Game update', desc: 'When a new Trove build goes live' },
  ];
  const WH_LABEL = Object.fromEntries(WH_EVENTS.map((e) => [e.type, e.label]));

  async function loadWebhooks() {
    const el = $('dash-webhooks-body');
    if (!el) return;
    el.innerHTML = `<p class="dash-loading">${esc(t('Loading…'))}</p>`;
    const [list, meta] = await Promise.all([
      Auth.callJSON('/v1/webhooks'),
      Object.keys(_whMeta).length ? Promise.resolve(null) : Auth.callJSON('/v1/webhooks/events'),
    ]);
    if (meta && meta.ok && meta.data) {
      (meta.data.meta || []).forEach((m) => { _whMeta[m.key] = m; });
    }
    await ensureDesigns();
    if (!list.ok) {
      el.innerHTML = `<article class="dash-card"><p class="dash-error">${esc(Auth.errorMessage(list.data) || t('Failed to load'))}</p></article>`;
      return;
    }
    renderWebhooks(el, (list.data && list.data.items) || []);
  }

  function renderWebhooks(el, items) {
    const evChecks = WH_EVENTS.map((e) =>
      `<label class="wh-ev-opt"><input type="checkbox" class="wh-ev" value="${esc(e.type)}" checked>
        <span><strong>${esc(t(e.label))}</strong><br><span class="dash-card-sub-mini">${esc(t(e.desc))}</span></span></label>`).join('');

    const createCard = `<article class="dash-card">
      <h2 class="dash-card-title">${esc(t('Add a webhook'))}</h2>
      <p class="dash-card-sub">${esc(t('In Discord: Server Settings → Integrations → Webhooks → New Webhook → Copy Webhook URL. Paste it below.'))}</p>
      <label class="wh-field"><span class="wh-label-text">${esc(t('Discord webhook URL'))}</span>
        <input id="wh-url" type="url" class="wh-input" placeholder="https://discord.com/api/webhooks/…" autocomplete="off"></label>
      <label class="wh-field"><span class="wh-label-text">${esc(t('Label (optional)'))}</span>
        <input id="wh-label" type="text" class="wh-input" maxlength="80" placeholder="${esc(t('e.g. My server #trove-news'))}"></label>
      <div class="wh-ev-list">${evChecks}</div>
      <p id="wh-error" class="dash-error" hidden></p>
      <button type="button" id="wh-add" class="dash-btn dash-btn-mini">${esc(t('Add webhook'))}</button>
    </article>`;

    _whItems = items;
    const list = items.length
      ? items.map(webhookCard).join('')
      : `<p class="dash-empty">${esc(t('No webhooks yet. Add one above.'))}</p>`;

    el.innerHTML = createCard + `<div class="wh-list">${list}</div>`;

    $('wh-add').addEventListener('click', () => createWebhook(el));
    el.querySelectorAll('[data-wh-act]').forEach((b) =>
      b.addEventListener('click', () => webhookAction(el, b.dataset.whAct, b.dataset.whId, b.dataset.whActive)));
  }

  function webhookCard(w) {
    const chips = (w.events || []).map((ev) =>
      `<span class="wh-chip">${esc(t(WH_LABEL[ev] || ev))}</span>`).join('');
    let status;
    if (!w.active) {
      status = `<span class="wh-status wh-status-off">${esc(t('Disabled'))}</span>`
        + (w.disabled_reason ? ` <span class="dash-card-sub-mini">${esc(w.disabled_reason)}</span>` : '');
    } else if (w.last_delivered_at) {
      status = `<span class="wh-status wh-status-on">${esc(t('Active'))}</span> <span class="dash-card-sub-mini">${esc(t('last delivered'))} ${esc(formatDate(w.last_delivered_at))}</span>`;
    } else {
      status = `<span class="wh-status wh-status-on">${esc(t('Active'))}</span> <span class="dash-card-sub-mini">${esc(t('no deliveries yet'))}</span>`;
    }
    return `<article class="dash-card wh-card">
      <div class="wh-card-head">
        <div>
          <p class="wh-card-title">${esc(w.label || t('Discord webhook'))}</p>
          <p class="wh-card-url">${esc(w.url)}</p>
        </div>
        <div class="wh-card-status">${status}</div>
      </div>
      <div class="wh-chips">${chips}</div>
      <div class="wh-card-actions">
        <button type="button" class="dash-btn dash-btn-mini dash-btn-ghost" data-wh-act="customize" data-wh-id="${esc(w.id)}">${esc(t('Customize embeds'))}</button>
        <button type="button" class="dash-btn dash-btn-mini dash-btn-ghost" data-wh-act="test" data-wh-id="${esc(w.id)}">${esc(t('Send test'))}</button>
        <button type="button" class="dash-btn dash-btn-mini dash-btn-ghost" data-wh-act="toggle" data-wh-id="${esc(w.id)}" data-wh-active="${w.active ? '1' : '0'}">${esc(w.active ? t('Disable') : t('Enable'))}</button>
        <button type="button" class="dash-btn dash-btn-mini dash-btn-danger" data-wh-act="delete" data-wh-id="${esc(w.id)}">${esc(t('Delete'))}</button>
      </div>
      <div class="wh-customize" data-wh-cust="${esc(w.id)}" hidden></div>
    </article>`;
  }

  // The webhook objects from the last load, so the customizer can read templates.
  let _whItems = [];

  function mountCustomizer(panel, w) {
    if (panel._mounted) return;
    panel._mounted = true;
    panel._editors = {};
    panel.innerHTML = '';
    (w.events || []).forEach((ev) => {
      const meta = _whMeta[ev];
      if (!meta) return;
      const block = document.createElement('div');
      block.className = 'wh-cust-event';
      const head = document.createElement('div');
      head.className = 'wh-cust-head';
      head.innerHTML = `<strong>${esc(t(WH_LABEL[ev] || ev))}</strong>`;
      const preview = document.createElement('button');
      preview.type = 'button'; preview.className = 'dash-btn dash-btn-mini dash-btn-ghost';
      preview.textContent = t('Send preview');
      preview.addEventListener('click', () => sendPreview(w.id, ev));
      head.appendChild(preview);
      block.appendChild(head);
      const edEl = document.createElement('div');
      block.appendChild(edEl);
      panel.appendChild(block);
      panel._editors[ev] = window.EmbedEditor.mount(edEl, {
        meta, template: (w.templates || {})[ev] || null, hasImage: true,
        designs: _designs,
      });
    });
    const save = document.createElement('button');
    save.type = 'button'; save.className = 'dash-btn dash-btn-mini'; save.textContent = t('Save embeds');
    const msg = document.createElement('span'); msg.className = 'wh-cust-msg';
    save.addEventListener('click', async () => {
      const templates = {};
      Object.keys(panel._editors).forEach((ev) => { templates[ev] = panel._editors[ev].getTemplate(); });
      save.disabled = true; msg.textContent = '';
      const r = await Auth.callJSON('/v1/webhooks/' + encodeURIComponent(w.id), {
        method: 'PATCH', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ templates }),
      });
      save.disabled = false;
      msg.textContent = r.ok ? t('Saved.') : (Auth.errorMessage(r.data) || t('Failed to save.'));
      if (r.ok && r.data) { w.templates = r.data.templates || {}; }
    });
    const footer = document.createElement('div');
    footer.className = 'wh-cust-footer';
    footer.appendChild(save); footer.appendChild(msg);
    panel.appendChild(footer);
  }

  async function sendPreview(id, ev) {
    const r = await Auth.callJSON('/v1/webhooks/' + encodeURIComponent(id) + '/test?event=' + encodeURIComponent(ev), { method: 'POST' });
    if (r.ok && r.data && r.data.ok) alert(t('Preview sent — check your Discord channel.'));
    else alert(t('Preview failed:') + ' ' + ((r.data && r.data.error) || Auth.errorMessage(r.data) || t('unknown error')));
  }

  async function createWebhook(el) {
    const err = $('wh-error');
    const url = ($('wh-url').value || '').trim();
    const label = ($('wh-label').value || '').trim();
    const events = Array.from(el.querySelectorAll('.wh-ev:checked')).map((c) => c.value);
    err.hidden = true;
    if (!url) { err.textContent = t('Paste your Discord webhook URL.'); err.hidden = false; return; }
    if (!events.length) { err.textContent = t('Pick at least one event.'); err.hidden = false; return; }
    const btn = $('wh-add'); btn.disabled = true;
    const r = await Auth.callJSON('/v1/webhooks', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ url, label, events }),
    });
    btn.disabled = false;
    if (!r.ok) { err.textContent = Auth.errorMessage(r.data) || t('Could not add that webhook.'); err.hidden = false; return; }
    loadWebhooks();
  }

  async function webhookAction(el, act, id, active) {
    if (act === 'customize') {
      const panel = el.querySelector('[data-wh-cust="' + (window.CSS && CSS.escape ? CSS.escape(id) : id) + '"]');
      if (!panel) return;
      if (panel.hidden) {
        const w = _whItems.find((x) => x.id === id);
        if (w) mountCustomizer(panel, w);
        panel.hidden = false;
      } else {
        panel.hidden = true;
      }
      return;
    }
    if (act === 'delete') {
      if (!confirm(t('Delete this webhook? Discord will stop receiving these events.'))) return;
      const r = await Auth.callJSON('/v1/webhooks/' + encodeURIComponent(id), { method: 'DELETE' });
      if (!r.ok && r.status !== 204) { alert(Auth.errorMessage(r.data) || t('Failed to delete.')); return; }
      loadWebhooks();
    } else if (act === 'toggle') {
      const r = await Auth.callJSON('/v1/webhooks/' + encodeURIComponent(id), {
        method: 'PATCH', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ active: active !== '1' }),
      });
      if (!r.ok) { alert(Auth.errorMessage(r.data) || t('Failed to update.')); return; }
      loadWebhooks();
    } else if (act === 'test') {
      const r = await Auth.callJSON('/v1/webhooks/' + encodeURIComponent(id) + '/test', { method: 'POST' });
      if (r.ok && r.data && r.data.ok) {
        alert(t('Test message sent — check your Discord channel.'));
      } else {
        const msg = (r.data && r.data.error) || Auth.errorMessage(r.data) || t('unknown error');
        alert(t('Test failed:') + ' ' + msg);
      }
    }
  }

  // DM Alerts section. Discord DM subscriptions: pick events (with per-type
  // challenge filters and a market price watchlist) and the Kiwi bot DMs you
  // when they fire. Backend: app/dm_subs/router.py (site-JWT, /v1/dm-subscriptions/*).
  let _dmsubsLoaded = false;
  let _dmItems = [];

  const DM_EVENTS = [
    { type: 'challenge', label: 'Hourly challenge', desc: 'Each new hourly challenge (filter by type below)' },
    { type: 'corruxion', label: 'Corruxion merchant', desc: 'When the Corruxion merchant arrives' },
    { type: 'fluxion', label: 'Fluxion merchant', desc: 'When the Fluxion merchant arrives' },
    { type: 'game_update', label: 'Game update', desc: 'When a new Trove build goes live' },
    { type: 'market_watch', label: 'Market watchlist', desc: 'When a watched item drops to your price' },
  ];
  const DM_LABEL = Object.fromEntries(DM_EVENTS.map((e) => [e.type, e.label]));
  const DM_CHALLENGE_TYPES = ['collection', 'rampage', 'racing', 'target', 'dungeon'];

  async function loadDmSubs() {
    const el = $('dash-dmsubs-body');
    if (!el) return;
    el.innerHTML = `<p class="dash-loading">${esc(t('Loading…'))}</p>`;
    const list = await Auth.callJSON('/v1/dm-subscriptions');
    if (!list.ok) {
      el.innerHTML = `<article class="dash-card"><p class="dash-error">${esc(Auth.errorMessage(list.data) || t('Failed to load'))}</p></article>`;
      return;
    }
    const d = list.data || {};
    renderDmSubs(el, d.items || [], d.discord_linked !== false);
  }

  function renderDmSubs(el, items, discordLinked) {
    _dmItems = items;
    if (!discordLinked) {
      el.innerHTML = `<article class="dash-card"><p class="dash-card-sub">${esc(t('Link your Discord account (sign in with Discord) to receive DM alerts.'))}</p></article>`;
      return;
    }

    const evChecks = DM_EVENTS.map((e) =>
      `<label class="wh-ev-opt"><input type="checkbox" class="dm-ev" value="${esc(e.type)}">
        <span><strong>${esc(t(e.label))}</strong><br><span class="dash-card-sub-mini">${esc(t(e.desc))}</span></span></label>`).join('');

    const typeChips = DM_CHALLENGE_TYPES.map((c) =>
      `<label class="dm-typechip"><input type="checkbox" class="dm-ctype" value="${c}"> ${esc(t(c[0].toUpperCase() + c.slice(1)))}</label>`).join('');

    const createCard = `<article class="dash-card">
      <h2 class="dash-card-title">${esc(t('Add a DM alert'))}</h2>
      <p class="dash-card-sub">${esc(t('Pick what you want the bot to DM you about.'))}</p>
      <label class="wh-field"><span class="wh-label-text">${esc(t('Label (optional)'))}</span>
        <input id="dm-label" type="text" class="wh-input" maxlength="80" placeholder="${esc(t('e.g. Dungeon challenges'))}"></label>
      <div class="wh-ev-list">${evChecks}</div>
      <div id="dm-challenge-filter" class="dm-subfilter" hidden>
        <p class="wh-label-text">${esc(t('Only these challenge types (all if none picked)'))}</p>
        <div class="dm-typechips">${typeChips}</div>
      </div>
      <div id="dm-watch-filter" class="dm-subfilter" hidden>
        <p class="wh-label-text">${esc(t('Watchlist — DM me when an item is at or below a price (each)'))}</p>
        <div id="dm-watch-rows"></div>
        <button type="button" id="dm-watch-add" class="dash-btn dash-btn-mini dash-btn-ghost">${esc(t('Add item'))}</button>
      </div>
      <p id="dm-error" class="dash-error" hidden></p>
      <button type="button" id="dm-add" class="dash-btn dash-btn-mini">${esc(t('Add alert'))}</button>
    </article>`;

    const list = items.length
      ? items.map(dmSubCard).join('')
      : `<p class="dash-empty">${esc(t('No DM alerts yet. Add one above.'))}</p>`;

    el.innerHTML = createCard + `<div class="wh-list">${list}</div>`;

    // Toggle the conditional filter blocks based on which events are checked.
    const syncFilters = () => {
      const checked = new Set(Array.from(el.querySelectorAll('.dm-ev:checked')).map((c) => c.value));
      $('dm-challenge-filter').hidden = !checked.has('challenge');
      $('dm-watch-filter').hidden = !checked.has('market_watch');
    };
    el.querySelectorAll('.dm-ev').forEach((c) => c.addEventListener('change', syncFilters));
    $('dm-watch-add').addEventListener('click', () => addWatchRow());
    $('dm-add').addEventListener('click', () => createDmSub(el));
    el.querySelectorAll('[data-dm-act]').forEach((b) =>
      b.addEventListener('click', () => dmSubAction(b.dataset.dmAct, b.dataset.dmId, b.dataset.dmActive)));
  }

  function addWatchRow(name, price) {
    const rows = $('dm-watch-rows');
    if (!rows) return;
    const row = document.createElement('div');
    row.className = 'dm-watch-row';
    row.innerHTML = `<input type="text" class="wh-input dm-watch-name" maxlength="120" placeholder="${esc(t('Item name'))}" value="${esc(name || '')}">
      <input type="number" class="wh-input dm-watch-price" min="1" step="1" placeholder="${esc(t('Max price/ea'))}" value="${price != null ? esc(price) : ''}">
      <button type="button" class="dash-btn dash-btn-mini dash-btn-ghost dm-watch-del">✕</button>`;
    row.querySelector('.dm-watch-del').addEventListener('click', () => row.remove());
    rows.appendChild(row);
  }

  function dmSubCard(s) {
    const chips = (s.events || []).map((ev) => {
      let extra = '';
      if (ev === 'challenge' && s.filters && s.filters.challenge_types && s.filters.challenge_types.length) {
        extra = ' · ' + s.filters.challenge_types.join(', ');
      }
      if (ev === 'market_watch' && s.filters && s.filters.watch && s.filters.watch.length) {
        extra = ' · ' + s.filters.watch.map((w) => `${w.name} ≤ ${w.max_price_each}`).join(', ');
      }
      return `<span class="wh-chip">${esc(t(DM_LABEL[ev] || ev))}${esc(extra)}</span>`;
    }).join('');
    let status;
    if (!s.active) {
      status = `<span class="wh-status wh-status-off">${esc(t('Disabled'))}</span>`
        + (s.disabled_reason ? ` <span class="dash-card-sub-mini">${esc(s.disabled_reason)}</span>` : '');
    } else if (s.last_delivered_at) {
      status = `<span class="wh-status wh-status-on">${esc(t('Active'))}</span> <span class="dash-card-sub-mini">${esc(t('last delivered'))} ${esc(formatDate(s.last_delivered_at))}</span>`;
    } else {
      status = `<span class="wh-status wh-status-on">${esc(t('Active'))}</span> <span class="dash-card-sub-mini">${esc(t('no deliveries yet'))}</span>`;
    }
    return `<article class="dash-card wh-card">
      <div class="wh-card-head">
        <div><p class="wh-card-title">${esc(s.label || t('DM alert'))}</p></div>
        <div class="wh-card-status">${status}</div>
      </div>
      <div class="wh-chips">${chips}</div>
      <div class="wh-card-actions">
        <button type="button" class="dash-btn dash-btn-mini dash-btn-ghost" data-dm-act="test" data-dm-id="${esc(s.id)}">${esc(t('Send test DM'))}</button>
        <button type="button" class="dash-btn dash-btn-mini dash-btn-ghost" data-dm-act="toggle" data-dm-id="${esc(s.id)}" data-dm-active="${s.active ? '1' : '0'}">${esc(s.active ? t('Disable') : t('Enable'))}</button>
        <button type="button" class="dash-btn dash-btn-mini dash-btn-danger" data-dm-act="delete" data-dm-id="${esc(s.id)}">${esc(t('Delete'))}</button>
      </div>
    </article>`;
  }

  function collectDmFilters(el, events) {
    const filters = {};
    if (events.includes('challenge')) {
      const types = Array.from(el.querySelectorAll('.dm-ctype:checked')).map((c) => c.value);
      if (types.length) filters.challenge_types = types;
    }
    if (events.includes('market_watch')) {
      const watch = [];
      el.querySelectorAll('.dm-watch-row').forEach((row) => {
        const name = (row.querySelector('.dm-watch-name').value || '').trim();
        const price = parseFloat(row.querySelector('.dm-watch-price').value);
        if (name && price > 0) watch.push({ name, max_price_each: price });
      });
      filters.watch = watch;
    }
    return filters;
  }

  async function createDmSub(el) {
    const err = $('dm-error');
    const label = ($('dm-label').value || '').trim();
    const events = Array.from(el.querySelectorAll('.dm-ev:checked')).map((c) => c.value);
    err.hidden = true;
    if (!events.length) { err.textContent = t('Pick at least one alert.'); err.hidden = false; return; }
    const filters = collectDmFilters(el, events);
    if (events.includes('market_watch') && (!filters.watch || !filters.watch.length)) {
      err.textContent = t('Add at least one item (with a price) to your watchlist.'); err.hidden = false; return;
    }
    const btn = $('dm-add'); btn.disabled = true;
    const r = await Auth.callJSON('/v1/dm-subscriptions', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ label, events, filters }),
    });
    btn.disabled = false;
    if (!r.ok) { err.textContent = Auth.errorMessage(r.data) || t('Could not add that alert.'); err.hidden = false; return; }
    loadDmSubs();
  }

  async function dmSubAction(act, id, active) {
    if (act === 'delete') {
      if (!confirm(t('Delete this DM alert?'))) return;
      const r = await Auth.callJSON('/v1/dm-subscriptions/' + encodeURIComponent(id), { method: 'DELETE' });
      if (!r.ok && r.status !== 204) { alert(Auth.errorMessage(r.data) || t('Failed to delete.')); return; }
      loadDmSubs();
    } else if (act === 'toggle') {
      const r = await Auth.callJSON('/v1/dm-subscriptions/' + encodeURIComponent(id), {
        method: 'PATCH', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ active: active !== '1' }),
      });
      if (!r.ok) { alert(Auth.errorMessage(r.data) || t('Failed to update.')); return; }
      loadDmSubs();
    } else if (act === 'test') {
      const r = await Auth.callJSON('/v1/dm-subscriptions/' + encodeURIComponent(id) + '/test', { method: 'POST' });
      if (r.ok && r.data && r.data.ok) {
        alert(t('Test DM sent — check your Discord DMs.'));
      } else {
        const msg = (r.data && r.data.error) || Auth.errorMessage(r.data) || t('unknown error');
        alert(t('Test failed:') + ' ' + msg);
      }
    }
  }

  function t(s) {
    return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s;
  }

})();
