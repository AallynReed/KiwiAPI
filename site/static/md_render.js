/* ═══════════════════════════════════════════════════════════════════════
   md_render.js - shared GitHub-flavored-ish Markdown renderer for the Mods Hub
   ───────────────────────────────────────────────────────────────────────
   Renders markdown + a SAFE subset of raw HTML (badges, <div align>, <br>,
   tables, …). Raw HTML passes through and the whole output is run through a DOM
   allowlist sanitizer, so author-supplied READMEs render like GitHub with no XSS
   surface. Exposed as `window.BTTMarkdown = { render, sanitize, inline }` and used
   by mods_project.js + mods_profile.js (one copy of the sanitizer, not two).

   Colour (one thing GitHub itself won't do) is supported three ways, all of
   which end up as an inline `style` filtered to colour declarations only:
     [text]{#ff8a3d}   [text]{gold}   [text]{#fff on #1f2733}   [text]{on crimson}
     <span style="color: #ff8a3d">text</span>   (also background-color)
     <font color="#ff8a3d">text</font>          (rewritten to a span)
   ═══════════════════════════════════════════════════════════════════════ */
(function () {
  'use strict';

  const { esc } = window.BTTUtil;

  const _MD_ALLOWED = { a: 1, p: 1, br: 1, div: 1, span: 1, img: 1, picture: 1, source: 1,
    h1: 1, h2: 1, h3: 1, h4: 1, h5: 1, h6: 1, b: 1, strong: 1, i: 1, em: 1, u: 1, s: 1,
    strike: 1, del: 1, ins: 1, sub: 1, sup: 1, code: 1, pre: 1, kbd: 1, samp: 1, mark: 1,
    ul: 1, ol: 1, li: 1, blockquote: 1, hr: 1, table: 1, thead: 1, tbody: 1, tfoot: 1,
    tr: 1, td: 1, th: 1, caption: 1, details: 1, summary: 1, dl: 1, dt: 1, dd: 1, abbr: 1,
    small: 1, q: 1, cite: 1, center: 1, figure: 1, figcaption: 1 };
  const _MD_DROP = { script: 1, style: 1, iframe: 1, object: 1, embed: 1, form: 1, input: 1,
    button: 1, textarea: 1, select: 1, option: 1, link: 1, meta: 1, base: 1, svg: 1, math: 1,
    title: 1, noscript: 1, template: 1, frame: 1, frameset: 1, applet: 1, audio: 1, video: 1,
    canvas: 1, map: 1, area: 1 };
  const _MD_ATTR = { href: 1, src: 1, srcset: 1, alt: 1, title: 1, width: 1, height: 1,
    align: 1, valign: 1, colspan: 1, rowspan: 1, start: 1, open: 1, type: 1, media: 1,
    style: 1 };

  // ── Colour support ───────────────────────────────────────────────────────
  // `style` is allowed but filtered down to colour declarations ONLY, so a
  // README can't inject `position:fixed` overlays, `url(...)` beacons, etc.
  // Values are checked twice: a charset guard, then the browser's own CSS
  // parser (assigning a non-colour to `.style.color` leaves it empty), which
  // also normalises whatever the author wrote into one clean declaration.
  const _STYLE_PROPS = { color: 1, 'background-color': 1 };
  const _COLOR_CHARS = /^[#a-z0-9%.,()\/\s+-]{1,64}$/i;
  let _colorProbe = null;
  function _safeColor(v) {
    v = String(v == null ? '' : v).trim();
    if (!v || !_COLOR_CHARS.test(v)) return null;
    _colorProbe = _colorProbe || document.createElement('span');
    _colorProbe.style.color = '';
    try { _colorProbe.style.color = v; } catch (e) { return null; }
    return _colorProbe.style.color || null;
  }
  function _cleanStyle(v) {
    return String(v || '').split(';').map((decl) => {
      const i = decl.indexOf(':');
      if (i < 0) return '';
      const prop = decl.slice(0, i).trim().toLowerCase();
      if (!_STYLE_PROPS[prop]) return '';
      const val = _safeColor(decl.slice(i + 1));
      return val ? prop + ':' + val : '';
    }).filter(Boolean).join(';');
  }

  // Allow http(s)/mailto/relative/anchor URLs; block javascript:/data:/vbscript:/etc.
  // (control chars are collapsed first so `java\nscript:` can't slip past.)
  function _safeUrl(u) {
    u = String(u == null ? '' : u).trim();
    if (!u) return null;
    const m = u.replace(/[\u0000- ]+/g, '').toLowerCase().match(/^([a-z][a-z0-9+.-]*):/);
    if (m && m[1] !== 'http' && m[1] !== 'https' && m[1] !== 'mailto') return null;
    return u;
  }
  function _cleanSrcset(v) {
    return String(v || '').split(',').map((part) => {
      const seg = part.trim().split(/\s+/);
      const u = _safeUrl(seg[0]);
      return u ? [u, ...seg.slice(1)].join(' ') : '';
    }).filter(Boolean).join(', ');
  }
  function _sanitizeInto(src, dst) {
    for (const node of src.childNodes) {
      if (node.nodeType === 3) { dst.appendChild(document.createTextNode(node.nodeValue)); continue; }
      if (node.nodeType !== 1) continue;               // drop comments etc.
      const tag = node.tagName.toLowerCase();
      if (_MD_DROP[tag]) continue;                     // drop element + subtree
      // <font color="..."> - dead in HTML5 but the first thing most authors
      // reach for; rewrite it as a coloured <span>.
      if (tag === 'font') {
        const span = document.createElement('span');
        const c = _safeColor(node.getAttribute('color'));
        const style = [c ? 'color:' + c : '', _cleanStyle(node.getAttribute('style'))].filter(Boolean).join(';');
        if (style) span.setAttribute('style', style);
        _sanitizeInto(node, span);
        dst.appendChild(span);
        continue;
      }
      if (!_MD_ALLOWED[tag]) { _sanitizeInto(node, dst); continue; }  // unwrap unknown tags
      const el = document.createElement(tag);
      for (const attr of node.attributes) {
        const name = attr.name.toLowerCase();
        if (name.startsWith('on') || !_MD_ATTR[name]) continue;
        let val = attr.value;
        if (name === 'href' || name === 'src') { val = _safeUrl(val); if (val == null) continue; }
        else if (name === 'srcset') { val = _cleanSrcset(val); if (!val) continue; }
        else if (name === 'style') { val = _cleanStyle(val); if (!val) continue; }
        el.setAttribute(name, val);
      }
      if (tag === 'a') { el.setAttribute('target', '_blank'); el.setAttribute('rel', 'noopener nofollow ugc'); }
      if (tag === 'img') { if (!el.getAttribute('src')) continue; el.setAttribute('loading', 'lazy'); }
      _sanitizeInto(node, el);
      dst.appendChild(el);
    }
  }
  // Parse into an INERT document (DOMParser never runs scripts or loads resources),
  // rebuild an allowlisted tree, return its HTML string.
  function sanitizeHTML(html) {
    const doc = new DOMParser().parseFromString(String(html || ''), 'text/html');
    const out = document.createElement('div');
    _sanitizeInto(doc.body, out);
    return out.innerHTML;
  }

  // Inline markdown -> HTML (raw HTML passes; inline-code content is escaped). Run
  // the result through sanitizeHTML before inserting. `refs` resolves [text][id].
  function mdInline(s, refs) {
    refs = refs || {};
    const codes = [];
    s = s.replace(/`([^`]+)`/g, (m, c) => { codes.push('<code>' + esc(c) + '</code>'); return '\u0000' + (codes.length - 1) + '\u0000'; });
    // Colour shorthand: [text]{#ff8a3d} - [text]{gold} - [text]{#fff on #1f2733}
    // - [text]{on crimson}. A spec that isn't a real colour is left exactly as
    // typed, so ordinary prose that happens to use braces survives untouched.
    s = s.replace(/\[([^\]\n]+)\]\{([^}\n]{1,80})\}/g, (m, txt, spec) => {
      const on = spec.match(/^(?:(.*?)\s+)?on\s+(.+)$/i);
      const fgSpec = (on ? (on[1] || '') : spec).trim();
      const fg = fgSpec ? _safeColor(fgSpec) : null;
      const bg = on ? _safeColor(on[2]) : null;
      if ((fgSpec && !fg) || (on && !bg) || (!fg && !bg)) return m;
      const style = [fg ? 'color:' + fg : '', bg ? 'background-color:' + bg : ''].filter(Boolean).join(';');
      return `<span style="${style}">${txt}</span>`;
    });
    s = s.replace(/!\[([^\]]*)\]\(\s*([^)\s]+)(?:\s+["'][^)]*)?\)/g, '<img src="$2" alt="$1">');     // image
    s = s.replace(/!\[([^\]]*)\]\[([^\]]*)\]/g, (m, alt, id) => { const u = refs[(id || alt).trim().toLowerCase()]; return u ? `<img src="${u}" alt="${alt}">` : m; });
    s = s.replace(/\[([^\]]+)\]\(\s*([^)\s]+)(?:\s+["'][^)]*)?\)/g, '<a href="$2">$1</a>');           // link (also wraps linked images)
    s = s.replace(/\[([^\]]+)\]\[([^\]]*)\]/g, (m, txt, id) => { const u = refs[(id || txt).trim().toLowerCase()]; return u ? `<a href="${u}">${txt}</a>` : m; });
    s = s.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
    s = s.replace(/__([^_]+)__/g, '<strong>$1</strong>');
    s = s.replace(/(^|[^*])\*([^*\s][^*]*?)\*/g, '$1<em>$2</em>');
    s = s.replace(/~~([^~]+)~~/g, '<del>$1</del>');
    s = s.replace(/(^|[\s(>])(https?:\/\/[^\s<)"']+)/g, '$1<a href="$2">$2</a>');                      // bare-URL autolink
    s = s.replace(/\u0000(\d+)\u0000/g, (m, i) => codes[+i]);
    return s;
  }

  function _mdTable(lines, start, refs) {
    const row = (line) => line.replace(/^\s*\|?|\|?\s*$/g, '').split('|').map((c) => c.trim());
    const head = row(lines[start]);
    const aligns = row(lines[start + 1]).map((c) =>
      /^:-+:$/.test(c) ? 'center' : /-+:$/.test(c) ? 'right' : /^:-+/.test(c) ? 'left' : '');
    let i = start + 2, bodyRows = '';
    for (; i < lines.length && lines[i].includes('|') && lines[i].trim(); i++) {
      const cells = row(lines[i]).map((c, j) => `<td${aligns[j] ? ` align="${aligns[j]}"` : ''}>${mdInline(c, refs)}</td>`).join('');
      bodyRows += `<tr>${cells}</tr>`;
    }
    const headRow = head.map((c, j) => `<th${aligns[j] ? ` align="${aligns[j]}"` : ''}>${mdInline(c, refs)}</th>`).join('');
    return { html: `<table class="mp-md-table"><thead><tr>${headRow}</tr></thead><tbody>${bodyRows}</tbody></table>`, consumed: i - start };
  }

  function renderMarkdown(src) {
    const text = String(src == null ? '' : src).replace(/\r\n/g, '\n');
    // Pull out reference definitions: [label]: url "title"
    const refs = {};
    const lines = text.replace(/^[ ]{0,3}\[([^\]]+)\]:\s*(\S+)(?:\s+["'(].*)?[ \t]*$/gm,
      (m, label, url) => { refs[label.trim().toLowerCase()] = url; return ''; }).split('\n');
    const RAW_BLOCK = /^\s*<(div|table|figure|picture|details|blockquote|section|p|ul|ol|h[1-6]|pre|hr|center|img|a|article|header|footer|nav|aside|main|sub|sup|span)\b/i;
    let html = '', inCode = false, list = null, para = [];
    const flushPara = () => {
      if (!para.length) return;
      const joined = para.join('\n');
      const inline = mdInline(joined, refs);
      // CommonMark/GitHub: a single newline in a paragraph is a SOFT break (a
      // space), so consecutive lines (e.g. badge rows) flow inline. A hard break
      // needs two trailing spaces; honour that.
      const body = inline.replace(/ {2,}\n/g, '<br>').replace(/\n/g, ' ');
      html += RAW_BLOCK.test(joined) ? inline : `<p>${body}</p>`;
      para = [];
    };
    const closeList = () => { if (list) { html += `</${list}>`; list = null; } };
    for (let i = 0; i < lines.length; i++) {
      const raw = lines[i];
      if (raw.trim().startsWith('```') || raw.trim().startsWith('~~~')) {
        if (inCode) { html += '</code></pre>'; inCode = false; }
        else { flushPara(); closeList(); html += '<pre class="mp-md-pre"><code>'; inCode = true; }
        continue;
      }
      if (inCode) { html += esc(raw) + '\n'; continue; }
      // GFM table: a line with '|' followed by a separator row.
      if (raw.includes('|') && i + 1 < lines.length && /^\s*\|?[\s:|-]*-[\s:|-]*\|?\s*$/.test(lines[i + 1])) {
        flushPara(); closeList();
        const tbl = _mdTable(lines, i, refs); html += tbl.html; i += tbl.consumed - 1; continue;
      }
      const h = raw.match(/^(#{1,6})\s+(.*?)\s*#*$/);
      if (h) { flushPara(); closeList(); html += `<h${h[1].length} class="mp-md-h">${mdInline(h[2], refs)}</h${h[1].length}>`; continue; }
      if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(raw)) { flushPara(); closeList(); html += '<hr>'; continue; }
      const bq = raw.match(/^\s*>\s?(.*)$/);
      if (bq) { flushPara(); closeList(); html += `<blockquote class="mp-md-quote">${mdInline(bq[1], refs)}</blockquote>`; continue; }
      const ul = raw.match(/^\s*[-*+]\s+(.*)$/);
      if (ul) { flushPara(); if (list && list !== 'ul') closeList(); if (!list) { html += '<ul class="mp-md-ul">'; list = 'ul'; } html += `<li>${mdInline(ul[1], refs)}</li>`; continue; }
      const ol = raw.match(/^\s*\d+[.)]\s+(.*)$/);
      if (ol) { flushPara(); if (list && list !== 'ol') closeList(); if (!list) { html += '<ol class="mp-md-ol">'; list = 'ol'; } html += `<li>${mdInline(ol[1], refs)}</li>`; continue; }
      if (!raw.trim()) { flushPara(); closeList(); continue; }
      closeList();
      para.push(raw);
    }
    flushPara();
    if (inCode) html += '</code></pre>';
    closeList();
    return sanitizeHTML(html);
  }

  window.BTTMarkdown = { render: renderMarkdown, sanitize: sanitizeHTML, inline: mdInline };
})();
