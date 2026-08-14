/* The /search results page: a subject sidebar with hit counts, and one subject's
   results paged beside it.

   The split exists because the subjects have wildly different sizes - a query can hit
   2 pages and 900 items - so a single merged list is dominated by whichever subject
   happens to be biggest. Choosing the subject first is what makes the long ones
   browsable and keeps the short ones visible.

   State lives in the URL (?q=&subject=), so a result view is a shareable link and Back
   works. */
(function () {
  'use strict';
  const { esc, fetchJSON } = window.BTTUtil;

  const PAGE = 30;
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

  const $ = (id) => document.getElementById(id);
  const state = { q: '', subject: null, offset: 0, total: 0, loading: false };

  function t(s) {
    return (window.BTTi18n && window.BTTi18n.t) ? window.BTTi18n.t(s) : s;
  }
  function rerunI18n(node) {
    if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh();
  }
  function fmt(n) { return Number(n || 0).toLocaleString(); }

  function readURL() {
    const p = new URLSearchParams(window.location.search);
    state.q = (p.get('q') || '').trim();
    state.subject = p.get('subject') || null;
    state.offset = 0;
  }

  function writeURL(replace) {
    const p = new URLSearchParams();
    if (state.q) p.set('q', state.q);
    if (state.subject) p.set('subject', state.subject);
    const url = '/search' + (p.toString() ? '?' + p.toString() : '');
    if (replace) window.history.replaceState({}, '', url);
    else window.history.pushState({}, '', url);
  }

  function subjectHTML(s, active) {
    const icon = SUBJECT_ICON[s.key] || 'fa-solid fa-circle';
    return `<li>
      <button type="button" class="srch-subject${active ? ' is-active' : ''}"
              data-subject="${esc(s.key)}" aria-current="${active ? 'true' : 'false'}">
        <i class="${esc(icon)}" aria-hidden="true"></i>
        <span class="srch-subject-label" data-i18n>${esc(s.label)}</span>
        <span class="srch-subject-count">${fmt(s.count)}</span>
      </button></li>`;
  }

  function resultHTML(item) {
    const icon = item.icon || SUBJECT_ICON[item.subject] || 'fa-solid fa-circle';
    const badge = esc(String(item.kind || item.subject || '').toUpperCase());
    const detail = item.detail ? `<span class="srch-res-detail">${esc(item.detail)}</span>` : '';
    return `<li class="srch-res">
      <a class="srch-res-link" href="${esc(item.path)}">
        <i class="${esc(icon)} srch-res-icon" aria-hidden="true"></i>
        <span class="srch-res-main">
          <span class="srch-res-name">${esc(item.name)}</span>
          ${detail}
        </span>
        <span class="srch-res-badge">${badge}</span>
      </a></li>`;
  }

  function setBusy(on) {
    const panel = $('srch-panel');
    if (panel) panel.setAttribute('aria-busy', on ? 'true' : 'false');
  }

  async function load(append) {
    if (state.loading) return;
    const input = $('srch-input');
    if (input && !append) input.value = state.q;

    const hint = $('srch-hint');
    if (state.q.length < 2) {
      $('srch-results').innerHTML = '';
      $('srch-subjects').innerHTML = '';
      $('srch-empty').hidden = true;
      $('srch-more').hidden = true;
      $('srch-summary').textContent = '';
      if (hint) hint.hidden = false;
      return;
    }
    if (hint) hint.hidden = true;

    state.loading = true;
    setBusy(true);
    let data;
    try {
      const p = new URLSearchParams({ q: state.q, limit: String(PAGE), offset: String(append ? state.offset : 0) });
      if (state.subject) p.set('subject', state.subject);
      data = await fetchJSON('/site/search?' + p.toString());
    } catch (_err) {
      state.loading = false;
      setBusy(false);
      return;
    }
    state.loading = false;
    setBusy(false);

    const subjects = data.subjects || [];
    // No subject chosen yet: land on the one with the most hits, which is almost
    // always what was meant. Doing this here rather than server-side keeps the
    // endpoint's preview mode honest (it really is "a bit of everything").
    if (!state.subject && subjects.length) {
      const best = subjects.slice().sort((a, b) => b.count - a.count)[0];
      if (best && best.count) {
        state.subject = best.key;
        writeURL(true);
        return load(false);
      }
    }

    // Only categories that actually hit. A row reading "Styles 0" is not an offer,
    // it's noise - and with eight subjects the zeros crowd out the ones with answers.
    // The active subject stays listed even at zero so the sidebar doesn't drop the
    // row you're currently looking at out from under you.
    $('srch-subjects').innerHTML = subjects
      .filter((s) => s.count > 0 || s.key === state.subject)
      .map((s) => subjectHTML(s, s.key === state.subject)).join('');

    const items = data.items || [];
    if (append) {
      $('srch-results').insertAdjacentHTML('beforeend', items.map(resultHTML).join(''));
      state.offset += items.length;
    } else {
      $('srch-results').innerHTML = items.map(resultHTML).join('');
      state.offset = items.length;
    }
    state.total = data.total || 0;

    $('srch-empty').hidden = state.offset > 0;
    $('srch-more').hidden = state.offset >= state.total;
    const label = (subjects.find((s) => s.key === state.subject) || {}).label || '';
    $('srch-summary').textContent = state.total
      ? `${fmt(state.total)} ${t('results in')} ${t(label)} ${t('for')} "${state.q}"`
      : `${t('No results for')} "${state.q}"`;
    rerunI18n($('srch-sidebar'));
  }

  function init() {
    readURL();

    $('srch-form').addEventListener('submit', (e) => {
      e.preventDefault();
      const next = $('srch-input').value.trim();
      if (next === state.q) return;
      state.q = next;
      state.subject = null;           // a new query re-picks the best subject
      writeURL(false);
      load(false);
    });

    $('srch-subjects').addEventListener('click', (e) => {
      const btn = e.target.closest('.srch-subject');
      if (!btn) return;
      state.subject = btn.dataset.subject;
      writeURL(false);
      load(false);
    });

    $('srch-more').addEventListener('click', () => load(true));

    window.addEventListener('popstate', () => { readURL(); load(false); });

    load(false);
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
