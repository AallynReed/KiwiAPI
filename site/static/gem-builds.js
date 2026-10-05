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
  const tr = (s) => s;   // marks a string handed to t() later, for the i18n audit
  const PER_PAGE = 25;
  const STORAGE_KEY = "troveapi.gemBuilds.v1";

  // High precision widens every number to at most 8 decimals and trims the
  // trailing zeros, so 78.93800000 still reads as 78.938. Off, the columns keep
  // their fixed widths - a table of ragged decimals is unreadable at a glance.
  const HP_DIGITS = 8;
  const hp = () => !!config.high_precision;
  const num = (v) => (Number(v) || 0).toLocaleString(undefined, { maximumFractionDigits: hp() ? HP_DIGITS : 3 });
  const round = (v) => (hp() ? num(v) : Math.round(Number(v) || 0).toLocaleString());
  // Fixed-width in normal mode (matching the old toFixed), trimmed in high precision.
  const dec = (v, digits) => (Number(v) || 0).toLocaleString(undefined, hp()
    ? { maximumFractionDigits: HP_DIGITS, useGrouping: false }
    : { minimumFractionDigits: digits, maximumFractionDigits: digits, useGrouping: false });

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
  // Build goals come from the API; named here so the labels reach the locale files.
  const BUILD_GOAL_LABEL = { Light: tr("Light"), Farm: tr("Farm"), Health: tr("Health") };

  // ── State ────────────────────────────────────────────────────────────────
  let options = null;
  const config = {
    build_type: "Light", character: "Bard", subclass: "Boomeranger",
    food: "", ally: "boot_clown", ally_buff: true, critical_damage_count: 3, no_face: false,
    light: 0, subclass_active: false, litany: false, berserker_battler: false,
    bounty_hunt: true, star_chart: "", high_precision: false,
    weapon_ch: false, ring_ch: false, hat_health: false, face_health: false,
  };
  let builds = [];
  let page = 0;
  let calculating = false;
  let starChartInfo = null;   // {paths_count, stats} | {error:true} | null
  let advancedOpen = false;

  let health = null;          // /health-optimizer response
  let hpPage = 0;
  let hpCalculating = false;
  const HP_PER_PAGE = 10;

  let elConfig, elResults, elHealth;
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
  function toggleRow(label, value, onChange, title, cls) {
    const c = h("input", { type: "checkbox" });
    c.checked = value;
    c.addEventListener("change", (e) => onChange(e.target.checked));
    const text = h("span", {}, t(label));
    const row = h("label", { class: "gb-toggle" + (cls ? " " + cls : ""), title: title ? t(title) : null },
      c, h("span", { class: "gb-toggle-mark" }), text);
    row.input = c;
    row.text = text;
    return row;
  }

  function renderConfig() {
    elConfig.textContent = "";
    const classEntries = options.character.map((c) => ({ value: c, label: c }));
    const buildTypeEntries = options.build_type.map((b) => ({ value: b, label: BUILD_GOAL_LABEL[b] || b }));
    const foodEntries = [{ value: "", label: tr("None") }].concat(options.food.map((f) => ({ value: f.key, label: f.label })));
    const allyEntries = options.ally.map((a) => ({ value: a.key, label: a.key === "boot_clown" ? tr("No ally") : a.label }));

    // Character
    const charSection = h("div", { class: "gb-section" },
      h("h4", {}, h("i", { class: "fa-solid fa-user-astronaut" }), " " + t("Character")),
      h("div", { class: "gb-row-2" },
        classSelectRow(tr("Class"), config.character, classEntries, (v) => { config.character = v; onConfigChange(); }),
        classSelectRow(tr("Subclass"), config.subclass, classEntries, (v) => { config.subclass = v; onConfigChange(); })));
    elConfig.appendChild(charSection);

    // Gear / goal
    const gearSection = h("div", { class: "gb-section" },
      h("h4", {}, h("i", { class: "fa-solid fa-shield-halved" }), " " + t("Build & gear")),
      h("div", { class: "gb-row-2" },
        selectRow(tr("Build goal"), config.build_type, buildTypeEntries, (v) => { config.build_type = v; onConfigChange(true); }),
        selectRow(tr("Food"), config.food, foodEntries, (v) => { config.food = v; onConfigChange(); })),
      selectRow(tr("Ally"), config.ally, allyEntries, (v) => { config.ally = v; onConfigChange(true); }));

    // Ally stats are the level-30 values; the Lilypad buff scales them further.
    if (config.ally !== "boot_clown") {
      gearSection.appendChild(toggleRow(tr("Blessing of the Lilypad"), config.ally_buff,
        (v) => { config.ally_buff = v; onConfigChange(); },
        tr("The ally buff, on top of level 30: +15.5% to the ally's light and +31% to its damage bonus.")));
    }

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
      gearSection.appendChild(toggleRow(tr("Face slot has no damage stat"), config.no_face, (v) => { config.no_face = v; onConfigChange(); }, tr("Simulates a face slot with no damage stat.")));
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
        toggleRow(tr("Berserker Battler"), config.berserker_battler, (v) => { config.berserker_battler = v; onConfigChange(); }, tr("Treats Berserker Battler as active (adds its light).")),
        toggleRow(tr("Enlightened / Litany"), config.litany, (v) => { config.litany = v; onConfigChange(); }, tr("Adds the light from the Enlightened (Litany) buff.")),
        toggleRow(tr("Subclass active"), config.subclass_active, (v) => { config.subclass_active = v; onConfigChange(); }, tr("Includes the passive stats from your chosen subclass.")));
      advSection.appendChild(grid);
    }
    elConfig.appendChild(advSection);

    renderStarChartSummary();
  }

  // Bounty Hunt is a 4-hour buff from a Sundered Uplands 5-star boss, and only
  // the star chart can unlock it - the Minor node upgrades the Major's +10% to
  // +15% rather than stacking. `config.bounty_hunt` is the reader's preference
  // (on by default, so a pasted chart brings the buff with it); it only counts
  // once the chart actually contains the node, which is why the toggle lives in
  // the chart summary and the preference is never cleared behind their back.
  function bountyBuff() {
    const bh = starChartInfo && !starChartInfo.error && starChartInfo.bounty_hunt;
    return bh && bh.available ? bh : null;
  }
  const bountyActive = () => !!bountyBuff() && !!config.bounty_hunt;

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
    const bh = bountyBuff();
    if (bh) {
      const row = toggleRow(tr("Bounty Hunt"), !!config.bounty_hunt,
        (v) => { config.bounty_hunt = v; onConfigChange(); },
        tr("The 4-hour buff from a Sundered Uplands 5-star boss. Untick it to rank your builds without it."),
        "gb-sc-buff");
      row.text.textContent = "";
      row.text.append(h("strong", {}, t("Bounty Hunt")),
        " +" + Math.max(bh.physical, bh.magic) + "% " + t("Physical and Magic Damage"));
      host.appendChild(row);
    }
  }

  // ── Calculate ────────────────────────────────────────────────────────────
  let calcTimer = null;
  function calculate() {
    // debounce rapid config changes into one request
    if (calcTimer) clearTimeout(calcTimer);
    calcTimer = setTimeout(runCalculate, 120);
    calculateHealth();
  }
  async function runCalculate() {
    calculating = true;
    renderResults();
    try {
      // The stored flag is only a preference - a chart without the node must not
      // silently ship the buff to the optimizer.
      const payload = Object.assign({}, config, { bounty_hunt: bountyActive() });
      const resp = await apiPost("/site/gems/builds/calculate", payload);
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
    window.BTTUtil.copy(text).then((ok) => toast(ok ? t("Layout copied") + ": " + text : text));
  }
  function exportCsv() {
    if (!builds.length) return;
    const head = ["Rank", "Layout", "Light", "BaseDamage", "BonusDamage%", "TotalDamage", "CritDamage%", "CritDamageBonus%", "Coefficient"];
    const cell = (v) => (hp() ? v : Math.round(v));
    const rows = builds.map((b) => [b.rank, b.layout, b.light, cell(b.base_dmg), b.bonus_dmg, cell(b.total_dmg), b.crit_dmg, b.crit_bonus || 0, b.coefficient]);
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
          statBox(tr("Coefficient"), num(top.coefficient)),
          statBox(tr("Light"), num(top.light)),
          statBox(tr("Crit dmg"), dec(top.crit_dmg, 1) + "%"),
          builds[1] ? statBox(tr("Lead vs #2"), "+" + dec(((top.coefficient - builds[1].coefficient) / builds[1].coefficient) * 100, 3) + "%") : null));
      elResults.appendChild(card);
    }

    // Toolbar
    const toolbar = h("div", { class: "gb-toolbar" },
      h("div", { class: "gb-metrics" },
        h("span", { class: "gb-chip" }, h("i", { class: "fa-solid fa-list-ol" }), " " + t("Builds") + ": " + builds.length),
        builds.length ? h("span", { class: "gb-chip" }, h("i", { class: "fa-solid fa-trophy" }), " " + t("Best") + ": " + num(bestCoeff)) : null,
        h("span", { class: "gb-chip" }, h("i", { class: "fa-solid fa-gem" }), " " + t(BUILD_GOAL_LABEL[config.build_type] || config.build_type))),
      h("div", { class: "gb-toolbar-right" },
        toggleRow(tr("High precision decimals"), config.high_precision, (v) => { config.high_precision = v; onConfigChange(); },
          tr("Shows up to 8 decimals on every number instead of rounding to 1-2."), "gb-toggle-inline"),
        h("div", { class: "gb-state" + (calculating ? " busy" : "") },
          h("i", { class: "fa-solid " + (calculating ? "fa-spinner fa-spin" : "fa-circle-check") }),
          " " + (calculating ? t("Calculating...") : t("Ready")))));
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
          h("td", { class: "r" }, dec(b.bonus_dmg, 2) + "%", b.class_bonus ? h("span", { class: "gb-bonus-extra" }, " + " + b.class_bonus + "%") : null),
          h("td", { class: "r" }, round(b.total_dmg)),
          h("td", { class: "r" }, dec(b.crit_dmg, 1) + "%",
            b.crit_bonus ? h("span", { class: "gb-bonus-extra" }, " + " + dec(b.crit_bonus, 1) + "%") : null),
          h("td", { class: "r sort strong" }, num(b.coefficient)),
          h("td", { class: "r" }, b.rank === 1
            ? h("span", { class: "gb-best-tag" }, t("Best"))
            : h("span", { class: "gb-diff" }, "-" + dec(((bestCoeff - b.coefficient) / bestCoeff) * 100, 3) + "%"))));
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

  // ── Health optimizer ─────────────────────────────────────────────────────
  // A damage build fills two stats on each gem; this spends the third on the
  // nine Fire/Water/Air gems. Meaningless for a Health build, so it hides there.
  const HP_STATS = [
    { key: "critical_hit", label: "Critical Hit" },
    { key: "maximum_health", label: "Maximum Health" },
    { key: "maximum_health_per", label: tr("Maximum Health %") },
  ];
  const hpApplies = () => config.build_type !== "Health";

  let hpTimer = null;
  function calculateHealth() {
    if (hpTimer) clearTimeout(hpTimer);
    hpTimer = setTimeout(runHealth, 120);
  }
  async function runHealth() {
    if (!elHealth) return;
    if (!hpApplies()) { renderHealth(); return; }
    hpCalculating = true;
    renderHealth();
    try {
      health = await apiPost("/site/gems/builds/health-optimizer", {
        character: config.character, star_chart: config.star_chart || null,
        weapon_ch: config.weapon_ch, ring_ch: config.ring_ch,
        hat_health: config.hat_health, face_health: config.face_health,
        high_precision: config.high_precision,
      });
      hpPage = 0;
    } catch (e) {
      health = null;
      toast(t("Could not optimize health") + ": " + e.message, true);
    } finally {
      hpCalculating = false;
      renderHealth();
    }
  }

  // "3 Empowered · 2 Lesser", skipping a gem type that carries none of it.
  function hpGems(b, key) {
    const parts = [];
    if (b.empowered[key]) parts.push(b.empowered[key] + " " + t("Empowered"));
    if (b.lesser[key]) parts.push(b.lesser[key] + " " + t("Lesser"));
    return parts.join(" · ");
  }

  function renderHealth() {
    if (!elHealth) return;
    elHealth.hidden = !hpApplies();
    if (elHealth.hidden) return;
    elHealth.textContent = "";
    const results = (health && health.results) || [];
    const best = results[0];
    const layoutHelp = t("How many gems put their spare stat on each one: Empowered, then Lesser - Critical Hit / Max Health / Max Health %.");

    elHealth.appendChild(h("div", { class: "gb-hp-head" },
      h("h2", { class: "gb-hp-title", id: "gb-hp-title" }, h("i", { class: "fa-solid fa-heart-pulse", "aria-hidden": "true" }), " " + t("Health optimizer")),
      h("p", { class: "gb-hp-lead" }, t("Turn the spare stat on each gem into as much health as possible, without dropping below 100% critical hit."),
        " ", helpIcon(t("Your damage build uses two of each gem's three stats. This picks the third on your Fire, Water and Air gems - Cosmic gems have no spare stat.")))));

    const gear = (key, label) => toggleRow(label, !!config[key], (v) => { config[key] = v; saveConfig(); calculateHealth(); }, null, "gb-toggle-inline");
    elHealth.appendChild(h("div", { class: "gb-hp-gear", role: "group", "aria-label": t("Your gear") },
      h("span", { class: "gb-field-label" }, t("Your gear")),
      h("div", { class: "gb-hp-gear-grid" },
        gear("weapon_ch", tr("Critical Hit on weapon")),
        gear("ring_ch", tr("Critical Hit on ring")),
        gear("hat_health", tr("Max Health % on hat")),
        gear("face_health", tr("Max Health % on face")))));

    if (best) {
      const rows = HP_STATS.map((s) => {
        const gems = hpGems(best, s.key);
        return gems ? h("li", {}, h("strong", {}, t(s.label)), h("span", {}, gems)) : null;
      });
      elHealth.appendChild(h("div", { class: "gb-top-card gb-hp-best" },
        h("div", { class: "gb-top-copy" },
          h("ul", { class: "gb-hp-split" }, rows),
          health.base ? h("p", { class: "gb-top-reason" },
            t("Before gems you have {n}% critical hit.").replace("{n}", num(health.base.critical_hit))) : null),
        h("div", { class: "gb-top-stats" },
          statBox(tr("Max health"), round(best.health)),
          statBox(tr("Crit hit"), num(best.critical_hit) + "%"),
          statBox(tr("Flat health"), round(best.maximum_health)),
          statBox(tr("Health %"), num(best.maximum_health_per) + "%"))));
    }

    const wrap = h("div", { class: "gb-table-wrap" });
    const table = h("table", { class: "gb-table" });
    table.appendChild(h("thead", {}, h("tr", {},
      h("th", { class: "c" }, "#"),
      h("th", { class: "l" }, t("Spare stat"), " ", helpIcon(layoutHelp)),
      h("th", { class: "r" }, t("Crit hit")),
      h("th", { class: "r" }, t("Flat health")),
      h("th", { class: "r" }, t("Health %")),
      h("th", { class: "r sort" }, t("Max health")),
      h("th", { class: "r" }, t("Diff")))));
    const tbody = h("tbody", {});
    if (hpCalculating && !results.length) {
      tbody.appendChild(h("tr", {}, h("td", { class: "c muted", colspan: 7 }, h("i", { class: "fa-solid fa-spinner fa-spin" }), " " + t("Crunching the math..."))));
    } else if (!results.length) {
      tbody.appendChild(h("tr", {}, h("td", { class: "c muted", colspan: 7 }, t("No split reaches 100% critical hit."))));
    } else {
      const start = hpPage * HP_PER_PAGE;
      results.slice(start, start + HP_PER_PAGE).forEach((b) => {
        tbody.appendChild(h("tr", { class: b.rank === 1 ? "best" : "" },
          h("td", { class: "c" }, b.rank),
          h("td", { class: "l layout gb-hp-layout" }, b.layout),
          h("td", { class: "r" }, num(b.critical_hit) + "%"),
          h("td", { class: "r" }, round(b.maximum_health)),
          h("td", { class: "r" }, num(b.maximum_health_per) + "%"),
          h("td", { class: "r sort strong" }, round(b.health)),
          h("td", { class: "r" }, b.rank === 1
            ? h("span", { class: "gb-best-tag" }, t("Best"))
            : h("span", { class: "gb-diff" }, "-" + dec(((best.health - b.health) / best.health) * 100, 3) + "%"))));
      });
    }
    table.appendChild(tbody);
    wrap.appendChild(table);
    elHealth.appendChild(wrap);

    const maxPages = Math.max(1, Math.ceil(results.length / HP_PER_PAGE));
    elHealth.appendChild(h("div", { class: "gb-pagination" },
      h("span", { class: "gb-page-note" }, t("Assumes every dragon, full mastery and a club.")),
      h("div", { class: "gb-pager" },
        h("button", { class: "gb-btn-icon", "aria-label": t("Previous page"), disabled: hpPage === 0, onClick: () => { hpPage = Math.max(0, hpPage - 1); renderHealth(); } }, h("i", { class: "fa-solid fa-chevron-left", "aria-hidden": "true" })),
        h("span", { class: "gb-page-label" }, `${t("Page")} ${hpPage + 1} / ${maxPages}`),
        h("button", { class: "gb-btn-icon", "aria-label": t("Next page"), disabled: hpPage >= maxPages - 1, onClick: () => { hpPage = Math.min(maxPages - 1, hpPage + 1); renderHealth(); } }, h("i", { class: "fa-solid fa-chevron-right", "aria-hidden": "true" })))));
  }

  // ── Last updated ─────────────────────────────────────────────────────────
  // The template stamps the UTC instant; this rewrites it in whatever timezone
  // and locale the reader's browser is set to, so nobody has to convert from
  // ours. Falls back to the raw ISO text if the date is unparseable.
  function renderUpdated() {
    const el = document.getElementById("gb-updated");
    if (!el) return;
    const when = new Date(el.getAttribute("datetime"));
    if (isNaN(when)) return;
    // Date format follows the site's language picker so it doesn't read as
    // English inside a translated page; the timezone is always the reader's.
    const lang = document.documentElement.lang || undefined;
    el.textContent = when.toLocaleString(lang, { dateStyle: "medium", timeStyle: "short" });
    el.title = when.toString();
  }
  document.addEventListener("btt-lang-changed", renderUpdated);

  // ── Init ─────────────────────────────────────────────────────────────────
  async function init() {
    renderUpdated();
    elConfig = document.getElementById("gb-config");
    elResults = document.getElementById("gb-results");
    elHealth = document.getElementById("gb-health");
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
    // An ally can leave the list. Without this, a saved config pointing at a
    // retired one silently contributes nothing while the picker shows the first
    // option, so the ranking would not match what the page says is selected.
    if (!options.ally.some((a) => a.key === config.ally)) config.ally = "boot_clown";

    renderConfig();
    renderResults();
    renderHealth();
    if (config.star_chart) fetchStarChart();
    calculate();
    // Labels go through t() once, which can beat the locale file to the page.
    document.addEventListener("btt-lang-changed", () => { renderConfig(); renderResults(); renderHealth(); });
  }

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init);
  else init();
})();
