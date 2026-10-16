/* ═══════════════════════════════════════════════════════════════════════
   swf-docs.js - hidden "Trove UI (Flash) decompile" reference.
   Fetches a grouped manifest, builds a searchable sidebar, and renders the
   selected markdown doc (via window.BTTMarkdown) into the content pane.
   Each doc is its own page, /swf-docs/<file-without-.md> (server-rendered by
   app/web/swf_docs.py); this switches between them in place with pushState.
   Old #<slug> links are redirected to the path form. Default view = README.
   ═══════════════════════════════════════════════════════════════════════ */
(function () {
  'use strict';

  const { esc: escapeHtml } = window.BTTUtil;

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
  let lastManifest = null;        // last-loaded manifest (re-render on language change)
  let currentFile = null;         // file currently shown in the content pane

  // Translate a JS-built string via the shared i18n runtime (English fallback).
  const t = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);

  const slug = (file) => file.replace(/\.md$/i, '');
  const urlFor = (file) => (slug(file) === 'README' ? '/swf-docs' : '/swf-docs/' + slug(file));
  const TITLE = 'Trove UI (Flash) Reference';
  const find = (want) => links.find((l) => slug(l.file).toLowerCase() === want.toLowerCase());

  // Same-page doc switch: update the address bar, then render.
  function go(file, replace) {
    const url = urlFor(file);
    if (location.pathname !== url || location.hash) history[replace ? 'replaceState' : 'pushState']({ file }, '', url);
    load(file);
  }

  const openSidebar  = () => { sidebar.classList.add('open'); backdrop.classList.add('show'); };
  const closeSidebar = () => { sidebar.classList.remove('open'); backdrop.classList.remove('show'); };

  function buildSidebar(manifest) {
    lastManifest = manifest;
    // Idempotent: strip any previously-built groups so this can re-run when
    // the language changes (to re-translate the generic labels below).
    Array.from(navHost.querySelectorAll('.nav-group, .nav-divider')).forEach((n) => n.remove());
    links = [];
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
        div.innerHTML = '<span>' + escapeHtml(t('Shared libraries')) + '</span><small>' + escapeHtml(t('Code shared across every SWF — not individual screens')) + '</small>';
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
        a.href = urlFor(it.file);
        a.dataset.file = it.file;
        a.title = it.summary || '';
        const label = (it.swf || it.file).replace(/\.swf$/i, '');
        const cls = it.mainClass && it.mainClass !== '—'
          ? '<span class="cls">' + escapeHtml(it.mainClass) + '</span>' : '';
        a.innerHTML = '<span>' + escapeHtml(label) + '</span>' + cls;
        a.addEventListener('click', (e) => {
          if (e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
          e.preventDefault();
          go(it.file);
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
    currentFile = file;
    const meta = byFile.get(file);
    if (cache.has(file)) { render(cache.get(file), meta); return; }
    content.innerHTML = '<div class="doc-loading"><i class="fa-solid fa-spinner fa-spin"></i> ' + escapeHtml(t('Loading…')) + '</div>';
    try {
      const res = await fetch(BASE + encodeURIComponent(file), { cache: 'no-cache' });
      if (!res.ok) throw new Error('HTTP ' + res.status);
      const md = await res.text();
      const html = window.BTTMarkdown.render(md);
      cache.set(file, html);
      render(html, meta);
    } catch (err) {
      content.innerHTML = '<div class="doc-error"><i class="fa-solid fa-triangle-exclamation"></i> ' + escapeHtml(t('Could not load this document.')) + ' ' + escapeHtml(String(err.message || err)) + '</div>';
    }
  }

  function render(html, meta) {
    const cls = meta && meta.mainClass && meta.mainClass !== '—' ? ' (' + meta.mainClass + ')' : '';
    document.title = meta ? (meta.swf || slug(currentFile)) + cls + ' - ' + TITLE : TITLE + ' - every Trove .swf screen decompiled';
    let chips = '';
    if (meta) {
      if (meta.swf)  chips += '<span class="doc-chip"><i class="fa-regular fa-file-code"></i> ' + escapeHtml(meta.swf) + '</span>';
      if (meta.size) chips += '<span class="doc-chip"><i class="fa-solid fa-weight-hanging"></i> ' + escapeHtml(meta.size) + '</span>';
      if (meta.mainClass && meta.mainClass !== '—') chips += '<span class="doc-chip">class <code>' + escapeHtml(meta.mainClass) + '</code></span>';
    }
    content.innerHTML =
      (chips ? '<div class="doc-meta">' + chips + '</div>' : '') +
      '<div class="md-body">' + html + '</div>' +
      '<div class="doc-foot"><span>' + escapeHtml(t('Trove UI (Flash) decompile reference')) + '</span><span><a href="/swf-docs">' + escapeHtml(t('Overview')) + '</a> · <a href="/">' + escapeHtml(t('Home')) + '</a></span></div>';
    rewriteDocLinks();
    content.scrollIntoView({ block: 'start' });
    window.scrollTo({ top: 0 });
  }

  // Cross-doc links in the rendered markdown point at sibling files
  // (e.g. `[atlas.swf](./atlas.md)`). Rewrite them to the doc's page path and
  // strip the target=_blank the sanitizer adds so they stay in this tab; the
  // click handler below then switches docs in place.
  function rewriteDocLinks() {
    content.querySelectorAll('.md-body a[href]').forEach((a) => {
      const m = (a.getAttribute('href') || '').match(/(?:^|\/)([^/?#]+)\.md(?:[?#].*)?$/i);
      if (!m) return;
      a.setAttribute('href', urlFor(m[1] + '.md'));
      a.removeAttribute('target');
      a.setAttribute('rel', 'noopener');
    });
  }

  content.addEventListener('click', (e) => {
    const a = e.target.closest('a[href^="/swf-docs"]');
    if (!a || e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
    const want = a.getAttribute('href').replace(/^\/swf-docs\/?/, '').replace(/[?#].*$/, '');
    const hit = want ? find(want) : null;
    if (want && !hit) return;
    e.preventDefault();
    go(hit ? hit.file : 'README.md');
  });

  function fromLocation() {
    const m = location.pathname.match(/^\/swf-docs\/([^/]+)\/?$/);
    const want = m ? decodeURIComponent(m[1]) : '';
    const legacy = (location.hash || '').replace(/^#/, '').trim();
    const hit = (want && find(want)) || (!want && legacy && find(legacy));
    if (hit && !want) { go(hit.file, true); return; }
    load(hit ? hit.file : 'README.md');
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
  window.addEventListener('popstate', fromLocation);
  // Old #<slug> links still work; any other hash is an in-page anchor, left alone.
  window.addEventListener('hashchange', () => {
    const hit = find((location.hash || '').replace(/^#/, '').trim());
    if (hit) go(hit.file, true);
  });
  window.addEventListener('scroll', () => { toTop.classList.toggle('show', window.scrollY > 500); }, { passive: true });
  toTop.addEventListener('click', () => window.scrollTo({ top: 0, behavior: 'smooth' }));
  document.addEventListener('keydown', (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
      e.preventDefault();
      if (window.innerWidth <= 980 && !sidebar.classList.contains('open')) openSidebar();
      search.focus(); search.select();
    }
  });

  // The floating support pill is wired by the shared app.js (loaded on this
  // page now), so no standalone copy is needed here.

  // The server stamps the UTC instant; show it in the reader's timezone and
  // the site language, keeping the UTC text if the stamp doesn't parse.
  function renderUpdated() {
    const el = document.getElementById('docsUpdated');
    if (!el) return;
    const when = new Date(el.getAttribute('datetime'));
    if (isNaN(when)) return;
    el.textContent = when.toLocaleString(document.documentElement.lang || undefined, { dateStyle: 'medium', timeStyle: 'short' });
    el.title = when.toString();
  }
  renderUpdated();

  // ---- Boot ----
  fetch(BASE + 'index.json', { cache: 'no-cache' })
    .then((r) => { if (!r.ok) throw new Error('HTTP ' + r.status); return r.json(); })
    .then((manifest) => { buildSidebar(manifest); fromLocation(); })
    .catch((err) => {
      content.innerHTML = '<div class="doc-error"><i class="fa-solid fa-triangle-exclamation"></i> ' + escapeHtml(t('Failed to load the document index.')) + ' ' + escapeHtml(String(err.message || err)) + '</div>';
    });

  // Re-translate the JS-built sidebar chrome when the user switches language.
  document.addEventListener('btt-lang-changed', () => {
    if (lastManifest) { buildSidebar(lastManifest); if (currentFile) setActive(currentFile); }
    renderUpdated();
  });
})();
