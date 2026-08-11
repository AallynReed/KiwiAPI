/* Embeddable audio player (/embed/viewer?mode=audio) — the fourth viewer.

   A Wwise sound bank is a bundle, not a file: one .bnk holds anywhere from one
   sound to the 1,600 in a UI bank. So this mounts a browser, not a single
   <audio> tag - a filterable list of what's in the bank, with a transport docked
   above it for whatever is loaded.

   Two requests do the work, matching how the server splits it: the bank index
   (names, codecs, durations - nothing decoded), then one sound when the visitor
   presses play. The waveform is a third, decoded in the browser from the bytes
   the player just fetched, so it lands on the HTTP cache the transport filled.

   ?sound=<id or name> pins one sound: the list disappears and the embed is a
   single player, which is what a page showing off one effect actually wants.

   Same mount contract as the 3D viewers (mount -> {dispose}), so the shell in
   embed_viewer.js swaps between all four without special-casing this one. */
(function () {
  'use strict';

  var ICONS = {
    play: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5.5v13l11-6.5z"/></svg>',
    pause: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M7 5h3.5v14H7zm6.5 0H17v14h-3.5z"/></svg>',
    loop: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M17 2l4 4-4 4"/><path d="M3 11v-1a4 4 0 0 1 4-4h14"/><path d="M7 22l-4-4 4-4"/><path d="M21 13v1a4 4 0 0 1-4 4H3"/></svg>',
    volume: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M11 5 6 9H2v6h4l5 4z"/><path d="M15.5 8.5a5 5 0 0 1 0 7"/></svg>',
    download: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 3v12"/><path d="m7 11 5 5 5-5"/><path d="M4 20h16"/></svg>',
  };

  var PREFETCH_SECONDS = 45;

  function esc(s) {
    return String(s == null ? '' : s).replace(/[&<>"']/g, function (c) {
      return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
    });
  }

  function clock(seconds) {
    var s = Math.max(0, Math.round(Number(seconds) || 0));
    return Math.floor(s / 60) + ':' + String(s % 60).padStart(2, '0');
  }

  function spec(sound) {
    return [
      sound.group || '',
      sound.codec || '',
      sound.channels === 2 ? 'stereo' : 'mono',
      sound.sample_rate ? Math.round(sound.sample_rate / 100) / 10 + ' kHz' : '',
    ].filter(Boolean).join(' · ');
  }

  /* The ?sound= a partner pinned. An id is exact; a name matches the whole name
     first and only then a fragment of one - so "hit" can't quietly resolve to
     "hit_critical_02" when the bank also holds a sound literally called "hit". */
  function findPinned(sounds, want) {
    var needle = String(want).trim().toLowerCase();
    if (!needle) return null;
    var byId = sounds.filter(function (s) { return String(s.id) === needle; })[0];
    if (byId) return byId;
    var lower = function (s) { return (s.name || '').toLowerCase(); };
    return sounds.filter(function (s) { return lower(s) === needle; })[0]
      || sounds.filter(function (s) { return lower(s).indexOf(needle) >= 0; })[0]
      || null;
  }

  function mount(stage, opts) {
    var bankUrl = opts.bankUrl;
    var soundUrl = opts.soundUrl;                 // function (id) -> url
    var onMeta = opts.onMeta || function () {};
    var onHint = opts.onHint || function () {};
    var pin = (opts.pin || '').trim();

    var alive = true;
    var sounds = [];
    var current = null;                           // the loaded sound record
    var peaks = null;                             // Float32Array of column peaks
    var peakToken = 0;
    var audioCtx = null;

    stage.className = 'kv-stage kv-audio';
    stage.innerHTML = '<p class="kv-msg">Loading sounds…</p>';
    // One sound and no list is a different shape of embed, not a shorter one: the
    // transport becomes a card centred in the frame instead of a header above a list.
    var solo = function (on) { stage.classList.toggle('kv-audio-solo', on); };

    var el = new Audio();
    el.preload = 'none';

    function message(text, isError) {
      stage.innerHTML = '<p class="kv-msg' + (isError ? ' kv-error' : '') + '">'
        + esc(text) + '</p>';
    }

    // ── chrome ─────────────────────────────────────────────────────────────

    function paintShell(single) {
      solo(single);
      stage.innerHTML =
        '<div class="kv-ap">'
        + '<button type="button" class="kv-ap-play" aria-label="Play">' + ICONS.play + '</button>'
        + '<div class="kv-ap-id"><span class="kv-ap-name"></span><span class="kv-ap-sub"></span></div>'
        + '<div class="kv-ap-seek" role="slider" tabindex="0" aria-label="Seek"'
        + ' aria-valuemin="0" aria-valuemax="100" aria-valuenow="0">'
        + '<canvas class="kv-ap-wave"></canvas><span class="kv-ap-played"></span></div>'
        + '<span class="kv-ap-time">0:00 / 0:00</span>'
        + '<button type="button" class="kv-ap-icon kv-ap-loop" aria-pressed="false"'
        + ' aria-label="Loop" title="Loop">' + ICONS.loop + '</button>'
        + '<label class="kv-ap-vol">' + ICONS.volume
        + '<input type="range" min="0" max="1" step="0.01" value="1" aria-label="Volume">'
        + '</label>'
        + '<a class="kv-ap-icon kv-ap-dl" download aria-label="Download this sound"'
        + ' title="Download">' + ICONS.download + '</a>'
        + '</div>'
        + (single ? '' :
          '<div class="kv-ap-search"><input type="search" class="kv-ap-filter"'
          + ' placeholder="Filter sounds" aria-label="Filter sounds"></div>'
          + '<div class="kv-ap-list"></div>');
      return {
        play: stage.querySelector('.kv-ap-play'),
        name: stage.querySelector('.kv-ap-name'),
        sub: stage.querySelector('.kv-ap-sub'),
        seek: stage.querySelector('.kv-ap-seek'),
        wave: stage.querySelector('.kv-ap-wave'),
        played: stage.querySelector('.kv-ap-played'),
        time: stage.querySelector('.kv-ap-time'),
        loop: stage.querySelector('.kv-ap-loop'),
        vol: stage.querySelector('.kv-ap-vol input'),
        dl: stage.querySelector('.kv-ap-dl'),
        filter: stage.querySelector('.kv-ap-filter'),
        list: stage.querySelector('.kv-ap-list'),
      };
    }

    var ui = null;

    function rowHTML(sound) {
      var label = sound.name || '#' + sound.id;
      var tag = sound.error
        ? '<span class="kv-snd-bad">can’t decode</span>'
        : '<span class="kv-snd-codec">' + esc(sound.codec || '') + '</span>';
      return '<button type="button" class="kv-snd" data-id="' + sound.id + '"'
        + (sound.error ? ' disabled' : '')
        + ' title="' + esc(sound.path || label) + '">'
        + '<span class="kv-snd-glyph">' + ICONS.play + '</span>'
        + '<span class="kv-snd-name">' + esc(label) + '</span>'
        + tag
        + '<span class="kv-snd-dur">' + esc(clock(sound.duration)) + '</span>'
        + '</button>';
    }

    function paintList() {
      if (!ui.list) return;
      var needle = (ui.filter.value || '').trim().toLowerCase();
      var shown = !needle ? sounds : sounds.filter(function (s) {
        return (s.name || '').toLowerCase().indexOf(needle) >= 0
          || (s.group || '').toLowerCase().indexOf(needle) >= 0
          || String(s.id).indexOf(needle) >= 0;
      });
      ui.list.innerHTML = shown.length
        ? shown.map(rowHTML).join('')
        : '<p class="kv-ap-empty">Nothing matches that filter.</p>';
      markActive();
    }

    function markActive() {
      if (!ui.list) return;
      var rows = ui.list.querySelectorAll('.kv-snd');
      for (var i = 0; i < rows.length; i++) {
        var on = !!(current && Number(rows[i].dataset.id) === current.id);
        rows[i].classList.toggle('active', on);
        rows[i].classList.toggle('playing', on && !el.paused);
        if (on) rows[i].setAttribute('aria-current', 'true');
        else rows[i].removeAttribute('aria-current');
        rows[i].querySelector('.kv-snd-glyph').innerHTML =
          (on && !el.paused) ? ICONS.pause : ICONS.play;
      }
    }

    // ── transport ──────────────────────────────────────────────────────────

    function load(sound, autoplay) {
      current = sound;
      var url = soundUrl(sound.id);
      el.src = url;
      el.loop = ui.loop.getAttribute('aria-pressed') === 'true';
      ui.name.textContent = sound.name || '#' + sound.id;
      ui.sub.textContent = sound.error ? sound.error : spec(sound);
      ui.dl.href = url;
      ui.dl.setAttribute('download', (sound.name || 'sound_' + sound.id));
      ui.dl.hidden = !!sound.error;
      ui.play.disabled = !!sound.error;
      peaks = null;
      peakToken++;
      paint();
      markActive();
      if (sound.error) return;
      // Drawing the waveform means downloading the sound, so an unplayed one is
      // only prefetched when it's short. A music bank's first track is minutes
      // long; nobody should pay for it just by having the embed on the page.
      if (autoplay || (sound.duration || 0) <= PREFETCH_SECONDS) loadPeaks(url, peakToken);
      if (autoplay) el.play().catch(function () { /* gesture policy - the button works */ });
    }

    function toggle(sound) {
      if (current && current.id === sound.id) {
        if (el.paused) el.play().catch(function () {}); else el.pause();
        return;
      }
      load(sound, true);
    }

    function paint() {
      var duration = Number.isFinite(el.duration) && el.duration
        ? el.duration : (current ? current.duration || 0 : 0);
      var at = el.currentTime || 0;
      var pct = duration ? Math.min(100, (at / duration) * 100) : 0;
      ui.played.style.width = pct + '%';
      ui.time.textContent = clock(at) + ' / ' + clock(duration);
      ui.seek.setAttribute('aria-valuenow', String(Math.round(pct)));
      ui.play.innerHTML = el.paused ? ICONS.play : ICONS.pause;
      ui.play.setAttribute('aria-label', el.paused ? 'Play' : 'Pause');
    }

    function seekTo(clientX) {
      var duration = Number.isFinite(el.duration) ? el.duration : 0;
      if (!duration) return;
      var box = ui.seek.getBoundingClientRect();
      var ratio = Math.min(1, Math.max(0, (clientX - box.left) / (box.width || 1)));
      el.currentTime = ratio * duration;
      paint();
    }

    // ── waveform ───────────────────────────────────────────────────────────
    // Drawn from a decoded copy of the same bytes the transport is playing. One
    // AudioContext for the life of the embed - browsers cap how many a page may
    // hold, and a bank browser can load a lot of sounds in a row.

    function loadPeaks(url, token) {
      drawWave();
      var Ctx = window.AudioContext || window.webkitAudioContext;
      if (!Ctx) return;
      try { audioCtx = audioCtx || new Ctx(); } catch (e) { return; }
      fetch(url).then(function (r) { return r.arrayBuffer(); }).then(function (bytes) {
        if (token !== peakToken || !alive) return null;
        return audioCtx.decodeAudioData(bytes);
      }).then(function (decoded) {
        if (!decoded || token !== peakToken || !alive) return;
        var COLUMNS = 700;
        var data = decoded.getChannelData(0);
        var out = new Float32Array(COLUMNS);
        for (var i = 0; i < COLUMNS; i++) {
          var from = Math.floor(i * data.length / COLUMNS);
          var to = Math.floor((i + 1) * data.length / COLUMNS);
          var peak = 0;
          for (var j = from; j < to; j++) {
            var v = data[j] < 0 ? -data[j] : data[j];
            if (v > peak) peak = v;
          }
          out[i] = peak;
        }
        peaks = out;
        drawWave();
      }).catch(function () { /* undecodable here is fine - playback is unaffected */ });
    }

    function waveColor() {
      // Read the theme's own ink rather than hard-coding one, so the light theme
      // doesn't draw a white waveform on a white card.
      var ink = getComputedStyle(stage).getPropertyValue('--kv-mute').trim();
      return ink || '#9aa4b2';
    }

    function drawWave() {
      if (!ui || !ui.wave) return;
      var canvas = ui.wave;
      var ratio = window.devicePixelRatio || 1;
      var width = canvas.clientWidth || 240;
      var height = canvas.clientHeight || 40;
      canvas.width = Math.round(width * ratio);
      canvas.height = Math.round(height * ratio);
      var g = canvas.getContext('2d');
      if (!g) return;
      g.setTransform(ratio, 0, 0, ratio, 0, 0);
      g.clearRect(0, 0, width, height);
      var mid = height / 2;
      g.fillStyle = waveColor();
      if (!peaks) {
        g.globalAlpha = 0.35;
        g.fillRect(0, mid - 0.5, width, 1);       // a flat rule reads as "loading"
        g.globalAlpha = 1;
        return;
      }
      g.globalAlpha = 0.7;
      var columns = Math.max(1, Math.floor(width / 2));
      for (var x = 0; x < columns; x++) {
        var peak = peaks[Math.floor(x * peaks.length / columns)] || 0;
        var h = Math.max(1, peak * (height - 4));
        g.fillRect(x * 2, mid - h / 2, 1, h);
      }
      g.globalAlpha = 1;
    }

    // ── wiring ─────────────────────────────────────────────────────────────

    function wire(single) {
      ui.play.addEventListener('click', function () {
        if (!current) return;
        if (el.paused) el.play().catch(function () {}); else el.pause();
      });
      ui.loop.addEventListener('click', function () {
        var on = ui.loop.getAttribute('aria-pressed') !== 'true';
        ui.loop.setAttribute('aria-pressed', String(on));
        ui.loop.classList.toggle('active', on);
        el.loop = on;
      });
      ui.vol.addEventListener('input', function () { el.volume = Number(ui.vol.value); });

      ui.seek.addEventListener('pointerdown', function (e) {
        ui.seek.setPointerCapture(e.pointerId);
        seekTo(e.clientX);
      });
      ui.seek.addEventListener('pointermove', function (e) {
        if (ui.seek.hasPointerCapture(e.pointerId)) seekTo(e.clientX);
      });
      ui.seek.addEventListener('keydown', function (e) {
        var duration = Number.isFinite(el.duration) ? el.duration : 0;
        if (!duration) return;
        var step = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0;
        if (!step) return;
        e.preventDefault();
        el.currentTime = Math.min(duration, Math.max(0, el.currentTime + step));
        paint();
      });

      if (!single) {
        ui.filter.addEventListener('input', paintList);
        ui.list.addEventListener('click', function (e) {
          var row = e.target.closest ? e.target.closest('.kv-snd') : null;
          if (!row) return;
          var hit = sounds.filter(function (s) { return s.id === Number(row.dataset.id); })[0];
          if (hit) toggle(hit);
        });
      }

      ['timeupdate', 'durationchange', 'seeked'].forEach(function (evt) {
        el.addEventListener(evt, paint);
      });
      ['play', 'pause', 'ended'].forEach(function (evt) {
        el.addEventListener(evt, function () { paint(); markActive(); });
      });
      el.addEventListener('play', function () {
        // A long sound skipped the prefetch above; now that it's actually being
        // listened to, the waveform is worth the bytes.
        if (!peaks && current && !current.error) loadPeaks(soundUrl(current.id), ++peakToken);
      });
      el.addEventListener('error', function () {
        if (current) ui.sub.textContent = 'This sound could not be played.';
      });
      if (window.ResizeObserver) {
        var ro = new ResizeObserver(drawWave);
        ro.observe(ui.seek);
      }
    }

    // ── boot ───────────────────────────────────────────────────────────────

    fetch(bankUrl).then(function (r) {
      return r.json().then(function (body) {
        if (!r.ok) {
          throw new Error((body && body.error && body.error.message)
            || 'These sounds could not be loaded.');
        }
        return body;
      }, function () { throw new Error('These sounds could not be loaded.'); });
    }).then(function (bank) {
      if (!alive) return;
      sounds = bank.sounds || [];
      if (!sounds.length) {
        message('This bank holds no sounds.');
        return;
      }
      var pinned = pin ? findPinned(sounds, pin) : null;
      if (pinned) sounds = [pinned];
      var single = sounds.length === 1;

      onMeta(single
        ? clock(sounds[0].duration)
        : sounds.length + ' sounds · ' + clock(bank.total_duration));
      onHint(single ? 'Press play to listen' : 'Pick a sound to play');

      ui = paintShell(single);
      wire(single);
      paintList();
      load(sounds[0], false);
    }).catch(function (err) {
      if (alive) message(err.message || 'These sounds could not be loaded.', true);
    });

    return {
      dispose: function () {
        alive = false;
        peakToken++;
        try { el.pause(); } catch (e) { /* already gone */ }
        el.removeAttribute('src');
        el.load();
        if (audioCtx) { try { audioCtx.close(); } catch (e) { /* already closed */ } }
        stage.innerHTML = '';
      },
    };
  }

  window.EmbedAudio = { mount: mount };
})();
