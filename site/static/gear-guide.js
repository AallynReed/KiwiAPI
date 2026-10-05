/* =========================================================================
   /gear-guide - rarities, Power Rank, forging, Pearls and banners.
   Everything is read from /gamedata/gear.json, which the gear decoder
   (app/trove/decode/gear.py) rebuilds from the game files after each patch.
   ========================================================================= */
(function () {
  "use strict";
  const { h } = window.BTTDom;
  const t = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);
  const tr = (s) => s;   // marks a string that reaches t() through the data, for the i18n audit

  // Names that arrive in gear.json rather than in this file.
  const DATA_STRINGS = [
    tr("Common"), tr("Uncommon"), tr("Rare"), tr("Epic"), tr("Legendary"), tr("Relic"),
    tr("Resplendent"), tr("Shadow"), tr("Radiant"), tr("Stellar"), tr("Crystal"), tr("Mystic"),
    tr("Weapon"), tr("Hat"), tr("Face"), tr("Ring"), tr("Banner"),
    tr("Gun"), tr("Bow"), tr("Staff"), tr("Spear"), tr("Fist"), tr("Melee"),
    tr("Cooldown Speed"), tr("Experience Gain"), tr("Flask Capacity"), tr("Incoming Damage"),
    tr("Lasermancy"), tr("Stability"), tr("Superstition"),
    tr("Improve Gear"), tr("Increase Rarity Level"), tr("Add a New Stat or Bonus Stat"),
    tr("Increase Stat Level"), tr("Reduce Required Level"), tr("Randomize Stat Types"), tr("Change Aura"),
  ];
  void DATA_STRINGS;

  // Still usable in game but no longer handed out (not marked in the game files).
  const RETIRED_ITEMS = new Set(["item/crafting/tome_notrade"]);

  let data = null;
  const state = { rarity: "Crystal5", star: 5, pearls: [2, 2, 2], costRarity: "Crystal5", mode: "pve", query: "" };
  const byKey = () => Object.fromEntries(data.rarities.map((r) => [r.key, r]));

  const lang = () => document.documentElement.lang || undefined;
  const fmt = (n, digits) => Number(n).toLocaleString(lang(), { maximumFractionDigits: digits == null ? 2 : digits });
  const rarityName = (r) => (r.level ? t(r.family) + " " + r.level : t(r.family));
  // The game's rarity list interleaves Radiant and Stellar (Radiant 1, Stellar 1,
  // Radiant 2...); show each family together, in order of its first appearance.
  function ordered() {
    const first = {};
    data.rarities.forEach((r, i) => { if (!(r.family in first)) first[r.family] = i; });
    return data.rarities.slice().sort((a, b) => (first[a.family] - first[b.family]) || ((a.level || 0) - (b.level || 0)));
  }

  // The game's own formula (gear.py power_rank): pearl boosts scale the base, then
  // each star adds 6% (at least 1), rounded half up.
  function powerRank(base, star, boosts) {
    const pr = data.power_rank;
    const p = base * (1 + pr.per_stat_boost * boosts);
    return Math.floor(Math.max(p * (1 + pr.per_star * star), p + star) + 0.5);
  }

  function segmented(label, values, current, onPick) {
    const group = h("div", { class: "gr-seg", role: "radiogroup", "aria-label": label });
    values.forEach((v) => {
      const on = v === current;
      group.appendChild(h("button", {
        type: "button", role: "radio", "aria-checked": on ? "true" : "false", class: on ? "on" : "",
        onClick: () => onPick(v),
      }, String(v)));
    });
    return h("div", { class: "gr-field" }, h("span", { class: "gr-field-label" }, label), group);
  }

  function raritySelect(id, current, onPick, filter) {
    const sel = h("select", { class: "gr-select", id: id, onChange: (e) => onPick(e.target.value) });
    ordered().filter(filter || (() => true)).forEach((r) => {
      const opt = h("option", { value: r.key }, rarityName(r));
      if (r.key === current) opt.selected = true;
      sel.appendChild(opt);
    });
    return sel;
  }

  // ── Power Rank calculator ────────────────────────────────────────────────
  function renderCalc() {
    const host = document.getElementById("gr-calc");
    host.textContent = "";
    const r = byKey()[state.rarity];
    const base = r.power_rank;
    const pearls = state.pearls.reduce((a, b) => a + b, 0);
    const withPearls = powerRank(base, 0, pearls);
    const total = powerRank(base, state.star, pearls);
    const perLine = Array.from({ length: data.forge.max_boosts_per_line + 1 }, (_, i) => i);

    const controls = h("div", { class: "gr-calc-controls" },
      h("label", { class: "gr-field", for: "gr-calc-rarity" },
        h("span", { class: "gr-field-label" }, t("Rarity")),
        raritySelect("gr-calc-rarity", state.rarity, (v) => { state.rarity = v; renderCalc(); },
          (x) => x.power_rank)),
      segmented(t("Star level"), [0, 1, 2, 3, 4, 5], state.star, (v) => { state.star = v; renderCalc(); }),
      h("div", { class: "gr-pearl-lines" }, state.pearls.map((n, i) =>
        segmented(t("Pearls on stat {n}").replace("{n}", i + 1), perLine, n,
          (v) => { state.pearls[i] = v; renderCalc(); }))));

    const result = h("div", { class: "gr-calc-result", "aria-live": "polite" },
      h("span", { class: "gr-calc-label" }, t("Power Rank")),
      h("strong", { class: "gr-calc-total" }, fmt(total, 0)),
      h("dl", { class: "gr-calc-steps" },
        h("div", {}, h("dt", {}, t("Base")), h("dd", {}, fmt(base, 0))),
        h("div", {}, h("dt", {}, t("With pearls")), h("dd", {}, fmt(withPearls, 0))),
        h("div", {}, h("dt", {}, t("At this star level")), h("dd", {}, fmt(total, 0)))));

    host.appendChild(controls);
    host.appendChild(result);
    host.appendChild(h("p", { class: "gr-fine" },
      t("Every stat line takes up to {n} pearls. Each pearl adds {p}% to the base, then each star adds {s}% (never less than 1).")
        .replace("{n}", data.forge.max_boosts_per_line)
        .replace("{p}", fmt(data.power_rank.per_stat_boost * 100))
        .replace("{s}", fmt(data.power_rank.per_star * 100))));
  }

  // ── Rarities ─────────────────────────────────────────────────────────────
  function renderRarities() {
    const host = document.getElementById("gr-rarities");
    host.textContent = "";
    const maxBoosts = data.forge.max_lines * data.forge.max_boosts_per_line;
    const yes = (on, what) => on
      ? h("i", { class: "fa-solid fa-check gr-yes", role: "img", "aria-label": t("Yes") + ", " + what })
      : h("span", { class: "gr-no", "aria-label": t("No") }, "-");
    const table = h("table", { class: "gr-table" },
      h("thead", {}, h("tr", {},
        h("th", { class: "l", scope: "col" }, t("Rarity")),
        h("th", { class: "r", scope: "col" }, t("Level")),
        h("th", { class: "r", scope: "col" }, t("Power Rank")),
        h("th", { class: "r", scope: "col" }, "+5"),
        h("th", { class: "r", scope: "col" }, t("+5, 2 pearls on every stat")),
        h("th", { class: "c", scope: "col" }, t("Weapon aura")),
        h("th", { class: "c", scope: "col" }, t("Hat aura")))));
    const body = h("tbody", {});
    let family = null;
    ordered().forEach((r) => {
      if (!r.power_rank) return;
      if (r.family !== family && r.level) {
        family = r.family;
        body.appendChild(h("tr", { class: "gr-family" }, h("th", { colspan: 7, scope: "colgroup", class: "l" }, t(r.family))));
      } else if (!r.level) {
        family = null;
      }
      const unreleased = !r.improve_cost;
      body.appendChild(h("tr", { class: unreleased ? "gr-unreleased" : "" },
        h("th", { class: "l", scope: "row" }, h("span", { class: "gr-rname" }, rarityName(r)),
          unreleased ? h("span", { class: "gr-tag" }, t("Not in game yet")) : null),
        h("td", { class: "r" }, r.required_level),
        h("td", { class: "r" }, fmt(r.power_rank, 0)),
        h("td", { class: "r" }, fmt(powerRank(r.power_rank, 5, 0), 0)),
        h("td", { class: "r strong" }, fmt(powerRank(r.power_rank, 5, maxBoosts), 0)),
        h("td", { class: "c" }, yes(r.aura.weapon, t("Weapon aura"))),
        h("td", { class: "c" }, yes(r.aura.hat, t("Hat aura")))));
    });
    table.appendChild(body);
    host.appendChild(table);
  }

  // ── Forging ──────────────────────────────────────────────────────────────
  const costText = (list) => (list || []).map((c) => fmt(c.count, 0) + " " + c.name).join(" · ");
  const retiredTag = (list) => ((list || []).some((c) => RETIRED_ITEMS.has(c.item))
    ? h("span", { class: "gr-tag" }, t("Retired")) : null);

  function renderForge() {
    const f = data.forge;
    const stars = document.getElementById("gr-stars");
    stars.textContent = "";
    const bar = h("ol", { class: "gr-star-bar", "aria-label": t("Share of each stat line's roll range, by star level") });
    for (let lv = 0; lv <= f.max_star; lv++) {
      const share = Math.round(lv * f.star_step * 100);
      bar.appendChild(h("li", {},
        h("span", { class: "gr-star-lv" }, "+" + lv),
        h("span", { class: "gr-star-track" }, h("span", { class: "gr-star-fill", style: { width: share + "%" } })),
        h("span", { class: "gr-star-pct" }, share + "%")));
    }
    stars.appendChild(bar);

    const costs = document.getElementById("gr-costs");
    costs.textContent = "";
    const r = byKey()[state.costRarity];
    const sel = raritySelect("gr-cost-rarity", state.costRarity, (v) => { state.costRarity = v; renderForge(); },
      (x) => x.improve_cost);
    costs.appendChild(h("div", { class: "gr-costs-head" },
      h("h3", {}, t("What each star costs")),
      h("label", { class: "gr-field gr-inline", for: "gr-cost-rarity" },
        h("span", { class: "gr-field-label" }, t("Rarity")), sel)));
    const rows = Object.keys(r.improve_cost || {}).sort((a, b) => a - b).map((lv) =>
      h("tr", {}, h("th", { class: "l", scope: "row" }, "+" + lv), h("td", { class: "l" }, costText(r.improve_cost[lv]))));
    costs.appendChild(h("div", { class: "gr-table-wrap" }, h("table", { class: "gr-table" },
      h("thead", {}, h("tr", {}, h("th", { class: "l", scope: "col" }, t("Star")), h("th", { class: "l", scope: "col" }, t("Materials")))),
      h("tbody", {}, rows))));

    if (f.raise_from && f.raise_to) {
      const to = byKey()[f.raise_to];
      costs.appendChild(h("div", { class: "gr-raise" },
        h("h3", {}, t("Raising the rarity")),
        h("p", {}, t("The only rarity you can raise is {from} at +5, which the Forge turns into {to}.")
          .replace("{from}", rarityName(byKey()[f.raise_from])).replace("{to}", rarityName(to))),
        to.raise_cost ? h("p", { class: "gr-cost-line" }, h("strong", {}, t("Cost") + ": "), costText(to.raise_cost)) : null));
    }
  }

  // ── Pearls and stations ──────────────────────────────────────────────────
  function renderPearls() {
    const f = data.forge;
    const rules = document.getElementById("gr-rules");
    rules.textContent = "";
    const m = f.pearl_multipliers;
    const rule = (icon, title, text, tag) => h("li", {},
      h("i", { class: "fa-solid " + icon, "aria-hidden": "true" }),
      h("div", {}, h("strong", {}, title, tag || null), h("p", {}, text)));
    rules.appendChild(rule("fa-plus", t("Add a stat line"),
      t("A Pearl of Wisdom adds a new stat line to an item that has fewer than {n}.").replace("{n}", f.max_lines)));
    rules.appendChild(rule("fa-arrow-up", t("Boost a stat line"),
      t("A Pearl of Wisdom can also boost a line, up to {n} times. The first boost multiplies it by {a}, the second by {b}, and each boost adds at least 1 (or 1% on a percentage line).")
        .replace("{n}", f.max_boosts_per_line).replace("{a}", fmt(m[1])).replace("{b}", fmt(m[2]))));
    rules.appendChild(rule("fa-ring", t("Rings"), t("Rings can't take Pearls of Wisdom at all. They can only be improved and raised in rarity.")));
    rules.appendChild(rule("fa-shuffle", t("Chaos Forge"),
      t("Rerolls which stats an item has. The item needs {n} stat lines first.").replace("{n}", f.chaos_min_lines)));
    rules.appendChild(rule("fa-book", t("Twinkling Tome"),
      t("Lowers the level needed to wear an item, by up to {n} levels in total. Tomes are retired, but any you already have still work.").replace("{n}", f.tome_max_reduction),
      h("span", { class: "gr-tag" }, t("Retired"))));

    const host = document.getElementById("gr-stations");
    host.textContent = "";
    const body = h("tbody", {});
    f.stations.forEach((s) => s.operations.forEach((op, i) => {
      body.appendChild(h("tr", {},
        i === 0 ? h("th", { class: "l", scope: "rowgroup", rowspan: s.operations.length }, s.name) : null,
        h("td", { class: "l" }, t(op.label)),
        h("td", { class: "l" }, op.cost ? costText(op.cost) : t("Depends on the item's rarity (see above)"), retiredTag(op.cost))));
    }));
    host.appendChild(h("table", { class: "gr-table" },
      h("thead", {}, h("tr", {},
        h("th", { class: "l", scope: "col" }, t("Station")),
        h("th", { class: "l", scope: "col" }, t("What it does")),
        h("th", { class: "l", scope: "col" }, t("Cost")))),
      body));
  }

  // ── Slots ────────────────────────────────────────────────────────────────
  function renderSlots() {
    const host = document.getElementById("gr-slots");
    host.textContent = "";
    const list = h("dl", { class: "gr-slot-list" });
    const single = [];
    data.slots.forEach((s) => {
      if (s.item_types.length < 2) { single.push(t(s.name)); return; }
      list.appendChild(h("div", {}, h("dt", {}, t(s.name)),
        h("dd", {}, s.item_types.map((it) => t(it.name)).join(", "))));
    });
    if (single.length) {
      list.appendChild(h("div", {}, h("dt", {}, single.join(", ")), h("dd", {}, t("One kind each"))));
    }
    host.appendChild(list);

    const named = document.getElementById("gr-named");
    named.textContent = "";
    if (!data.named_items.length) return;
    named.appendChild(h("h3", {}, t("Items with set stat lines")));
    named.appendChild(h("table", { class: "gr-table" },
      h("thead", {}, h("tr", {},
        h("th", { class: "l", scope: "col" }, t("Item")),
        h("th", { class: "l", scope: "col" }, t("Slot")),
        h("th", { class: "l", scope: "col" }, t("Rarity")),
        h("th", { class: "l", scope: "col" }, t("Always rolls")))),
      h("tbody", {}, data.named_items.map((n) => h("tr", {},
        h("th", { class: "l", scope: "row" }, n.name),
        h("td", { class: "l" }, t(n.item_type)),
        h("td", { class: "l" }, rarityName(byKey()[n.rarity] || { family: n.rarity })),
        h("td", { class: "l" }, n.stats.map((x) => t(x.replace(/ %$/, "")) + (/ %$/.test(x) ? " %" : "")).join(", ")))))));
  }

  // ── Banners and torches ──────────────────────────────────────────────────
  const statText = (s) => "+" + fmt(s.value) + (s.percent ? "%" : "");

  function renderBannerControls() {
    const host = document.getElementById("gr-banner-controls");
    host.textContent = "";
    const modes = [["pve", t("Adventure")], ["pvp", t("PvP")]];
    const seg = h("div", { class: "gr-seg", role: "radiogroup", "aria-label": t("Where the stats apply") });
    modes.forEach(([key, label]) => {
      const on = state.mode === key;
      seg.appendChild(h("button", { type: "button", role: "radio", "aria-checked": on ? "true" : "false", class: on ? "on" : "",
        onClick: () => { state.mode = key; renderBannerControls(); renderBanners(); } }, label));
    });
    const search = h("input", { class: "gr-input", type: "search", value: state.query, placeholder: t("Search by name"),
      "aria-label": t("Search banners and torches by name") });
    search.addEventListener("input", (e) => { state.query = e.target.value; renderBanners(); });
    host.appendChild(seg);
    host.appendChild(search);
  }

  function renderBanners() {
    const host = document.getElementById("gr-banners");
    host.textContent = "";
    const q = state.query.trim().toLowerCase();
    const shown = data.banners.filter((b) => b.pvp === (state.mode === "pvp")
      && (!q || b.items.some((i) => i.name.toLowerCase().includes(q))));
    host.appendChild(h("p", { class: "gr-count", role: "status" },
      t("{n} stat sets").replace("{n}", fmt(shown.length, 0))));
    shown.forEach((b) => {
      const names = b.items.map((i) => i.name + (i.rarity ? " (" + rarityName(byKey()[i.rarity] || { family: i.rarity }) + ")" : ""));
      const head = names.slice(0, 3).join(", ");
      const more = names.length > 3
        ? h("details", { class: "gr-more" }, h("summary", {}, t("+{n} more").replace("{n}", names.length - 3)),
          h("p", {}, names.slice(3).join(", ")))
        : null;
      let stats;
      if (b.tiers.length) {
        const statNames = b.tiers[0].stats.map((s) => s.name);
        stats = h("div", { class: "gr-table-wrap" }, h("table", { class: "gr-table gr-tier-table" },
          h("thead", {}, h("tr", {}, h("th", { class: "l", scope: "col" }, t("Stat")),
            b.tiers.map((_, i) => h("th", { class: "r", scope: "col" }, t("Tier") + " " + (i + 1))))),
          h("tbody", {}, statNames.map((name) => h("tr", {}, h("th", { class: "l", scope: "row" }, t(name)),
            b.tiers.map((tier) => {
              const s = tier.stats.find((x) => x.name === name);
              return h("td", { class: "r" }, s ? statText(s) : "-");
            }))))));
      } else {
        stats = h("ul", { class: "gr-stat-list" }, b.stats.map((s) =>
          h("li", {}, h("span", {}, t(s.name)), h("strong", {}, statText(s)))));
      }
      host.appendChild(h("article", { class: "gr-banner" },
        h("h3", {}, head), more, stats));
    });
  }

  function renderAll() {
    if (!data) return;
    renderCalc();
    renderRarities();
    renderForge();
    renderPearls();
    renderSlots();
    renderBannerControls();
    renderBanners();
  }

  async function init() {
    const status = document.getElementById("gr-status");
    try {
      const r = await fetch("/gamedata/gear.json", { headers: { Accept: "application/json" } });
      if (!r.ok) throw new Error("HTTP " + r.status);
      data = await r.json();
    } catch (e) {
      if (status) status.textContent = t("The gear data could not be loaded. Reload the page to try again.");
      return;
    }
    if (status) status.remove();
    if (!byKey()[state.rarity]) state.rarity = state.costRarity = data.rarities[data.rarities.length - 1].key;
    renderAll();
    document.addEventListener("btt-lang-changed", renderAll);
  }

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init);
  else init();
})();
