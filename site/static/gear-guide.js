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
    tr("Cooldown Speed"), tr("Experience Gain"), tr("Flask Capacity"), tr("Incoming Damage"),
    tr("Lasermancy"), tr("Stability"), tr("Superstition"),
    tr("Improve Gear"), tr("Increase Rarity Level"), tr("Add a New Stat or Bonus Stat"),
    tr("Increase Stat Level"), tr("Reduce Required Level"), tr("Randomize Stat Types"), tr("Change Aura"),
  ];
  void DATA_STRINGS;

  // Still usable in game but no longer handed out (not marked in the game files).
  const RETIRED_ITEMS = new Set(["item/crafting/tome_notrade"]);
  // Stations still in the files but no longer used in game (not marked in the files).
  const LEGACY_STATIONS = new Set(["placeable/crafting/forge_vfx_interactive"]);

  let data = null;
  const state = { rarity: "Crystal5", star: 5, pearls: [2, 2, 2], costRarity: "Crystal5", mode: "pve", query: "" };
  // Max-out calculator. Hat, face and weapon belong to each class; every ring in the
  // files is Common, so rings never go through these rarities.
  const PIECES = [["weapon", tr("Weapon")], ["hat", tr("Hat")], ["face", tr("Face")]];
  const maxOut = { rarity: "Mystic1", fromBelow: true, star: 0, classes: 18, pieces: { weapon: true, hat: true, face: true }, pearls: true, lines: 3 };
  const byKey = () => Object.fromEntries(data.rarities.map((r) => [r.key, r]));

  const lang = () => document.documentElement.lang || undefined;
  const apiUrl = (window.BTTUtil && window.BTTUtil.apiUrl) || ((u) => u);

  // The item's own model, rendered by the codex (same thumbnails the codex and the
  // fishing guide use). No model in gear.json, or a failed render: no icon at all.
  function itemIcon(ref) {
    const bp = data.icons && data.icons[ref];
    if (!bp) return null;
    const img = h("img", { class: "gr-ic", loading: "lazy", alt: "", width: 28, height: 28,
      src: apiUrl("/site/codexes/render?blueprint=" + encodeURIComponent(bp) + "&dim=96") });
    img.addEventListener("error", () => img.remove());
    return img;
  }
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
  const costList = (list) => h("span", { class: "gr-mats" }, (list || []).map((c) =>
    h("span", { class: "gr-mat" }, itemIcon(c.item), h("strong", {}, fmt(c.count, 0)), " " + c.name)));
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
      h("tr", {}, h("th", { class: "l", scope: "row" }, "+" + lv), h("td", { class: "l" }, costList(r.improve_cost[lv]))));
    costs.appendChild(h("div", { class: "gr-table-wrap" }, h("table", { class: "gr-table" },
      h("thead", {}, h("tr", {}, h("th", { class: "l", scope: "col" }, t("Star")), h("th", { class: "l", scope: "col" }, t("Materials")))),
      h("tbody", {}, rows))));

    if (f.raise_from && f.raise_to) {
      const to = byKey()[f.raise_to];
      costs.appendChild(h("div", { class: "gr-raise" },
        h("h3", {}, t("Raising the rarity")),
        h("p", {}, t("The only rarity you can raise is {from} at +5, which the Forge turns into {to}.")
          .replace("{from}", rarityName(byKey()[f.raise_from])).replace("{to}", rarityName(to))),
        to.raise_cost ? h("p", { class: "gr-cost-line" }, h("strong", {}, t("Cost") + ": "), costList(to.raise_cost)) : null));
    }
  }

  // ── Max-out calculator ───────────────────────────────────────────────────
  function maxOutBag() {
    const f = data.forge;
    const bag = new Map();
    const add = (list, times) => (list || []).forEach((c) => {
      const e = bag.get(c.item) || { item: c.item, name: c.name, count: 0 };
      e.count += c.count * times;
      bag.set(c.item, e);
    });
    const stars = (r, from) => { for (let lv = from + 1; lv <= f.max_star; lv++) add(r.improve_cost[String(lv)], 1); };
    const target = byKey()[maxOut.rarity];
    if (maxOut.rarity === f.raise_to && maxOut.fromBelow) {
      stars(byKey()[f.raise_from], maxOut.star);
      add(target.raise_cost, 1);
      stars(target, 0);
    } else {
      stars(target, maxOut.star);
    }
    if (maxOut.pearls) {
      const op = f.stations.flatMap((s) => s.operations).find((o) => o.key === "AddStatBonus");
      const pearl = op && op.cost && op.cost[0];
      if (pearl) add([pearl], (f.max_lines - maxOut.lines) + f.max_lines * f.max_boosts_per_line);
    }
    return bag;
  }

  function renderMaxOut() {
    const f = data.forge;
    const host = document.getElementById("gr-maxout");
    host.textContent = "";
    const raising = maxOut.rarity === f.raise_to;
    const startName = rarityName(byKey()[raising && maxOut.fromBelow ? f.raise_from : maxOut.rarity]);
    const toggle = (label, on, onFlip) => h("button", {
      type: "button", class: "gr-chk" + (on ? " on" : ""), "aria-pressed": on ? "true" : "false", onClick: onFlip,
    }, h("i", { class: on ? "fa-solid fa-square-check" : "fa-regular fa-square", "aria-hidden": "true" }), " " + label);

    const classes = h("input", { class: "gr-input gr-num", id: "gr-mo-classes", type: "number", min: 1, max: 18, value: maxOut.classes });
    classes.addEventListener("change", (e) => {
      maxOut.classes = Math.min(18, Math.max(1, parseInt(e.target.value, 10) || 1));
      renderMaxOut();
    });

    const controls = h("div", { class: "gr-calc-controls" },
      h("label", { class: "gr-field", for: "gr-mo-rarity" },
        h("span", { class: "gr-field-label" }, t("Target rarity")),
        raritySelect("gr-mo-rarity", maxOut.rarity, (v) => { maxOut.rarity = v; renderMaxOut(); }, (x) => x.improve_cost)),
      raising ? toggle(t("Start from a {r} item").replace("{r}", rarityName(byKey()[f.raise_from])), maxOut.fromBelow,
        () => { maxOut.fromBelow = !maxOut.fromBelow; renderMaxOut(); }) : null,
      segmented(t("{r} star level it starts at").replace("{r}", startName), [0, 1, 2, 3, 4], maxOut.star,
        (v) => { maxOut.star = v; renderMaxOut(); }),
      h("label", { class: "gr-field", for: "gr-mo-classes" }, h("span", { class: "gr-field-label" }, t("Classes")), classes),
      h("div", { class: "gr-field", role: "group", "aria-label": t("Pieces per class") },
        h("span", { class: "gr-field-label" }, t("Pieces per class")),
        h("div", { class: "gr-chks" }, PIECES.map(([key, label]) => toggle(t(label), maxOut.pieces[key],
          () => { maxOut.pieces[key] = !maxOut.pieces[key]; renderMaxOut(); })))),
      toggle(t("Include Pearls of Wisdom"), maxOut.pearls, () => { maxOut.pearls = !maxOut.pearls; renderMaxOut(); }),
      maxOut.pearls ? segmented(t("Stat lines each item already has"), [1, 2, 3], maxOut.lines,
        (v) => { maxOut.lines = v; renderMaxOut(); }) : null);

    const pieces = PIECES.filter(([key]) => maxOut.pieces[key]).length;
    const items = pieces * maxOut.classes;
    const bag = maxOutBag();
    const rows = [...bag.values()].map((e) => h("tr", {},
      h("th", { class: "l", scope: "row" }, h("span", { class: "gr-named" }, itemIcon(e.item), e.name)),
      h("td", { class: "r" }, fmt(e.count, 0)),
      h("td", { class: "r strong" }, fmt(e.count * items, 0))));
    const result = h("div", { class: "gr-maxout-result" },
      h("p", { class: "gr-count", role: "status" },
        t("{c} classes × {p} pieces = {n} items").replace("{c}", maxOut.classes).replace("{p}", pieces).replace("{n}", fmt(items, 0))),
      items ? h("div", { class: "gr-table-wrap" }, h("table", { class: "gr-table" },
        h("thead", {}, h("tr", {},
          h("th", { class: "l", scope: "col" }, t("Material")),
          h("th", { class: "r", scope: "col" }, t("Per item")),
          h("th", { class: "r", scope: "col" }, t("All {n} items").replace("{n}", fmt(items, 0))))),
        h("tbody", {}, rows))) : null,
      h("p", { class: "gr-fine" }, raising && maxOut.fromBelow
        ? t("Takes each item to {from} +5, raises it to {to}, then takes it to +5 again. Pearls: 2 on every stat line, plus 1 for each line an item is still missing.")
          .replace("{from}", rarityName(byKey()[f.raise_from])).replace("{to}", rarityName(byKey()[f.raise_to]))
        : t("Takes each item to +5. Pearls: 2 on every stat line, plus 1 for each line an item is still missing.")));

    host.appendChild(controls);
    host.appendChild(result);
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
        i === 0 ? h("th", { class: "l", scope: "rowgroup", rowspan: s.operations.length },
          h("span", { class: "gr-named" }, itemIcon(s.prefab.replace(/_interactive$/, "")), s.name),
          LEGACY_STATIONS.has(s.prefab) ? h("span", { class: "gr-tag" }, t("Legacy")) : null) : null,
        h("td", { class: "l" }, t(op.label)),
        h("td", { class: "l" }, op.cost ? costList(op.cost) : t("Depends on the item's rarity (see above)"), retiredTag(op.cost))));
    }));
    host.appendChild(h("table", { class: "gr-table" },
      h("thead", {}, h("tr", {},
        h("th", { class: "l", scope: "col" }, t("Station")),
        h("th", { class: "l", scope: "col" }, t("What it does")),
        h("th", { class: "l", scope: "col" }, t("Cost")))),
      body));
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
      const pic = b.items.map((i) => itemIcon(i.prefab)).find(Boolean) || null;
      host.appendChild(h("article", { class: "gr-banner" },
        h("h3", { class: "gr-named" }, pic, h("span", {}, head)), more, stats));
    });
  }

  function renderAll() {
    if (!data) return;
    renderCalc();
    renderRarities();
    renderForge();
    renderMaxOut();
    renderPearls();
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
