/* ═══════════════════════════════════════════════════════════════════════
   /codexes - page logic (Beta)
   ───────────────────────────────────────────────────────────────────────
   Filter bar (branch + type tabs + search + category + tradable + sort) over
   a paginated card grid; clicking a card opens a detail modal with the full
   decoded bonuses. Backed by the same-origin /site/codexes/* JSON proxies; no
   token required. Search rows already carry the full `data` blob, so the modal
   renders from memory (no extra fetch).

   URL hash: #type=<type>&q=<query> for deep-linking / refresh-stability.
   ═══════════════════════════════════════════════════════════════════════ */

(function () {
  'use strict';

  const { esc, fetchJSON, apiUrl, debounce } = window.BTTUtil;

  const PAGE_SIZE = 60;

  // codex_type -> display labels (plural for tabs, singular for the card chip).
  const TYPE_TABS = {
    ally: 'Allies', mount: 'Mounts', dragon: 'Dragons', memento: 'Mementos',
    style: 'Styles', recipe: 'Recipes', item: 'Items', fish: 'Fish', badge: 'Badges',
  };
  const TYPE_CHIP = {
    ally: 'Ally', mount: 'Mount', dragon: 'Dragon', memento: 'Memento',
    style: 'Style', recipe: 'Recipe', item: 'Item', fish: 'Fish', badge: 'Badge',
  };
  // Stable tab order (types not present are dropped after the /types fetch).
  const TYPE_ORDER = ['ally', 'mount', 'dragon', 'badge', 'memento', 'style', 'fish', 'recipe', 'item'];

  // `$Stat_…` key -> human label (mirrors the in-game stat names).
  const STAT_LABELS = {
    $Stat_PhysicalDamage: 'Physical Damage', $Stat_SpellDamage: 'Magic Damage',
    $Stat_MaxHealth: 'Maximum Health', $Stat_MaxEnergy: 'Maximum Energy',
    $Stat_HealthRegen_controller: 'Health Regen', $Stat_EnergyRegen_controller: 'Energy Regen',
    $Stat_Stability: 'Stability', $Stat_CriticalHitChance: 'Critical Hit',
    $Stat_MovementSpeed: 'Movement Speed', $Stat_Jump: 'Jump', $Stat_MagicFind: 'Magic Find',
    $Stat_Mining: 'Lasermancy', $Stat_AttackSpeed: 'Attack Speed', $Stat_MaxFlasks: 'Flask Capacity',
    $Stat_CraftingSpeed: 'Crafting Speed', $Stat_CooldownSpeed: 'Cooldown Speed',
    $Stat_Acceleration: 'Acceleration', $Stat_TurningRate: 'Turning Rate',
    $Stat_ExperienceBoost: 'Experience Boost', $Stat_CriticalHitDamage: 'Critical Damage',
    $Stat_CriticalHitDamageBonus: 'Critical Damage Bonus',
    $Stat_Glide: 'Glide', $Stat_Light: 'Light', $Stat_MaxExploration: 'Maximum Exploration',
  };

  const state = {
    branch: 'live-us',
    type: null,          // null = all types
    search: '',
    category: '',
    tradable: '',        // '' | 'true' | 'false'
    sort: 'name',
    offset: 0,
    items: [],           // accumulated rows (each carries its own full `data`)
    total: 0,
    loading: false,
  };

  const $ = (id) => document.getElementById(id);
  const $tabs = $('cdx-tabs');
  const $grid = $('cdx-grid');
  const $meta = $('cdx-results-meta');
  const $foot = $('cdx-foot');
  const $loadMore = $('cdx-load-more');
  const $search = $('cdx-search');
  const $category = $('cdx-category');
  const $tradable = $('cdx-tradable');
  const $sort = $('cdx-sort');
  const $modal = $('cdx-modal');
  const $modalBody = $('cdx-modal-body');

  // ─── Boot ──────────────────────────────────────────────────────────
  init().catch((err) => {
    console.error('[codexes] boot failed', err);
    $grid.innerHTML = errorHTML(err);
    rerunI18n();
  });

  async function init() {
    applyHash();
    wireEvents();
    await loadTypes();
    await loadCategories();
    await loadPage(true);
  }

  // ─── Type tabs ─────────────────────────────────────────────────────
  async function loadTypes() {
    let rows = [];
    try {
      const data = await fetchJSON('/site/codexes/types?branch=' + enc(state.branch));
      rows = data.items || [];
    } catch (_) { /* empty branch (e.g. PTS not indexed) -> just "All" */ }

    const counts = {};
    let all = 0;
    for (const r of rows) { counts[r.type] = r.count; all += r.count; }

    const present = TYPE_ORDER.filter((tp) => counts[tp]);
    const tabs = [`
      <button type="button" class="cdx-tab${state.type === null ? ' active' : ''}" data-type="">
        <span data-i18n>All</span><span class="cdx-tab-count">${formatInt(all)}</span>
      </button>`];
    for (const tp of present) {
      tabs.push(`
        <button type="button" class="cdx-tab${state.type === tp ? ' active' : ''}" data-type="${esc(tp)}">
          <span data-i18n>${esc(TYPE_TABS[tp])}</span><span class="cdx-tab-count">${formatInt(counts[tp])}</span>
        </button>`);
    }
    // A selected type that vanished on a branch switch falls back to All.
    if (state.type && !counts[state.type]) state.type = null;
    $tabs.innerHTML = tabs.join('');
    rerunI18n();
  }

  // ─── Category dropdown (depends on the active type) ────────────────
  async function loadCategories() {
    let rows = [];
    try {
      const qs = '?branch=' + enc(state.branch) + (state.type ? '&type=' + enc(state.type) : '');
      const data = await fetchJSON('/site/codexes/categories' + qs);
      rows = data.items || [];
    } catch (_) { /* leave just "All categories" */ }

    const keep = state.category;
    const opts = [`<option value="" data-i18n>All categories</option>`];
    for (const r of rows) {
      opts.push(`<option value="${esc(r.category)}">${esc(r.category)} (${formatInt(r.count)})</option>`);
    }
    $category.innerHTML = opts.join('');
    // Preserve the current selection if it still exists in the new type.
    $category.value = rows.some((r) => r.category === keep) ? keep : '';
    state.category = $category.value;
    rerunI18n();
  }

  // ─── Results grid ──────────────────────────────────────────────────
  function buildQuery() {
    const p = new URLSearchParams();
    p.set('branch', state.branch);
    if (state.type) p.set('type', state.type);
    if (state.search) p.set('q', state.search);
    if (state.category) p.set('category', state.category);
    if (state.tradable !== '') p.set('tradable', state.tradable);
    p.set('sort', state.sort);
    p.set('limit', String(PAGE_SIZE));
    p.set('offset', String(state.offset));
    return p.toString();
  }

  async function loadPage(reset) {
    if (state.loading) return;
    if (reset) {
      state.offset = 0;
      state.items = [];
      $grid.innerHTML = `<p class="cdx-loading" data-i18n>${esc(t('Loading codexes…'))}</p>`;
    } else if ($loadMore) {
      $loadMore.disabled = true;
      $loadMore.textContent = t('Loading…');
    }
    state.loading = true;
    try {
      const data = await fetchJSON('/site/codexes/search?' + buildQuery());
      state.total = data.total || 0;
      state.items = reset ? (data.items || []) : state.items.concat(data.items || []);
      state.offset = state.items.length;
    } catch (err) {
      state.loading = false;
      $grid.innerHTML = errorHTML(err);
      rerunI18n();
      return;
    }
    state.loading = false;
    renderGrid();
  }

  function renderGrid() {
    if (!state.items.length) {
      $grid.innerHTML = `<p class="cdx-empty" data-i18n>No codexes match these filters.</p>`;
      $meta.textContent = '';
      $foot.hidden = true;
      rerunI18n();
      return;
    }

    $grid.innerHTML = state.items.map((e, i) => cardHTML(e, i)).join('');

    $meta.textContent = t('Showing {n} of {total}')
      .replace('{n}', formatInt(state.items.length))
      .replace('{total}', formatInt(state.total));

    const more = state.items.length < state.total;
    $foot.hidden = !more;
    if (more && $loadMore) {
      $loadMore.disabled = false;
      $loadMore.textContent = t('Load more');
    }
    rerunI18n();
  }

  function cardHTML(e, i) {
    const badges = [];
    if (e.mastery != null) badges.push(badge('mastery', t('Mastery'), e.mastery, 'fa-star'));
    if (e.mastery_geode != null) badges.push(badge('geode', t('Geode'), e.mastery_geode, 'fa-gem'));
    if (e.power_rank != null) badges.push(badge('pr', t('PR'), e.power_rank, 'fa-bolt'));
    const trade = e.tradable === true
      ? `<span class="cdx-trade cdx-trade-yes" title="${esc(t('Tradable'))}"><i class="fa-solid fa-right-left" aria-hidden="true"></i></span>`
      : e.tradable === false
        ? `<span class="cdx-trade cdx-trade-no" title="${esc(t('Bound'))}"><i class="fa-solid fa-lock" aria-hidden="true"></i></span>`
        : '';
    return `
      <button type="button" class="cdx-card" data-idx="${i}">
        <span class="cdx-card-top">
          <span class="cdx-chip cdx-type-${esc(e.type)}">${esc(TYPE_CHIP[e.type] || e.type)}</span>
          ${trade}
        </span>
        ${thumbHTML(e)}
        <span class="cdx-card-name" title="${esc(e.name)}">${esc(e.name)}</span>
        ${e.category ? `<span class="cdx-card-cat">${esc(e.category)}</span>` : ''}
        ${badges.length ? `<span class="cdx-card-badges">${badges.join('')}</span>` : ''}
      </button>`;
  }

  // Creature types are built from parts on a skeleton, so the single blueprint on the
  // row is a torso or a jaw (and the game's own _ui icon is a small stand-in). Sending
  // the prefab path lets the renderer assemble the whole animal; it falls back to the
  // blueprint by itself whenever it can't supply every part.
  const RIGGED_TYPES = { mount: 1, dragon: 1 };

  // Blueprint render thumbnail (lazy so only on-screen cards trigger a render);
  // a missing/unrenderable blueprint 404s and onerror drops the image cleanly.
  function thumbHTML(e, size) {
    if (!e.blueprint) return '';
    const src = apiUrl('/site/codexes/render?blueprint=' + enc(e.blueprint)
      + '&branch=' + enc(state.branch) + (size ? '&dim=' + size : '')
      + (RIGGED_TYPES[e.codex_type] && e.path ? '&prefab=' + enc(e.path) : ''));
    return `<span class="cdx-card-img"><img loading="lazy" decoding="async" alt=""
      src="${esc(src)}" onerror="this.closest('.cdx-card-img').remove()"></span>`;
  }

  function badge(kind, label, value, icon) {
    return `<span class="cdx-badge cdx-badge-${kind}" title="${esc(label)}">
      <i class="fa-solid ${icon}" aria-hidden="true"></i>${esc(formatInt(value))}</span>`;
  }

  // ─── Detail modal ──────────────────────────────────────────────────
  let modalRelease = null;
  function openModal(e) {
    $modalBody.innerHTML = modalHTML(e);
    $modal.hidden = false;
    document.body.classList.add('cdx-modal-open');
    rerunI18n();
    loadRelated(e);
    const card = $modal.querySelector('.cdx-modal-card');
    if (card && window.BTTUtil && window.BTTUtil.trapFocus) {
      modalRelease = window.BTTUtil.trapFocus(card, { onEscape: closeModal });
    } else {
      const close = $('cdx-modal-close');
      if (close) close.focus();
    }
  }
  function closeModal() {
    if (modalRelease) { modalRelease(); modalRelease = null; }
    $modal.hidden = true;
    document.body.classList.remove('cdx-modal-open');
  }

  function modalHTML(e) {
    const data = e.data || {};
    const parts = [];

    parts.push(`
      <header class="cdx-modal-head">
        <span class="cdx-chip cdx-type-${esc(e.type)}">${esc(TYPE_CHIP[e.type] || e.type)}</span>
        <h2 class="cdx-modal-title" id="cdx-modal-title">${esc(e.name)}</h2>
        ${e.category ? `<p class="cdx-modal-cat">${esc(e.category)}</p>` : ''}
      </header>`);

    if (e.blueprint) {
      parts.push(`<div class="cdx-modal-img">${thumbHTML(e, 256)}</div>`);
    }

    if (e.description) parts.push(`<p class="cdx-modal-desc">${esc(e.description)}</p>`);

    // Headline stats (mastery / geode / power rank / tradability)
    const facts = [];
    if (e.mastery != null) facts.push(fact(t('Mastery'), formatInt(e.mastery), 'fa-star'));
    if (e.mastery_geode != null) facts.push(fact(t('Geode mastery'), formatInt(e.mastery_geode), 'fa-gem'));
    if (e.power_rank != null) facts.push(fact(t('Power rank'), formatInt(e.power_rank), 'fa-bolt'));
    if (e.tradable != null) facts.push(fact(t('Tradable'), e.tradable ? t('Yes') : t('No'),
      e.tradable ? 'fa-right-left' : 'fa-lock'));
    if (facts.length) parts.push(`<div class="cdx-facts">${facts.join('')}</div>`);

    // Stat bonuses (stat_name / slot_name resolved server-side from the locale tables)
    const stats = (data.stats || []).filter((s) => s.stat);
    if (stats.length) {
      const rows = stats.map((s) => `
        <li class="cdx-stat">
          <span class="cdx-stat-val ${Number(s.value) < 0 ? 'neg' : 'pos'}">${esc(statValue(s))}</span>
          <span class="cdx-stat-name">${esc(statName(s))}</span>
          ${s.slot_name ? `<span class="cdx-stat-slot">${esc(s.slot_name)}</span>` : ''}
        </li>`).join('');
      parts.push(section(t('Stat bonuses'), `<ul class="cdx-stats">${rows}</ul>`));
    }

    // Visible ability bonuses (name + resolved description; hidden refs are filtered)
    const abilities = (data.abilities || []).filter((a) => a && !a.hidden);
    if (abilities.length) {
      // No name fallback. Most abilities carry no display name in the game data, and
      // prettifying the ref produced internal ids wearing title case ("Enemydeath
      // Damagebuff"). When there is no name, the description stands alone.
      const rows = abilities.map((a) => {
        const desc = a.description ? `<span class="cdx-ability-desc">${esc(a.description)}</span>` : '';
        const name = a.name ? `<span class="cdx-ability-name">${esc(a.name)}</span>` : '';
        if (!name && !desc) return '';
        return `<li class="cdx-ability"><i class="fa-solid fa-wand-sparkles" aria-hidden="true"></i>
          <span class="cdx-ability-main">${name}${desc}</span></li>`;
      }).filter(Boolean).join('');
      parts.push(section(t('Ability bonuses'), `<ul class="cdx-abilities">${rows}</ul>`));
    }

    // Recipe (output + ingredients + requirements)
    const rec = data.recipe;
    if (rec) {
      const lines = [];
      if (rec.output) {
        lines.push(`<li class="cdx-craft cdx-craft-out">
          <span class="cdx-craft-amt">${esc(formatInt(rec.output.amount || 1))}×</span>
          <span class="cdx-craft-name">${esc(rec.output.name || prettyAbility(rec.output.path))}</span>
          <span class="cdx-craft-tag" data-i18n>Crafts</span></li>`);
      }
      for (const ing of (rec.ingredients || [])) {
        lines.push(`<li class="cdx-craft">
          <span class="cdx-craft-amt">${esc(formatInt(ing.amount))}×</span>
          <span class="cdx-craft-name">${esc(ing.name || prettyAbility(ing.path))}</span></li>`);
      }
      if (lines.length) parts.push(section(t('Recipe'), `<ul class="cdx-craft-list">${lines.join('')}</ul>`));
      if ((rec.requirements || []).length) {
        const reqs = rec.requirements.map((r) => `<li class="cdx-req">${esc(r)}</li>`).join('');
        parts.push(section(t('Requirements'), `<ul class="cdx-reqs">${reqs}</ul>`));
      }
      // Where it's crafted: the bench/profession prefabs that reference this recipe.
      const provs = rec.providers || [];
      if (provs.length) {
        const benchName = (p) => {
          const stem = String(p.provider || '').split('/').pop()
            .replace(/_(interactive|interactable|hub)$/g, '').replace(/_/g, ' ').trim();
          return stem.replace(/\b\w/g, (ch) => ch.toUpperCase()) || p.provider;
        };
        const rows = provs.map((p) => `<li class="cdx-req">${esc(benchName(p))}${
          p.lane ? ` <span class="cdx-stat-slot">${esc(p.lane)}</span>` : ''}</li>`).join('');
        parts.push(section(t('Craftable at'), `<ul class="cdx-reqs">${rows}</ul>`));
      }
      // Source-only recipes (not part of the current catalogue lane) get a small note.
      if (rec.in_catalogue === false) {
        parts.push(section(t('Catalog'),
          `<p class="cdx-modal-cat">${esc(t('Not in the current recipe catalog'))}</p>`));
      }
    }

    // Style identity: the equipment id it restyles (preserved verbatim, aliases intact)
    const sty = data.style;
    if (sty && sty.equipment_ref) {
      parts.push(section(t('Style'),
        `<div class="cdx-ref"><span class="cdx-ref-row"><span class="cdx-ref-k" data-i18n>Restyles</span>` +
        `<code>${esc(sty.equipment_ref)}</code></span></div>`));
    }

    // Geode companion upgrade-tree levels
    const gc = data.geode_companion;
    if (gc && (gc.levels || []).length) {
      const lvls = gc.levels.map((lv) => {
        const bits = [];
        for (const s of (lv.stats || [])) {
          bits.push(`${esc(statValue(s))} ${esc(statName(s))}`);
        }
        for (const ab of (lv.abilities || [])) {
          // Same rule as the ability list: show what the game names, never a
          // prettified ref standing in for a name it doesn't have.
          const label = (ab && (ab.description || ab.name)) || '';
          if (label) bits.push(esc(label));
        }
        return `<li class="cdx-level"><span class="cdx-level-n">${t('Lvl {n}').replace('{n}', esc(lv.level))}</span>
          <span class="cdx-level-bits">${bits.join(' · ') || '—'}</span></li>`;
      }).join('');
      const head = gc.rarity ? `${t('Geode companion')} · ${esc(gc.rarity)}` : t('Geode companion');
      parts.push(section(head, `<ul class="cdx-levels">${lvls}</ul>`));
    }

    // Source reference (always available - ties the row to its game file)
    const ref = [`<span class="cdx-ref-row"><span class="cdx-ref-k" data-i18n>Source</span><code>${esc(e.path)}</code></span>`];
    if (e.blueprint) ref.push(`<span class="cdx-ref-row"><span class="cdx-ref-k" data-i18n>Blueprint</span><code>${esc(e.blueprint)}</code></span>`);
    parts.push(section(t('Reference'), `<div class="cdx-ref">${ref.join('')}</div>`));

    return parts.join('');
  }

  // ─── Related data (links, badge ranks) ──────────────────────────────
  // Fetched after the modal paints rather than inlined into the search row: it is a
  // per-entry join, and most entries have none of it. A failure here leaves the modal
  // exactly as it was - the panel is additive, never load-bearing.
  let relatedToken = 0;
  async function loadRelated(entry) {
    const token = ++relatedToken;
    let data;
    try {
      data = await fetchJSON('/site/codexes/related?branch=' + enc(state.branch)
        + '&path=' + enc(entry.path));
    } catch (_err) {
      return;
    }
    if (token !== relatedToken || $modal.hidden) return;   // modal moved on
    const html = relatedHTML(data);
    if (!html) return;
    const anchor = $modalBody.querySelector('.cdx-sec:last-of-type');
    if (anchor) anchor.insertAdjacentHTML('beforebegin', html);
    else $modalBody.insertAdjacentHTML('beforeend', html);
    rerunI18n();
  }

  function linkRows(items) {
    return items.map((r) => {
      const qty = (r.qty !== null && r.qty !== undefined && r.qty !== 1)
        ? `<span class="cdx-craft-amt">${esc(formatInt(r.qty))}×</span>` : '';
      const name = r.name || String(r.path || '').split('/').pop().replace(/\.binfab$/, '');
      const tag = (r.data && r.data.category)
        ? `<span class="cdx-stat-slot">${esc(String(r.data.category).replace(/^\$|_name$/g, ''))}</span>` : '';
      return `<li class="cdx-craft">${qty}<span class="cdx-craft-name">${esc(name)}</span>${tag}</li>`;
    }).join('');
  }

  function relatedHTML(d) {
    const parts = [];
    // The reverse lookups first - they are the ones you cannot get any other way.
    if ((d.made_by || []).length) {
      parts.push(section(t('Made by'), `<ul class="cdx-craft-list">${linkRows(d.made_by)}</ul>`));
    }
    if ((d.used_in || []).length) {
      parts.push(section(t('Used in'), `<ul class="cdx-craft-list">${linkRows(d.used_in)}</ul>`));
    }
    if ((d.unlocked_by || []).length) {
      parts.push(section(t('Unlocked by'), `<ul class="cdx-craft-list">${linkRows(d.unlocked_by)}</ul>`));
    }
    if ((d.unlocks || []).length) {
      parts.push(section(t('Unlocks'), `<ul class="cdx-craft-list">${linkRows(d.unlocks)}</ul>`));
    }
    if ((d.upgrade_cost_of || []).length) {
      parts.push(section(t('Spent on'), `<ul class="cdx-craft-list">${linkRows(d.upgrade_cost_of)}</ul>`));
    }
    if ((d.requirements || []).length) {
      const rows = d.requirements.map((r) => {
        const amt = (r.amount !== null && r.amount !== undefined && r.amount !== 0)
          ? `<span class="cdx-craft-amt">${esc(formatInt(r.amount))}</span>` : '';
        return `<li class="cdx-craft">
          <span class="cdx-level-n">${esc(r.rank_name || ('#' + r.rank))}</span>
          <span class="cdx-craft-name">${esc(r.label || r.completion_kind)}</span>${amt}</li>`;
      }).join('');
      parts.push(section(t('Ranks'), `<ul class="cdx-craft-list">${rows}</ul>`));
    }
    return parts.join('');
  }

  function section(title, inner) {
    return `<section class="cdx-sec"><h3 class="cdx-sec-title">${esc(title)}</h3>${inner}</section>`;
  }
  function fact(label, value, icon) {
    return `<div class="cdx-fact"><i class="fa-solid ${icon}" aria-hidden="true"></i>
      <span class="cdx-fact-v">${esc(value)}</span><span class="cdx-fact-l">${esc(label)}</span></div>`;
  }

  // ─── Value / name formatting ───────────────────────────────────────
  function statValue(s) {
    const v = Number(s.value || 0);
    const num = formatNum(v);
    const signed = v > 0 ? '+' + num : num;
    return s.is_percent ? signed + '%' : signed;
  }
  // Prefer the server-resolved name (real in-game / locale string); fall back to
  // the built-in label, then a derived one.
  function statName(s) {
    return s.stat_name || STAT_LABELS[s.stat] || prettyStat(s.stat);
  }
  function prettyStat(key) {
    return String(key || '').replace(/^\$Stat_/, '').replace(/_controller$/, '')
      .replace(/([a-z])([A-Z])/g, '$1 $2');
  }
  function prettyAbility(ref) {
    const seg = String(ref || '').split('/').pop() || ref;
    return seg.replace(/_/g, ' ').replace(/\b\w/g, (c) => c.toUpperCase());
  }
  function formatNum(n) {
    n = Number(n || 0);
    if (Number.isInteger(n)) return n.toLocaleString();
    return (Math.round(n * 100) / 100).toLocaleString();
  }
  function formatInt(n) { return Number(n || 0).toLocaleString(); }

  // ─── Events ────────────────────────────────────────────────────────
  function wireEvents() {
    $tabs.addEventListener('click', (ev) => {
      const btn = ev.target.closest('.cdx-tab');
      if (!btn) return;
      const tp = btn.getAttribute('data-type') || null;
      if (tp === state.type) return;
      state.type = tp;
      state.category = '';
      setActive($tabs, btn);
      updateHash();
      loadCategories().then(() => loadPage(true));
    });

    document.querySelectorAll('.cdx-branch-btn').forEach((btn) => {
      btn.addEventListener('click', () => {
        const b = btn.getAttribute('data-branch');
        if (b === state.branch) return;
        state.branch = b;
        document.querySelectorAll('.cdx-branch-btn').forEach((x) =>
          x.classList.toggle('active', x === btn));
        // a fresh branch can have a different type/category set
        loadTypes().then(loadCategories).then(() => loadPage(true));
      });
    });

    $search.addEventListener('input', debounce(() => {
      state.search = $search.value.trim();
      updateHash();
      loadPage(true);
    }, 280));

    $category.addEventListener('change', () => { state.category = $category.value; loadPage(true); });
    $tradable.addEventListener('change', () => { state.tradable = $tradable.value; loadPage(true); });
    $sort.addEventListener('change', () => { state.sort = $sort.value; loadPage(true); });

    if ($loadMore) $loadMore.addEventListener('click', () => loadPage(false));

    $grid.addEventListener('click', (ev) => {
      const card = ev.target.closest('.cdx-card');
      if (!card) return;
      const e = state.items[Number(card.getAttribute('data-idx'))];
      if (e) openModal(e);
    });

    $('cdx-modal-close').addEventListener('click', closeModal);
    $('cdx-modal-backdrop').addEventListener('click', closeModal);
    document.addEventListener('keydown', (ev) => {
      if (ev.key === 'Escape' && !$modal.hidden) closeModal();
    });

    // Re-render on language change so dynamic labels (badges, facts) re-translate.
    document.addEventListener('btt-lang-changed', () => { if (state.items.length) renderGrid(); });
  }

  function setActive(container, btn) {
    container.querySelectorAll('.cdx-tab').forEach((x) => x.classList.toggle('active', x === btn));
  }

  // ─── Hash deep-linking ─────────────────────────────────────────────
  function applyHash() {
    const raw = location.hash.replace(/^#/, '');
    if (!raw) return;
    const p = new URLSearchParams(raw);
    const tp = p.get('type');
    if (tp && TYPE_TABS[tp]) state.type = tp;
    if (p.get('q')) { state.search = p.get('q'); $search.value = state.search; }
  }
  function updateHash() {
    const p = new URLSearchParams();
    if (state.type) p.set('type', state.type);
    if (state.search) p.set('q', state.search);
    const s = p.toString();
    history.replaceState(null, '', s ? '#' + s : location.pathname);
  }

  // ─── Fetch + util ──────────────────────────────────────────────────
  function enc(s) { return encodeURIComponent(s); }

  function errorHTML(err) {
    const msg = (err && err.message) || String(err);
    return `<p class="cdx-error">${esc(t('Failed to load'))}: ${esc(msg)}</p>`;
  }

  function t(s) { return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s; }
  function rerunI18n() { if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh(); }

})();
