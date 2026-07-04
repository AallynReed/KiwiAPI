/* ═══════════════════════════════════════════════════════════════════════
   swf-docs.js - hidden "Trove UI (Flash) decompile" reference.
   Fetches a grouped manifest, builds a searchable sidebar, and renders the
   selected markdown doc (via window.BTTMarkdown) into the content pane.
   Deep-links: #<file-without-.md> (e.g. #atlas). Default view = README.
   ═══════════════════════════════════════════════════════════════════════ */
(function () {
  'use strict';

  const BASE = '/static/swf-docs/';
  const sidebar  = document.getElementById('sidebar');
  const backdrop = document.getElementById('backdrop');
  const menuBtn  = document.getElementById('menuBtn');
  const navHost  = document.getElementById('sideNav');
  const navEmpty = document.getElementById('navEmpty');
  const search   = document.getElementById('navSearch');
  const content  = document.getElementById('docBody');
  const toTop    = document.getElementById('to-top');

  let links = [];                 // {a, file, hay} for each sidebar entry
  const cache = new Map();        // file -> rendered HTML
  const byFile = new Map();       // file -> {swf, mainClass, summary}

  const slug = (file) => file.replace(/\.md$/i, '');

  const openSidebar  = () => { sidebar.classList.add('open'); backdrop.classList.add('show'); };
  const closeSidebar = () => { sidebar.classList.remove('open'); backdrop.classList.remove('show'); };

  function buildSidebar(manifest) {
    const frag = document.createDocumentFragment();
    let dividedShared = false;
    (manifest.groups || []).forEach((group) => {
      const shared = group.kind === 'shared';
      // Shared component libraries aren't screens — fence them off below the
      // UI groups with a labelled divider so they read as a distinct section.
      if (shared && !dividedShared) {
        dividedShared = true;
        const div = document.createElement('div');
        div.className = 'nav-divider';
        div.innerHTML = '<span>Shared libraries</span><small>Code shared across every SWF — not individual screens</small>';
        frag.appendChild(div);
      }
      const wrap = document.createElement('div');
      wrap.className = 'nav-group' + (shared ? ' nav-group--shared' : '');
      const title = document.createElement('div');
      title.className = 'nav-group-title';
      title.innerHTML = '<i class="fa-solid fa-' + (shared ? 'layer-group' : 'folder') + '"></i> ' + escapeHtml(group.title);
      wrap.appendChild(title);
      const ul = document.createElement('ul');
      (group.items || []).forEach((it) => {
        byFile.set(it.file, it);
        const li = document.createElement('li');
        const a = document.createElement('a');
        a.href = '#' + slug(it.file);
        a.dataset.file = it.file;
        a.title = it.summary || '';
        const label = (it.swf || it.file).replace(/\.swf$/i, '');
        const cls = it.mainClass && it.mainClass !== '—'
          ? '<span class="cls">' + escapeHtml(it.mainClass) + '</span>' : '';
        a.innerHTML = '<span>' + escapeHtml(label) + '</span>' + cls;
        a.addEventListener('click', (e) => {
          e.preventDefault();
          if (location.hash === a.getAttribute('href')) load(it.file);
          else location.hash = a.getAttribute('href');
          closeSidebar();
        });
        li.appendChild(a);
        ul.appendChild(li);
        links.push({ a, file: it.file, group: wrap, hay: (label + ' ' + (it.mainClass || '') + ' ' + (it.summary || '')).toLowerCase() });
      });
      wrap.appendChild(ul);
      frag.appendChild(wrap);
    });
    navHost.insertBefore(frag, navEmpty);
  }

  function setActive(file) {
    links.forEach((l) => l.a.classList.toggle('active', l.file === file));
  }

  async function load(file) {
    setActive(file);
    const meta = byFile.get(file);
    if (cache.has(file)) { render(cache.get(file), meta); return; }
    content.innerHTML = '<div class="doc-loading"><i class="fa-solid fa-spinner fa-spin"></i> Loading…</div>';
    try {
      const res = await fetch(BASE + encodeURIComponent(file), { cache: 'no-cache' });
      if (!res.ok) throw new Error('HTTP ' + res.status);
      const md = await res.text();
      const html = window.BTTMarkdown.render(md);
      cache.set(file, html);
      render(html, meta);
    } catch (err) {
      content.innerHTML = '<div class="doc-error"><i class="fa-solid fa-triangle-exclamation"></i> Could not load this document. ' + escapeHtml(String(err.message || err)) + '</div>';
    }
  }

  function render(html, meta) {
    let chips = '';
    if (meta) {
      if (meta.swf)  chips += '<span class="doc-chip"><i class="fa-regular fa-file-code"></i> ' + escapeHtml(meta.swf) + '</span>';
      if (meta.size) chips += '<span class="doc-chip"><i class="fa-solid fa-weight-hanging"></i> ' + escapeHtml(meta.size) + '</span>';
      if (meta.mainClass && meta.mainClass !== '—') chips += '<span class="doc-chip">class <code>' + escapeHtml(meta.mainClass) + '</code></span>';
    }
    content.innerHTML =
      (chips ? '<div class="doc-meta">' + chips + '</div>' : '') +
      '<div class="md-body">' + html + '</div>' +
      '<div class="doc-foot"><span>Trove UI (Flash) decompile reference</span><span><a href="#README">Overview</a> · <a href="/">Home</a></span></div>';
    rewriteDocLinks();
    content.scrollIntoView({ block: 'start' });
    window.scrollTo({ top: 0 });
  }

  // Cross-doc links in the rendered markdown point at sibling files
  // (e.g. `[atlas.swf](./atlas.md)`). Rewrite them to the in-page `#slug`
  // form so they switch docs via hashchange instead of navigating away, and
  // strip the target=_blank the sanitizer adds so they stay in this tab.
  function rewriteDocLinks() {
    content.querySelectorAll('.md-body a[href]').forEach((a) => {
      const m = (a.getAttribute('href') || '').match(/(?:^|\/)([^/?#]+)\.md(?:[?#].*)?$/i);
      if (!m) return;
      a.setAttribute('href', '#' + m[1]);
      a.removeAttribute('target');
      a.setAttribute('rel', 'noopener');
    });
  }

  function fromHash() {
    const want = (location.hash || '').replace(/^#/, '').trim();
    if (want) {
      const hit = links.find((l) => slug(l.file).toLowerCase() === want.toLowerCase());
      if (hit) { load(hit.file); return; }
    }
    load('README.md');
  }

  // ---- Search filter ----
  search.addEventListener('input', () => {
    const q = search.value.trim().toLowerCase();
    let any = false;
    const groupsSeen = new Set();
    links.forEach((l) => {
      const match = !q || l.hay.includes(q);
      l.a.parentElement.style.display = match ? '' : 'none';
      if (match) { any = true; groupsSeen.add(l.group); }
    });
    document.querySelectorAll('#sideNav .nav-group').forEach((g) => {
      g.style.display = (!q || groupsSeen.has(g)) ? '' : 'none';
    });
    // Hide the "Shared libraries" divider when its group is filtered out.
    const div = document.querySelector('#sideNav .nav-divider');
    if (div) {
      const sharedVisible = Array.from(document.querySelectorAll('#sideNav .nav-group--shared'))
        .some((g) => g.style.display !== 'none');
      div.style.display = sharedVisible ? '' : 'none';
    }
    navEmpty.style.display = any ? 'none' : 'block';
  });

  // ---- Chrome wiring ----
  if (menuBtn) menuBtn.addEventListener('click', () => sidebar.classList.contains('open') ? closeSidebar() : openSidebar());
  backdrop.addEventListener('click', closeSidebar);
  window.addEventListener('hashchange', fromHash);
  window.addEventListener('scroll', () => { toTop.classList.toggle('show', window.scrollY > 500); }, { passive: true });
  toTop.addEventListener('click', () => window.scrollTo({ top: 0, behavior: 'smooth' }));
  document.addEventListener('keydown', (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
      e.preventDefault();
      if (window.innerWidth <= 980 && !sidebar.classList.contains('open')) openSidebar();
      search.focus(); search.select();
    }
  });

  // ---- Support pill (corner) ----
  // Standalone copy of app.js's toggle, since this page doesn't load app.js.
  (function () {
    const widget = document.getElementById('support-widget');
    const trigger = document.getElementById('support-trigger');
    const panel = document.getElementById('support-panel');
    if (!widget || !trigger || !panel) return;
    panel.removeAttribute('hidden');   // CSS handles closed-state visibility
    const setOpen = (open) => {
      widget.classList.toggle('open', open);
      trigger.setAttribute('aria-expanded', open ? 'true' : 'false');
      panel.setAttribute('aria-hidden', open ? 'false' : 'true');
    };
    setOpen(false);
    trigger.addEventListener('click', (e) => { e.stopPropagation(); setOpen(!widget.classList.contains('open')); });
    document.addEventListener('click', (e) => { if (!widget.contains(e.target) && widget.classList.contains('open')) setOpen(false); });
    document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && widget.classList.contains('open')) { setOpen(false); trigger.focus(); } });
  })();

  function escapeHtml(s) {
    return String(s == null ? '' : s).replace(/[&<>"']/g,
      (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  }

  // ---- Boot ----
  fetch(BASE + 'index.json', { cache: 'no-cache' })
    .then((r) => { if (!r.ok) throw new Error('HTTP ' + r.status); return r.json(); })
    .then((manifest) => { buildSidebar(manifest); fromHash(); })
    .catch((err) => {
      content.innerHTML = '<div class="doc-error"><i class="fa-solid fa-triangle-exclamation"></i> Failed to load the document index. ' + escapeHtml(String(err.message || err)) + '</div>';
    });
})();
