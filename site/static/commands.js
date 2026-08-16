/* /commands page. Client-side render from commands.json so language switching
   is instant (no reload/flicker); re-renders on i18n.js's `btt-lang-changed`. */

(function () {
  'use strict';

  const { esc: escapeHtml } = window.BTTUtil;

  const SUPPORTED_LANGS = new Set(['en', 'fr', 'de', 'pt-PT', 'ru', 'ja', 'ko', 'zh-CN', 'es', 'th']);
  const STORAGE_KEY = 'btt_docs_lang';  // shared with i18n.js for consistency

  let data = null;     // commands.json, lazy-loaded (see loadData)
  let currentLang = pickInitialLang();

  // The list is server-rendered from commands.json in English (see
  // commands.html + commands_page.py), so the page is complete without JS.
  // When the server DOM already matches the active language we hydrate in
  // place - no fetch, no rebuild; otherwise (a non-English visitor, or a
  // future non-SSR deploy) we fall back to fetching + building as before.
  const listEl = document.getElementById('commands-list');
  const ssrLang = (listEl && listEl.dataset.ssrLang) || '';

  // ── Boot ────────────────────────────────────────────────────────────
  if (ssrLang && currentLang === ssrLang) {
    wireChips();
    applyFilter(document.getElementById('commands-search').value || '');
  } else {
    loadData()
      .then(() => renderAll())
      .catch((err) => {
        // Keep the server-rendered English DOM if there was one; only the
        // pure-client path shows the hard failure message.
        if (ssrLang) return;
        const list = document.getElementById('commands-list');
        list.innerHTML = `<p class="commands-empty">Failed to load commands.json (${err.message}). Try refreshing the page.</p>`;
      });
  }

  // Lazy-load + cache commands.json (needed for language switching, and for the
  // non-SSR build path). Resolves to the parsed data.
  function loadData() {
    if (data) return Promise.resolve(data);
    return fetch('/static/commands.json?v=20260706k', { cache: 'force-cache' })
      .then((r) => { if (!r.ok) throw new Error('HTTP ' + r.status); return r.json(); })
      .then((json) => { data = json; return json; });
  }


  function pickInitialLang() {
    try {
      const saved = localStorage.getItem(STORAGE_KEY);
      if (saved && SUPPORTED_LANGS.has(saved)) return saved;
    } catch (e) {}
    // Best-effort detect from navigator.language; fall back to en.
    const nav = (navigator.language || '').toLowerCase();
    if (nav.startsWith('zh')) return 'zh-CN';
    if (nav.startsWith('ja')) return 'ja';
    if (nav.startsWith('ko')) return 'ko';
    if (nav.startsWith('ru')) return 'ru';
    if (nav.startsWith('pt')) return 'pt-PT';
    if (nav.startsWith('fr')) return 'fr';
    if (nav.startsWith('de')) return 'de';
    if (nav.startsWith('es')) return 'es';
    if (nav.startsWith('th')) return 'th';
    return 'en';
  }


  // i18n.js dispatches this when the user switches languages. We just
  // re-render the localised strings in place - no DOM rebuild.
  document.addEventListener('btt-lang-changed', (e) => {
    if (e && e.detail && e.detail.lang && SUPPORTED_LANGS.has(e.detail.lang)) {
      currentLang = e.detail.lang;
    }
    // The SSR path may not have fetched the data yet - load it, then re-render
    // the whole page in the new language.
    loadData().then(() => renderAll()).catch(() => {});
  });


  function t(field) {
    // field is an object like { en, fr, ja, ... }; falls back to en if
    // the active locale is missing.
    if (!field) return '';
    return field[currentLang] || field.en || '';
  }


  // ── Render ──────────────────────────────────────────────────────────
  function renderAll() {
    renderIntro();
    renderChips();
    renderList();
    applyFilter(document.getElementById('commands-search').value || '');
  }


  function renderIntro() {
    // Translate title / subtitle / placeholder / rules from commands.json
    // (these strings aren't in the locale JSON files i18n.js loads -
    // they live in commands.json so the page is self-contained).
    document.querySelectorAll('[data-cmd-text]').forEach((el) => {
      const key = el.dataset.cmdText;
      const txt = t(data.intro[key]);
      if (txt) el.textContent = txt;
    });
    const search = document.getElementById('commands-search');
    search.placeholder = t(data.intro.search_placeholder) || search.placeholder;

    const rulesList = document.getElementById('commands-rules-list');
    rulesList.innerHTML = '';
    const rules = (data.intro.rules && (data.intro.rules[currentLang] || data.intro.rules.en)) || [];
    for (const rule of rules) {
      const li = document.createElement('li');
      li.textContent = rule;
      rulesList.appendChild(li);
    }

    document.getElementById('commands-source').textContent = t(data.intro.source_note);
  }


  function renderChips() {
    const chips = document.getElementById('commands-chips');
    chips.innerHTML = '';
    for (const cat of data.categories) {
      const count = data.commands.filter((c) => c.category === cat.key).length;
      const btn = document.createElement('button');
      btn.type = 'button';
      btn.className = 'commands-chip';
      btn.dataset.cat = cat.key;
      btn.innerHTML = `${escapeHtml(t(cat.name))}<span class="commands-chip-count">${count}</span>`;
      chips.appendChild(btn);
    }
    wireChips();
  }


  // Attach the scroll-to-category handler to every chip - server-rendered or
  // freshly built by renderChips. Idempotent (the _wired flag) so re-renders
  // on language switch don't stack duplicate listeners.
  function wireChips() {
    document.querySelectorAll('.commands-chip').forEach((btn) => {
      if (btn._wired) return;
      btn._wired = true;
      btn.addEventListener('click', () => {
        const target = document.getElementById('cat-' + btn.dataset.cat);
        if (target) target.scrollIntoView({ behavior: 'smooth', block: 'start' });
      });
    });
  }


  function renderList() {
    const list = document.getElementById('commands-list');
    list.innerHTML = '';
    const byCat = new Map(data.categories.map((c) => [c.key, []]));
    for (const cmd of data.commands) {
      if (byCat.has(cmd.category)) byCat.get(cmd.category).push(cmd);
    }
    let total = 0;
    for (const cat of data.categories) {
      const items = byCat.get(cat.key) || [];
      if (!items.length) continue;
      total += items.length;
      const section = document.createElement('section');
      section.className = 'commands-category';
      section.id = 'cat-' + cat.key;
      section.dataset.cat = cat.key;
      section.innerHTML = `
        <h2 class="commands-category-title">
          ${escapeHtml(t(cat.name))}
          <span class="commands-category-count">${items.length}</span>
        </h2>
        <div class="commands-rows"></div>
      `;
      const rowsEl = section.querySelector('.commands-rows');
      for (const cmd of items) {
        rowsEl.appendChild(renderRow(cmd));
      }
      list.appendChild(section);
    }
    document.getElementById('commands-count-num').textContent = total;
  }


  function renderRow(cmd) {
    const row = document.createElement('div');
    row.className = 'command-row';
    // Precompute lowercased searchable text into a dataset attr so the filter
    // never re-pulls innerText (which would force layout).
    const aliasStr = (cmd.aliases || []).join(' ');
    const desc = t(cmd.description);
    const note = cmd.note ? t(cmd.note) : '';
    row.dataset.search = (cmd.syntax + ' ' + aliasStr + ' ' + desc + ' ' + note).toLowerCase();

    const syntaxCell = document.createElement('div');
    syntaxCell.className = 'command-syntax';
    syntaxCell.textContent = cmd.syntax;
    if (cmd.aliases && cmd.aliases.length) {
      const a = document.createElement('span');
      a.className = 'command-aliases';
      a.textContent = cmd.aliases.join(', ');
      syntaxCell.appendChild(a);
    }
    row.appendChild(syntaxCell);

    const textCell = document.createElement('div');
    textCell.className = 'command-text';
    const descEl = document.createElement('div');
    descEl.textContent = desc;
    textCell.appendChild(descEl);
    // Yellow usage-caution block when the syntax has a <placeholder>. Regex
    // matches "<…>" but rejects nested angle brackets (we have none).
    if (/<[^<>]+>/.test(cmd.syntax)) {
      const warnEl = document.createElement('div');
      warnEl.className = 'command-warning';
      const icon = document.createElement('i');
      icon.className = 'fa-solid fa-triangle-exclamation';
      icon.setAttribute('aria-hidden', 'true');
      warnEl.appendChild(icon);
      const warnText = document.createElement('span');
      warnText.textContent = t(data.intro.placeholder_warning);
      warnEl.appendChild(warnText);
      textCell.appendChild(warnEl);
    }
    if (note) {
      const noteEl = document.createElement('div');
      noteEl.className = 'command-note';
      noteEl.textContent = note;
      textCell.appendChild(noteEl);
    }
    row.appendChild(textCell);
    return row;
  }


  // ── Search filter ───────────────────────────────────────────────────
  const searchInput = document.getElementById('commands-search');
  searchInput.addEventListener('input', (e) => applyFilter(e.target.value));

  function applyFilter(query) {
    const q = (query || '').trim().toLowerCase();
    let visibleTotal = 0;
    document.querySelectorAll('.commands-category').forEach((section) => {
      let visibleInCat = 0;
      section.querySelectorAll('.command-row').forEach((row) => {
        const match = !q || row.dataset.search.includes(q);
        row.classList.toggle('no-match', !match);
        if (match) visibleInCat++;
      });
      section.style.display = visibleInCat ? '' : 'none';
      visibleTotal += visibleInCat;
    });
    const countEl = document.getElementById('commands-count-num');
    if (countEl) countEl.textContent = visibleTotal;
    // Empty-state fallback when nothing matches.
    const list = document.getElementById('commands-list');
    let emptyEl = list.querySelector('.commands-empty');
    if (visibleTotal === 0) {
      if (!emptyEl) {
        emptyEl = document.createElement('p');
        emptyEl.className = 'commands-empty';
        emptyEl.textContent = q ? 'No commands match "' + q + '".' : 'No commands.';
        list.appendChild(emptyEl);
      } else {
        emptyEl.textContent = q ? 'No commands match "' + q + '".' : 'No commands.';
      }
    } else if (emptyEl) {
      emptyEl.remove();
    }
  }


  // ── Chip highlighting on scroll ─────────────────────────────────────
  // Watch each category section; the topmost one currently in view
  // gets its chip marked active. IntersectionObserver instead of a
  // scroll listener keeps it cheap.
  if ('IntersectionObserver' in window) {
    const io = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        const cat = entry.target.dataset.cat;
        document.querySelectorAll('.commands-chip').forEach((c) => {
          c.classList.toggle('active', c.dataset.cat === cat);
        });
        break;  // first intersecting is the top one we want
      }
    }, { rootMargin: '-140px 0px -70% 0px', threshold: 0 });
    // Need to re-observe after each renderList since the sections are
    // freshly built. Watch a MutationObserver on the list parent.
    const watchSections = () => {
      io.disconnect();
      document.querySelectorAll('.commands-category').forEach((s) => io.observe(s));
    };
    const mo = new MutationObserver(watchSections);
    mo.observe(document.getElementById('commands-list'), { childList: true });
    watchSections();
  }



  // ── Support widget ──────────────────────────────────────────────────
  // Duplicated verbatim from section 7 of landing.js because /commands
  // doesn't load landing.js (canvas particles + slideshow + live data
  // would all be wasted work here). Keep these two copies in sync if the
  // widget markup ever changes.
  (function () {
    const widget  = document.getElementById('support-widget');
    const trigger = document.getElementById('support-trigger');
    const panel   = document.getElementById('support-panel');
    if (!widget || !trigger || !panel) return;
    const setOpen = (open) => {
      widget.classList.toggle('open', open);
      trigger.setAttribute('aria-expanded', open ? 'true' : 'false');
      panel.hidden = !open;
    };
    trigger.addEventListener('click', (e) => {
      e.stopPropagation();
      setOpen(!widget.classList.contains('open'));
    });
    document.addEventListener('click', (e) => {
      if (!widget.contains(e.target) && widget.classList.contains('open')) setOpen(false);
    });
    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && widget.classList.contains('open')) {
        setOpen(false); trigger.focus();
      }
    });
  })();
})();
