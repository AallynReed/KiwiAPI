/* =========================================================================
   Gem Builds - rank the top gem proc layouts for a class by damage.
   Vanilla-JS re-implementation of Better Trove Tools' Vue gem builds page. The
   optimization runs server-side: the page POSTs a build config to the
   same-origin /site/gems/builds/* proxies (a token-free mirror of the
   /v1/gems/builds/* optimizer). CSP-clean: no eval, no inline handlers.
   ========================================================================= */
(function () {
  "use strict";
  const toast = window.BTTToast.show;
  const { h } = window.BTTDom;

  const t = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);
  const PER_PAGE = 25;
  const STORAGE_KEY = "troveapi.gemBuilds.v1";

  const num = (v) => (Number(v) || 0).toLocaleString();
  const round = (v) => Math.round(Number(v) || 0).toLocaleString();

  // Class display name -> icon token (Trove's internal class name, which is what
  // /static/class-icons/*.png are keyed by - not the qualified_name). Same tokens
  // the /classes page uses. Unknown names fall back to a hidden icon.
  const CLASS_TECH = {
    "Bard": "bard", "Boomeranger": "adventurer", "Candy Barbarian": "candybarbarian",
    "Chloromancer": "chloromancer", "Dino Tamer": "dinotamer", "Dracolyte": "dracolyte",
    "Fae Trickster": "faetrickster", "Gunslinger": "gunslinger", "Ice Sage": "icemage",
    "Knight": "knight", "Lunar Lancer": "lunarlancer", "Neon Ninja": "neonninja",
    "Pirate Captain": "piratelord", "Revenant": "spirittank", "Shadow Hunter": "shadowhunter",
    "Solarion": "solarion", "Tomb Raiser": "tombraiser", "Vanguardian": "crimefighter",
  };
  const classIcon = (name) => `/static/class-icons/${CLASS_TECH[name] || ""}.png`;

  // ── State ────────────────────────────────────────────────────────────────
  let options = null;
  const config = {
    build_type: "Light", character: "Bard", subclass: "Boomeranger",
    food: "", ally: "boot_clown", critical_damage_count: 3, no_face: false,
    light: 0, subclass_active: false, litany: false, berserker_battler: false,
    star_chart: "",
  };
  let builds = [];
  let page = 0;
  let calculating = false;
  let starChartInfo = null;   // {paths_count, stats} | {error:true} | null
  let advancedOpen = false;

  let elConfig, elResults;
  let scInput = null;   // the star-chart code field, kept in sync by the editor

  function saveConfig() {
    try { localStorage.setItem(STORAGE_KEY, JSON.stringify(config)); } catch (e) { /* non-fatal */ }
  }
  function loadConfig() {
    try {
      const d = JSON.parse(localStorage.getItem(STORAGE_KEY) || "null");
      if (d && typeof d === "object") Object.assign(config, d);
    } catch (e) { /* ignore */ }
  }

  // ── API ──────────────────────────────────────────────────────────────────
  async function apiGet(url) {
    const r = await fetch(url, { headers: { Accept: "application/json" } });
    if (!r.ok) throw new Error("HTTP " + r.status);
    return r.json();
  }
  async function apiPost(url, body) {
    const r = await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json", Accept: "application/json" },
      body: JSON.stringify(body),
    });
    const data = await r.json().catch(() => null);
    if (!r.ok) throw new Error((data && (data.detail || data.message)) || ("HTTP " + r.status));
    return data;
  }

  // ── Config form ──────────────────────────────────────────────────────────
  function selectRow(label, value, entries, onChange, opts) {
    // entries: [{value, label}]
    // Stable key so focus can be returned to this control if the panel is
    // rebuilt underneath it - see onConfigChange.
    const sel = h("select", { class: "gb-select", "data-field": label, onChange: (e) => onChange(e.target.value) });
    entries.forEach((o) => {
      const opt = h("option", { value: o.value }, t(o.label));
      if (String(o.value) === String(value)) opt.selected = true;
      sel.appendChild(opt);
    });
    return h("label", { class: "gb-field" + ((opts && opts.cls) ? " " + opts.cls : "") },
      h("span", { class: "gb-field-label" }, t(label)), sel);
  }
  function classSelectRow(label, value, entries, onChange) {
    const img = h("img", { class: "gb-class-icon", src: classIcon(value), alt: "" });
    img.addEventListener("error", () => { img.style.visibility = "hidden"; });
    const sel = h("select", { class: "gb-select", "data-field": label, onChange: (e) => { img.src = classIcon(e.target.value); img.style.visibility = "visible"; onChange(e.target.value); } });
    entries.forEach((o) => {
      const opt = h("option", { value: o.value }, t(o.label));
      if (String(o.value) === String(value)) opt.selected = true;
      sel.appendChild(opt);
    });
    return h("label", { class: "gb-field" },
      h("span", { class: "gb-field-label" }, t(label)),
      h("div", { class: "gb-select-with-icon" }, img, sel));
  }
  function toggleRow(label, value, onChange, title) {
    const c = h("input", { type: "checkbox" });
    c.checked = value;
    c.addEventListener("change", (e) => onChange(e.target.checked));
    return h("label", { class: "gb-toggle", title: title ? t(title) : null },
      c, h("span", { class: "gb-toggle-mark" }), h("span", {}, t(label)));
  }

  function renderConfig() {
    elConfig.textContent = "";
    const classEntries = options.character.map((c) => ({ value: c, label: c }));
    const buildTypeEntries = options.build_type.map((b) => ({ value: b, label: b }));
    const foodEntries = [{ value: "", label: "None" }].concat(options.food.map((f) => ({ value: f.key, label: f.label })));
    const allyEntries = options.ally.map((a) => ({ value: a.key, label: a.label }));

    // Character
    const charSection = h("div", { class: "gb-section" },
      h("h4", {}, h("i", { class: "fa-solid fa-user-astronaut" }), " " + t("Character")),
      h("div", { class: "gb-row-2" },
        classSelectRow("Class", config.character, classEntries, (v) => { config.character = v; onConfigChange(); }),
        classSelectRow("Subclass", config.subclass, classEntries, (v) => { config.subclass = v; onConfigChange(); })));
    elConfig.appendChild(charSection);

    // Gear / goal
    const gearSection = h("div", { class: "gb-section" },
      h("h4", {}, h("i", { class: "fa-solid fa-shield-halved" }), " " + t("Build & gear")),
      h("div", { class: "gb-row-2" },
        selectRow("Build goal", config.build_type, buildTypeEntries, (v) => { config.build_type = v; onConfigChange(true); }),
        selectRow("Food", config.food, foodEntries, (v) => { config.food = v; onConfigChange(); })),
      selectRow("Ally", config.ally, allyEntries, (v) => { config.ally = v; onConfigChange(); }));

    // Farm-only light target
    if (config.build_type === "Farm") {
      const li = h("input", { class: "gb-input", type: "number", min: 0, value: config.light });
      li.addEventListener("input", (e) => { config.light = parseInt(e.target.value) || 0; saveConfig(); });
      gearSection.appendChild(h("label", { class: "gb-field" },
        h("span", { class: "gb-field-label" }, t("Current base light"),
          h("span", { class: "gb-hint", title: t("Only used for Farm builds - the optimizer targets layouts near this base light value.") }, " ", h("i", { class: "fa-solid fa-circle-info" }))),
        li));
    }

    // Crit damage rolls slider (not for Health)
    if (config.build_type !== "Health") {
      const slider = h("input", { class: "gb-slider", type: "range", min: 0, max: 3, step: 1, value: config.critical_damage_count });
      const val = h("span", { class: "gb-slider-val" }, config.critical_damage_count);
      slider.addEventListener("input", (e) => { config.critical_damage_count = Number(e.target.value); val.textContent = e.target.value; saveConfig(); });
      slider.addEventListener("change", () => calculate());
      gearSection.appendChild(h("label", { class: "gb-field" },
        h("span", { class: "gb-field-label" }, t("Gear crit-damage rolls"), " ", val),
        slider));
      gearSection.appendChild(toggleRow("Face slot has no damage stat", config.no_face, (v) => { config.no_face = v; onConfigChange(); }, "Simulates a face slot with no damage stat."));
    }
    elConfig.appendChild(gearSection);

    // Star chart — paste a code, or build one right here in the editor.
    const scSection = h("div", { class: "gb-section" },
      h("h4", {}, h("i", { class: "fa-solid fa-chart-network" }), " " + t("Star Chart")));
    scInput = h("input", { class: "gb-input", type: "text", value: config.star_chart, placeholder: t("Paste a star-chart build code (e.g. SC:...)") });
    scInput.addEventListener("input", (e) => { config.star_chart = e.target.value.trim(); saveConfig(); scheduleStarChart(); });
    const scEdit = h("button", {
      class: "gb-sc-edit", type: "button",
      title: t("Build your star chart without leaving this page"),
      onClick: openStarChartEditor,
    }, h("i", { class: "fa-solid fa-star" }), " ", h("span", {}, t("Edit")));
    scSection.appendChild(h("div", { class: "gb-sc-row" },
      h("label", { class: "gb-field gb-sc-field" },
        h("span", { class: "gb-field-label" }, t("Star chart build code")), scInput),
      scEdit));
    scSection.appendChild(h("div", { class: "gb-sc-summary", id: "gb-sc-summary" }));
    elConfig.appendChild(scSection);

    // Advanced toggles
    const advSection = h("div", { class: "gb-section gb-advanced" + (advancedOpen ? " open" : "") });
    const advBtn = h("button", { class: "gb-collapse-btn", onClick: () => { advancedOpen = !advancedOpen; renderConfig(); } },
      h("h4", {}, h("i", { class: "fa-solid fa-sliders" }), " " + t("Advanced buff toggles")),
      h("i", { class: "fa-solid fa-chevron-down gb-caret" }));
    advSection.appendChild(advBtn);
    if (advancedOpen) {
      const grid = h("div", { class: "gb-toggle-grid" },
        toggleRow("Berserker Battler", config.berserker_battler, (v) => { config.berserker_battler = v; onConfigChange(); }, "Treats Berserker Battler as active (adds its light)."),
        toggleRow("Enlightened / Litany", config.litany, (v) => { config.litany = v; onConfigChange(); }, "Adds the light from the Enlightened (Litany) buff."),
        toggleRow("Subclass active", config.subclass_active, (v) => { config.subclass_active = v; onConfigChange(); }, "Includes the passive stats from your chosen subclass."));
      advSection.appendChild(grid);
    }
    elConfig.appendChild(advSection);

    renderStarChartSummary();
  }

  function onConfigChange(rerender) {
    saveConfig();
    if (rerender) {
      // Changing the build goal adds/removes the light field, so the panel has
      // to be rebuilt - which replaces the very <select> that triggered it. In
      // Chrome each arrow-key press on a closed select fires `change`, so
      // without this a keyboard user loses focus on the first press and cannot
      // keep arrowing. Restore focus to the same field after the rebuild.
      const active = document.activeElement;
      const field = active && active.dataset ? active.dataset.field : null;
      renderConfig();
      if (field) {
        const next = elConfig.querySelector('[data-field="' + CSS.escape(field) + '"]');
        if (next) next.focus();
      }
    }
    calculate();
  }

  // ── Star chart editor ────────────────────────────────────────────────────
  // Clones the shared star-chart widget into a modal and mounts it in embed
  // mode, so every node you click streams straight back into the build config
  // and the results behind the modal re-rank as you go.
  function openStarChartEditor() {
    const tpl = document.getElementById("gb-sc-template");
    if (!tpl || !window.BTTModal || !window.BTTStarChart) {
      toast(t("The star chart editor couldn't load."), true);
      return;
    }
    const m = window.BTTModal.open({ title: t("Star Chart") });
    m.wrap.classList.add("gb-sc-wrap");
    const card = m.wrap.querySelector(".mp-modal-card");
    card.classList.add("gb-sc-modal");
    card.appendChild(tpl.content.cloneNode(true));
    // Only the node counter survives from the build panel - lift it into the
    // toolbar so it stays visible once CSS hides the panel.
    const hints = card.querySelector(".sc-hints");
    const counter = card.querySelector("#sc-meta-nodes");
    if (hints && counter) hints.replaceWith(counter);
    // BTTModal translates before we append, so re-run over the cloned markup.
    if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh();
    window.BTTStarChart.mount({
      embed: true,
      initialCode: config.star_chart,
      onChange: (code) => {
        config.star_chart = code;
        saveConfig();
        if (scInput) scInput.value = code;
        scheduleStarChart();
      },
    });
  }

  // ── Star chart preview ───────────────────────────────────────────────────
  let scTimer = null;
  function scheduleStarChart() {
    if (scTimer) clearTimeout(scTimer);
    scTimer = setTimeout(fetchStarChart, 350);
  }
  async function fetchStarChart() {
    const code = config.star_chart;
    if (!code) { starChartInfo = null; renderStarChartSummary(); calculate(); return; }
    try {
      const r = await apiGet(`/site/gems/parse-star-chart?code=${encodeURIComponent(code)}`);
      starChartInfo = (r.paths_count > 0) ? r : { error: true };
    } catch (e) {
      starChartInfo = { error: true };
    }
    renderStarChartSummary();
    calculate();
  }
  function renderStarChartSummary() {
    const host = document.getElementById("gb-sc-summary");
    if (!host) return;
    host.textContent = "";
    if (!starChartInfo) return;
    if (starChartInfo.error) {
      host.className = "gb-sc-summary err";
      host.appendChild(h("span", {}, h("i", { class: "fa-solid fa-triangle-exclamation" }), " " + t("Invalid build code")));
      return;
    }
    host.className = "gb-sc-summary ok";
    host.appendChild(h("div", { class: "gb-sc-head" }, h("i", { class: "fa-solid fa-chart-network" }), " ",
      t("Star chart loaded"), " · ", h("b", {}, starChartInfo.paths_count), " " + t("nodes")));
    const relevant = ["Physical Damage", "Magic Damage", "Critical Damage", "Light", "Magic Find"];
    const stats = starChartInfo.stats || {};
    const rows = relevant.filter((n) => stats[n] && (stats[n].flat || stats[n].pct));
    if (rows.length) {
      const ul = h("div", { class: "gb-sc-stats" });
      rows.forEach((n) => {
        const v = stats[n];
        const parts = [];
        if (v.flat) parts.push("+" + num(v.flat));
        if (v.pct) parts.push("+" + v.pct + "%");
        ul.appendChild(h("div", {}, h("strong", {}, t(n) + ": "), h("span", {}, parts.join(" / "))));
      });
      host.appendChild(ul);
    }
  }

  // ── Calculate ────────────────────────────────────────────────────────────
  let calcTimer = null;
  function calculate() {
    // debounce rapid config changes into one request
    if (calcTimer) clearTimeout(calcTimer);
    calcTimer = setTimeout(runCalculate, 120);
  }
  async function runCalculate() {
    calculating = true;
    renderResults();
    try {
      const resp = await apiPost("/site/gems/builds/calculate", config);
      builds = resp.results || [];
      page = 0;
    } catch (e) {
      builds = [];
      toast(t("Could not calculate builds") + ": " + e.message, true);
    } finally {
      calculating = false;
      renderResults();
    }
  }

  // ── Results ──────────────────────────────────────────────────────────────
  // Display a build layout with its gem-groups spaced for clarity:
  // "9/0/0/18 + 3/0/0/1/1/4" -> "9/0 0/18 + 3/0/0 1/1/4". A 4-number segment
  // is an Empowered/Lesser pair (split after 2); a 6-number one is the Cosmic
  // Emp/Lesser triples (split after 3). Elemental->Cosmic keeps its " + ".
  function fmtLayout(layout) {
    if (typeof layout !== "string") return layout;
    return layout.split(" + ").map(function (seg) {
      var p = seg.trim().split("/");
      if (p.length === 4) return p[0] + "/" + p[1] + " " + p[2] + "/" + p[3];
      if (p.length === 6) return p.slice(0, 3).join("/") + " " + p.slice(3).join("/");
      return p.join("/");
    }).join(" + ");
  }
  // Small "?" affordance with a hover/focus tooltip (concise format explainer).
  function helpIcon(txt) {
    return h("span", { class: "gb-help", tabindex: "0", role: "note", "aria-label": txt },
      h("i", { class: "fa-solid fa-circle-question", "aria-hidden": "true" }),
      h("span", { class: "gb-help-tip" }, txt));
  }
  function copyLayout(text) {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).then(() => toast(t("Layout copied") + ": " + text), () => toast(text));
    } else {
      toast(text);
    }
  }
  function exportCsv() {
    if (!builds.length) return;
    const head = ["Rank", "Layout", "Light", "BaseDamage", "BonusDamage%", "TotalDamage", "CritDamage%", "Coefficient"];
    const rows = builds.map((b) => [b.rank, b.layout, b.light, Math.round(b.base_dmg), b.bonus_dmg, Math.round(b.total_dmg), b.crit_dmg, b.coefficient]);
    const csv = [head, ...rows].map((r) => r.join(",")).join("\n");
    const blob = new Blob([csv], { type: "text/csv" });
    const a = h("a", { href: URL.createObjectURL(blob), download: `gem-build-${config.character}-${config.build_type}.csv` });
    document.body.appendChild(a); a.click(); a.remove();
  }

  function renderResults() {
    elResults.textContent = "";
    const bestCoeff = builds.length ? builds[0].coefficient : 0;
    const buildHelp = t("Boosts placed on each stat, by gem group. The first pair is Empowered (Damage / Crit Damage), the second is Lesser (Damage / Crit Damage). After the + are the Cosmic gems (Damage / Crit / Light).");

    // Top build card
    if (builds.length) {
      const top = builds[0];
      const card = h("div", { class: "gb-top-card" },
        h("div", { class: "gb-top-copy" },
          h("div", { class: "gb-eyebrow-sm" }, t("Top build")),
          h("div", { class: "gb-top-layout-row" },
            h("div", { class: "gb-top-layout", title: t("Click to copy"), onClick: () => copyLayout(fmtLayout(top.layout)) }, fmtLayout(top.layout)),
            helpIcon(buildHelp)),
          h("p", { class: "gb-top-reason" }, buildHeadline(top))),
        h("div", { class: "gb-top-stats" },
          statBox("Coefficient", num(top.coefficient)),
          statBox("Light", num(top.light)),
          statBox("Crit dmg", top.crit_dmg.toFixed(1) + "%"),
          builds[1] ? statBox("Lead vs #2", "+" + (((top.coefficient - builds[1].coefficient) / builds[1].coefficient) * 100).toFixed(3) + "%") : null));
      elResults.appendChild(card);
    }

    // Toolbar
    const toolbar = h("div", { class: "gb-toolbar" },
      h("div", { class: "gb-metrics" },
        h("span", { class: "gb-chip" }, h("i", { class: "fa-solid fa-list-ol" }), " " + t("Builds") + ": " + builds.length),
        builds.length ? h("span", { class: "gb-chip" }, h("i", { class: "fa-solid fa-trophy" }), " " + t("Best") + ": " + num(bestCoeff)) : null,
        h("span", { class: "gb-chip" }, h("i", { class: "fa-solid fa-gem" }), " " + t(config.build_type))),
      h("div", { class: "gb-state" + (calculating ? " busy" : "") },
        h("i", { class: "fa-solid " + (calculating ? "fa-spinner fa-spin" : "fa-circle-check") }),
        " " + (calculating ? t("Calculating...") : t("Ready"))));
    elResults.appendChild(toolbar);

    // Table
    const wrap = h("div", { class: "gb-table-wrap" });
    const table = h("table", { class: "gb-table" });
    table.appendChild(h("thead", {}, h("tr", {},
      h("th", { class: "c" }, "#"),
      h("th", { class: "l" }, t("Build"), " ", helpIcon(buildHelp)),
      h("th", { class: "r" }, t("Light")),
      h("th", { class: "r" }, t("Base dmg")),
      h("th", { class: "r" }, t("Bonus dmg")),
      h("th", { class: "r" }, t("Total dmg")),
      h("th", { class: "r" }, t("Crit dmg")),
      h("th", { class: "r sort" }, t("Coefficient")),
      h("th", { class: "r" }, t("Diff")))));
    const tbody = h("tbody", {});
    if (calculating && !builds.length) {
      tbody.appendChild(h("tr", {}, h("td", { class: "c muted", colspan: 9 }, h("i", { class: "fa-solid fa-spinner fa-spin" }), " " + t("Crunching the math..."))));
    } else if (!builds.length) {
      tbody.appendChild(h("tr", {}, h("td", { class: "c muted", colspan: 9 }, t("No builds generated - check your config."))));
    } else {
      const start = page * PER_PAGE;
      builds.slice(start, start + PER_PAGE).forEach((b) => {
        tbody.appendChild(h("tr", { class: b.rank === 1 ? "best" : "" },
          h("td", { class: "c" }, b.rank),
          h("td", { class: "l layout", title: t("Click to copy"), onClick: () => copyLayout(fmtLayout(b.layout)) }, fmtLayout(b.layout)),
          h("td", { class: "r" }, num(b.light)),
          h("td", { class: "r" }, round(b.base_dmg)),
          h("td", { class: "r" }, b.bonus_dmg.toFixed(2) + "%", b.class_bonus ? h("span", { class: "gb-bonus-extra" }, " + " + b.class_bonus + "%") : null),
          h("td", { class: "r" }, round(b.total_dmg)),
          h("td", { class: "r" }, b.crit_dmg.toFixed(1) + "%"),
          h("td", { class: "r sort strong" }, num(b.coefficient)),
          h("td", { class: "r" }, b.rank === 1
            ? h("span", { class: "gb-best-tag" }, t("Best"))
            : h("span", { class: "gb-diff" }, "-" + (((bestCoeff - b.coefficient) / bestCoeff) * 100).toFixed(3) + "%"))));
      });
    }
    table.appendChild(tbody);
    wrap.appendChild(table);
    elResults.appendChild(wrap);

    // Pagination + export
    const maxPages = Math.max(1, Math.ceil(builds.length / PER_PAGE));
    const pag = h("div", { class: "gb-pagination" },
      h("button", { class: "gb-btn-icon", disabled: !builds.length, onClick: exportCsv, title: t("Export to CSV") },
        h("i", { class: "fa-solid fa-file-csv" }), " " + t("Export")),
      h("div", { class: "gb-pager" },
        h("button", { class: "gb-btn-icon", "aria-label": t("Previous page"), disabled: page === 0, onClick: () => { page = Math.max(0, page - 1); renderResults(); } }, h("i", { class: "fa-solid fa-chevron-left", "aria-hidden": "true" })),
        h("span", { class: "gb-page-label" }, `${t("Page")} ${page + 1} / ${maxPages}`),
        h("button", { class: "gb-btn-icon", "aria-label": t("Next page"), disabled: page >= maxPages - 1, onClick: () => { page = Math.min(maxPages - 1, page + 1); renderResults(); } }, h("i", { class: "fa-solid fa-chevron-right", "aria-hidden": "true" }))));
    elResults.appendChild(pag);
  }

  function statBox(label, value) {
    return h("div", { class: "gb-stat-box" }, h("span", {}, t(label)), h("strong", {}, value));
  }
  function buildHeadline(b) {
    if (config.build_type === "Farm") return t("Best balance of light and damage for farming.");
    if (config.build_type === "Health") return t("Highest effective health from your gem layout.");
    return t("Highest damage coefficient for your setup.");
  }

  // ── Init ─────────────────────────────────────────────────────────────────
  async function init() {
    elConfig = document.getElementById("gb-config");
    elResults = document.getElementById("gb-results");
    if (!elConfig) return;

    loadConfig();
    try {
      options = await apiGet("/site/gems/builds/options");
    } catch (e) {
      elConfig.appendChild(h("div", { class: "gb-error" }, t("Could not load build options. Please reload.")));
      return;
    }
    // sanitize saved config against current options
    if (!options.character.includes(config.character)) config.character = options.character[0];
    if (!options.character.includes(config.subclass)) config.subclass = options.character[0];
    if (!options.build_type.includes(config.build_type)) config.build_type = options.build_type[0];

    renderConfig();
    renderResults();
    if (config.star_chart) fetchStarChart();
    calculate();
  }

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init);
  else init();
})();
