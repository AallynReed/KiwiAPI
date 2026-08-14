/* ═══════════════════════════════════════════════════════════════════════
   MP3 encoding, in the browser
   ───────────────────────────────────────────────────────────────────────
   The server hands sounds back as Ogg (Wwise Vorbis, remuxed untouched) or
   WAV (ADPCM and PCM, decoded). Both are formats the browser already
   decodes natively, which is what makes MP3 a client-side job: no Vorbis
   decoder and no encoder has to exist on the server, and nothing is
   uploaded to get one.

   lamejs is 156 KB, so it is fetched the first time someone actually asks
   for an MP3 rather than on every page load. It is vendored under
   /static/vendor/ and served from our own origin - never a CDN.

   Encoding runs on the main thread in slices, yielding between them, so a
   long track reports progress instead of freezing the page.
   ═══════════════════════════════════════════════════════════════════════ */

(function () {
  'use strict';

  var LAME_URL = '/static/vendor/lame.min.js';

  // MPEG-1 and MPEG-2 Layer III are only defined at these rates, so anything
  // else has to be resampled before it can be encoded at all.
  var LEGAL_RATES = [8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000];

  var BLOCK = 1152;            // one MP3 granule pair - what lamejs wants at a time
  var BLOCKS_PER_SLICE = 32;   // ≈100 ms of work before handing the thread back

  var _lame = null;

  /** Load lamejs once. Resolves with the `lamejs` global. */
  function lame() {
    if (_lame) return _lame;
    _lame = new Promise(function (resolve, reject) {
      if (window.lamejs) { resolve(window.lamejs); return; }
      var tag = document.createElement('script');
      tag.src = LAME_URL;
      tag.onload = function () {
        if (window.lamejs) resolve(window.lamejs);
        else reject(new Error('The MP3 encoder did not load.'));
      };
      tag.onerror = function () { reject(new Error('The MP3 encoder could not be loaded.')); };
      document.head.appendChild(tag);
    });
    // A failed load should not poison every later attempt.
    _lame.catch(function () { _lame = null; });
    return _lame;
  }

  function nearestLegalRate(rate) {
    if (LEGAL_RATES.indexOf(rate) >= 0) return rate;
    return LEGAL_RATES.reduce(function (best, r) {
      return Math.abs(r - rate) < Math.abs(best - rate) ? r : best;
    }, LEGAL_RATES[0]);
  }

  /** A bitrate the chosen rate can actually carry.
   *
   * Below 32 kHz the stream is MPEG-2, whose Layer III ceiling is 160 kbps -
   * asking for more there produces a file players disagree about. */
  function bitrateFor(rate, channels) {
    if (rate < 32000) return 96;
    return channels === 2 ? 192 : 128;
  }

  function yieldToPage() {
    return new Promise(function (resolve) { setTimeout(resolve, 0); });
  }

  /** Decode any browser-readable audio, landed on an MP3-legal sample rate.
   *
   * `decodeAudioData` always resamples to the context's own rate, so decoding on
   * an OfflineAudioContext built at the rate we want is the resample - no render
   * pass needed. Doing it on a plain AudioContext instead would silently drag
   * every sound up to whatever the sound card runs at, which for a 32 kHz game
   * effect means a bigger MP3 carrying no more detail.
   *
   * `hintRate` is the sound's own rate from the bank manifest; without one there
   * is nothing to preserve, so CD rate is the safe default. */
  async function toBuffer(bytes, hintRate) {
    var Offline = window.OfflineAudioContext || window.webkitOfflineAudioContext;
    if (!Offline) throw new Error('This browser cannot decode audio.');

    var rate = nearestLegalRate(hintRate || 44100);
    var decoded = await new Offline(1, 1, rate).decodeAudioData(bytes.slice(0));

    var channels = Math.min(decoded.numberOfChannels, 2) || 1;
    if (decoded.numberOfChannels === channels) return decoded;

    // Only a source with more than two channels gets this far, and it is here to
    // be folded down to something lamejs will take.
    var offline = new Offline(channels, decoded.length, decoded.sampleRate);
    var source = offline.createBufferSource();
    source.buffer = decoded;
    source.connect(offline.destination);
    source.start();
    return offline.startRendering();
  }

  /** One channel of an AudioBuffer as the 16-bit samples lamejs expects. */
  function toInt16(buffer, channel) {
    var data = buffer.getChannelData(Math.min(channel, buffer.numberOfChannels - 1));
    var out = new Int16Array(data.length);
    for (var i = 0; i < data.length; i++) {
      var v = data[i];
      v = v < -1 ? -1 : (v > 1 ? 1 : v);
      out[i] = v < 0 ? v * 32768 : v * 32767;
    }
    return out;
  }

  /**
   * Encode audio bytes to MP3.
   *
   * @param {ArrayBuffer} bytes    any audio the browser can decode (our .ogg / .wav)
   * @param {object} [opts]        {rate: source sample rate to preserve,
   *                                onProgress: fn(0..1)}
   * @returns {Promise<Blob>}      audio/mpeg
   */
  async function encode(bytes, opts) {
    opts = opts || {};
    var progress = opts.onProgress || function () {};
    var lamejs = await lame();

    var buffer = await toBuffer(bytes, opts.rate);
    var channels = Math.min(buffer.numberOfChannels, 2) || 1;
    var rate = buffer.sampleRate;
    var encoder = new lamejs.Mp3Encoder(channels, rate, bitrateFor(rate, channels));

    var left = toInt16(buffer, 0);
    var right = channels === 2 ? toInt16(buffer, 1) : null;
    var parts = [];
    var total = left.length || 1;

    progress(0);
    for (var at = 0, n = 0; at < left.length; at += BLOCK, n++) {
      var chunk = right
        ? encoder.encodeBuffer(left.subarray(at, at + BLOCK), right.subarray(at, at + BLOCK))
        : encoder.encodeBuffer(left.subarray(at, at + BLOCK));
      if (chunk.length) parts.push(chunk);
      if (n % BLOCKS_PER_SLICE === BLOCKS_PER_SLICE - 1) {
        progress(Math.min(1, at / total));
        await yieldToPage();
      }
    }
    var tail = encoder.flush();
    if (tail.length) parts.push(tail);
    progress(1);

    return new Blob(parts, { type: 'audio/mpeg' });
  }

  /** Fetch a sound and hand back an MP3 of it. */
  async function encodeUrl(url, opts) {
    var res = await fetch(url);
    if (!res.ok) throw new Error('HTTP ' + res.status);
    return encode(await res.arrayBuffer(), opts);
  }

  /** Swap a filename's extension for .mp3. */
  function mp3Name(name) {
    return String(name || 'sound').replace(/\.[^.\/\\]*$/, '') + '.mp3';
  }

  window.KiwiMp3 = { encode: encode, encodeUrl: encodeUrl, name: mp3Name };
})();
