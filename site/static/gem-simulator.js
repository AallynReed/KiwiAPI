/* =========================================================================
   Gem Simulator - client-only page.
   Vanilla-JS re-implementation of Better Trove Tools' Vue gem simulator. All
   gem math runs in window.GemEngine (see gem-engine.js, a faithful JS port of
   the Python model); this file is UI + state only. State persists to the
   browser's localStorage - no backend, no /v1 API. CSP-clean: no eval, no
   inline handlers, no external hosts.
   ========================================================================= */
(function () {
  "use strict";

  if (!window.GemEngine) {
    console.error("GemEngine failed to load - gem-simulator cannot start.");
    return;
  }

  // ── i18n + small helpers (mirrors star-chart.js) ───────────────────────
  const t = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);
  const fmt = (s, p) => {
    let out = t(s);
    if (p) for (const k in p) out = out.split("{" + k + "}").join(p[k]);
    return out;
  };

  const ASSET = "/static/assets/gems";
  const STORAGE_KEY = "troveapi.gemSimulator.v1";
  const INV_SIZE = 104;
  const EQ_SIZE = 12;

  const ELEMENT_COLORS = { Fire: "#e57373", Water: "#64b5f6", Air: "#fff59d", Cosmic: "#4db6ac" };
  const ELEMENT_DEFAULT_COLOR = "#888888";

  // DOM builder: h('div', {class, onClick, dataset, style, html}, ...children)
  function h(tag, attrs) {
    const e = document.createElement(tag);
    if (attrs) {
      for (const k in attrs) {
        const v = attrs[k];
        if (v == null || v === false) continue;
        if (k === "class") e.className = v;
        else if (k === "html") e.innerHTML = v;
        else if (k === "style" && typeof v === "object") Object.assign(e.style, v);
        else if (k === "dataset") Object.assign(e.dataset, v);
        else if (k.slice(0, 2) === "on" && typeof v === "function") e.addEventListener(k.slice(2).toLowerCase(), v);
        else e.setAttribute(k, v);
      }
    }
    for (let i = 2; i < arguments.length; i++) {
      const kids = arguments[i];
      (Array.isArray(kids) ? kids : [kids]).forEach((kid) => {
        if (kid == null || kid === false) return;
        e.appendChild(typeof kid === "object" ? kid : document.createTextNode(String(kid)));
      });
    }
    return e;
  }
  const clone = (o) => JSON.parse(JSON.stringify(o));

  // ── State ───────────────────────────────────────────────────────────────
  let lookups = {};
  let inventory = new Array(INV_SIZE).fill(null);
  let equipped = new Array(EQ_SIZE).fill(null);
  const primordialToggles = {}; // { [elementId]: bool }
  let selected = null;
  let selectedSource = null; // { pane, idx } | null
  let selectedStatIdx = 0;
  let selectedActionKey = null; // 'augment-1' | 'augment-2' | 'augment-3' | 'spark' | 'flare'
  const creatorParams = { type: "", tier: "", element: "", restriction: "", level: 1, augmentNull: true, augment: 0 };
  let dragState = { pane: null, idx: -1, gem: null };

  // DOM refs
  let elEquipped, elPrimordial, elTotals, elForge, elDetail, elInventory, elTrash, elRecalc, elTooltip, elModal, elToastHost;

  // ── Lookup name helpers ─────────────────────────────────────────────────
  const nameById = (obj, id) => {
    const found = Object.entries(obj || {}).find(([, v]) => String(v) === String(id));
    return found ? found[0] : null;
  };
  const elementName = (id) => nameById(lookups.elements, id) || "Unknown";
  const tierName = (id) => nameById(lookups.tiers, id) || id;
  const typeName = (id) => nameById(lookups.types, id) || id;
  const elementColor = (id) => ELEMENT_COLORS[elementName(id)] || ELEMENT_DEFAULT_COLOR;
  const elementsSorted = () => Object.entries(lookups.elements || {}).sort((a, b) => a[1] - b[1]);

  const statName = (gem, i) => Object.keys(gem.stat_values[i])[0] || "Stat " + (i + 1);
  const statValue = (gem, i) => gem.stat_values[i][statName(gem, i)];
  const formatStat = (v) => (Math.round(v * 100) / 100).toLocaleString();
  const barColor = (v) => (v < 0.33 ? "#d32f2f" : v < 0.66 ? "#fbc02d" : "#34d058");

  // ── Asset URLs ──────────────────────────────────────────────────────────
  const tierBg = (gem) => `${ASSET}/gem_tiers/${gem.tier}.png`;
  const gemImg = (gem) => `${ASSET}/gem_types/${gem.type}/elements/${gem.element}.png`;
  const placeholderImg = (elementId, typeRestriction) => `${ASSET}/gem_types/${typeRestriction}/elements/${elementId}.png`;
  const augmentImg = (id) => `${ASSET}/augments/${id}.png`;
  const modifierImg = (key) => `${ASSET}/modifiers/${key}.png`;

  // ── Persistence ─────────────────────────────────────────────────────────
  let saveTimer = null;
  function save() {
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(() => {
      try {
        localStorage.setItem(STORAGE_KEY, JSON.stringify({ inventory, equipped, toggles: primordialToggles }));
      } catch (e) { /* quota / private mode - non-fatal */ }
    }, 200);
  }
  function loadStorage() {
    let data = null;
    try { data = JSON.parse(localStorage.getItem(STORAGE_KEY) || "null"); } catch (e) { data = null; }
    if (data) {
      if (Array.isArray(data.inventory)) {
        inventory = data.inventory.slice(0, INV_SIZE);
        while (inventory.length < INV_SIZE) inventory.push(null);
      }
      if (Array.isArray(data.equipped)) {
        equipped = data.equipped.slice(0, EQ_SIZE);
        while (equipped.length < EQ_SIZE) equipped.push(null);
      }
      if (data.toggles) for (const k in data.toggles) primordialToggles[k] = !!data.toggles[k];
    }
    // default every element's primordial toggle ON
    Object.values(lookups.elements || {}).forEach((id) => {
      if (primordialToggles[id] === undefined) primordialToggles[id] = true;
    });
  }

  // ── Toast + confirm modal (self-contained) ──────────────────────────────
  function toast(msg, isErr) {
    const el = h("div", { class: "gem-toast" + (isErr ? " error" : "") }, msg);
    elToastHost.appendChild(el);
    setTimeout(() => { el.style.transition = "opacity .3s"; el.style.opacity = "0"; setTimeout(() => el.remove(), 300); }, 2600);
  }
  function confirmModal(opts) {
    return new Promise((resolve) => {
      const close = (val) => { elModal.classList.remove("show"); elModal.innerHTML = ""; resolve(val); };
      const content = h("div", { class: "gem-modal-content" },
        h("h3", null, opts.title || t("Confirm")),
        h("p", null, opts.message || ""),
        h("div", { class: "gem-modal-actions" },
          h("button", { class: "cancel-btn", onClick: () => close(false) }, opts.cancelLabel || t("Cancel")),
          h("button", { class: "danger-btn", onClick: () => close(true) }, opts.confirmLabel || t("Delete"))
        )
      );
      elModal.innerHTML = "";
      elModal.appendChild(content);
      elModal.classList.add("show");
      elModal.onclick = (e) => { if (e.target === elModal) close(false); };
    });
  }

  // ── Tooltip ─────────────────────────────────────────────────────────────
  function showTooltip(e, gem) {
    const rows = [
      h("div", { class: "gem-tooltip-title" }, gem.gem_name ? t(gem.gem_name) : "(" + t("Unnamed gem") + ")"),
      tipRow(t("Power Rank"), gem.power_rank),
      tipRow(t("Level"), gem.level),
      tipRow(t("Type"), t(typeName(gem.type))),
      tipRow(t("Tier"), t(tierName(gem.tier))),
      tipRow(t("Quality"), (gem.quality * 100).toFixed(1) + "%"),
      h("hr", { class: "gem-tooltip-hr" }),
    ];
    gem.stats.forEach((stat, i) => {
      rows.push(tipRow(t(statName(gem, i)) + " (" + ((stat.augmentation_progress || 0) * 100).toFixed(1) + "%)", statValue(gem, i).toFixed(2)));
    });
    elTooltip.innerHTML = "";
    rows.forEach((r) => elTooltip.appendChild(r));
    elTooltip.classList.add("show");
    moveTooltip(e);
  }
  function tipRow(label, val) {
    return h("div", { class: "gem-tooltip-row" },
      h("span", { class: "gem-tooltip-muted" }, label),
      h("span", { class: "gem-tooltip-stat" }, val));
  }
  function moveTooltip(e) {
    if (!elTooltip.classList.contains("show")) return;
    let x = e.clientX + 15, y = e.clientY + 15;
    if (x + elTooltip.offsetWidth > window.innerWidth) x = e.clientX - elTooltip.offsetWidth - 15;
    if (y + elTooltip.offsetHeight > window.innerHeight) y = e.clientY - elTooltip.offsetHeight - 15;
    elTooltip.style.left = x + "px";
    elTooltip.style.top = y + "px";
  }
  function hideTooltip() { elTooltip.classList.remove("show"); }

  // ── Gem item / slot builders ────────────────────────────────────────────
  function gemItem(gem, pane, idx) {
    const item = h("div", { class: "item", draggable: "true" },
      h("div", { class: "item-img-holder" },
        h("img", { class: "item-tier-bg", src: tierBg(gem), draggable: "false", alt: "" }),
        h("img", { class: "item-gem-img", src: gemImg(gem), draggable: "false", alt: "" }),
        h("div", { class: "item-lv" }, "Lv." + gem.level),
        h("div", { class: "item-power" }, gem.power_rank)
      )
    );
    item.addEventListener("dragstart", (e) => onDragStart(e, pane, idx));
    item.addEventListener("click", () => selectGem(gem, pane, idx));
    item.addEventListener("contextmenu", (e) => { e.preventDefault(); selectGem(gem, pane, idx); });
    item.addEventListener("mouseenter", (e) => showTooltip(e, gem));
    item.addEventListener("mousemove", moveTooltip);
    item.addEventListener("mouseleave", hideTooltip);
    return item;
  }
  function slotEl(pane, idx, placeholder) {
    const gem = pane === "inventory" ? inventory[idx] : equipped[idx];
    const slot = h("div", { class: "slot", dataset: { hasItem: gem ? "true" : "false", pane: pane, idx: String(idx) } });
    slot.addEventListener("dragenter", (e) => onSlotDragOver(e, pane, idx));
    slot.addEventListener("dragover", (e) => onSlotDragOver(e, pane, idx));
    slot.addEventListener("dragleave", (e) => onSlotDragLeave(e, slot));
    slot.addEventListener("drop", (e) => onDrop(e, pane, idx));
    if (gem) slot.appendChild(gemItem(gem, pane, idx));
    else if (placeholder) slot.appendChild(h("div", { class: "equipped-slot-placeholder" }, h("img", { src: placeholder, alt: "", draggable: "false" })));
    return slot;
  }

  // ── Rendering: equipped column ──────────────────────────────────────────
  function renderEquipped() {
    elEquipped.innerHTML = "";
    elementsSorted().forEach(([name, elementId], rowIdx) => {
      const color = elementColor(elementId);
      const row = h("div", { class: "equipped-row", style: { border: "1px dashed " + color } });
      [0, 1, 2].forEach((slotPos) => {
        const idx = rowIdx * 3 + slotPos;
        const typeRestriction = slotPos < 2 ? 1 : 2;
        row.appendChild(slotEl("equipped", idx, placeholderImg(elementId, typeRestriction)));
        if (slotPos === 1) row.appendChild(h("div", { class: "slot-vertical-separator", style: { borderColor: color } }));
      });
      elEquipped.appendChild(h("div", { class: "equipped-block" },
        h("div", { class: "equipped-row-label", style: { color: color } }, t(name)),
        row));
    });
  }

  // ── Rendering: primordial toggles (built once) ──────────────────────────
  function buildPrimordial() {
    elPrimordial.innerHTML = "";
    elementsSorted().forEach(([name, elementId]) => {
      const color = elementColor(elementId);
      const on = !!primordialToggles[elementId];
      const cb = h("input", { type: "checkbox", class: "primordial-toggle-checkbox" });
      cb.checked = on;
      const slider = h("span", { class: "primordial-toggle-slider", style: { background: on ? color : "#333" } });
      cb.addEventListener("change", () => {
        primordialToggles[elementId] = cb.checked;
        slider.style.background = cb.checked ? color : "#333";
        renderTotals();
        save();
      });
      elPrimordial.appendChild(h("label", { class: "primordial-toggle-label" }, cb, slider,
        h("span", { style: { color: color } }, t(name))));
    });
  }

  // ── Rendering: stat totals ──────────────────────────────────────────────
  function renderTotals() {
    const totals = {};
    let totalPR = 0;
    equipped.filter(Boolean).forEach((gem) => {
      const buff = primordialToggles[gem.element] ? 1.1 : 1;
      gem.stats.forEach((_, i) => {
        const n = statName(gem, i);
        totals[n] = (totals[n] || 0) + statValue(gem, i) * buff;
      });
      totalPR += (Number(gem.power_rank) || 0) * buff;
    });
    const grid = h("div", { class: "totals-grid" });
    Object.keys(totals).sort().forEach((k) => {
      if (totals[k] > 0) grid.appendChild(h("div", null,
        h("span", { class: "muted" }, t(k)), h("br"), h("b", null, formatStat(totals[k]))));
    });
    elTotals.innerHTML = "";
    elTotals.appendChild(h("div", { class: "totals-pr" }, fmt("Total Power Rank: {pr}", { pr: Math.round(totalPR) })));
    elTotals.appendChild(h("hr", { class: "totals-hr" }));
    elTotals.appendChild(grid);
  }

  // ── Rendering: forge (built once; reflects creatorParams via events) ────
  function selectField(label, key, obj, anyLabel) {
    const sel = h("select");
    sel.appendChild(h("option", { value: "" }, anyLabel));
    Object.entries(obj || {}).sort((a, b) => a[1] - b[1]).forEach(([n, id]) => {
      sel.appendChild(h("option", { value: String(id) }, t(n)));
    });
    sel.value = creatorParams[key];
    sel.addEventListener("change", () => { creatorParams[key] = sel.value; onForgeChange(key); });
    return { wrap: h("label", null, h("span", null, t(label)), sel), sel };
  }
  let restrictionSel = null;
  function onForgeChange(key) {
    if (key === "type" && restrictionSel) {
      const isLesser = creatorParams.type === "1";
      restrictionSel.disabled = !isLesser;
      if (!isLesser) { creatorParams.restriction = ""; restrictionSel.value = ""; }
    }
  }
  function buildForge() {
    elForge.innerHTML = "";
    const typeF = selectField("Type", "type", lookups.types, t("Any"));
    const tierF = selectField("Tier", "tier", lookups.tiers, t("Any"));
    const elemF = selectField("Element", "element", lookups.elements, t("Any"));
    const restF = selectField("Restriction", "restriction", lookups.restrictions, t("Any"));
    restrictionSel = restF.sel;
    restrictionSel.disabled = creatorParams.type !== "1";

    const lvlVal = h("span", { class: "slider-value" }, creatorParams.level);
    const lvlInput = h("input", { class: "slider-input", type: "range", min: "1", max: "35" });
    lvlInput.value = creatorParams.level;
    lvlInput.addEventListener("input", () => { creatorParams.level = parseInt(lvlInput.value, 10); lvlVal.textContent = creatorParams.level; });
    const lvlField = h("label", { class: "slider-field" }, h("span", null, t("Level")),
      h("div", { class: "slider-inline" }, lvlInput, lvlVal));

    const augVal = h("span", { class: "slider-value" }, creatorParams.augment);
    const augInput = h("input", { class: "slider-input", type: "range", min: "0", max: "100" });
    augInput.value = creatorParams.augment;
    augInput.addEventListener("input", () => { creatorParams.augment = parseInt(augInput.value, 10); augVal.textContent = creatorParams.augment; });
    const augField = h("label", { class: "slider-field augment-field" }, h("span", null, t("Augment %")),
      h("div", { class: "slider-inline" }, augInput, augVal));
    const syncAug = () => { augField.classList.toggle("disabled", creatorParams.augmentNull); augInput.disabled = creatorParams.augmentNull; };

    const randCb = h("input", { type: "checkbox" });
    randCb.checked = creatorParams.augmentNull;
    randCb.addEventListener("change", () => { creatorParams.augmentNull = randCb.checked; syncAug(); });
    const randToggle = h("label", { class: "augment-toggle-inline" }, randCb, h("span", null, t("Random augment")));

    const genBtn = h("button", { class: "primary-btn creator-generate-btn" }, t("Generate random gem"));
    genBtn.addEventListener("click", generateGem);

    elForge.appendChild(h("div", { class: "gem-form-row" }, typeF.wrap, tierF.wrap));
    elForge.appendChild(h("div", { class: "gem-form-row" }, elemF.wrap, restF.wrap));
    elForge.appendChild(h("div", { class: "gem-form-row creator-advanced-row" }, lvlField, augField));
    elForge.appendChild(h("div", { class: "creator-generate-row" }, randToggle, genBtn));
    syncAug();
  }

  // ── Rendering: selected-gem detail ──────────────────────────────────────
  function renderDetail() {
    elDetail.innerHTML = "";
    if (!selected) {
      elDetail.appendChild(h("div", { class: "placeholder-text" }, t("Select a gem to view its details.")));
      return;
    }
    const gem = selected;

    elDetail.appendChild(h("div", { class: "selected-name" }, gem.gem_name ? t(gem.gem_name) : "(" + t("Unnamed gem") + ")"));

    const bigSlot = h("div", { class: "big-slot", draggable: "true" },
      h("div", { class: "big-slot-inner-holder" },
        h("img", { class: "big-slot-tier-bg", src: tierBg(gem), draggable: "false", alt: "" }),
        h("img", { class: "big-slot-gem-img", src: gemImg(gem), draggable: "false", alt: "" }),
        h("div", { class: "big-slot-lv" }, "Lv." + gem.level),
        h("div", { class: "big-slot-power" }, gem.power_rank)
      ));
    bigSlot.addEventListener("dragstart", (e) => onDragStart(e, "selected", 0));

    const metaLine = (label, val) => h("div", { class: "meta-line" }, h("span", null, t(label) + " "), h("b", null, val));
    const meta = h("div", { class: "selected-stats-square" },
      metaLine("Power", gem.power_rank),
      metaLine("Level", gem.level),
      metaLine("Type", t(typeName(gem.type))),
      metaLine("Tier", t(tierName(gem.tier))),
      metaLine("Quality", (gem.quality * 100).toFixed(1) + "%"));
    elDetail.appendChild(h("div", { class: "selected-container" }, bigSlot, meta));

    const statCol = h("div", { class: "stat-list-column" });
    gem.stats.forEach((stat, i) => {
      const chips = h("div", { class: "container-chip-row" });
      stat.containers.forEach((c) => {
        chips.appendChild(h("div", { class: "container-chip-vert" },
          h("div", { class: "container-chip-val" }, (c.real_value * 100).toFixed(0) + "%"),
          h("div", { class: "container-chip-bar-wrap" },
            h("div", { class: "container-chip-bar", style: { background: barColor(c.value), width: (c.value * 100).toFixed(1) + "%" } }))));
      });
      const card = h("div", { class: "stat-vert-square" + (selectedStatIdx === i ? " selected" : "") },
        h("div", { class: "stat-vert-head" },
          h("div", { class: "stat-label" }, h("b", null, statValue(gem, i).toFixed(2) + " "), h("span", null, t(statName(gem, i)))),
          h("div", { class: "stat-augment-pct" }, ((stat.augmentation_progress || 0) * 100).toFixed(1) + "%")),
        chips);
      card.addEventListener("click", () => { selectedStatIdx = i; renderDetail(); });
      statCol.appendChild(card);
    });
    elDetail.appendChild(statCol);

    const actionGroup = h("div", { class: "action-group" });
    Object.entries(lookups.augment_types || {}).sort((a, b) => a[1] - b[1]).forEach(([, id]) => {
      const key = "augment-" + id;
      const sq = h("div", { class: "action-square" + (selectedActionKey === key ? " selected" : "") }, h("img", { src: augmentImg(id), alt: "" }));
      sq.addEventListener("click", () => { selectedActionKey = key; renderDetail(); });
      actionGroup.appendChild(sq);
    });
    actionGroup.appendChild(h("div", { class: "action-separator" }));
    ["spark", "flare"].forEach((key) => {
      const sq = h("div", { class: "action-square" + (selectedActionKey === key ? " selected" : "") }, h("img", { src: modifierImg(key), alt: "" }));
      sq.addEventListener("click", () => { selectedActionKey = key; renderDetail(); });
      actionGroup.appendChild(sq);
    });
    elDetail.appendChild(h("div", { class: "button-row" }, actionGroup));

    const lvlBtn = h("button", { class: "gem-action-btn" }, gem.is_max_level ? t("Max Level") : t("Level Up"));
    lvlBtn.disabled = !!gem.is_max_level;
    lvlBtn.addEventListener("click", levelUpSelected);

    const actBtn = h("button", { class: "gem-action-btn" }, t(actionButtonText()));
    actBtn.disabled = !selectedActionKey;
    actBtn.addEventListener("click", doSelectedAction);
    elDetail.appendChild(h("div", { class: "gem-actions-row" }, lvlBtn, actBtn));

    // add-to-inventory (only when the selected gem isn't already stored)
    if (!isSelectedInStorage()) {
      const addBtn = h("button", { class: "save-gem-btn" }, t("Add to Inventory"));
      addBtn.addEventListener("click", saveSelectedToInventory);
      elDetail.appendChild(addBtn);
    }
  }
  function actionButtonText() {
    if (!selectedActionKey) return "Action";
    if (selectedActionKey === "spark") return "Change Stat";
    if (selectedActionKey === "flare") return "Move Boost";
    return "Augment Stat";
  }
  function isSelectedInStorage() {
    if (!selected || !selected.id) return true;
    return inventory.some((g) => g && g.id === selected.id) || equipped.some((g) => g && g.id === selected.id);
  }

  function render() {
    renderEquipped();
    renderInventory();
    renderTotals();
    renderDetail();
  }
  function renderInventory() {
    elInventory.innerHTML = "";
    for (let i = 0; i < inventory.length; i++) elInventory.appendChild(slotEl("inventory", i));
  }

  // ── Selection ───────────────────────────────────────────────────────────
  function selectGem(gem, pane, idx) {
    if (selected && selected.id !== gem.id) { selectedActionKey = null; selectedStatIdx = 0; }
    selected = gem;
    selectedSource = { pane, idx };
    hideTooltip();
    renderDetail();
  }
  function updateSelectedInPlace(gem) {
    selected = gem;
    if (selectedSource) {
      if (selectedSource.pane === "inventory") inventory[selectedSource.idx] = gem;
      else if (selectedSource.pane === "equipped") equipped[selectedSource.idx] = gem;
    }
  }

  // ── Gem operations (via GemEngine) ──────────────────────────────────────
  function generateGem() {
    const body = {};
    if (creatorParams.type) body.type = parseInt(creatorParams.type, 10);
    if (creatorParams.tier) body.tier = parseInt(creatorParams.tier, 10);
    if (creatorParams.element) body.element = parseInt(creatorParams.element, 10);
    if (creatorParams.type === "1" && creatorParams.restriction) body.restriction = parseInt(creatorParams.restriction, 10);
    if (creatorParams.level) body.level = parseInt(creatorParams.level, 10);
    if (!creatorParams.augmentNull) body.augmentation = creatorParams.augment / 100;

    const resp = window.GemEngine.createGem(body);
    if (resp && resp.success) {
      selected = resp.gem;
      selectedSource = null;
      selectedActionKey = null;
      selectedStatIdx = 0;
      renderDetail();
    } else {
      toast(fmt("Could not generate gem: {error}", { error: (resp && resp.error) || t("Unknown error") }), true);
    }
  }
  function levelUpSelected() {
    if (!selected) return;
    const resp = window.GemEngine.levelUpGem(selected);
    if (resp && resp.success) { updateSelectedInPlace(resp.gem); render(); save(); }
    else toast(fmt("Could not level up: {error}", { error: (resp && resp.error) || t("Unknown error") }), true);
  }
  function doSelectedAction() {
    if (!selectedActionKey || !selected) return;
    const statTypeId = selected.stats[selectedStatIdx].type;
    let resp;
    if (selectedActionKey.slice(0, 8) === "augment-") resp = window.GemEngine.augmentGem(selected, statTypeId, parseInt(selectedActionKey.split("-")[1], 10));
    else if (selectedActionKey === "spark") resp = window.GemEngine.sparkGem(selected, statTypeId);
    else if (selectedActionKey === "flare") resp = window.GemEngine.flareGem(selected, statTypeId);
    if (resp && resp.success) { updateSelectedInPlace(resp.gem); render(); save(); }
    else toast(fmt("Action failed: {error}", { error: (resp && resp.error) || t("Unknown error") }), true);
  }
  function saveSelectedToInventory() {
    const empty = inventory.findIndex((g) => !g);
    if (empty === -1) return toast(t("Inventory is full."), true);
    inventory[empty] = clone(selected);
    selected = inventory[empty];
    selectedSource = { pane: "inventory", idx: empty };
    render();
    save();
  }
  function recalcBases() {
    const invResp = window.GemEngine.massUpdate(inventory);
    if (invResp && invResp.success) inventory = invResp.gems;
    const eqResp = window.GemEngine.massUpdate(equipped);
    if (eqResp && eqResp.success) equipped = eqResp.gems;
    if (selectedSource) {
      const arr = selectedSource.pane === "inventory" ? inventory : equipped;
      selected = arr[selectedSource.idx] || null;
      if (!selected) selectedSource = null;
    }
    render();
    save();
    toast(t("Gems recalculated."));
  }

  // ── Trash ───────────────────────────────────────────────────────────────
  async function trashSelected() {
    if (!selected) return toast(t("No gem selected to trash."), true);
    const ok = await confirmModal({
      title: t("Trash gem"), message: t("Are you sure you want to permanently delete this gem?"),
      confirmLabel: t("Delete"), cancelLabel: t("Cancel"),
    });
    if (!ok) return;
    if (selectedSource) {
      if (selectedSource.pane === "equipped") equipped[selectedSource.idx] = null;
      if (selectedSource.pane === "inventory") inventory[selectedSource.idx] = null;
    }
    selected = null; selectedSource = null;
    render(); save();
    toast(t("Gem deleted."));
  }

  // ── Drag & drop ─────────────────────────────────────────────────────────
  function onDragStart(e, pane, idx) {
    hideTooltip();
    const resolved = resolveDraggedGem(pane, idx);
    dragState = { pane, idx, gem: resolved ? resolved.gem : null };
    if (e.dataTransfer) {
      e.dataTransfer.setData("text/plain", JSON.stringify({ pane, idx }));
      e.dataTransfer.effectAllowed = "move";
    }
  }
  function clearDragHighlights() {
    document.querySelectorAll(".gem-sim .slot.slot-drop-valid, .gem-sim .slot.slot-drop-invalid, .gem-sim .slot.slot-drop-hint")
      .forEach((el) => el.classList.remove("slot-drop-valid", "slot-drop-invalid", "slot-drop-hint"));
  }
  function onSlotDragOver(e, pane, idx) {
    if (!dragState.gem) return;
    e.preventDefault();
    const valid = pane === "equipped" ? validateEquip(dragState.gem, idx).valid : true;
    const slot = e.currentTarget;
    slot.classList.remove("slot-drop-valid", "slot-drop-invalid");
    slot.classList.add(valid ? "slot-drop-valid" : "slot-drop-invalid");
    if (e.dataTransfer) e.dataTransfer.dropEffect = valid ? "move" : "none";
  }
  function onSlotDragLeave(e, slot) {
    if (e.relatedTarget && slot.contains(e.relatedTarget)) return;
    slot.classList.remove("slot-drop-valid", "slot-drop-invalid", "slot-drop-hint");
  }
  function resolveDraggedGem(pane, idx) {
    if (pane === "selected") {
      const hasSource = !!selectedSource;
      return {
        source: hasSource ? { pane: selectedSource.pane, idx: selectedSource.idx } : { pane: "selected", idx: 0 },
        gem: selected,
        fromDetached: !hasSource,
      };
    }
    const gem = pane === "inventory" ? inventory[idx] : equipped[idx];
    return { source: { pane, idx }, gem, fromDetached: false };
  }
  function readDragPayload(e) {
    if (dragState.pane !== null) return { pane: dragState.pane, idx: dragState.idx };
    try {
      const raw = e && e.dataTransfer ? e.dataTransfer.getData("text/plain") : "";
      if (!raw) return null;
      const parsed = JSON.parse(raw);
      return parsed && typeof parsed === "object" ? parsed : null;
    } catch (err) { return null; }
  }
  function slotElementFor(slotIdx) {
    const rowIdx = Math.floor(slotIdx / 3);
    const slotPos = slotIdx % 3;
    const entry = elementsSorted()[rowIdx];
    return { element: entry ? entry[1] : null, type: slotPos < 2 ? 1 : 2 };
  }
  function validateEquip(gem, slotIdx) {
    const target = slotElementFor(slotIdx);
    if (String(gem.element) !== String(target.element)) return { valid: false, error: fmt("Requires a {element} gem.", { element: t(elementName(target.element)) }) };
    if (String(gem.type) !== String(target.type)) return { valid: false, error: fmt("Requires a {type} gem.", { type: t(typeName(target.type)) }) };
    if (gem.ability) {
      const dup = equipped.some((g, i) => g && i !== slotIdx && String(g.ability) === String(gem.ability));
      if (dup) return { valid: false, error: t("Can't equip two empowered gems with the same ability.") };
    }
    return { valid: true };
  }
  function onDrop(e, toPane, toIdx) {
    e.preventDefault();
    clearDragHighlights();
    const parsed = readDragPayload(e);
    if (!parsed) return;
    const { source, gem: draggedGem, fromDetached } = resolveDraggedGem(parsed.pane, parsed.idx);
    if (!draggedGem || !source) return;
    if (source.pane === toPane && source.idx === toIdx) return;

    if (toPane === "equipped") {
      const val = validateEquip(draggedGem, toIdx);
      if (!val.valid) return toast(val.error, true);
    }
    const targetGem = toPane === "inventory" ? inventory[toIdx] : equipped[toIdx];

    if (source.pane === "inventory" || source.pane === "equipped") {
      // moving a gem out of its source slot into the target; swap the target back
      if (source.pane === "equipped" && targetGem) {
        // target must be legal in the source (equipped) slot
        const back = validateEquip(targetGem, source.idx);
        if (!back.valid) return toast(back.error, true);
      }
      const srcArr = source.pane === "inventory" ? inventory : equipped;
      const dstArr = toPane === "inventory" ? inventory : equipped;
      srcArr[source.idx] = targetGem;
      dstArr[toIdx] = draggedGem;
      // keep selection pointer aligned to wherever each gem landed
      if (selected) {
        if (targetGem && selected.id === targetGem.id) selectedSource = { pane: source.pane, idx: source.idx };
        if (selected.id === draggedGem.id) selectedSource = { pane: toPane, idx: toIdx };
      }
    } else if (fromDetached || source.pane === "selected") {
      const dstArr = toPane === "inventory" ? inventory : equipped;
      dstArr[toIdx] = draggedGem;
      if (targetGem) { selected = targetGem; selectedSource = null; }
      else { selected = draggedGem; selectedSource = { pane: toPane, idx: toIdx }; }
    }
    render();
    save();
  }
  async function onDropTrash(e) {
    e.preventDefault();
    clearDragHighlights();
    const parsed = readDragPayload(e);
    if (!parsed) return;
    const { source, gem: draggedGem } = resolveDraggedGem(parsed.pane, parsed.idx);
    if (!draggedGem) return;
    const ok = await confirmModal({
      title: t("Trash gem"), message: t("Are you sure you want to permanently delete this gem?"),
      confirmLabel: t("Delete"), cancelLabel: t("Cancel"),
    });
    if (!ok) return;
    if (source.pane === "inventory") inventory[source.idx] = null;
    if (source.pane === "equipped") equipped[source.idx] = null;
    if (selected && selected.id === draggedGem.id) { selected = null; selectedSource = null; }
    render(); save();
    toast(t("Gem deleted."));
  }

  // ── Init ────────────────────────────────────────────────────────────────
  function init() {
    elEquipped = document.getElementById("gs-equipped");
    elPrimordial = document.getElementById("gs-primordial");
    elTotals = document.getElementById("gs-totals");
    elForge = document.getElementById("gs-forge");
    elDetail = document.getElementById("gs-detail");
    elInventory = document.getElementById("gs-inventory");
    elTrash = document.getElementById("gs-trash");
    elRecalc = document.getElementById("gs-recalc");
    elTooltip = document.getElementById("gs-tooltip");
    elModal = document.getElementById("gs-modal");
    elToastHost = document.getElementById("gs-toast-host");
    if (!elInventory) return;

    const look = window.GemEngine.getLookups();
    lookups = (look && look.data) || {};

    loadStorage();
    buildForge();
    buildPrimordial();
    render();

    elTrash.addEventListener("click", trashSelected);
    elTrash.addEventListener("dragover", (e) => e.preventDefault());
    elTrash.addEventListener("drop", onDropTrash);
    elRecalc.addEventListener("click", recalcBases);

    const endDrag = () => { clearDragHighlights(); dragState = { pane: null, idx: -1, gem: null }; };
    document.addEventListener("dragend", endDrag);
    document.addEventListener("drop", endDrag);
  }

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init);
  else init();
})();
