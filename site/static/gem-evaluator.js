/* =========================================================================
   Gem Evaluator - score a typed-in gem and cost out perfecting it.
   Vanilla-JS re-implementation of Better Trove Tools' Vue gem evaluator. The
   gem math runs server-side: the page POSTs to the same-origin /site/gems/*
   proxies (a token-free mirror of /v1/gems/evaluate + stat-range + lookups).
   Recent gems persist to localStorage. CSP-clean: no eval, no inline handlers.
   ========================================================================= */
(function () {
  "use strict";
  const { h } = window.BTTDom;

  // ── i18n + helpers (mirrors gem-simulator.js) ──────────────────────────
  const t = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);
  const fmt = (s, p) => {
    let out = t(s);
    if (p) for (const k in p) out = out.split("{" + k + "}").join(p[k]);
    return out;
  };

  const ASSET = "/static/assets/gems";
  const STORAGE_KEY = "troveapi.gemEvaluator.v1";
  const MAX_HISTORY = 20;

  const fmtNum = (v) => {
    const n = Number(v) || 0;
    return Number.isInteger(n) ? n.toLocaleString() : n.toLocaleString(undefined, { maximumFractionDigits: 2 });
  };
  const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));

  // ── Static reference (matches the gem lookups payload) ───────────────────
  const FOCUS_ICON = { rough: `${ASSET}/augments/1.png`, precise: `${ASSET}/augments/2.png`, superior: `${ASSET}/augments/3.png` };
  const FOCUS_LABEL = { superior: "Superior", precise: "Precise", rough: "Rough" };
  const MATERIAL_LABEL = {
    bound_brilliance: "Bound Brilliance", heart_of_darkness: "Heart of Darkness", flux: "Flux",
    water_gem_dust: "Water Gem Dust", air_gem_dust: "Air Gem Dust", fire_gem_dust: "Fire Gem Dust",
    diamond_dragonite: "Diamond Dragonite", titan_soul: "Titan Soul",
  };
  const materialIcon = (key) => `${ASSET}/misc/items/${key}.png`;
  const gemIcon = (type) => `${ASSET}/misc/${Number(type) === 2 ? "empowered" : "lesser"}.png`;

  // ── State ────────────────────────────────────────────────────────────────
  const MODE_KEY = "troveapi.gemEvaluator.mode";
  let mode = "simple";               // "simple" (PR-only) | "full" (per-stat)
  let lookups = { tiers: [], types: [], stat_types: [] };
  const form = {
    tier: 4, type: 2, level: 25, autoGuess: true,
    stats: [
      { type: 1, value: "", extra: 0 },
      { type: 3, value: "", extra: 0 },
      { type: 7, value: "", extra: 0 },
    ],
  };
  const simpleForm = { tier: 4, type: 2, powerRank: "", level: 25 };
  let ranges = [null, null, null];   // stat-range hint per row
  let result = null;                 // full-mode result
  let simpleResult = null;           // simple-mode result
  let evaluating = false;
  let history = [];

  let elForm, elResults, elHistory, elToastHost;

  // ── Toast ────────────────────────────────────────────────────────────────
  function toast(msg, isErr) {
    if (!elToastHost) return;
    const el = h("div", { class: "gem-toast" + (isErr ? " error" : "") }, msg);
    elToastHost.appendChild(el);
    setTimeout(() => { el.classList.add("out"); setTimeout(() => el.remove(), 300); }, 2600);
  }

  // ── Persistence ──────────────────────────────────────────────────────────
  function saveHistory() {
    try { localStorage.setItem(STORAGE_KEY, JSON.stringify(history.slice(0, MAX_HISTORY))); } catch (e) { /* non-fatal */ }
  }
  function loadHistory() {
    try {
      const data = JSON.parse(localStorage.getItem(STORAGE_KEY) || "null");
      if (Array.isArray(data)) history = data.slice(0, MAX_HISTORY);
    } catch (e) { history = []; }
  }

  // ── Lookup helpers ───────────────────────────────────────────────────────
  const tierName = (id) => (lookups.tiers.find((x) => x.id === Number(id)) || {}).name || ("Tier " + id);
  const tierMaxLevel = (id) => (lookups.tiers.find((x) => x.id === Number(id)) || {}).max_level || 35;
  const typeName = (id) => (lookups.types.find((x) => x.id === Number(id)) || {}).name || ("Type " + id);
  const statName = (id) => (lookups.stat_types.find((x) => x.id === Number(id)) || {}).name || ("Stat " + id);
  // Which stat types can a player actually put on a gem (excludes movement/jump).
  const SELECTABLE_STATS = [1, 2, 3, 4, 5, 6, 7];
  const statOptions = () => lookups.stat_types.filter((s) => SELECTABLE_STATS.includes(s.id));

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

  // ── Stat-range hints (debounced) ─────────────────────────────────────────
  let rangeTimer = null;
  function scheduleRanges() {
    if (rangeTimer) clearTimeout(rangeTimer);
    rangeTimer = setTimeout(fetchRanges, 250);
  }
  async function fetchRanges() {
    const jobs = form.stats.map((s, i) => {
      const params = new URLSearchParams({
        tier: form.tier, type: form.type, stat_type: s.type,
        level: form.level, extra_containers: s.extra,
      });
      return apiGet(`/site/gems/stat-range?${params}`).then(
        (r) => { ranges[i] = r; },
        () => { ranges[i] = null; }
      );
    });
    await Promise.all(jobs);
    renderRangeHints();
  }

  // ── Render: form ─────────────────────────────────────────────────────────
  function selectField(label, value, options, onChange) {
    const sel = h("select", { class: "ge-select", onChange: (e) => onChange(Number(e.target.value)) });
    options.forEach((o) => {
      const opt = h("option", { value: o.id }, t(o.name));
      if (Number(o.id) === Number(value)) opt.selected = true;
      sel.appendChild(opt);
    });
    return h("label", { class: "ge-field" }, h("span", { class: "ge-field-label" }, t(label)), sel);
  }
  function numberField(label, value, attrs, onChange) {
    const inp = h("input", Object.assign({ class: "ge-input", type: "number", value: value }, attrs || {}));
    inp.addEventListener("input", (e) => onChange(e.target.value));
    return h("label", { class: "ge-field" }, h("span", { class: "ge-field-label" }, t(label)), inp);
  }

  function modeTab(key, label) {
    return h("button", { class: "ge-mode-tab" + (mode === key ? " active" : ""), onClick: () => {
      if (mode === key) return;
      mode = key;
      try { localStorage.setItem(MODE_KEY, mode); } catch (e) { /* non-fatal */ }
      renderForm(); renderResults();
    } }, t(label));
  }

  function renderForm() {
    elForm.textContent = "";
    elForm.appendChild(h("div", { class: "ge-mode-tabs" },
      modeTab("simple", "Simple evaluator"), modeTab("full", "Full evaluator")));
    elForm.appendChild(h("p", { class: "ge-mode-intro" }, t(mode === "simple"
      ? "Estimate quality from just the gem's Power Rank - quick when you don't want to type in every stat."
      : "Back-calculate quality from the gem's three stats for a precise, per-stat breakdown.")));
    if (mode === "simple") buildSimpleForm();
    else buildFullForm();
    renderHistory();
  }

  function buildFullForm() {
    const topRow = h("div", { class: "ge-row ge-row-3" },
      selectField("Type", form.type, lookups.types, (v) => { form.type = v; scheduleRanges(); renderForm(); }),
      selectField("Rarity", form.tier, lookups.tiers, (v) => { form.tier = v; clampLevel(); scheduleRanges(); renderForm(); }),
      numberField("Level", form.level, { min: 1, max: tierMaxLevel(form.tier), step: 1 }, (v) => {
        form.level = clamp(parseInt(v) || 1, 1, tierMaxLevel(form.tier)); scheduleRanges(); updateProcSummary();
      }),
    );
    elForm.appendChild(topRow);

    // Proc-spread summary + auto-guess toggle
    const available = Math.floor(Math.min(form.level, 15) / 5);
    const usedProcs = form.stats.reduce((a, s) => a + (Number(s.extra) || 0), 0);
    const procRow = h("div", { class: "ge-proc-row" },
      h("span", { class: "ge-proc-summary" },
        h("b", {}, t("Procs")), " ",
        h("span", { class: usedProcs === available || form.autoGuess ? "" : "ge-proc-warn" },
          `${form.autoGuess ? available : usedProcs} / ${available}`)),
      h("label", { class: "ge-toggle" },
        (() => {
          const c = h("input", { type: "checkbox" });
          c.checked = form.autoGuess;
          c.addEventListener("change", (e) => { form.autoGuess = e.target.checked; renderForm(); });
          return c;
        })(),
        h("span", { class: "ge-toggle-mark" }),
        h("span", {}, t("Auto-guess proc spread"))),
    );
    elForm.appendChild(procRow);

    // Stat rows
    const statList = h("div", { class: "ge-stat-list" });
    form.stats.forEach((s, i) => {
      const opts = statOptions().filter((o) => {
        // no duplicate stat types across rows; Physical + Magic can't coexist
        const chosen = form.stats.filter((_, j) => j !== i).map((x) => Number(x.type));
        if (chosen.includes(o.id)) return false;
        if (o.id === 1 && chosen.includes(2)) return false;
        if (o.id === 2 && chosen.includes(1)) return false;
        return true;
      });
      // keep current selection available
      if (!opts.find((o) => o.id === Number(s.type))) opts.unshift({ id: s.type, name: statName(s.type) });

      const valInp = h("input", { class: "ge-input ge-stat-value", type: "number", step: "any", min: 0, value: s.value, placeholder: t("In-game value") });
      valInp.addEventListener("input", (e) => { s.value = e.target.value; renderRangeHints(); });

      const typeSel = h("select", { class: "ge-select", onChange: (e) => { s.type = Number(e.target.value); scheduleRanges(); renderForm(); } });
      opts.forEach((o) => {
        const opt = h("option", { value: o.id }, t(o.name));
        if (Number(o.id) === Number(s.type)) opt.selected = true;
        typeSel.appendChild(opt);
      });

      const procSel = h("select", { class: "ge-select ge-proc-select", disabled: form.autoGuess, onChange: (e) => { s.extra = Number(e.target.value); scheduleRanges(); updateProcSummary(); } });
      for (let p = 0; p <= 3; p++) {
        const opt = h("option", { value: p }, "+" + p);
        if (Number(p) === Number(s.extra)) opt.selected = true;
        procSel.appendChild(opt);
      }

      const row = h("div", { class: "ge-stat-block" },
        h("div", { class: "ge-stat-row" },
          valInp,
          typeSel,
          h("div", { class: "ge-proc-field", title: t("Extra containers (procs) on this stat") },
            h("span", { class: "ge-proc-cap" }, t("procs")), procSel)),
        h("div", { class: "ge-range-hint", dataset: { row: i } }),
      );
      statList.appendChild(row);
    });
    elForm.appendChild(statList);

    const btn = h("button", { class: "ge-calc-btn", disabled: evaluating, onClick: submit },
      evaluating ? h("span", {}, h("i", { class: "fa-solid fa-spinner fa-spin" }), " " + t("Evaluating...")) : t("Evaluate gem"));
    elForm.appendChild(btn);

    renderRangeHints();
  }

  // ── Simple form (PR-only) ────────────────────────────────────────────────
  function buildSimpleForm() {
    const simpleTierMax = () => tierMaxLevel(simpleForm.tier);
    elForm.appendChild(h("div", { class: "ge-row ge-row-2" },
      selectField("Type", simpleForm.type, lookups.types, (v) => { simpleForm.type = v; renderForm(); }),
      selectField("Rarity", simpleForm.tier, lookups.tiers, (v) => {
        simpleForm.tier = v;
        simpleForm.level = clamp(simpleForm.level, 1, simpleTierMax());
        renderForm();
      })));
    elForm.appendChild(h("div", { class: "ge-row ge-row-2" },
      numberField("Power Rank", simpleForm.powerRank, { min: 0, step: 1, placeholder: t("The gem's Power Rank") }, (v) => { simpleForm.powerRank = v; }),
      numberField("Level", simpleForm.level, { min: 1, max: simpleTierMax(), step: 1 }, (v) => { simpleForm.level = clamp(parseInt(v) || 1, 1, simpleTierMax()); })));

    const btn = h("button", { class: "ge-calc-btn", disabled: evaluating, onClick: submit },
      evaluating ? h("span", {}, h("i", { class: "fa-solid fa-spinner fa-spin" }), " " + t("Evaluating...")) : t("Estimate quality"));
    elForm.appendChild(btn);
  }

  function clampLevel() { form.level = clamp(form.level, 1, tierMaxLevel(form.tier)); }
  function updateProcSummary() {
    const el = elForm.querySelector(".ge-proc-summary span:last-child");
    if (!el) return;
    const available = Math.floor(Math.min(form.level, 15) / 5);
    const usedProcs = form.stats.reduce((a, s) => a + (Number(s.extra) || 0), 0);
    el.textContent = `${form.autoGuess ? available : usedProcs} / ${available}`;
    el.className = usedProcs === available || form.autoGuess ? "" : "ge-proc-warn";
  }

  function renderRangeHints() {
    form.stats.forEach((s, i) => {
      const host = elForm.querySelector(`.ge-range-hint[data-row="${i}"]`);
      if (!host) return;
      host.textContent = "";
      const r = ranges[i];
      if (!r) return;
      const val = Number(s.value);
      const out = val && (val < r.min_value || val > r.max_value);
      host.className = "ge-range-hint" + (out ? " out" : "");
      host.appendChild(h("span", {},
        t("Typical range"), ": ", h("b", {}, `${fmtNum(r.min_value)} – ${fmtNum(r.max_value)}`)));
      if (val > 0 && r.max_value > r.min_value) {
        const pct = clamp(((val - r.min_value) / (r.max_value - r.min_value)) * 100, 0, 100);
        host.appendChild(h("span", { class: "ge-range-pct" }, h("i", { class: "fa-solid fa-location-arrow" }), " " + Math.round(pct) + "%"));
      }
    });
  }

  // ── Submit ───────────────────────────────────────────────────────────────
  function submit() { return mode === "simple" ? submitSimple() : submitFull(); }

  async function submitFull() {
    if (evaluating) return;
    for (const s of form.stats) {
      if (s.value === "" || s.value == null || isNaN(Number(s.value))) {
        toast(t("Enter a value for all three stats."), true);
        return;
      }
    }
    evaluating = true;
    renderForm();
    try {
      const body = {
        tier: Number(form.tier), type: Number(form.type), level: Number(form.level),
        auto_guess_procs: !!form.autoGuess,
        stats: form.stats.map((s) => ({ type: Number(s.type), value: Number(s.value), extra_containers: Number(s.extra) || 0 })),
      };
      result = await apiPost("/site/gems/evaluate", body);
      pushHistoryFull(result);
      renderResults();
    } catch (e) {
      toast(t("Could not evaluate gem") + ": " + e.message, true);
    } finally {
      evaluating = false;
      renderForm();
    }
  }

  async function submitSimple() {
    if (evaluating) return;
    if (simpleForm.powerRank === "" || simpleForm.powerRank == null || isNaN(Number(simpleForm.powerRank))) {
      toast(t("Enter the gem's Power Rank."), true);
      return;
    }
    evaluating = true;
    renderForm();
    try {
      const body = {
        tier: Number(simpleForm.tier), type: Number(simpleForm.type),
        power_rank: Number(simpleForm.powerRank), level: Number(simpleForm.level),
      };
      simpleResult = await apiPost("/site/gems/evaluate-simple", body);
      pushHistorySimple(simpleResult);
      renderResults();
    } catch (e) {
      toast(t("Could not evaluate gem") + ": " + e.message, true);
    } finally {
      evaluating = false;
      renderForm();
    }
  }

  function recordHistory(entry) {
    history = [entry, ...history.filter((x) => x.hash !== entry.hash)].slice(0, MAX_HISTORY);
    saveHistory();
    renderHistory();
  }
  function pushHistoryFull(res) {
    recordHistory({
      mode: "full",
      hash: `full-${res.tier}-${res.type}-${res.level}-${(res.stats || []).map((s) => `${s.type}:${s.entered_value}:${s.containers}`).join(",")}`,
      summary: {
        quality_percent: res.quality_percent, pr: res.calculated_power_rank,
        tier: res.tier, type: res.type, type_name: res.type_name, level: res.level,
        stat_names: (res.stats || []).map((s) => s.display_name),
      },
      form: { tier: form.tier, type: form.type, level: form.level, autoGuess: form.autoGuess, stats: form.stats.map((s) => ({ type: s.type, value: s.value, extra: s.extra })) },
    });
  }
  function pushHistorySimple(res) {
    recordHistory({
      mode: "simple",
      hash: `simple-${res.tier}-${res.type}-${res.level}-${res.power_rank}`,
      summary: {
        quality_percent: res.quality_percent, pr: res.power_rank,
        tier: res.tier, type: res.type, type_name: res.type_name, level: res.level,
        stat_names: [], pr_label: true,
      },
      simpleForm: { tier: res.tier, type: res.type, powerRank: res.power_rank, level: res.level },
    });
  }

  // ── Render: results ──────────────────────────────────────────────────────
  function renderResults() {
    elResults.textContent = "";
    const active = mode === "simple" ? simpleResult : result;
    if (!active) {
      elResults.appendChild(h("div", { class: "ge-placeholder" },
        h("i", { class: "fa-solid fa-gem" }),
        h("p", {}, t(mode === "simple"
          ? "Enter your gem's Power Rank and hit Estimate to see its quality and focus plan."
          : "Enter your gem's details and hit Evaluate to see its quality, Power Rank and focus plan."))));
      return;
    }
    if (mode === "simple") renderSimpleResults();
    else renderFullResults();
  }

  // Shared focus-plan card (both modes carry an identical focus_totals shape).
  function appendFocusPlan(r) {
    const hc = r.headline_cost;
    if (hc && hc.total > 0) {
      elResults.appendChild(h("div", { class: "ge-total-cost" },
        h("i", { class: "fa-solid fa-coins" }),
        h("span", {}, `${t("Total to perfect")} (${t(hc.label)}): `),
        h("strong", {}, `${hc.total} ${t("focuses")}`)));
    }
    const anyCost = Object.values(r.focus_totals || {}).some((p) => p.total > 0);
    const planCard = h("div", { class: "ge-focus-card" },
      h("div", { class: "ge-focus-title" }, t("Focuses to finish upgrading")));
    if (!anyCost) {
      planCard.appendChild(h("div", { class: "ge-focus-empty" },
        h("img", { class: "ge-focus-gem", src: gemIcon(r.type), alt: "" }),
        h("div", {},
          h("strong", {}, t("No more focuses needed")),
          h("span", {}, t("This gem is already fully upgraded.")))));
    } else {
      const grid = h("div", { class: "ge-focus-grid" });
      ["optimized_all", "optimized_precise_rough", "rough_only"].forEach((key) => {
        const plan = r.focus_totals[key];
        if (!plan) return;
        const tile = h("div", { class: "ge-focus-tile" + (hc && key === hc.key ? " recommended" : "") });
        tile.appendChild(h("div", { class: "ge-focus-tile-head" },
          h("div", { class: "ge-focus-label" }, t(plan.label)),
          h("div", { class: "ge-focus-total" }, `${plan.total} ${t("focuses")}`)));
        const chips = h("div", { class: "ge-focus-chips" });
        ["superior", "precise", "rough"].forEach((fk) => {
          if (!plan[fk]) return;
          chips.appendChild(h("div", { class: "ge-focus-chip" },
            h("img", { src: FOCUS_ICON[fk], alt: "" }),
            h("span", {}, `${plan[fk]} ${t(FOCUS_LABEL[fk])}`)));
        });
        tile.appendChild(chips);
        const rec = h("div", { class: "ge-recipe-chips" });
        Object.entries(plan.recipe_totals || {}).forEach(([mk, amt]) => {
          rec.appendChild(h("div", { class: "ge-recipe-chip" + (mk === "flux" ? " flux" : "") },
            h("img", { src: materialIcon(mk), alt: "" }),
            h("span", {}, `${fmtNum(amt)} ${t(MATERIAL_LABEL[mk] || mk)}`)));
        });
        tile.appendChild(rec);
        grid.appendChild(tile);
      });
      planCard.appendChild(grid);
    }
    elResults.appendChild(planCard);
  }

  function renderFullResults() {
    const r = result;
    elResults.appendChild(h("div", { class: "ge-headline" + (r.has_issues ? " has-issues" : "") },
      h("img", { class: "ge-headline-icon", src: gemIcon(r.type), alt: "" }),
      h("div", { class: "ge-headline-metrics" },
        h("div", { class: "ge-metric" }, h("span", {}, t("Quality")), h("strong", {}, r.quality_percent.toFixed(2) + "%")),
        h("div", { class: "ge-metric" }, h("span", {}, t("Power Rank")), h("strong", {}, fmtNum(r.calculated_power_rank)))),
      h("div", { class: "ge-headline-tags" },
        h("span", { class: "ge-tag" }, t(r.type_name)),
        h("span", { class: "ge-tag" }, t(r.element_name)),
        r.restriction_name && r.restriction_name !== "Any" ? h("span", { class: "ge-tag" }, t(r.restriction_name)) : null)));

    if (r.has_issues && r.issues.length) {
      elResults.appendChild(h("div", { class: "ge-warning" },
        h("strong", {}, h("i", { class: "fa-solid fa-triangle-exclamation" }), " " + t("Input warning")),
        ...r.issues.map((i) => h("div", {}, t(i)))));
    }

    appendFocusPlan(r);

    const table = h("div", { class: "ge-stat-table" },
      h("div", { class: "ge-stat-tr ge-stat-th" },
        h("span", {}, t("Stat")), h("span", {}, t("Value")),
        h("span", {}, t("Containers")), h("span", {}, t("Quality"))));
    r.stats.forEach((s) => {
      table.appendChild(h("div", { class: "ge-stat-tr" + (s.is_within_range ? "" : " invalid") },
        h("span", {}, t(s.display_name)),
        h("span", {}, fmtNum(s.entered_value)),
        h("span", {}, s.containers),
        h("span", {}, s.quality_percent.toFixed(2) + "%",
          s.is_within_range ? null : h("i", { class: "fa-solid fa-triangle-exclamation ge-stat-flag", title: t("Outside the valid range for this level and proc spread") }))));
    });
    elResults.appendChild(table);
  }

  function renderSimpleResults() {
    const r = simpleResult;
    elResults.appendChild(h("div", { class: "ge-headline" + (r.is_within_range ? "" : " has-issues") },
      h("img", { class: "ge-headline-icon", src: gemIcon(r.type), alt: "" }),
      h("div", { class: "ge-headline-metrics" },
        h("div", { class: "ge-metric" }, h("span", {}, t("Est. quality")), h("strong", {}, r.quality_percent.toFixed(2) + "%")),
        h("div", { class: "ge-metric" }, h("span", {}, t("Power Rank")), h("strong", {}, fmtNum(r.power_rank)))),
      h("div", { class: "ge-headline-tags" },
        h("span", { class: "ge-tag" }, t(r.tier_name)),
        h("span", { class: "ge-tag" }, t(r.type_name)),
        h("span", { class: "ge-tag" }, t("Lv") + " " + r.level))));

    // PR range meta + within-range warning
    elResults.appendChild(h("div", { class: "ge-meta-row" },
      h("div", { class: "ge-meta" }, h("span", {}, t("Power Rank range")), h("strong", {}, `${fmtNum(r.min_power_rank)} – ${fmtNum(r.max_power_rank)}`)),
      h("div", { class: "ge-meta" }, h("span", {}, t("Estimated quality")), h("strong", {}, r.quality_percent.toFixed(2) + "%"))));
    if (!r.is_within_range) {
      elResults.appendChild(h("div", { class: "ge-warning" },
        h("strong", {}, h("i", { class: "fa-solid fa-triangle-exclamation" }), " " + t("Out of range")),
        h("div", {}, t("This Power Rank is outside what this gem's tier / type / level can normally reach - double-check the entered values."))));
    }

    appendFocusPlan(r);
  }

  // ── Render: history ──────────────────────────────────────────────────────
  function renderHistory() {
    if (!elHistory) return;
    elHistory.textContent = "";
    const head = h("div", { class: "ge-history-head" },
      h("span", {}, h("i", { class: "fa-solid fa-clock-rotate-left" }), " " + t("Recent gems")));
    if (history.length) {
      head.appendChild(h("button", { class: "ge-history-clear", title: t("Clear history"), onClick: () => { history = []; saveHistory(); renderHistory(); } },
        h("i", { class: "fa-solid fa-trash-can" })));
    }
    elHistory.appendChild(head);

    if (!history.length) {
      elHistory.appendChild(h("div", { class: "ge-history-empty" }, t("Evaluate a gem to save it here.")));
      return;
    }
    const list = h("div", { class: "ge-history-list" });
    history.forEach((entry) => {
      const s = entry.summary;
      const isSimple = entry.mode === "simple";
      const sub = isSimple ? t("Power Rank estimate") : (s.stat_names || []).map(t).join(" · ");
      list.appendChild(h("div", { class: "ge-history-item" },
        h("button", { class: "ge-history-restore", title: t("Click to re-evaluate this gem"), onClick: () => restore(entry) },
          h("img", { class: "ge-history-icon", src: gemIcon(s.type), alt: "" }),
          h("div", { class: "ge-history-body" },
            h("div", { class: "ge-history-title" },
              `${s.quality_percent.toFixed(1)}% · ${t("PR")} ${fmtNum(s.pr != null ? s.pr : s.calculated_power_rank)}`,
              h("span", { class: "ge-history-mode" }, isSimple ? t("Simple") : t("Full"))),
            h("div", { class: "ge-history-sub" }, `${t(tierName(s.tier))} · ${t(s.type_name)} · ${t("Lv")} ${s.level}`),
            h("div", { class: "ge-history-stats" }, sub))),
        h("button", { class: "ge-history-del", title: t("Delete"), onClick: () => { history = history.filter((x) => x.hash !== entry.hash); saveHistory(); renderHistory(); } },
          h("i", { class: "fa-solid fa-xmark" }))));
    });
    elHistory.appendChild(list);
  }

  function restore(entry) {
    if (entry.mode === "simple") {
      const f = entry.simpleForm;
      mode = "simple";
      simpleForm.tier = f.tier; simpleForm.type = f.type; simpleForm.powerRank = f.powerRank; simpleForm.level = f.level;
    } else {
      const f = entry.form;
      mode = "full";
      form.tier = f.tier; form.type = f.type; form.level = f.level; form.autoGuess = f.autoGuess;
      form.stats = f.stats.map((s) => ({ type: s.type, value: s.value, extra: s.extra }));
    }
    try { localStorage.setItem(MODE_KEY, mode); } catch (e) { /* non-fatal */ }
    renderForm();
    if (mode === "full") scheduleRanges();
    window.scrollTo({ top: 0, behavior: "smooth" });
    submit();
  }

  // ── Init ─────────────────────────────────────────────────────────────────
  async function init() {
    elForm = document.getElementById("ge-form");
    elResults = document.getElementById("ge-results");
    elHistory = document.getElementById("ge-history");
    elToastHost = document.getElementById("ge-toast-host");
    if (!elForm) return;

    loadHistory();
    // drop any pre-mode history entries that lack the new shape (defensive)
    history = history.filter((e) => e && e.summary && (e.mode === "simple" ? e.simpleForm : e.form));
    try { const m = localStorage.getItem(MODE_KEY); if (m === "simple" || m === "full") mode = m; } catch (e) { /* non-fatal */ }
    try {
      lookups = await apiGet("/site/gems/lookups");
    } catch (e) {
      elForm.appendChild(h("div", { class: "ge-warning" }, t("Could not load gem reference data. Please reload.")));
      return;
    }
    renderForm();
    renderResults();
    if (mode === "full") fetchRanges();
  }

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init);
  else init();
})();
