/* Kiwi Flash code viewer — read a mod's ActionScript.

   An interface mod is a .swf, and everything it actually does is compiled
   bytecode inside it. The server decompiles that back to source (JPEXS FFDec,
   cached per movie); this reads the result.

   One request brings the whole class tree back, because a movie is a few hundred
   KB of source at most and having all of it client-side is what makes switching
   class instant and makes search cover the WHOLE mod rather than the open file.

   Highlighting is a small AS3 tokenizer rather than a library: the page runs under
   a strict CSP, and one pass over a file is cheaper than shipping a highlighter.

   Public API:
     SwfCode.open({ url, title, subtitle, fetcher })

   `fetcher(url)` is the caller's own authenticated GET when it has one (the mod
   page's, which refreshes an aged-out session so an owner still sees their own
   drafts); without it a plain same-origin fetch is used.

   Styles live in swf_code.css. */
(function () {
  'use strict';

  var esc = (window.BTTUtil && window.BTTUtil.esc) || function (s) {
    return String(s == null ? '' : s).replace(/[&<>"']/g, function (c) {
      return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
    });
  };

  function t(s) {
    return (window.BTTi18n && window.BTTi18n.t) ? window.BTTi18n.t(s) : s;
  }

  var KEYWORDS = ('package import class interface extends implements public private '
    + 'protected internal static final override native dynamic var const function '
    + 'return if else for each while do switch case default break continue new '
    + 'delete typeof instanceof is as in try catch finally throw super this null '
    + 'true false undefined void get set namespace use with NaN Infinity').split(' ');

  var KEYWORD_SET = Object.create(null);
  KEYWORDS.forEach(function (k) { KEYWORD_SET[k] = true; });

  /* One pass, longest-match-first: a `//` inside a string must stay string, and a
     keyword inside a comment must stay comment, so the alternation order below is
     the whole correctness argument. Anything unmatched falls through as plain
     text, which is what keeps a tokenizer this small from mangling code. */
  var TOKEN = new RegExp([
    '\\/\\*[\\s\\S]*?(?:\\*\\/|$)',          // block comment
    '\\/\\/[^\\n]*',                          // line comment
    '"(?:\\\\.|[^"\\\\\\n])*"?',              // double-quoted string
    "'(?:\\\\.|[^'\\\\\\n])*'?",              // single-quoted string
    '\\b0[xX][0-9a-fA-F]+\\b',                // hex literal
    '\\b\\d+(?:\\.\\d+)?(?:[eE][+-]?\\d+)?\\b', // number
    '[A-Za-z_$][\\w$]*',                      // identifier / keyword
  ].join('|'), 'g');

  function highlight(src) {
    var out = '';
    var last = 0;
    var m;
    TOKEN.lastIndex = 0;
    while ((m = TOKEN.exec(src)) !== null) {
      var text = m[0];
      if (m.index > last) out += esc(src.slice(last, m.index));
      last = m.index + text.length;
      var cls = '';
      var head = text.charAt(0);
      if (head === '/') cls = 'c';                                  // comment
      else if (head === '"' || head === "'") cls = 's';             // string
      else if (head >= '0' && head <= '9') cls = 'n';               // number
      else if (KEYWORD_SET[text]) cls = 'k';                        // keyword
      out += cls ? '<span class="swfc-' + cls + '">' + esc(text) + '</span>' : esc(text);
      // A zero-length match would spin forever; the patterns above can't produce
      // one, but the guard costs nothing next to a hung tab.
      if (m.index === TOKEN.lastIndex) TOKEN.lastIndex++;
    }
    out += esc(src.slice(last));
    return out;
  }

  /* Flat `a/b/C.as` paths -> nested packages, so the rail reads as the package
     structure the code was written in rather than a wall of paths. */
  function tree(scripts) {
    var root = { dirs: new Map(), files: [] };
    scripts.forEach(function (s) {
      var parts = String(s.path || '').split('/').filter(Boolean);
      var node = root;
      parts.slice(0, -1).forEach(function (part) {
        if (!node.dirs.has(part)) node.dirs.set(part, { dirs: new Map(), files: [] });
        node = node.dirs.get(part);
      });
      node.files.push(s);
    });
    return root;
  }

  function className(path) {
    return String(path).split('/').pop().replace(/\.as$/i, '');
  }

  function fmtBytes(n) {
    n = Number(n) || 0;
    if (n < 1024) return n + ' B';
    if (n < 1024 * 1024) return (n / 1024).toFixed(1) + ' KB';
    return (n / (1024 * 1024)).toFixed(1) + ' MB';
  }

  function open(opts) {
    opts = opts || {};
    var ov = document.createElement('div');
    ov.className = 'swfc-overlay';
    ov.setAttribute('role', 'dialog');
    ov.setAttribute('aria-modal', 'true');
    // Tells a modal underneath that something is stacked over it, so Escape
    // closes this layer first rather than both at once (see modal.js).
    ov.setAttribute('data-overlay-layer', '');
    ov.setAttribute('aria-label', (opts.title || 'Flash movie') + ' — ' + t('source code'));
    ov.innerHTML = '<div class="swfc-modal">'
      + '<div class="swfc-head">'
      + '<div class="swfc-head-text"><span class="swfc-title"></span><span class="swfc-meta"></span></div>'
      + '<button class="swfc-close" type="button" aria-label="' + esc(t('Close')) + '">'
      + '<i class="fa-solid fa-xmark"></i></button>'
      + '</div>'
      + '<div class="swfc-body"><p class="swfc-state">' + esc(t('Decompiling…')) + '</p></div>'
      + '</div>';
    ov.querySelector('.swfc-title').textContent = opts.title || 'Flash movie';
    if (opts.subtitle) ov.querySelector('.swfc-meta').textContent = opts.subtitle;
    document.body.appendChild(ov);

    var releaseFocus = null;
    function close() {
      document.removeEventListener('keydown', onKey);
      if (releaseFocus) { releaseFocus(); releaseFocus = null; }
      ov.remove();
      if (typeof opts.onClose === 'function') opts.onClose();
    }
    function onKey(e) { if (e.key === 'Escape') { e.stopPropagation(); close(); } }
    ov.querySelector('.swfc-close').addEventListener('click', close);
    ov.addEventListener('mousedown', function (e) { if (e.target === ov) close(); });
    // Capture phase: this viewer opens ON TOP of the build-contents modal, and
    // Escape has to close the top layer only - the modal below keeps its own
    // listener and would otherwise close with it.
    document.addEventListener('keydown', onKey, true);
    if (window.BTTUtil && window.BTTUtil.trapFocus) {
      releaseFocus = window.BTTUtil.trapFocus(ov.querySelector('.swfc-modal'));
    }

    load(ov, opts);
    return { close: close };
  }

  function load(ov, opts) {
    var body = ov.querySelector('.swfc-body');
    var get = typeof opts.fetcher === 'function'
      ? opts.fetcher(opts.url)
      : fetch(opts.url, { credentials: 'same-origin' });
    Promise.resolve(get).then(function (r) {
      return r.json().then(function (j) {
        // The server explains its own refusals - a movie too large, a decompiler
        // this box doesn't have - and those messages are worth more than a
        // generic failure, so they are shown verbatim when present.
        if (!r.ok) throw new Error((j && j.error && j.error.message) || '');
        return j;
      }, function () { throw new Error(''); });
    }).then(function (data) {
      render(ov, body, data, opts);
    }).catch(function (err) {
      body.innerHTML = '<p class="swfc-state swfc-error"></p>';
      body.querySelector('.swfc-state').textContent =
        (err && err.message) ? err.message : t('This movie could not be decompiled.');
    });
  }

  function render(ov, body, data, opts) {
    var scripts = (data && data.scripts) || [];
    var meta = ov.querySelector('.swfc-meta');
    meta.textContent = scripts.length
      ? (scripts.length === 1 ? t('1 class')
        : t('%n classes').replace('%n', String(scripts.length)))
        + ' · ' + fmtBytes(data.size) + ' · ' + (data.decompiler || 'ffdec')
      : (opts.subtitle || '');

    if (!scripts.length) {
      body.innerHTML = '<p class="swfc-state"></p>';
      body.querySelector('.swfc-state').textContent =
        t('This movie carries no code — it is artwork only.');
      return;
    }

    body.innerHTML = '<div class="swfc-rail">'
      + '<label class="swfc-search"><i class="fa-solid fa-magnifying-glass" aria-hidden="true"></i>'
      + '<input type="search" placeholder="' + esc(t('Search this movie…')) + '"'
      + ' aria-label="' + esc(t('Search this movie')) + '"></label>'
      + '<div class="swfc-tree" role="tree"></div>'
      + '</div>'
      + '<div class="swfc-main">'
      + '<div class="swfc-crumb"><span class="swfc-path"></span>'
      + '<button type="button" class="swfc-act swfc-copy"><i class="fa-regular fa-copy"></i> '
      + esc(t('Copy')) + '</button>'
      + '<button type="button" class="swfc-act swfc-dl"><i class="fa-solid fa-download"></i> '
      + esc(t('Save .as')) + '</button></div>'
      // Focusable: the code pane is a scroll container, and a keyboard-only
      // reader needs to be able to reach it to scroll a long class at all.
      + '<div class="swfc-code" tabindex="0" role="region" aria-label="'
      + esc(t('source code')) + '"><div class="swfc-gutter" aria-hidden="true"></div>'
      + '<pre class="swfc-src"><code></code></pre></div>'
      + (data.truncated ? '<p class="swfc-trunc">'
        + esc(t('This movie is too large to show in full — some classes were left out.'))
        + '</p>' : '')
      + '</div>';

    var railTree = body.querySelector('.swfc-tree');
    var search = body.querySelector('.swfc-search input');
    var pathEl = body.querySelector('.swfc-path');
    var gutter = body.querySelector('.swfc-gutter');
    var codeEl = body.querySelector('.swfc-src code');
    var current = null;

    function show(script) {
      current = script;
      pathEl.textContent = script.path;
      codeEl.innerHTML = highlight(script.source || '');
      var lines = (script.source || '').split('\n').length;
      var nums = new Array(lines);
      for (var i = 0; i < lines; i++) nums[i] = i + 1;
      gutter.textContent = nums.join('\n');
      body.querySelector('.swfc-code').scrollTop = 0;
      railTree.querySelectorAll('.swfc-file').forEach(function (b) {
        b.classList.toggle('is-on', b.getAttribute('data-path') === script.path);
      });
    }

    function paint(query) {
      var q = String(query || '').trim().toLowerCase();
      // An empty query lists everything; otherwise a class stays when its PATH
      // matches, or when its source does - the second is what turns the rail into
      // a search over the whole mod rather than over a list of filenames.
      var hits = scripts.map(function (s) {
        if (!q) return { s: s, n: 0 };
        if (s.path.toLowerCase().indexOf(q) >= 0) return { s: s, n: 0 };
        var n = countHits(s.source || '', q);
        return n ? { s: s, n: n } : null;
      }).filter(Boolean);
      railTree.innerHTML = hits.length
        ? treeHTML(tree(hits.map(function (h) { return h.s; })),
                   hitMap(hits), !!q, 0)
        : '<p class="swfc-none">' + esc(t('Nothing matches.')) + '</p>';
      railTree.querySelectorAll('.swfc-file').forEach(function (b) {
        b.addEventListener('click', function () {
          var want = b.getAttribute('data-path');
          var found = scripts.filter(function (s) { return s.path === want; })[0];
          if (found) show(found);
        });
      });
      if (current) {
        railTree.querySelectorAll('.swfc-file').forEach(function (b) {
          b.classList.toggle('is-on', b.getAttribute('data-path') === current.path);
        });
      }
    }

    paint('');
    // Open on the movie's own document class where we can spot it - the biggest
    // top-level class is that, near enough, and it is the file anyone reading a
    // mod wants first. Anything else means starting on an interface stub.
    show(scripts.slice().sort(function (a, b) {
      var depth = a.path.split('/').length - b.path.split('/').length;
      return depth || (b.size - a.size);
    })[0]);

    search.addEventListener('input',
      window.BTTUtil.debounce(function () { paint(search.value); }, 140));
    body.querySelector('.swfc-copy').addEventListener('click', function () {
      if (!current) return;
      window.BTTUtil.copy(current.source || '').then(function (ok) {
        if (ok && window.BTTToast) window.BTTToast.show(t('Copied.'));
      });
    });
    body.querySelector('.swfc-dl').addEventListener('click', function () {
      if (!current) return;
      var a = document.createElement('a');
      a.href = URL.createObjectURL(new Blob([current.source || ''], { type: 'text/plain' }));
      a.download = className(current.path) + '.as';
      a.click();
      setTimeout(function () { URL.revokeObjectURL(a.href); }, 4000);
    });
  }

  function countHits(src, q) {
    var n = 0;
    var i = src.toLowerCase().indexOf(q);
    while (i >= 0 && n < 999) { n++; i = src.toLowerCase().indexOf(q, i + q.length); }
    return n;
  }

  function hitMap(hits) {
    var map = Object.create(null);
    hits.forEach(function (h) { map[h.s.path] = h.n; });
    return map;
  }

  function treeHTML(node, hits, expanded, depth) {
    var dirs = [];
    node.dirs.forEach(function (child, name) { dirs.push([name, child]); });
    dirs.sort(function (a, b) { return a[0].toLowerCase() < b[0].toLowerCase() ? -1 : 1; });
    var folders = dirs.map(function (pair) {
      return '<details class="swfc-pkg"' + (expanded || depth < 1 ? ' open' : '') + '>'
        + '<summary><i class="fa-solid fa-folder" aria-hidden="true"></i> '
        + esc(pair[0]) + '</summary>'
        + '<div class="swfc-kids">' + treeHTML(pair[1], hits, expanded, depth + 1) + '</div>'
        + '</details>';
    }).join('');
    var files = node.files.slice().sort(function (a, b) {
      return a.path.toLowerCase() < b.path.toLowerCase() ? -1 : 1;
    }).map(function (s) {
      var n = hits[s.path] || 0;
      return '<button type="button" class="swfc-file" data-path="' + esc(s.path) + '">'
        + '<i class="fa-regular fa-file-code" aria-hidden="true"></i>'
        + '<span class="swfc-fname">' + esc(className(s.path)) + '</span>'
        + (n ? '<span class="swfc-hits">' + n + '</span>' : '')
        + '</button>';
    }).join('');
    return folders + files;
  }

  window.SwfCode = { open: open };
})();
