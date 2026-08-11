/* ═══════════════════════════════════════════════════════════════════════
   /sound-studio - replace, silence or add Trove's sounds
   ───────────────────────────────────────────────────────────────────────
   The browser does the audio work. `decodeAudioData` already reads every
   format a user is likely to drop on this page (mp3, wav, ogg, flac, m4a),
   and an OfflineAudioContext resamples it to whatever the sound it's
   replacing runs at - so the upload is plain 16-bit samples and the server
   never needs an audio decoder of its own. It only writes Wwise's own
   encoding, which is the half a browser can't do.

   Nothing is uploaded until Build is pressed, and nothing is stored after
   the response is handed back.
   ═══════════════════════════════════════════════════════════════════════ */

(function () {
  'use strict';

  const { esc, fetchJSON, apiUrl } = window.BTTUtil;

  const BRANCH = 'live-us';
  // A sound bank is only worth showing if the game can still load it; these are
  // the ones with audio in them.
  const MAX_CLIP_SECONDS = 60;
  // Web Audio won't render outside this range, and a declared rate has to match
  // the rate we actually rendered at or the game plays it at the wrong speed.
  const MIN_RATE = 8000;
  const MAX_RATE = 48000;

  const state = {
    banks: null,
    bank: null,          // {path, size}
    sounds: [],
    filter: '',
    codec: 'all',
    changedOnly: false,
    edits: new Map(),    // media id -> {kind:'mute'} | {kind:'replace', clip}
    adds: [],            // [{event, clip}]
    loading: false,
    building: false,
  };

  const $ = (id) => document.getElementById(id);

  const $banks = $('ss-banks');
  const $work = $('ss-work');
  const $workHint = $('ss-work-hint');
  const $list = $('ss-list');
  const $filter = $('ss-filter');
  const $codec = $('ss-codec');
  const $changedOnly = $('ss-changed-only');
  const $changes = $('ss-changes');
  const $changeCount = $('ss-change-count');
  const $clear = $('ss-clear');
  const $addName = $('ss-add-name');
  const $addPick = $('ss-add-pick');
  const $addNote = $('ss-add-note');
  const $modFields = $('ss-modfields');
  const $modTitle = $('ss-mod-title');
  const $modAuthor = $('ss-mod-author');
  const $build = $('ss-build');
  const $status = $('ss-status');
  const $file = $('ss-file');

  const $player = $('ss-player');
  const $playerPlay = $('ss-player-play');
  const $playerName = $('ss-player-name');
  const $playerSub = $('ss-player-sub');
  const $playerTime = $('ss-player-time');
  const $playerClose = $('ss-player-close');

  // ─── Boot ──────────────────────────────────────────────────────────
  loadBanks().catch((err) => {
    $banks.innerHTML = errorHTML(err);
  });
  wire();

  // ─── Step 1: banks ─────────────────────────────────────────────────

  async function loadBanks() {
    const data = await fetchJSON(
      `/site/updates/${BRANCH}/search?q=${encodeURIComponent('.bnk')}&limit=200`);
    state.banks = (data.entries || [])
      .filter((e) => !e.is_dir && e.path.toLowerCase().endsWith('.bnk'))
      .sort((a, b) => b.size - a.size);
    renderBanks();
  }

  function renderBanks() {
    if (!state.banks.length) {
      $banks.innerHTML = `<p class="ss-empty">${esc(t('No sound banks were found.'))}</p>`;
      return;
    }
    $banks.innerHTML = state.banks.map((b) => {
      const name = b.path.slice(b.path.lastIndexOf('/') + 1);
      const active = state.bank && state.bank.path === b.path;
      return `<button type="button" class="ss-bank${active ? ' active' : ''}"
                data-bank="${esc(b.path)}">
        <i class="fa-solid fa-compact-disc" aria-hidden="true"></i>
        <span class="ss-bank-name">${esc(bankLabel(name))}</span>
        <span class="ss-bank-file">${esc(name)}</span>
        <span class="ss-bank-size">${esc(formatBytes(b.size))}</span>
      </button>`;
    }).join('');
  }

  // Bank file names are the audio team's own shorthand; say what's actually in
  // them so someone who has never opened a .bnk can still pick the right one.
  function bankLabel(file) {
    const known = {
      'ui.bnk': t('Interface & pickups'),
      'foley.bnk': t('Weapons, abilities & footsteps'),
      'mobs.bnk': t('Monsters & creatures'),
      'mus_main.bnk': t('Music'),
      'muzak_q00bz.bnk': t('Jukebox music'),
      'amb_biome_01.bnk': t('Biome ambience'),
      'amb_dynamic.bnk': t('Dynamic ambience'),
      'init.bnk': t('Setup (no sounds)'),
      'weapons.bnk': t('Unused'),
    };
    return known[file.toLowerCase()] || file.replace(/\.bnk$/i, '');
  }

  // ─── Step 2: the bank's sounds ─────────────────────────────────────

  async function openBank(path) {
    if (state.loading) return;
    if (state.bank && state.bank.path === path) return;
    if (state.edits.size || state.adds.length) {
      if (!window.confirm(t('Switching banks will discard the changes you have made. Continue?'))) return;
    }
    state.loading = true;
    state.bank = { path };
    state.sounds = [];
    state.edits.clear();
    state.adds = [];
    closePlayer();
    renderBanks();
    $work.hidden = false;
    $workHint.textContent = '';
    $list.innerHTML = `<p class="ss-loading">${esc(t('Loading…'))}</p>`;
    renderChanges();
    $work.scrollIntoView({ behavior: 'smooth', block: 'start' });

    try {
      const data = await fetchJSON(
        `/site/updates/${BRANCH}/file/bnk?path=${encodeURIComponent(path)}`);
      state.sounds = data.sounds || [];
      $workHint.textContent = t('{n} sounds in this bank. Play one to find what you are after.')
        .replace('{n}', formatInt(data.count || 0));
      if (!$modTitle.value) {
        $modTitle.value = t('My {bank} Sounds')
          .replace('{bank}', bankLabel(path.slice(path.lastIndexOf('/') + 1)));
      }
      renderList();
    } catch (err) {
      $list.innerHTML = errorHTML(err);
    } finally {
      state.loading = false;
    }
  }

  function visible() {
    const needle = state.filter.trim().toLowerCase();
    return state.sounds.filter((s) => {
      if (state.changedOnly && !state.edits.has(s.id)) return false;
      if (state.codec !== 'all' && s.codec !== state.codec) return false;
      if (!needle) return true;
      return (s.name || '').toLowerCase().includes(needle)
        || (s.group || '').toLowerCase().includes(needle)
        || String(s.id).includes(needle);
    });
  }

  function renderList() {
    const rows = visible();
    if (!rows.length) {
      $list.innerHTML = `<p class="ss-empty">${esc(
        state.changedOnly ? t('You have not changed anything yet.') : t('Nothing matches that filter.'))}</p>`;
      return;
    }
    $list.innerHTML = rows.slice(0, 500).map(rowHTML).join('')
      + (rows.length > 500
        ? `<p class="ss-empty">${esc(t('Showing the first 500 — narrow the filter to see the rest.'))}</p>`
        : '');
  }

  /** Repaint one row in place.
   *
   * Re-rendering the whole list on every toggle would throw away the scroll
   * position and the focused control, which in a list this long is the
   * difference between editing and hunting. Falls back to a full render when
   * the row is filtered out of the current view anyway. */
  function refreshRow(id) {
    const row = $list.querySelector(`.ss-row[data-id="${id}"]`);
    const sound = state.sounds.find((s) => s.id === id);
    if (!row || !sound) { renderList(); return; }
    if (state.changedOnly && !state.edits.has(id)) { renderList(); return; }
    const held = document.activeElement;
    const action = held && row.contains(held) ? held.dataset.act : null;
    row.outerHTML = rowHTML(sound);
    if (action) {
      const again = $list.querySelector(`.ss-row[data-id="${id}"] [data-act="${action}"]`);
      if (again) again.focus();
    }
  }

  function rowHTML(s) {
    const edit = state.edits.get(s.id);
    const label = s.name || `#${s.id}`;
    let mark = '';
    if (edit && edit.kind === 'mute') {
      mark = `<span class="ss-mark ss-mark-mute"><i class="fa-solid fa-volume-xmark" aria-hidden="true"></i> ${esc(t('Silenced'))}</span>`;
    } else if (edit) {
      mark = `<span class="ss-mark ss-mark-swap"><i class="fa-solid fa-arrows-rotate" aria-hidden="true"></i> ${esc(edit.clip.name)}</span>`;
    }
    return `<div class="ss-row${edit ? ' edited' : ''}" data-id="${s.id}">
      <button type="button" class="ss-icon ss-play" data-act="play" data-id="${s.id}"
              aria-label="${esc(t('Play')) + ' ' + esc(label)}"${s.error ? ' disabled' : ''}>
        <i class="fa-solid fa-play" aria-hidden="true"></i>
      </button>
      <span class="ss-row-id">
        <span class="ss-row-name" title="${esc(s.path || label)}">${esc(label)}</span>
        <span class="ss-row-sub">${esc(s.group || '')}${s.group ? ' · ' : ''}${esc(formatClock(s.duration))}${mark ? ' · ' : ''}</span>
      </span>
      ${mark}
      <span class="ss-row-actions">
        <button type="button" class="ss-chip" data-act="mute" data-id="${s.id}"
                aria-pressed="${edit && edit.kind === 'mute' ? 'true' : 'false'}">
          <i class="fa-solid fa-volume-xmark" aria-hidden="true"></i>
          <span data-i18n>Silence</span>
        </button>
        <button type="button" class="ss-chip" data-act="replace" data-id="${s.id}">
          <i class="fa-solid fa-file-audio" aria-hidden="true"></i>
          <span data-i18n>Replace</span>
        </button>
        ${edit ? `<button type="button" class="ss-icon" data-act="undo" data-id="${s.id}"
                    aria-label="${esc(t('Undo'))}"><i class="fa-solid fa-rotate-left" aria-hidden="true"></i></button>` : ''}
      </span>
    </div>`;
  }

  // ─── Changes + build panel ─────────────────────────────────────────

  function renderChanges() {
    const total = state.edits.size + state.adds.length;
    $changeCount.textContent = formatInt(total);
    $clear.hidden = !total;
    $build.disabled = !total || state.building;
    if (!total) {
      $changes.innerHTML =
        `<p class="ss-empty">${esc(t('Nothing changed yet. Mute a sound or replace one to get started.'))}</p>`;
      return;
    }
    const byId = new Map(state.sounds.map((s) => [s.id, s]));
    const parts = [];
    for (const [id, edit] of state.edits) {
      const sound = byId.get(id);
      const label = (sound && sound.name) || `#${id}`;
      parts.push(`<div class="ss-change">
        <i class="fa-solid ${edit.kind === 'mute' ? 'fa-volume-xmark' : 'fa-arrows-rotate'}" aria-hidden="true"></i>
        <span class="ss-change-name">${esc(label)}</span>
        <span class="ss-change-what">${esc(edit.kind === 'mute' ? t('silenced') : edit.clip.name)}</span>
        <button type="button" class="ss-icon" data-act="undo" data-id="${id}"
                aria-label="${esc(t('Undo'))}"><i class="fa-solid fa-xmark" aria-hidden="true"></i></button>
      </div>`);
    }
    state.adds.forEach((add, i) => {
      parts.push(`<div class="ss-change">
        <i class="fa-solid fa-plus" aria-hidden="true"></i>
        <span class="ss-change-name">${esc(add.event)}</span>
        <span class="ss-change-what">${esc(add.clip.name)}</span>
        <button type="button" class="ss-icon" data-act="unadd" data-index="${i}"
                aria-label="${esc(t('Undo'))}"><i class="fa-solid fa-xmark" aria-hidden="true"></i></button>
      </div>`);
    });
    $changes.innerHTML = parts.join('');
  }

  // ─── Audio in ──────────────────────────────────────────────────────

  let _pending = null;    // what the shared file input is currently collecting for

  function pickFile(target) {
    _pending = target;
    $file.value = '';
    $file.click();
  }

  async function onFilePicked() {
    const file = $file.files && $file.files[0];
    const target = _pending;
    _pending = null;
    if (!file || !target) return;

    // A replacement inherits the shape of the sound it stands in for, so it
    // drops into the game like for like; a brand-new sound keeps its own.
    let rate = 32000;
    let channels = 1;
    if (target.kind === 'replace') {
      const sound = state.sounds.find((s) => s.id === target.id);
      if (sound) {
        rate = sound.sample_rate || rate;
        channels = sound.channels === 2 ? 2 : 1;
      }
    }
    rate = Math.max(MIN_RATE, Math.min(MAX_RATE, rate));

    setStatus(t('Reading {file}…').replace('{file}', file.name));
    let clip;
    try {
      clip = await decodeToPcm(file, rate, channels);
    } catch (err) {
      setStatus(t('That file could not be read as audio.'), true);
      return;
    }
    clip.name = file.name;

    if (target.kind === 'replace') {
      state.edits.set(target.id, { kind: 'replace', clip });
      refreshRow(target.id);
    } else {
      state.adds.push({ event: target.event, clip });
      $addName.value = '';
      $addNote.hidden = false;
      $addNote.innerHTML = t('Added. To hear it in game, a modded interface file has to call {code} — nothing in Trove triggers it on its own.')
        .replace('{code}', `<code>POST_SOUND_EVENT "${esc(target.event)}"</code>`);
    }
    renderChanges();
    setStatus(t('{name} is ready ({dur}).')
      .replace('{name}', clip.name).replace('{dur}', formatClock(clip.duration)));
  }

  /** Decode any browser-readable audio file, resample it, and interleave to 16-bit. */
  async function decodeToPcm(file, rate, channels) {
    const Ctx = window.AudioContext || window.webkitAudioContext;
    const Offline = window.OfflineAudioContext || window.webkitOfflineAudioContext;
    if (!Ctx || !Offline) throw new Error('no Web Audio');
    const bytes = await file.arrayBuffer();
    const ctx = new Ctx();
    let decoded;
    try {
      decoded = await ctx.decodeAudioData(bytes);
    } finally {
      if (ctx.close) ctx.close();
    }
    const seconds = Math.min(decoded.duration, MAX_CLIP_SECONDS);
    const frames = Math.max(1, Math.round(seconds * rate));
    const offline = new Offline(channels, frames, rate);
    const source = offline.createBufferSource();
    source.buffer = decoded;
    source.connect(offline.destination);
    source.start();
    const out = await offline.startRendering();

    const length = out.length;
    const pcm = new Int16Array(length * channels);
    for (let c = 0; c < channels; c++) {
      const data = out.getChannelData(Math.min(c, out.numberOfChannels - 1));
      for (let i = 0; i < length; i++) {
        let v = data[i];
        v = v < -1 ? -1 : (v > 1 ? 1 : v);
        pcm[i * channels + c] = v < 0 ? v * 32768 : v * 32767;
      }
    }
    return {
      pcm, channels, rate, frames: length,
      duration: length / rate,
      trimmed: decoded.duration > MAX_CLIP_SECONDS,
    };
  }

  // ─── Preview ───────────────────────────────────────────────────────

  let _audio = null;
  let _objectUrl = null;

  function audio() {
    if (!_audio) {
      _audio = new Audio();
      _audio.crossOrigin = 'anonymous';
      _audio.addEventListener('timeupdate', () => {
        $playerTime.textContent = formatClock(_audio.currentTime);
      });
      _audio.addEventListener('play', syncPlay);
      _audio.addEventListener('pause', syncPlay);
      _audio.addEventListener('ended', syncPlay);
    }
    return _audio;
  }

  function syncPlay() {
    const icon = $playerPlay.querySelector('i');
    if (icon) icon.className = `fa-solid ${_audio && !_audio.paused ? 'fa-pause' : 'fa-play'}`;
    for (const button of $list.querySelectorAll('.ss-play i')) {
      button.className = 'fa-solid fa-play';
    }
    if (_audio && !_audio.paused && _audio.dataset.id) {
      const row = $list.querySelector(`.ss-play[data-id="${_audio.dataset.id}"] i`);
      if (row) row.className = 'fa-solid fa-pause';
    }
  }

  function playOriginal(id) {
    const sound = state.sounds.find((s) => s.id === id);
    if (!sound || sound.error) return;
    const element = audio();
    if (element.dataset.id === String(id) && !element.paused) { element.pause(); return; }
    revoke();
    element.dataset.id = String(id);
    element.src = apiUrl(`/site/updates/${BRANCH}/file/bnk/audio`
      + `?path=${encodeURIComponent(state.bank.path)}&id=${id}`);
    element.play().catch(() => {});
    $player.hidden = false;
    $playerName.textContent = sound.name || `#${id}`;
    $playerSub.textContent = t('the game’s sound');
    syncPlay();
  }

  function playClip(clip, label) {
    const element = audio();
    revoke();
    element.dataset.id = '';
    _objectUrl = URL.createObjectURL(new Blob([wav(clip)], { type: 'audio/wav' }));
    element.src = _objectUrl;
    element.play().catch(() => {});
    $player.hidden = false;
    $playerName.textContent = label;
    $playerSub.textContent = t('your replacement');
    syncPlay();
  }

  function revoke() {
    if (_objectUrl) { URL.revokeObjectURL(_objectUrl); _objectUrl = null; }
  }

  function closePlayer() {
    if (_audio) { _audio.pause(); _audio.removeAttribute('src'); _audio.load(); }
    revoke();
    $player.hidden = true;
    syncPlay();
  }

  /** Wrap a clip's samples in a WAV header so the browser can play it back. */
  function wav(clip) {
    const bytes = clip.pcm.byteLength;
    const out = new ArrayBuffer(44 + bytes);
    const view = new DataView(out);
    const tag = (at, s) => { for (let i = 0; i < s.length; i++) view.setUint8(at + i, s.charCodeAt(i)); };
    tag(0, 'RIFF'); view.setUint32(4, 36 + bytes, true); tag(8, 'WAVE');
    tag(12, 'fmt '); view.setUint32(16, 16, true);
    view.setUint16(20, 1, true); view.setUint16(22, clip.channels, true);
    view.setUint32(24, clip.rate, true);
    view.setUint32(28, clip.rate * clip.channels * 2, true);
    view.setUint16(32, clip.channels * 2, true); view.setUint16(34, 16, true);
    tag(36, 'data'); view.setUint32(40, bytes, true);
    new Uint8Array(out, 44).set(new Uint8Array(clip.pcm.buffer, clip.pcm.byteOffset, bytes));
    return out;
  }

  // ─── Build ─────────────────────────────────────────────────────────

  async function build() {
    if (state.building) return;
    const output = document.querySelector('input[name="ss-output"]:checked').value;
    const codec = document.querySelector('input[name="ss-codec"]:checked').value;
    const edits = [];
    const form = new FormData();
    let n = 0;

    for (const [id, edit] of state.edits) {
      if (edit.kind === 'mute') { edits.push({ kind: 'mute', id }); continue; }
      const key = `clip_${n++}`;
      edits.push({ kind: 'replace', id, clip: key,
                   channels: edit.clip.channels, rate: edit.clip.rate });
      form.append('clips', new Blob([edit.clip.pcm.buffer]), key);
    }
    for (const add of state.adds) {
      const key = `clip_${n++}`;
      edits.push({ kind: 'add', event: add.event, clip: key,
                   channels: add.clip.channels, rate: add.clip.rate });
      form.append('clips', new Blob([add.clip.pcm.buffer]), key);
    }

    // Must match app/trove/audio/studio.safe_title exactly: Trove checks a
    // .tmod's filename against the title stored inside it, and the server names
    // the mod from the sanitised title while the browser names the download.
    const title = safeTitle($modTitle.value);
    form.append('spec', JSON.stringify({
      branch: BRANCH, path: state.bank.path, codec, output,
      mod: { title, author: ($modAuthor.value || '').trim() },
      edits,
    }));

    state.building = true;
    $build.disabled = true;
    setStatus(t('Building… large banks take a moment.'));
    try {
      const res = await fetch(apiUrl('/site/sound-studio/build'),
                             { method: 'POST', body: form });
      if (!res.ok) {
        let message = `HTTP ${res.status}`;
        try {
          const body = await res.json();
          message = (body.error && body.error.message) || body.detail || message;
        } catch (_) { /* not JSON - keep the status line */ }
        throw new Error(message);
      }
      const blob = await res.blob();
      const stem = state.bank.path.slice(state.bank.path.lastIndexOf('/') + 1);
      save(blob, output === 'tmod' ? `${title}.tmod` : stem);
      setStatus(t('Done — check your downloads.'));
    } catch (err) {
      setStatus((err && err.message) || String(err), true);
    } finally {
      state.building = false;
      renderChanges();
    }
  }

  function save(blob, filename) {
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = filename;
    document.body.appendChild(link);
    link.click();
    link.remove();
    setTimeout(() => URL.revokeObjectURL(url), 30000);
  }

  // ─── Wiring ────────────────────────────────────────────────────────

  function wire() {
    $banks.addEventListener('click', (e) => {
      const card = e.target.closest('[data-bank]');
      if (card) openBank(card.dataset.bank);
    });

    $list.addEventListener('click', (e) => {
      const button = e.target.closest('[data-act]');
      if (!button) return;
      const id = Number(button.dataset.id);
      switch (button.dataset.act) {
        case 'play': playOriginal(id); break;
        case 'mute': toggleMute(id); break;
        case 'replace': pickFile({ kind: 'replace', id }); break;
        case 'undo': state.edits.delete(id); refreshRow(id); renderChanges(); break;
      }
    });

    $changes.addEventListener('click', (e) => {
      const button = e.target.closest('[data-act]');
      if (!button) return;
      if (button.dataset.act === 'undo') {
        state.edits.delete(Number(button.dataset.id));
      } else if (button.dataset.act === 'unadd') {
        state.adds.splice(Number(button.dataset.index), 1);
      }
      renderList();
      renderChanges();
    });

    $filter.addEventListener('input', () => { state.filter = $filter.value || ''; renderList(); });
    $codec.addEventListener('change', () => { state.codec = $codec.value; renderList(); });
    $changedOnly.addEventListener('change', () => {
      state.changedOnly = $changedOnly.checked;
      renderList();
    });
    $clear.addEventListener('click', () => {
      state.edits.clear();
      state.adds = [];
      $addNote.hidden = true;
      renderList();
      renderChanges();
      setStatus('');
    });

    $addPick.addEventListener('click', () => {
      const name = ($addName.value || '').trim();
      if (!/^[A-Za-z0-9_]{3,96}$/.test(name)) {
        setStatus(t('Give the new sound a name first — letters, digits and underscores.'), true);
        $addName.focus();
        return;
      }
      if (state.adds.some((a) => a.event.toLowerCase() === name.toLowerCase())) {
        setStatus(t('You have already added a sound with that name.'), true);
        return;
      }
      pickFile({ kind: 'add', event: name });
    });

    $file.addEventListener('change', () => { onFilePicked().catch(() => {}); });
    $build.addEventListener('click', () => { build(); });

    for (const input of document.querySelectorAll('input[name="ss-output"]')) {
      input.addEventListener('change', () => {
        $modFields.hidden = document.querySelector('input[name="ss-output"]:checked').value !== 'tmod';
      });
    }

    $playerPlay.addEventListener('click', () => {
      if (!_audio) return;
      if (_audio.paused) _audio.play().catch(() => {}); else _audio.pause();
    });
    $playerClose.addEventListener('click', closePlayer);

    // Losing a page of work to a stray Back is worse than a redundant prompt.
    window.addEventListener('beforeunload', (e) => {
      if (!state.edits.size && !state.adds.length) return;
      e.preventDefault();
      e.returnValue = '';
    });
  }

  function toggleMute(id) {
    const current = state.edits.get(id);
    if (current && current.kind === 'mute') state.edits.delete(id);
    else state.edits.set(id, { kind: 'mute' });
    refreshRow(id);
    renderChanges();
  }

  // ─── Util ──────────────────────────────────────────────────────────

  function setStatus(message, bad) {
    $status.textContent = message || '';
    $status.classList.toggle('bad', !!bad);
  }

  function t(s) {
    return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s;
  }

  function errorHTML(err) {
    return `<p class="ss-empty">${esc(t('Failed to load'))}: ${esc((err && err.message) || String(err))}</p>`;
  }

  /** Mirror of app/trove/audio/studio.safe_title. */
  function safeTitle(title) {
    const cleaned = String(title || '')
      .replace(/[^A-Za-z0-9 _.\-()&'!,]+/g, ' ')
      .replace(/\s+/g, ' ')
      .trim()
      .slice(0, 80)
      .trim();
    return cleaned || 'Trove Sound Pack';
  }

  function formatInt(n) { return Number(n || 0).toLocaleString(); }

  function formatBytes(n) {
    n = Number(n || 0);
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  }

  function formatClock(seconds) {
    seconds = Math.max(0, Number(seconds) || 0);
    return `${Math.floor(seconds / 60)}:${String(Math.floor(seconds % 60)).padStart(2, '0')}`;
  }

  // The preview player is also reachable from a change row, so expose the one
  // entry point the markup needs.
  $changes.addEventListener('dblclick', (e) => {
    const row = e.target.closest('.ss-change');
    if (!row) return;
    const name = row.querySelector('.ss-change-name');
    const add = state.adds.find((a) => a.event === (name && name.textContent));
    if (add) playClip(add.clip, add.event);
  });
})();
