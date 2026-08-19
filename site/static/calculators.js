/* =========================================================================
   Calculators - Power Rank, Mastery, Magic Find & Light.
   Vanilla-JS re-implementation of Better Trove Tools' Vue calculators. All the
   maths is client-side (ported 1:1 from BTT's calculators.js) off the static
   stat tables in /static/assets/data/stats/*.json. The Magic Find tab's
   optional star-chart preview uses the /site/gems/parse-star-chart proxy.
   State persists to localStorage. CSP-clean: no eval, no inline handlers.
   ========================================================================= */
(function () {
  "use strict";
  const toast = window.BTTToast.show;
  const { h } = window.BTTDom;

  const t = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);
  const STORAGE_KEY = "troveapi.calculators.v1";

  // Callbacks that refresh the DERIVED display (totals, summary chips, badges)
  // without rebuilding the DOM. Re-rendering the body on every `input` event
  // destroyed the very control being dragged or typed into, so focus was lost
  // on each keystroke and sliders stopped tracking the mouse. Rebuilt only on a
  // structural change (tab switch, data load).
  let liveUpdaters = [];

  // 4 decimals, not the locale default of 3: the buffed ally lands on a
  // fraction (98.25 PR, 909.5625 light) the default would clip.
  const num = (v) => (Number(v) || 0).toLocaleString(undefined, { maximumFractionDigits: 4 });
  const clampN = (v, lo, hi) => Math.max(lo, Math.min(hi, v));
  const sliderFill = (v, lo, hi) => (hi <= lo ? "0%" : clampN(((v - lo) / (hi - lo)) * 100, 0, 100) + "%");

  // Blessing of the Lilypad - the ally buff. The stat tables already hold the
  // level-30 ally values (75 PR, 787.5 light), so the buff multiplies those.
  // Magic Find has no confirmed buff component, so its Ally row is left alone.
  const LILYPAD = { "Power Rank": 1.31, Light: 1.155 };
  const isAlly = (item) => String(item && item.name).trim() === "Ally";

  const TABS = [
    { key: "pr", label: "Power Rank", icon: "fa-star", color: "#fbc02d" },
    { key: "mastery", label: "Mastery", icon: "fa-crown", color: "#ff9800" },
    { key: "mf", label: "Magic Find", icon: "fa-gem", color: "var(--accent-blue)" },
    { key: "light", label: "Light", icon: "fa-sun", color: "#00bcd4" },
  ];

  // ── State ────────────────────────────────────────────────────────────────
  let activeTab = "pr";
  let prData = [];
  let mfData = [];
  let lightData = [];
  let troveMastery = 900;
  let geodeMastery = 100;
  let starChartCode = "";
  let starChartMf = { flat: 0, pct: 0, pathsCount: 0, error: false };
  let lilypad = true;

  // An ally row's value with the Lilypad buff folded in; every other row is
  // returned untouched - the buff only ever scales the ally's own stats.
  const withLilypad = (item, value, statClass) =>
    (lilypad && isAlly(item) ? value * LILYPAD[statClass] : value);

  let elTabs, elBody;

  // ── Persistence ──────────────────────────────────────────────────────────
  function keyOf(item) { return item.name || item.type; }
  function save() {
    const snap = {
      activeTab, troveMastery, geodeMastery, starChartCode, lilypad,
      pr: prData.reduce((o, i) => (o[keyOf(i)] = i.currentValue, o), {}),
      mf: mfData.reduce((o, i) => (o[keyOf(i)] = i.currentValue, o), {}),
      light: lightData.reduce((o, i) => (o[keyOf(i)] = i.currentValue, o), {}),
    };
    try { localStorage.setItem(STORAGE_KEY, JSON.stringify(snap)); } catch (e) { /* non-fatal */ }
  }
  function restore(list, saved) {
    if (!saved) return;
    list.forEach((item) => {
      const k = keyOf(item);
      if (!Object.prototype.hasOwnProperty.call(saved, k)) return;
      item.currentValue = (item.type && item.type.includes("switch")) ? !!saved[k] : Number(saved[k]) || 0;
    });
  }

  // ── Helpers shared with BTT ──────────────────────────────────────────────
  const isPercentBonusValue = (value) => {
    const n = Number(value);
    return Number.isFinite(n) && !Number.isInteger(n) && Math.abs(n) < 1;
  };
  const isLightGeodeMastery = (item) => item && item.type === "slider" && String(item.name || "").toLowerCase().includes("geode mastery");
  const getLightSliderMax = (item) => (isLightGeodeMastery(item) ? 150 : Number(item.max || item.value || 0));
  const getLightNumberMax = (item) => (isLightGeodeMastery(item) ? 200 : Number(item.max || item.value || 0));
  const getLightStep = (item) => { const s = Number(item && item.step); return Number.isFinite(s) && s > 0 ? s : 1; };
  const getLightApplied = (item) => {
    const n = Number(item.currentValue || 0);
    return isLightGeodeMastery(item) ? clampN(n, 0, 100) * 10 : n;
  };

  // ── Compute: Power Rank ──────────────────────────────────────────────────
  function totalPR() {
    let total = 0;
    prData.forEach((item) => {
      if (item.type === "switch") total += item.currentValue ? withLilypad(item, item.value, "Power Rank") : 0;
      else if (item.type === "pr_mastery") { const c = Math.min(item.currentValue || 0, 1000); total += Math.min(c, 500) * 4 + Math.max(0, c - 500) * 1; }
      else if (item.type === "pr_geode_mastery") total += Math.min(item.currentValue || 0, 100) * 5;
      else total += item.currentValue || 0;
    });
    return total;
  }
  function prBadge(item) {
    let v = 0;
    if (item.type === "pr_mastery") { const c = Math.min(item.currentValue || 0, 1000); v = Math.min(c, 500) * 4 + Math.max(0, c - 500) * 1; }
    else if (item.type === "pr_geode_mastery") v = Math.min(item.currentValue || 0, 100) * 5;
    else if (item.type === "switch") v = item.currentValue ? withLilypad(item, item.value, "Power Rank") : 0;
    else v = item.currentValue || 0;
    return "+" + num(v) + " PR";
  }

  // ── Compute: Magic Find ──────────────────────────────────────────────────
  function mfStats() {
    let flat = 0, bonus = 0, patron = 1;
    mfData.forEach((item) => {
      let val = 0;
      if (item.type.includes("switch")) val = item.currentValue ? item.value : 0;
      else if (item.type === "mastery") val = Math.max(0, (item.currentValue || 0) - 500);
      else val = item.currentValue || 0;
      if (item.type === "patron_switch") patron = item.currentValue ? item.value / 100 + 1 : 1;
      else if (item.percentage) bonus += val;
      else flat += val;
    });
    const starFlat = starChartMf.flat || 0;
    const starPct = starChartMf.pct || 0;
    const totalFlat = flat + starFlat;
    const totalBonus = bonus + starPct;
    return { flat: totalFlat, bonus: totalBonus, patron, total: Math.floor(totalFlat * (1 + totalBonus / 100) * patron), starFlat, starPct };
  }
  function mfBadge(item) {
    if (item.type === "patron_switch") return "x" + item.value;
    let v;
    if (item.type === "switch") v = item.value;
    else if (item.type === "mastery") v = Math.max(0, (item.currentValue || 0) - 500);
    else v = item.currentValue || 0;
    return item.percentage ? "+" + v + "% MF" : "+" + num(v) + " MF";
  }

  // ── Compute: Light ───────────────────────────────────────────────────────
  function lightStats() {
    let flat = 0, bonusMult = 1;
    lightData.forEach((item) => {
      const raw = Number(item.value || 0);
      const applied = withLilypad(item, item.type === "switch" ? (item.currentValue ? raw : 0) : getLightApplied(item), "Light");
      if (isPercentBonusValue(raw)) bonusMult += applied;
      else flat += applied;
    });
    return { flat, bonusPct: Math.max(0, (bonusMult - 1) * 100), total: Math.floor(flat * bonusMult) };
  }
  function lightBadge(item) {
    const raw = Number(item.value || 0);
    const display = withLilypad(item, item.type === "switch" ? raw : (isLightGeodeMastery(item) ? getLightApplied(item) : Number(item.currentValue || 0)), "Light");
    if (isPercentBonusValue(raw)) return "+" + (Math.abs(display) * 100).toFixed(0) + "% Light";
    return "+" + num(display) + " Light";
  }

  // ── Compute: Mastery ─────────────────────────────────────────────────────
  function masteryPR() {
    const tc = Math.min(troveMastery || 0, 1000), gc = Math.min(geodeMastery || 0, 100);
    return Math.min(tc, 500) * 4 + Math.max(0, tc - 500) * 1 + gc * 5;
  }

  // ── Star chart (Magic Find) ──────────────────────────────────────────────
  let scTimer = null;
  function scheduleStarChart() { if (scTimer) clearTimeout(scTimer); scTimer = setTimeout(fetchStarChartMf, 350); }
  async function fetchStarChartMf() {
    const code = starChartCode.trim();
    if (!code) { starChartMf = { flat: 0, pct: 0, pathsCount: 0, error: false }; refresh(); return; }
    try {
      const r = await fetch(`/site/gems/parse-star-chart?code=${encodeURIComponent(code)}`, { headers: { Accept: "application/json" } });
      const data = await r.json();
      if (!data || !data.paths_count) throw new Error("empty");
      let flat = 0, pct = 0;
      Object.entries(data.stats || {}).forEach(([name, v]) => {
        const norm = String(name || "").toLowerCase().replace(/[\s_-]+/g, "");
        if (norm === "magicfind" || norm.includes("magicfind")) { flat += Number(v.flat) || 0; pct += Number(v.pct) || 0; }
      });
      starChartMf = { flat, pct, pathsCount: data.paths_count, error: false };
    } catch (e) {
      starChartMf = { flat: 0, pct: 0, pathsCount: 0, error: true };
    }
    refresh();
  }

  // ── Records sync ─────────────────────────────────────────────────────────
  let syncing = false;

  // Push the current record-high mastery levels onto every mastery field. Returns
  // the applied {trove, geode} levels (null when the record didn't carry one).
  function applyRecordMastery(rec) {
    let dt = null, dg = null;
    if (rec && rec.trove_mastery && Number.isFinite(rec.trove_mastery.level)) dt = rec.trove_mastery.level;
    if (rec && rec.geode_mastery && Number.isFinite(rec.geode_mastery.level)) dg = rec.geode_mastery.level;
    if (dt != null) {
      troveMastery = dt;
      prData.forEach((i) => { if (i.type === "pr_mastery") { i.currentValue = dt; i.max = Math.max(i.max || 0, dt); } });
      mfData.forEach((i) => { if (i.type === "mastery") { i.currentValue = dt; i.max = Math.max(i.max || 0, dt); } });
    }
    if (dg != null) {
      geodeMastery = dg;
      prData.forEach((i) => { if (i.type === "pr_geode_mastery") { i.currentValue = dg; i.max = Math.max(i.max || 0, dg); } });
    }
    return { dt, dg };
  }

  async function syncRecords() {
    if (syncing) return;
    syncing = true;
    renderTabs();
    try {
      const rec = await fetchJson("/site/leaderboards/records");
      const { dt, dg } = applyRecordMastery(rec);
      if (dt == null && dg == null) throw new Error("no record mastery");
      save();
      renderBody();
      const bits = [];
      if (dt != null) bits.push(t("Trove") + " " + num(dt));
      if (dg != null) bits.push(t("Geode") + " " + num(dg));
      toast(t("Synced to current record mastery") + " · " + bits.join(" · "));
    } catch (e) {
      toast(t("Could not sync records - the leaderboard record is unavailable right now."), true);
    } finally {
      syncing = false;
      renderTabs();
    }
  }

  // ── Render: tabs ─────────────────────────────────────────────────────────
  function renderTabs() {
    elTabs.textContent = "";
    TABS.forEach((tab) => {
      elTabs.appendChild(h("button", { class: "calc-tab" + (activeTab === tab.key ? " active" : ""), onClick: () => { activeTab = tab.key; save(); renderTabs(); renderBody(); } },
        h("i", { class: "fa-solid " + tab.icon, "aria-hidden": "true", style: { color: tab.color } }), " " + t(tab.label)));
    });
    elTabs.appendChild(h("button", {
      class: "calc-sync-btn", disabled: syncing, onClick: syncRecords,
      title: t("Set the mastery fields to the current highest Trove & Geode Mastery in the game"),
    },
      h("i", { class: "fa-solid " + (syncing ? "fa-spinner fa-spin" : "fa-arrows-rotate"), "aria-hidden": "true" }),
      " " + t(syncing ? "Syncing..." : "Sync records")));
  }

  // Generic input row (slider + number OR switch)
  function calcItem(item, cfg) {
    // cfg: { min, max, step, badge, onInput, sliderMax, numberMax }
    const name = cfg.title || t(item.name);
    const badge = h("span", { class: "calc-badge" }, cfg.badge());
    liveUpdaters.push(() => { badge.textContent = cfg.badge(); });
    const header = h("div", { class: "calc-item-header" },
      h("span", {}, name),
      badge);
    let control;
    if (item.type && item.type.includes("switch")) {
      const c = h("input", { type: "checkbox", "aria-label": name });
      c.checked = !!item.currentValue;
      c.addEventListener("change", (e) => { item.currentValue = e.target.checked; cfg.onInput(); });
      control = h("label", { class: "calc-switch" }, c, h("span", { class: "calc-switch-track" }));
    } else {
      const lo = cfg.min, hi = cfg.sliderMax;
      const range = h("input", { class: "calc-slider", type: "range", min: lo, max: hi, step: cfg.step || 1, value: item.currentValue, "aria-label": name, style: { "--val-pct": sliderFill(item.currentValue, lo, hi) } });
      const number = h("input", { class: "calc-number", type: "number", min: lo, max: cfg.numberMax, step: cfg.step || 1, value: item.currentValue, "aria-label": name });
      const sync = (v, fromNumber) => {
        item.currentValue = v;
        range.style.setProperty("--val-pct", sliderFill(v, lo, hi));
        if (fromNumber) range.value = clampN(v, lo, hi); else number.value = v;
        cfg.onInput();
      };
      range.addEventListener("input", (e) => sync(Number(e.target.value), false));
      number.addEventListener("input", (e) => sync(Number(e.target.value) || 0, true));
      number.addEventListener("change", (e) => { const v = clampN(Number(e.target.value) || 0, lo, cfg.numberMax); sync(v, true); number.value = v; });
      control = h("div", { class: "calc-slider-wrap" }, range, number);
    }
    return h("div", { class: "calc-item" + (cfg.accentClass || "") }, header, control);
  }

  // The ally buff toggle, rendered right under the Ally row of whichever tab it
  // affects. One shared flag, so switching it on either tab moves both.
  function lilypadItem(gain, unit) {
    const item = { name: "Blessing of the Lilypad", type: "switch", currentValue: lilypad };
    return calcItem(item, {
      badge: () => "+" + gain + "% " + unit,
      onInput: () => { lilypad = item.currentValue; refresh(); },
    });
  }

  // `value` and `summaryChildren` are thunks so the header can refresh its own
  // derived text in place - see `liveUpdaters`.
  function header(label, value, accent, summaryChildren) {
    const total = h("span", { class: "calc-total-value" }, value());
    const strip = summaryChildren ? h("div", { class: "calc-summary-strip" }, summaryChildren()) : null;
    liveUpdaters.push(() => {
      total.textContent = value();
      if (!strip) return;
      // The strip's chips are conditional (a Star Chart chip appears only once a
      // code is entered), so replace its children wholesale. It holds no inputs.
      strip.textContent = "";
      const kids = summaryChildren();
      for (const k of (Array.isArray(kids) ? kids : [kids])) if (k) strip.appendChild(k);
    });
    return h("div", { class: "calc-header", style: accent ? { "--calc-accent": accent } : null },
      h("div", { class: "calc-total-box" },
        h("span", { class: "calc-total-label" }, t(label)),
        total),
      strip);
  }

  // ── Render: body per tab ─────────────────────────────────────────────────
  function renderBody() {
    elBody.textContent = "";
    liveUpdaters = [];          // the nodes they targeted just went away
    if (activeTab === "pr") return renderPr();
    if (activeTab === "mastery") return renderMastery();
    if (activeTab === "mf") return renderMf();
    if (activeTab === "light") return renderLight();
  }

  // Called on every input event: persist and refresh the derived text only. The
  // controls keep their own value/fill in sync, so nothing here touches them -
  // which is what lets focus, drag state and caret position survive.
  function refresh() { save(); for (const fn of liveUpdaters) fn(); }

  function renderPr() {
    const strip = () => [h("span", { class: "calc-chip" }, h("b", {}, t("Mastery PR") + " "), num(prBreakdownMasteryPR()))];
    elBody.appendChild(header("Total Power Rank", () => num(totalPR()), "#fbc02d", strip));
    const grid = h("div", { class: "calc-grid" });
    prData.forEach((item) => {
      const isMastery = String(item.type).includes("mastery");
      grid.appendChild(calcItem(item, {
        min: isMastery ? 1 : 0,
        sliderMax: item.max || item.value,
        numberMax: item.type === "pr_mastery" ? 2000 : (item.type === "pr_geode_mastery" ? 200 : item.value),
        step: 1, badge: () => prBadge(item), onInput: refresh,
      }));
      if (isAlly(item)) grid.appendChild(lilypadItem("31", "PR"));
    });
    elBody.appendChild(grid);
  }
  function prBreakdownMasteryPR() {
    let v = 0;
    prData.forEach((item) => {
      if (item.type === "pr_mastery") { const c = Math.min(item.currentValue || 0, 1000); v += Math.min(c, 500) * 4 + Math.max(0, c - 500) * 1; }
      else if (item.type === "pr_geode_mastery") v += Math.min(item.currentValue || 0, 100) * 5;
    });
    return v;
  }

  function renderMastery() {
    const strip = () => h("div", { class: "calc-mastery-strip" },
      masteryBox("+" + (Math.min(troveMastery || 0, 500) * 0.2).toFixed(1) + "%", "fa-burst", "Damage"),
      masteryBox("+" + (Math.min(troveMastery || 0, 500) * 0.6).toFixed(1) + "%", "fa-heart", "Health"),
      masteryBox("+" + num(Math.min(geodeMastery || 0, 100) * 10), "fa-sun", "Light", "#00bcd4"),
      masteryBox("+" + Math.max(0, Math.min(troveMastery || 0, 1000) - 500), "fa-gem", "Magic Find"));
    elBody.appendChild(header("Total Mastery Power Rank", () => num(masteryPR()), "#ff9800", strip));

    const grid = h("div", { class: "calc-grid" });
    grid.appendChild(masterySlider("Trove Mastery", "fa-crown", "#fbc02d", "Soft cap 1000", troveMastery, Math.max(1100, troveMastery), 2000, (v) => { troveMastery = clampN(v, 0, 2000); refresh(); }));
    grid.appendChild(masterySlider("Geode Mastery", "fa-gem", "#00bcd4", "Soft cap 100", geodeMastery, Math.max(150, geodeMastery), 200, (v) => { geodeMastery = clampN(v, 0, 200); refresh(); }));
    elBody.appendChild(grid);
  }
  function masteryBox(val, icon, label, color) {
    return h("div", { class: "calc-mastery-box" },
      h("span", { class: "calc-mastery-val", style: color ? { color } : null }, val),
      h("span", { class: "calc-mastery-label" }, h("i", { class: "fa-solid " + icon, "aria-hidden": "true" }), " " + t(label)));
  }
  function masterySlider(label, icon, color, badge, value, sliderMax, numberMax, onInput) {
    const range = h("input", { class: "calc-slider", type: "range", min: 0, max: sliderMax, value, "aria-label": t(label), style: { "--val-pct": sliderFill(value, 0, sliderMax) } });
    const number = h("input", { class: "calc-number", type: "number", min: 0, max: numberMax, value, "aria-label": t(label) });
    // The track used to stretch past its start value only because a re-render
    // recomputed max from the new value. Nothing re-renders now, so grow it here
    // - otherwise typing 1500 would peg the handle at the old 1100 ceiling.
    let hi = sliderMax;
    const grow = (v) => { if (v > hi) { hi = v; range.max = hi; } };
    const sync = (v) => { grow(v); range.style.setProperty("--val-pct", sliderFill(v, 0, hi)); onInput(v); };
    range.addEventListener("input", (e) => { number.value = e.target.value; sync(Number(e.target.value)); });
    number.addEventListener("input", (e) => {
      const v = Number(e.target.value) || 0;
      grow(v);
      range.value = clampN(v, 0, hi);
      sync(v);
    });
    return h("div", { class: "calc-item accent", style: { "--calc-accent": color } },
      h("div", { class: "calc-item-header" },
        h("span", {}, h("i", { class: "fa-solid " + icon, "aria-hidden": "true", style: { color } }), " " + t(label)),
        h("span", { class: "calc-badge" }, t(badge))),
      h("div", { class: "calc-slider-wrap" }, range, number));
  }

  function renderMf() {
    const strip = () => {
      const s = mfStats();
      const chips = [
        h("span", { class: "calc-chip" }, h("b", {}, t("Base MF") + " "), num(s.flat)),
        h("span", { class: "calc-chip" }, h("b", {}, t("Bonus") + " "), "+" + s.bonus + "%"),
      ];
      if (s.starFlat > 0 || s.starPct > 0) {
        const parts = [];
        if (s.starFlat > 0) parts.push("+" + num(s.starFlat));
        if (s.starPct > 0) parts.push("+" + s.starPct + "%");
        chips.push(h("span", { class: "calc-chip" }, h("b", {}, t("Star Chart") + " "), parts.join(" / ")));
      }
      if (s.patron > 1) chips.push(h("span", { class: "calc-chip" }, h("b", {}, t("Patron") + " "), "x" + s.patron));
      return chips;
    };
    elBody.appendChild(header("Total Magic Find", () => num(mfStats().total), "var(--accent-blue)", strip));

    const grid = h("div", { class: "calc-grid" });
    mfData.forEach((item) => {
      const isMastery = item.type === "mastery";
      grid.appendChild(calcItem(item, {
        min: isMastery ? 1 : 0, sliderMax: item.max || item.value, numberMax: item.max || item.value,
        step: 1, badge: () => mfBadge(item), onInput: refresh,
      }));
    });
    // Star chart item
    grid.appendChild(starChartItem());
    elBody.appendChild(grid);
  }
  function starChartItem() {
    const input = h("input", { class: "calc-number calc-sc-input", type: "text", value: starChartCode, "aria-label": t("Star Chart build code"), placeholder: t("Paste a star-chart build code") });
    input.addEventListener("input", (e) => { starChartCode = e.target.value.trim(); save(); scheduleStarChart(); });
    // The breakdown node always exists and is filled in place. Rebuilding the
    // item when the parse resolved would have yanked the text field out from
    // under the caret 350ms into typing - the same defect as the sliders.
    const bd = h("div", { class: "calc-sc-breakdown" });
    const paintBreakdown = () => {
      bd.textContent = "";
      bd.hidden = !starChartCode;
      if (!starChartCode) return;
      if (starChartMf.error) {
        bd.appendChild(h("span", { class: "err" }, h("i", { class: "fa-solid fa-triangle-exclamation", "aria-hidden": "true" }), " " + t("Invalid build code")));
        return;
      }
      const parts = [];
      if (starChartMf.flat > 0) parts.push("+" + num(starChartMf.flat));
      if (starChartMf.pct > 0) parts.push("+" + starChartMf.pct + "%");
      bd.appendChild(h("span", {},
        h("b", {}, t("Nodes") + ": "), starChartMf.pathsCount, "  ",
        h("b", {}, t("Magic Find") + ": "), parts.length ? parts.join(" / ") : t("none detected")));
    };
    paintBreakdown();
    liveUpdaters.push(paintBreakdown);
    return h("div", { class: "calc-item accent", style: { "--calc-accent": "#ff9800" } },
      h("div", { class: "calc-item-header" },
        h("span", {}, h("i", { class: "fa-solid fa-chart-network", "aria-hidden": "true", style: { color: "#ff9800" } }), " " + t("Star Chart")),
        h("span", { class: "calc-badge" }, t("MF only"))),
      h("div", { class: "calc-sc-controls" }, input),
      bd);
  }

  function renderLight() {
    const strip = () => {
      const s = lightStats();
      const chips = [h("span", { class: "calc-chip" }, h("b", {}, t("Base Light") + " "), num(s.flat))];
      if (s.bonusPct > 0) chips.push(h("span", { class: "calc-chip" }, h("b", {}, t("Bonus") + " "), "+" + s.bonusPct.toFixed(0) + "%"));
      return chips;
    };
    elBody.appendChild(header("Total Light", () => num(lightStats().total), "#00bcd4", strip));

    const grid = h("div", { class: "calc-grid" });
    lightData.forEach((item) => {
      grid.appendChild(calcItem(item, {
        min: 0, sliderMax: getLightSliderMax(item), numberMax: getLightNumberMax(item),
        step: getLightStep(item), badge: () => lightBadge(item), onInput: refresh,
      }));
      if (isAlly(item)) grid.appendChild(lilypadItem("15.5", t("Light")));
    });
    elBody.appendChild(grid);
  }

  // ── Data loading ─────────────────────────────────────────────────────────
  async function fetchJson(url) {
    const r = await fetch(url, { headers: { Accept: "application/json" } });
    if (!r.ok) throw new Error("HTTP " + r.status);
    return r.json();
  }

  async function init() {
    elTabs = document.getElementById("calc-tabs");
    elBody = document.getElementById("calc-body");
    if (!elTabs) return;

    let saved = null;
    try { saved = JSON.parse(localStorage.getItem(STORAGE_KEY) || "null"); } catch (e) { saved = null; }

    // Default the mastery fields to the current record-high mastery (from the
    // leaderboards records endpoint) rather than a hardcoded 900/100. Non-fatal:
    // if records is unavailable (leaderboards off / cold cache) we keep 900/100.
    let defTrove = 900, defGeode = 100;
    try {
      const rec = await fetchJson("/site/leaderboards/records");
      if (rec && rec.trove_mastery && Number.isFinite(rec.trove_mastery.level)) defTrove = rec.trove_mastery.level;
      if (rec && rec.geode_mastery && Number.isFinite(rec.geode_mastery.level)) defGeode = rec.geode_mastery.level;
    } catch (e) { /* records unavailable - keep the classic 900/100 defaults */ }
    troveMastery = defTrove;
    geodeMastery = defGeode;

    try {
      const [pr, mf, light] = await Promise.all([
        fetchJson("/static/assets/data/stats/power_rank.json"),
        fetchJson("/static/assets/data/stats/magic_find.json"),
        fetchJson("/static/assets/data/stats/light.json"),
      ]);
      prData = [
        { name: "Trove Mastery", type: "pr_mastery", max: Math.max(1100, defTrove), default: defTrove },
        { name: "Geode Mastery", type: "pr_geode_mastery", max: Math.max(150, defGeode), default: defGeode },
        ...pr,
      ].map((item) => ({ ...item, currentValue: item.type === "switch" ? true : (item.default !== undefined ? item.default : (item.value || 0)) }));
      mfData = [
        { name: "Mastery", type: "mastery", percentage: false, max: Math.max(1000, defTrove), default: defTrove },
        ...mf,
        { name: "Patron", type: "patron_switch", percentage: true, value: 100, default_checked: false },
      ].map((item) => ({ ...item, currentValue: item.type.includes("switch") ? (item.default_checked !== undefined ? item.default_checked : true) : (item.default !== undefined ? item.default : (item.value || 0)) }));
      lightData = light.map((item) => ({
        ...item,
        currentValue: item.type === "switch" ? item.perm === true
          : (isLightGeodeMastery(item) ? (item.perm === true ? 100 : 0) : (item.perm === true ? Number(item.value || 0) : 0)),
      }));
    } catch (e) {
      elBody.appendChild(h("div", { class: "calc-error" }, t("Could not load calculator data. Please reload.")));
      renderTabs();
      return;
    }

    if (saved) {
      if (typeof saved.activeTab === "string") activeTab = saved.activeTab;
      if (saved.troveMastery !== undefined) troveMastery = Number(saved.troveMastery) || 0;
      if (saved.geodeMastery !== undefined) geodeMastery = Number(saved.geodeMastery) || 0;
      if (typeof saved.starChartCode === "string") starChartCode = saved.starChartCode;
      if (typeof saved.lilypad === "boolean") lilypad = saved.lilypad;
      restore(prData, saved.pr); restore(mfData, saved.mf); restore(lightData, saved.light);
    }

    renderTabs();
    renderBody();
    if (starChartCode) fetchStarChartMf();
  }

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init);
  else init();
})();
