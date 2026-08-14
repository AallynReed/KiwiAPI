/* Site-wide search: the navbar dropdown and the home-page box.

   Both are the same thing in two shells, so the type-ahead lives here once and each
   shell just hands it an input and a panel. The /search results page is separate
   (search_page.js) - it browses one subject at a time rather than previewing all.

   Behaviour worth knowing:
   - Requests are debounced AND sequenced. A slow response for "gan" must never paint
     over a fast one for "ganda", so every response carries the token of the request
     that asked for it and stale tokens are dropped.
   - Enter goes to /search?q=… unless a result is highlighted, in which case it opens
     that result. That is the "if not selected the subject" behaviour: pick something
     and you go straight there, otherwise you get the full categorised page.
   - Keyboard: ArrowUp/Down move, Enter opens, Escape closes (and collapses the navbar
     field). The listbox/option roles are real, so it is operable without sight. */
(function () {
  'use strict';
  const { esc, fetchJSON, debounce, apiUrl } = window.BTTUtil;

  const MIN_QUERY = 2;
  const SUBJECT_ICON = {
    pages: 'fa-solid fa-compass',
    collections: 'fa-solid fa-paw',
    items: 'fa-solid fa-cube',
    recipes: 'fa-solid fa-flask',
    styles: 'fa-solid fa-shirt',
    players: 'fa-solid fa-user',
    mods: 'fa-solid fa-cubes',
    modpacks: 'fa-solid fa-box-open',
  };

  function t(s) {
    return (window.BTTi18n && window.BTTi18n.t) ? window.BTTi18n.t(s) : s;
  }

  /* A result's picture, when it has one. Codex rows render their voxel model and mod
     rows their banner; pages and players have no image and keep the icon.

     The <img> is layered OVER the icon rather than replacing it, and removes itself
     on error - the render endpoint legitimately 422s for a blueprint that decodes to
     an empty placeholder, and a broken-image glyph is worse than the icon we already
     had. */
  function thumbHTML(item, cls) {
    const icon = item.icon || SUBJECT_ICON[item.subject] || 'fa-solid fa-circle';
    const fallback = `<i class="${esc(icon)} ${cls}-icon" aria-hidden="true"></i>`;
    if (!item.image) return fallback;
    // `apiUrl` because /site/* has to be rewritten onto window.API_BASE - production
    // serves pages from the website container, which has no data plane, so a raw
    // relative src would request an image from a host that cannot render one.
    //
    // Eager, not lazy: these are the point of the row, they are small, and a lazy
    // image inside a dropdown that starts hidden may never enter a viewport at all.
    //
    // No inline onerror - strict CSP blocks inline handlers; failures are caught by
    // the capture-phase listener in `dropBrokenImages` (`error` does not bubble
    // from <img>).
    return `<span class="${cls}-thumb">${fallback}` +
      `<img src="${esc(apiUrl(item.image))}" alt="" decoding="async"></span>`;
  }

  function rowHTML(item, active) {
    // `kind` is the precise thing (mount, recipe, mod); `subject` is the column it
    // was found under. Showing the precise one matches how people describe results.
    const badge = esc(String(item.kind || item.subject || '').toUpperCase());
    const detail = item.detail ? `<span class="ss-row-detail">${esc(item.detail)}</span>` : '';
    return `<li class="ss-row${active ? ' is-active' : ''}" role="option"
                aria-selected="${active ? 'true' : 'false'}" data-path="${esc(item.path)}">
      ${thumbHTML(item, 'ss-row')}
      <span class="ss-row-main">
        <span class="ss-row-name">${esc(item.name)}</span>
        ${detail}
      </span>
      <span class="ss-row-badge">${badge}</span>
    </li>`;
  }


  /* Thumbnail lifecycle, for a whole list at once.

     The icon sits UNDER the image as a fallback, so it has to disappear the moment a
     real render arrives - these PNGs are transparent, so an icon left underneath
     shows straight through the model. Marking the wrapper on `load` is what hides it.

     The mirror case: the render endpoint legitimately 422s for a blueprint that
     decodes to an empty placeholder, and a mod banner can 404. Then the <img> is
     removed, the wrapper never gets marked, and the icon stays as intended.

     Both listeners are capture phase - neither `load` nor `error` bubbles from
     <img>, so a plain listener on the container would never see them. */
  function wireThumbnails(container) {
    if (!container) return;
    container.addEventListener('load', (e) => {
      const img = e.target;
      if (img && img.tagName === 'IMG' && img.parentElement) {
        img.parentElement.classList.add('has-image');
      }
    }, true);
    container.addEventListener('error', (e) => {
      if (e.target && e.target.tagName === 'IMG') e.target.remove();
    }, true);
  }

  /* Wire one input + panel into a live search. Returns a small controller so the
     navbar can also close it from its own toggle. */
  function attach(opts) {
    const input = opts.input;
    const panel = opts.panel;
    const list = opts.list;
    const empty = opts.empty;
    const seeAll = opts.seeAll;
    if (!input || !panel || !list) return null;

    let items = [];
    let cursor = -1;
    let token = 0;
    wireThumbnails(list);

    function close() {
      panel.hidden = true;
      input.setAttribute('aria-expanded', 'false');
      cursor = -1;
    }

    function open() {
      panel.hidden = false;
      input.setAttribute('aria-expanded', 'true');
    }

    function paint() {
      list.innerHTML = items.map((it, i) => rowHTML(it, i === cursor)).join('');
      if (empty) empty.hidden = items.length > 0;
      if (seeAll) {
        const q = input.value.trim();
        seeAll.href = '/search?q=' + encodeURIComponent(q);
        seeAll.hidden = !q;
      }
      if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh();
    }

    function go(path) {
      if (path) window.location.href = path;
    }

    const run = debounce(async () => {
      const q = input.value.trim();
      if (q.length < MIN_QUERY) {
        items = [];
        close();
        return;
      }
      const mine = ++token;
      let data;
      try {
        data = await fetchJSON('/site/search?q=' + encodeURIComponent(q));
      } catch (_err) {
        return;                       // leave the last good results up
      }
      if (mine !== token) return;     // a newer keystroke already answered
      items = data.items || [];
      cursor = -1;
      paint();
      open();
    }, 160);

    input.addEventListener('input', run);
    input.addEventListener('focus', () => { if (items.length) open(); });

    input.addEventListener('keydown', (e) => {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        if (!items.length) return;
        e.preventDefault();
        // cursor ranges -1 (nothing highlighted) .. last. Wrapping THROUGH -1 is
        // deliberate: arrowing past the end returns you to the plain query, so Enter
        // still means "see all results" without having to Escape first.
        const last = items.length - 1;
        cursor += (e.key === 'ArrowDown' ? 1 : -1);
        if (cursor > last) cursor = -1;
        else if (cursor < -1) cursor = last;
        paint();
        open();
        const active = list.querySelector('.is-active');
        if (active && active.scrollIntoView) active.scrollIntoView({ block: 'nearest' });
      } else if (e.key === 'Enter') {
        // Nothing highlighted => the categorised results page. Something highlighted
        // => straight to it.
        if (cursor >= 0 && items[cursor]) {
          e.preventDefault();
          go(items[cursor].path);
        }
      } else if (e.key === 'Escape') {
        close();
        if (opts.onEscape) opts.onEscape();
      }
    });

    list.addEventListener('mousedown', (e) => {
      const row = e.target.closest('.ss-row');
      if (!row) return;
      e.preventDefault();             // beat the blur, or the panel closes first
      go(row.dataset.path);
    });

    document.addEventListener('click', (e) => {
      if (!panel.hidden && !panel.contains(e.target) && e.target !== input) close();
    });

    return { close, open, input };
  }

  // ─── navbar shell ──────────────────────────────────────────────────
  function initNavbar() {
    const wrap = document.getElementById('nav-search');
    if (!wrap) return;
    const toggle = document.getElementById('nav-search-toggle');
    const input = document.getElementById('nav-search-field');
    const closeBtn = document.getElementById('nav-search-close');
    const form = document.getElementById('nav-search-form');

    const ctl = attach({
      input,
      panel: document.getElementById('nav-search-panel'),
      list: document.getElementById('nav-search-results'),
      empty: document.getElementById('nav-search-empty'),
      seeAll: document.getElementById('nav-search-all'),
      onEscape: () => collapse(),
    });

    function expand() {
      wrap.classList.add('is-open');
      if (toggle) toggle.setAttribute('aria-expanded', 'true');
      input.focus();
      input.select();
    }
    function collapse() {
      wrap.classList.remove('is-open');
      if (toggle) toggle.setAttribute('aria-expanded', 'false');
      if (ctl) ctl.close();
    }

    if (toggle) toggle.addEventListener('click', () => {
      if (wrap.classList.contains('is-open')) collapse(); else expand();
    });
    if (closeBtn) closeBtn.addEventListener('click', collapse);

    // The form still submits normally (Enter with nothing highlighted), which is
    // what takes you to /search - no JS required for the core journey.
    if (form) form.addEventListener('submit', (e) => {
      if (!input.value.trim()) e.preventDefault();
    });

    document.addEventListener('click', (e) => {
      if (wrap.classList.contains('is-open') && !wrap.contains(e.target)) collapse();
    });

    // Ctrl/Cmd+K focuses search. The Pages menu already claimed it for its own
    // filter; site-wide search is the better default for that muscle memory, and
    // the Pages filter is still one click away.
    document.addEventListener('keydown', (e) => {
      if ((e.ctrlKey || e.metaKey) && (e.key === 'k' || e.key === 'K')) {
        e.preventDefault();
        expand();
      }
    });
  }

  // ─── home-page shell ───────────────────────────────────────────────
  function initHome() {
    const input = document.getElementById('home-search-input');
    if (!input) return;
    attach({
      input,
      panel: document.getElementById('home-search-panel'),
      list: document.getElementById('home-search-results'),
      empty: document.getElementById('home-search-empty'),
      seeAll: document.getElementById('home-search-all'),
    });
  }

  function init() {
    // The navbar partial ships this script on every page, so a template that also
    // includes it would bind every handler twice - and two toggle handlers cancel
    // out exactly: the first opens the field, the second sees it open and collapses
    // it, so the button looks dead. Bind once, whatever the page asks for.
    if (window.__bttSiteSearchReady) return;
    window.__bttSiteSearchReady = true;
    initNavbar();
    initHome();
  }
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
