/* ═══════════════════════════════════════════════════════════════════════
   /codexes/crafting - Recipe Cost Calculator (Beta)
   ───────────────────────────────────────────────────────────────────────
   Pick a craftable item (recipe search over /site/codexes/search?type=recipe),
   then fetch its full priced dependency tree from /site/codexes/crafting. The
   server walks the recipe graph and joins market medians; this file just paints
   the summary + the collapsible tree and the craft-vs-buy call per node.

   Prices are best-effort: ingredients the market bot doesn't track come back
   price-unknown, shown as "no market data" and never treated as free.

   URL hash: #path=<recipe source path>&branch=<live-us|pts> for deep-linking.
   ═══════════════════════════════════════════════════════════════════════ */

(function () {
  'use strict';

  const state = {
    branch: 'live-us',
    query: '',
    suggestions: [],
    activePath: null,   // recipe source path currently shown
    tree: null,
  };

  const $ = (id) => document.getElementById(id);
  const $search = $('craft-search');
  const $suggest = $('craft-suggest');
  const $empty = $('craft-empty');
  const $panel = $('craft-panel');

  // ─── Boot ──────────────────────────────────────────────────────────
  init().catch((err) => {
    console.error('[crafting] boot failed', err);
    showError(err);
  });

  async function init() {
    applyHash();
    wireEvents();
    if (state.activePath) {
      await loadTree(state.activePath);
    }
  }

  function applyHash() {
    const h = new URLSearchParams((location.hash || '').replace(/^#/, ''));
    const b = h.get('branch');
    if (b === 'live-us' || b === 'pts') state.branch = b;
    const p = h.get('path');
    if (p) state.activePath = p;
    for (const btn of document.querySelectorAll('.cdx-branch-btn')) {
      btn.classList.toggle('active', btn.dataset.branch === state.branch);
    }
  }

  function updateHash() {
    const parts = [];
    if (state.activePath) parts.push('path=' + enc(state.activePath));
    if (state.branch !== 'live-us') parts.push('branch=' + enc(state.branch));
    const next = parts.length ? '#' + parts.join('&') : '';
    if (next !== location.hash) history.replaceState(null, '', location.pathname + next);
  }

  // ─── Events ────────────────────────────────────────────────────────
  function wireEvents() {
    $search.addEventListener('input', debounce(() => {
      state.query = $search.value.trim();
      runSuggest();
    }, 200));

    $search.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && state.suggestions.length) {
        e.preventDefault();
        selectRecipe(state.suggestions[0]);
      } else if (e.key === 'Escape') {
        closeSuggest();
      }
    });

    document.addEventListener('click', (e) => {
      if (!e.target.closest('.craft-search-wrap')) closeSuggest();
    });

    for (const btn of document.querySelectorAll('.cdx-branch-btn')) {
      btn.addEventListener('click', () => {
        if (btn.dataset.branch === state.branch) return;
        state.branch = btn.dataset.branch;
        for (const b of document.querySelectorAll('.cdx-branch-btn')) {
          b.classList.toggle('active', b === btn);
        }
        updateHash();
        if (state.query) runSuggest();
        if (state.activePath) loadTree(state.activePath);
      });
    }

    document.addEventListener('btt-lang-changed', () => {
      if (state.tree) renderPanel(state.tree);
    });
  }

  // ─── Recipe suggestions ────────────────────────────────────────────
  let _suggestToken = 0;
  async function runSuggest() {
    const q = state.query;
    if (!q) { closeSuggest(); return; }
    const token = ++_suggestToken;
    try {
      const data = await fetchJSON(
        `/site/codexes/search?type=recipe&branch=${enc(state.branch)}`
        + `&q=${enc(q)}&limit=12&sort=name`);
      if (token !== _suggestToken) return;
      state.suggestions = data.items || [];
      renderSuggest();
    } catch (err) {
      if (token !== _suggestToken) return;
      $suggest.innerHTML = `<p class="craft-suggest-empty">${esc(errMsg(err))}</p>`;
      openSuggest();
    }
  }

  function renderSuggest() {
    if (!state.suggestions.length) {
      $suggest.innerHTML = `<p class="craft-suggest-empty" data-i18n>No recipes match.</p>`;
      openSuggest();
      rerunI18n();
      return;
    }
    $suggest.innerHTML = state.suggestions.map((r, i) => {
      const out = (r.data && r.data.recipe && r.data.recipe.output) || null;
      const amt = out && out.amount > 1 ? `<span class="craft-suggest-amt">×${out.amount}</span>` : '';
      const cat = r.category ? `<span class="craft-suggest-cat">${esc(r.category)}</span>` : '';
      return `<button type="button" class="craft-suggest-item" role="option"
                      data-idx="${i}">
                <span class="craft-suggest-name">${esc(r.name)}${amt}</span>${cat}
              </button>`;
    }).join('');
    for (const el of $suggest.querySelectorAll('[data-idx]')) {
      el.addEventListener('click', () => selectRecipe(state.suggestions[Number(el.dataset.idx)]));
    }
    openSuggest();
  }

  function openSuggest() { $suggest.hidden = false; $search.setAttribute('aria-expanded', 'true'); }
  function closeSuggest() { $suggest.hidden = true; $search.setAttribute('aria-expanded', 'false'); }

  function selectRecipe(row) {
    if (!row) return;
    closeSuggest();
    $search.value = row.name;
    state.query = row.name;
    loadTree(row.path);
  }

  // ─── Tree fetch + render ───────────────────────────────────────────
  async function loadTree(path) {
    state.activePath = path;
    updateHash();
    $empty.hidden = true;
    $panel.hidden = false;
    $panel.innerHTML = `<p class="cdx-loading" data-i18n>Building the crafting tree…</p>`;
    rerunI18n();
    try {
      const tree = await fetchJSON(
        `/site/codexes/crafting?branch=${enc(state.branch)}&path=${enc(path)}`);
      state.tree = tree;
      renderPanel(tree);
    } catch (err) {
      state.tree = null;
      $panel.innerHTML = errorHTML(err);
      rerunI18n();
    }
  }

  function renderPanel(tree) {
    const root = tree.root;
    const out = tree.output || root;
    const amt = out.amount || 1;
    const title = amt > 1
      ? t('Craft {n}× {name}').replace('{n}', amt).replace('{name}', out.name)
      : t('Craft {name}').replace('{name}', out.name);

    const best = root.best_cost;
    const craft = root.craft_cost;
    const buy = root.buy_cost;
    const rec = root.recommendation;

    const headline = best == null
      ? `<span class="craft-unknown">${esc(t('Not enough market data'))}</span>`
      : `<span class="craft-total-num">${fmt(best)}</span> <span class="craft-total-unit" data-i18n>flux</span>`;

    let verdict = '';
    if (rec === 'craft') {
      verdict = (buy != null)
        ? t('Cheaper to craft — you save {n} flux vs. buying it outright.')
            .replace('{n}', fmt(Math.max(0, buy - craft)))
        : t('Craft it — no direct market listings to compare against.');
    } else if (rec === 'buy') {
      verdict = (craft != null)
        ? t('Cheaper to buy — crafting would cost {n} flux more.')
            .replace('{n}', fmt(Math.max(0, craft - buy)))
        : t('Buy it — some ingredients have no market price, so a full craft cost is unknown.');
    } else {
      verdict = t('Not enough market data on the ingredients to price this yet.');
    }

    const rows = [];
    rows.push(costLine(t('Craft cost'), root.craftable ? craft : null,
                       root.craft_cost_partial, rec === 'craft'));
    rows.push(costLine(t('Buy from market'), buy, null, rec === 'buy'));

    const warnings = [];
    if (root.unpriced_count > 0) {
      warnings.push(t('{n} ingredient(s) have no market data')
        .replace('{n}', root.unpriced_count));
    }
    if (tree.truncated) {
      warnings.push(t('Tree was very large and got truncated.'));
    }

    $panel.innerHTML = `
      <div class="craft-summary">
        <div class="craft-summary-head">
          <p class="craft-summary-eyebrow">${esc(t('Recommended'))}</p>
          <h2 class="craft-summary-title">${esc(title)}</h2>
        </div>
        <div class="craft-total ${best == null ? 'is-unknown' : 'rec-' + rec}">
          ${headline}
          <span class="craft-verdict-chip rec-${rec}">${esc(recLabel(rec))}</span>
        </div>
        <p class="craft-verdict">${esc(verdict)}</p>
        <div class="craft-costlines">${rows.join('')}</div>
        ${warnings.length
          ? `<ul class="craft-warnings">${warnings.map((w) =>
              `<li><i class="fa-solid fa-triangle-exclamation" aria-hidden="true"></i> ${esc(w)}</li>`).join('')}</ul>`
          : ''}
      </div>
      <div class="craft-tree-wrap">
        <div class="craft-tree-head">
          <h3 data-i18n>Ingredient breakdown</h3>
          <div class="craft-tree-actions">
            <button type="button" class="craft-expand-all" data-i18n>Expand all</button>
            <button type="button" class="craft-collapse-all" data-i18n>Collapse all</button>
          </div>
        </div>
        <div class="craft-tree" id="craft-tree">
          ${renderNode(root, 0)}
        </div>
      </div>`;

    wireTree();
    rerunI18n();
  }

  function costLine(label, value, partial, active) {
    let v;
    if (value != null) v = `${fmt(value)} <span data-i18n>flux</span>`;
    else if (partial != null && partial > 0)
      v = `<span class="craft-partial">≥ ${fmt(partial)}</span> <span data-i18n>flux</span>`;
    else v = `<span class="craft-na" data-i18n>no data</span>`;
    return `<div class="craft-costline${active ? ' is-best' : ''}">
              <span class="craft-costline-label">${esc(label)}</span>
              <span class="craft-costline-value">${v}</span>
            </div>`;
  }

  // Recursive tree node. Craftable nodes with children collapse below depth 2.
  function renderNode(node, depth) {
    const hasKids = node.craftable && node.children && node.children.length;
    const collapsed = hasKids && depth >= 2 ? ' collapsed' : '';
    const qty = `<span class="craft-qty">×${fmt(node.need)}</span>`;

    // Right-hand cost cell.
    let costCell;
    if (node.best_cost != null) {
      costCell = `<span class="craft-node-cost rec-${node.recommendation}">${fmt(node.best_cost)}</span>`;
    } else if (node.craft_cost_partial != null && node.craft_cost_partial > 0) {
      costCell = `<span class="craft-node-cost is-partial">≥ ${fmt(node.craft_cost_partial)}</span>`;
    } else {
      costCell = `<span class="craft-node-cost is-na" data-i18n>no market data</span>`;
    }

    const unit = node.market_price_each != null
      ? `<span class="craft-node-unit">@ ${fmt(node.market_price_each)}/ea</span>`
      : '';
    const chip = hasKids
      ? `<span class="craft-node-tag rec-${node.recommendation}">${esc(recLabel(node.recommendation))}</span>`
      : (node.market_price_each == null
          ? ''
          : `<span class="craft-node-tag rec-buy" data-i18n>market</span>`);

    const toggle = hasKids
      ? `<button type="button" class="craft-toggle" aria-label="Toggle">
           <i class="fa-solid fa-chevron-down" aria-hidden="true"></i>
         </button>`
      : `<span class="craft-toggle-spacer"></span>`;

    const children = hasKids
      ? `<div class="craft-children">${node.children.map((c) => renderNode(c, depth + 1)).join('')}</div>`
      : '';

    return `
      <div class="craft-node${collapsed}" data-depth="${depth}">
        <div class="craft-row">
          <span class="craft-row-main">
            ${toggle}
            <i class="fa-solid ${hasKids ? 'fa-hammer' : 'fa-cube'} craft-row-icon" aria-hidden="true"></i>
            <span class="craft-node-name">${esc(node.name)}</span>
            ${qty}
          </span>
          <span class="craft-row-side">
            ${unit}
            ${chip}
            ${costCell}
          </span>
        </div>
        ${children}
      </div>`;
  }

  function wireTree() {
    const tree = $('craft-tree');
    if (!tree) return;
    for (const btn of tree.querySelectorAll('.craft-toggle')) {
      btn.addEventListener('click', () => {
        btn.closest('.craft-node').classList.toggle('collapsed');
      });
    }
    const ea = $panel.querySelector('.craft-expand-all');
    const ca = $panel.querySelector('.craft-collapse-all');
    if (ea) ea.addEventListener('click', () => setAllCollapsed(false));
    if (ca) ca.addEventListener('click', () => setAllCollapsed(true));
  }

  function setAllCollapsed(collapsed) {
    const tree = $('craft-tree');
    if (!tree) return;
    // Never collapse the root node itself (depth 0) - just its descendants.
    for (const n of tree.querySelectorAll('.craft-node')) {
      if (n.dataset.depth === '0') { n.classList.remove('collapsed'); continue; }
      if (n.querySelector(':scope > .craft-children')) {
        n.classList.toggle('collapsed', collapsed);
      }
    }
  }

  function recLabel(rec) {
    if (rec === 'craft') return t('Craft');
    if (rec === 'buy') return t('Buy');
    return t('Unknown');
  }

  function showError(err) {
    $empty.hidden = true;
    $panel.hidden = false;
    $panel.innerHTML = errorHTML(err);
    rerunI18n();
  }

  // ─── Helpers ───────────────────────────────────────────────────────
  function fmt(n) {
    n = Number(n || 0);
    if (Math.abs(n) >= 1_000_000) return (n / 1_000_000).toFixed(2).replace(/\.?0+$/, '') + 'M';
    if (Math.abs(n) >= 10_000) return Math.round(n).toLocaleString();
    // Keep sub-unit precision for cheap per-each prices.
    return (Number.isInteger(n) ? n : Number(n.toFixed(2))).toLocaleString();
  }

  async function fetchJSON(path) {
    const res = await fetch(path, { headers: { Accept: 'application/json' } });
    if (!res.ok) {
      let msg = `HTTP ${res.status}`;
      try {
        const body = await res.json();
        if (body && body.detail) msg = body.detail;
        else if (body && body.error && body.error.message) msg = body.error.message;
      } catch (_) {}
      throw new Error(msg);
    }
    return res.json();
  }

  function enc(s) { return encodeURIComponent(s); }
  function esc(s) {
    return String(s ?? '').replace(/[&<>"']/g, (c) =>
      ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  }
  function errMsg(err) { return (err && err.message) || String(err); }
  function errorHTML(err) {
    return `<p class="cdx-error">${esc(t('Failed to load'))}: ${esc(errMsg(err))}</p>`;
  }
  function t(s) { return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s; }
  function rerunI18n() { if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh(); }

  function debounce(fn, ms) {
    let h;
    return function (...a) { clearTimeout(h); h = setTimeout(() => fn.apply(this, a), ms); };
  }
})();
