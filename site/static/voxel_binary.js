/* Kiwi voxel payload reader (KVX1).

   A model's bulk is six parallel arrays per part. As JSON that's millions of
   decimal digits, and the browser spends longer in JSON.parse - boxing every one
   into a JS number - than the network spends fetching them. The server can hand
   back the same payload with those arrays as raw bytes instead (`?fmt=bin`), and
   this reader rebuilds the SAME object shape with typed arrays in their place, so
   the viewers' rendering code is untouched: X[i], (rgb >> 16) & 255 and friends
   all still work.

   Container (little-endian):
     0  'KVX1'
     4  u32 header length
     8  header JSON, zero-padded to the next 4-byte boundary
     …  the arrays, each 4-byte aligned

   The header's `_bin` maps a dotted path ("x", "parts.3.rgb") to
   [offset-from-body, count, dtype].

   Degrades on its own: a server that doesn't know `fmt=bin` (or an older deploy)
   answers with JSON, which this parses instead. Nothing to configure. */
(function () {
  'use strict';

  var VIEWS = { i16: Int16Array, u32: Uint32Array, u8: Uint8Array };

  function isKvx(buf) {
    if (!buf || buf.byteLength < 8) return false;
    var m = new Uint8Array(buf, 0, 4);
    return m[0] === 75 && m[1] === 86 && m[2] === 88 && m[3] === 49;   // 'KVX1'
  }

  /* Header paths name fields of the payload the server just built, so a segment
     that walks off the object (`__proto__`, an inherited key, a missing part) is
     malformed - drop the field rather than write through Object.prototype. */
  function unsafeSeg(seg) {
    return seg === '__proto__' || seg === 'constructor' || seg === 'prototype';
  }

  function setPath(root, path, value) {
    var segs = path.split('.'), cur = root, i;
    for (i = 0; i < segs.length; i++) {
      if (!segs[i] || unsafeSeg(segs[i])) return false;
    }
    for (i = 0; i < segs.length - 1; i++) {
      if (cur === null || typeof cur !== 'object'
          || !Object.prototype.hasOwnProperty.call(cur, segs[i])) return false;
      cur = cur[segs[i]];
    }
    if (cur === null || typeof cur !== 'object') return false;
    cur[segs[segs.length - 1]] = value;
    return true;
  }

  /* ArrayBuffer -> payload object. Throws on a malformed container. */
  function decode(buf) {
    var dv = new DataView(buf);
    var headLen = dv.getUint32(4, true);
    var head = JSON.parse(new TextDecoder().decode(new Uint8Array(buf, 8, headLen)));
    var body = 8 + headLen;
    body += (4 - (body % 4)) % 4;
    var bin = head._bin || {};
    delete head._bin;
    Object.keys(bin).forEach(function (path) {
      var spec = bin[path], View = VIEWS[spec[2]];
      if (!View) return;                        // unknown dtype -> leave the field alone
      setPath(head, path, new View(buf, body + spec[0], spec[1]));
    });
    return head;
  }

  function withFmt(url) {
    return url + (url.indexOf('?') >= 0 ? '&' : '?') + 'fmt=bin';
  }

  /* The error body is JSON even on a binary request - pull the server's message
     out of it so the viewer can show why a model won't load. */
  function errorFrom(buf, status) {
    try {
      var j = JSON.parse(new TextDecoder().decode(new Uint8Array(buf)));
      if (j && j.error && j.error.message) return new Error(j.error.message);
    } catch (e) { /* not JSON - fall through to the generic message */ }
    return new Error('Could not load model (HTTP ' + status + ').');
  }

  /* Fetch a model payload, binary when the server supports it. Same resolved shape
     either way, so callers can't tell which they got. */
  function fetchModel(url, init) {
    return fetch(withFmt(url), init || { credentials: 'same-origin' })
      .then(function (r) {
        return r.arrayBuffer().then(function (buf) {
          if (!r.ok) throw errorFrom(buf, r.status);
          if (isKvx(buf)) return decode(buf);
          return JSON.parse(new TextDecoder().decode(new Uint8Array(buf)));
        });
      });
  }

  window.VoxelBinary = { fetchModel: fetchModel, decode: decode, isKvx: isKvx };
})();
