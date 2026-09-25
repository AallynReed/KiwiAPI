/* ===========================================================================
   classes.js - the Trove class reference (/classes).

   Fetches every class once from the same-origin /site/stats/classes proxy,
   renders a picker, and shows the selected class's stats / subclass / abilities.
   Deep-links via the URL hash (#knight), so a class page is shareable.
   ========================================================================== */
(function () {
  "use strict";

  const { getJSON } = window.BTTUtil;
  const { fmtStat, sheetAt, wireLevel } = window.BTTClassLevel;
  function tr(s) { return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s; }
  function el(tag, cls, txt) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (txt != null) e.textContent = txt;
    return e;
  }
  // Decorative Font Awesome icon — hidden from assistive tech.
  function iconEl(cls) { var e = el("i", cls); e.setAttribute("aria-hidden", "true"); return e; }
  // Ability damage: `multiplier` scales the class's damage stat, so 3.5 reads as
  // 350%. Mirrors _fmt_stages() in classes_page.py so SSR and the client agree.
  function fmtNum(v) {
    return (v % 1 === 0) ? v.toLocaleString() : v.toLocaleString(undefined, { maximumFractionDigits: 2 });
  }
  function fmtBonus(s) {
    // Same as fmtStat but signed, for values shown as a bonus. `absolute` stats
    // (Fae Trickster's Flying Speed) name a resulting value, not a delta.
    var out = fmtStat(s);
    if (s.absolute) return out;
    return out.charAt(0) === "-" ? out : "+" + out;
  }
  function dmgClass(d) { return (d || "").toLowerCase() === "physical" ? "physical" : "magic"; }
  // Some classes carry placeholder subclass rows ({name:"", value:0}); a bonus is
  // only worth showing if it has a real stat name or a non-zero value.
  function meaningful(list) {
    return (list || []).filter(function (b) { return b && ((b.name && b.name.trim()) || b.value); });
  }
  function iconUrl(tech) { return "/static/class-icons/" + encodeURIComponent(tech || "") + ".png"; }

  var CLASSES = [], byTech = {}, selectedTech = null, loaded = false;

  // The nav + the first class's detail are server-rendered (English) - see
  // classes.html + classes_page.py. We still fetch the full set once (needed to
  // render OTHER classes on click / hash / language switch), but when the server
  // already rendered the class we're showing we skip the initial rebuild and
  // just hydrate the existing DOM.
  var detailEl = document.getElementById("cls-detail");
  var ssrTech = (detailEl && detailEl.dataset.ssrTech) || "";

  getJSON("/site/stats/classes").then(function (d) {
    CLASSES = (d && d.items) || [];
    CLASSES.forEach(function (c) { byTech[c.tech_name] = c; });
    loaded = true;
    if (!CLASSES.length) { if (!ssrTech) setEmpty(); return; }
    var hash = (location.hash || "").replace(/^#/, "");
    var initial = byTech[hash] ? hash : (ssrTech && byTech[ssrTech] ? ssrTech : CLASSES[0].tech_name);
    if (ssrTech) {
      hydrateNav();
      if (initial === ssrTech) {
        // Server already rendered this class - just sync selection state.
        selectedTech = ssrTech;
        markActive(ssrTech);
        wireLevel(detailEl, byTech[ssrTech]);
      } else {
        select(initial, false);
      }
    } else {
      buildNav();
      select(initial, false);
    }
  }).catch(function () { if (!ssrTech) setEmpty(); });

  function setEmpty() {
    var nav = document.getElementById("cls-nav"), det = document.getElementById("cls-detail");
    if (nav) { nav.textContent = ""; nav.appendChild(el("p", "cls-empty", tr("Couldn't load classes."))); }
    if (det) det.textContent = "";
  }

  function buildNav() {
    var nav = document.getElementById("cls-nav");
    if (!nav) return;
    nav.textContent = "";
    CLASSES.forEach(function (c) {
      var btn = el("button", "cls-navitem");
      btn.type = "button";
      btn.dataset.tech = c.tech_name;
      var img = document.createElement("img");
      img.src = iconUrl(c.tech_name); img.alt = ""; img.loading = "lazy";
      img.onerror = function () { img.style.visibility = "hidden"; };
      btn.appendChild(img);
      btn.appendChild(el("span", "cls-navitem-name", c.name));
      btn.appendChild(el("span", "cls-navitem-dmg dmg-" + dmgClass(c.damage_type), c.damage_type || ""));
      nav.appendChild(btn);
    });
    hydrateNav();
  }

  // Attach the select-on-click handler to every nav button - server-rendered or
  // freshly built by buildNav. Idempotent (the _wired flag) so a language-switch
  // rebuild doesn't stack listeners.
  function hydrateNav() {
    document.querySelectorAll(".cls-navitem").forEach(function (b) {
      if (b._wired) return;
      b._wired = true;
      b.addEventListener("click", function () { select(b.dataset.tech, true); });
    });
  }

  function markActive(tech) {
    document.querySelectorAll(".cls-navitem").forEach(function (b) {
      var on = b.dataset.tech === tech;
      b.classList.toggle("active", on);
      if (on) b.setAttribute("aria-current", "true"); else b.removeAttribute("aria-current");
    });
  }

  function select(tech, updateHash) {
    var c = byTech[tech];
    if (!c) return;
    selectedTech = tech;
    markActive(tech);
    if (updateHash && location.hash.replace(/^#/, "") !== tech) {
      history.replaceState(null, "", "#" + tech);
    }
    renderDetail(c);
  }

  function renderDetail(c) {
    var box = document.getElementById("cls-detail");
    if (!box) return;
    box.textContent = "";

    // Head
    var head = el("div", "cls-detail-head");
    var icon = document.createElement("img");
    icon.className = "cls-detail-icon"; icon.src = iconUrl(c.tech_name); icon.alt = c.name || "";
    icon.onerror = function () { icon.style.visibility = "hidden"; };
    head.appendChild(icon);
    var title = el("div", "cls-detail-title");
    title.appendChild(el("h2", "cls-detail-name", c.name || ""));
    var tags = el("div", "cls-detail-tags");
    if (c.damage_type) {
      var dt = el("span", "cls-tag is-" + dmgClass(c.damage_type));
      dt.appendChild(iconEl("fa-solid " + (dmgClass(c.damage_type) === "magic" ? "fa-wand-sparkles" : "fa-hand-fist")));
      dt.appendChild(el("span", null, c.damage_type + " " + tr("damage")));
      tags.appendChild(dt);
    }
    (c.weapons || []).forEach(function (w) {
      var t = el("span", "cls-tag");
      t.appendChild(iconEl("fa-solid fa-khanda"));
      t.appendChild(el("span", null, w));
      tags.appendChild(t);
    });
    if (c.shorts && c.shorts.length) {
      var s = el("span", "cls-tag");
      s.appendChild(iconEl("fa-solid fa-tag"));
      s.appendChild(el("span", null, c.shorts.join(" / ")));
      tags.appendChild(s);
    }
    title.appendChild(tags);
    head.appendChild(title);
    box.appendChild(head);

    // Base stats, with a class-level control when the level table is known
    var stats = (c.stats || []).filter(function (s) { return s && s.name; });
    var growth = growthTable(c.levels);
    if (stats.length) {
      var statSec = section(tr("Base stats"), "fa-chart-simple", (function () {
        var grid = el("div", "cls-stats");
        stats.forEach(function (s) {
          var row = el("div", "cls-stat");
          row.appendChild(el("span", "cls-stat-name", s.name));
          row.appendChild(el("span", "cls-stat-val", fmtStat(s)));
          grid.appendChild(row);
        });
        return grid;
      })());
      var headRow = el("div", "cls-section-head");
      statSec.insertBefore(headRow, statSec.firstChild);
      headRow.appendChild(statSec.querySelector(".cls-section-title"));
      if (growth) {
        var lvl = el("div", "cls-lvl");
        var lab = el("label", "cls-lvl-label", tr("Class level"));
        lab.htmlFor = "cls-lvl-input";
        var input = el("input", "cls-lvl-input");
        input.id = "cls-lvl-input"; input.type = "range";
        input.min = "1"; input.max = "30"; input.step = "1"; input.value = "30";
        var out = el("output", "cls-lvl-out", "30");
        out.htmlFor = "cls-lvl-input";
        lvl.appendChild(lab); lvl.appendChild(input); lvl.appendChild(out);
        headRow.appendChild(lvl);
      }
      statSec.appendChild(el("p", "cls-note", tr("No gear, gems or subclass.")));
      box.appendChild(statSec);
    }

    // Level scaling: what each level adds, or the whole sheet at each level.
    if (growth) {
      var totals = totalsTable(stats, c.levels);
      var gsec = section(tr("Level scaling"), "fa-arrow-trend-up", levelTable(growth, "added", false));
      gsec.classList.add("cls-growth");
      var ghead = el("div", "cls-section-head");
      gsec.insertBefore(ghead, gsec.firstChild);
      ghead.appendChild(gsec.querySelector(".cls-section-title"));
      if (totals) {
        var seg = el("div", "cls-seg");
        seg.setAttribute("role", "group");
        seg.setAttribute("aria-label", tr("Level scaling view"));
        [["added", tr("Added per level")], ["total", tr("Total at level")]].forEach(function (v, i) {
          var b = el("button", "cls-seg-btn", v[1]);
          b.type = "button"; b.dataset.view = v[0];
          b.setAttribute("aria-pressed", i === 0 ? "true" : "false");
          seg.appendChild(b);
        });
        ghead.appendChild(seg);
        gsec.appendChild(levelTable(totals, "total", true));
      }
      box.appendChild(gsec);
    }

    // Class gem / bonus stats (non-zero only)
    var bonuses = (c.bonuses || []).filter(function (b) { return b && b.name && b.value; });
    if (bonuses.length) {
      box.appendChild(section(tr("Class bonuses"), "fa-plus", (function () {
        var chips = el("div", "cls-chips");
        bonuses.forEach(function (b) {
          chips.appendChild(el("span", "cls-chip", b.name + " " + fmtBonus(b)));
        });
        return chips;
      })()));
    }

    // Subclass. Some classes ship placeholder-only subclass data (no name/desc,
    // all-zero level rows) - only render the section when there's real content.
    var sc = c.subclass || {};
    var lv = sc.level || {};
    var scTiers = Object.keys(lv).sort(function (a, b) { return Number(a) - Number(b); })
      .map(function (tier) { return { tier: tier, bonuses: meaningful(lv[tier]) }; })
      .filter(function (t) { return t.bonuses.length; });
    if (sc.name || sc.description || scTiers.length) {
      box.appendChild(section(tr("Subclass"), "fa-star", (function () {
        var wrap = el("div", "cls-subclass");
        if (sc.name) wrap.appendChild(el("div", "cls-subclass-name", sc.name));
        if (sc.description) wrap.appendChild(el("div", "cls-subclass-desc", sc.description));
        if (scTiers.length) {
          var grid = el("div", "cls-levels");
          scTiers.forEach(function (t) {
            var cell = el("div", "cls-level");
            cell.appendChild(el("div", "cls-level-tier", tr("Level") + " " + t.tier));
            t.bonuses.forEach(function (b) {
              var line = el("div", "cls-level-bonus");
              if (b.name && b.name.trim()) line.appendChild(document.createTextNode(b.name + " "));
              line.appendChild(el("strong", null, fmtBonus(b)));
              cell.appendChild(line);
            });
            grid.appendChild(cell);
          });
          wrap.appendChild(grid);
        }
        return wrap;
      })()));
    }

    // Abilities. The ones the live class prefab no longer reaches still load in
    // game but are not what the class does now, so they fold away separately.
    var all = (c.abilities || []).filter(function (a) { return a && (a.name || a.description); });
    var abilities = all.filter(function (a) { return a.active !== false; });
    var legacy = all.filter(function (a) { return a.active === false; });

    if (abilities.length) box.appendChild(section(tr("Abilities"), "fa-bolt", abilityList(abilities)));
    if (legacy.length) {
      var det = el("details", "cls-legacy");
      var sum = el("summary");
      sum.appendChild(iconEl("fa-solid fa-clock-rotate-left"));
      sum.appendChild(el("span", "", tr("Legacy abilities") + " (" + legacy.length + ")"));
      det.appendChild(sum);
      det.appendChild(el("p", "cls-legacy-note",
        tr("Still present in the game files, but the class no longer uses them.")));
      det.appendChild(abilityList(legacy));
      var sec = el("section", "cls-section");
      sec.appendChild(det);
      box.appendChild(sec);
    }
    wireLevel(box, c);
  }

  // {level: [stat]} -> {cols, rows:[{level, cells}]}. Mirrors _growth() in classes_page.py.
  function growthTable(levels) {
    var order = Object.keys(levels || {}).sort(function (a, b) { return Number(a) - Number(b); });
    var cols = [];
    order.forEach(function (lv) {
      levels[lv].forEach(function (s) { if (s && s.name && cols.indexOf(s.name) < 0) cols.push(s.name); });
    });
    if (!cols.length) return null;
    return {
      cols: cols,
      rows: order.map(function (lv) {
        var by = {};
        levels[lv].forEach(function (s) { if (s && s.name) by[s.name] = s; });
        return { level: lv, cells: cols.map(function (n) { return by[n] ? fmtBonus(by[n]) : ""; }) };
      })
    };
  }

  // The sheet at every level, limited to the stats that change with level.
  // Mirrors _totals() in classes_page.py.
  function totalsTable(stats, levels) {
    if (!levels || !Object.keys(levels).length) return null;
    var grid = [];
    for (var lv = 1; lv <= 30; lv++) grid.push(stats.map(function (s) { return sheetAt(s, levels, lv); }));
    var keep = [];
    stats.forEach(function (s, i) { if (grid[0][i] !== grid[29][i]) keep.push(i); });
    if (!keep.length) return null;
    return {
      cols: keep.map(function (i) { return stats[i].name; }),
      rows: grid.map(function (row, k) {
        return { level: String(k + 1), cells: keep.map(function (i) {
          return fmtStat({ value: row[i], percentage: stats[i].percentage });
        }) };
      })
    };
  }

  // Mirrors the level_table macro in partials/class_detail.html.
  function levelTable(t, view, hidden) {
    var scroll = el("div", "cls-growth-scroll");
    scroll.dataset.view = view;
    scroll.hidden = hidden;
    var table = el("table", "cls-growth-table");
    var thead = el("thead"), htr = el("tr");
    [tr("Level")].concat(t.cols).forEach(function (n) {
      var th = el("th", null, n); th.scope = "col"; htr.appendChild(th);
    });
    thead.appendChild(htr); table.appendChild(thead);
    var tbody = el("tbody");
    t.rows.forEach(function (r) {
      var rtr = el("tr");
      rtr.dataset.level = r.level;
      var th = el("th", null, r.level); th.scope = "row"; rtr.appendChild(th);
      r.cells.forEach(function (v) { rtr.appendChild(el("td", null, v)); });
      tbody.appendChild(rtr);
    });
    table.appendChild(tbody);
    scroll.appendChild(table);
    return scroll;
  }

  // Mirrors the ability_card macro in partials/class_detail.html.
  function abilityList(abilities) {
    var list = el("div", "cls-abilities");
    abilities.forEach(function (a) {
      var card = el("div", "cls-ability");
      if (a.name) {
        var head = el("div", "cls-ability-name", a.name);
        if (a.type) head.appendChild(el("span", "cls-ability-kind", tr(a.type)));
        card.appendChild(head);
      }
      if (a.description) card.appendChild(el("div", "cls-ability-desc", a.description));
      var stages = (a.stages || []).filter(function (s) { return s && (s.multiplier || s.base); });
      if (stages.length) {
        var ul = el("ul", "cls-stages");
        stages.forEach(function (s) {
          var li = el("li");
          li.appendChild(el("span", "cls-stage-name", s.name || ""));
          var val = el("span", "cls-stage-val", s.multiplier ? fmtNum(s.multiplier * 100) + "%" : "");
          if (s.base) val.appendChild(el("span", "cls-stage-base", "+" + fmtNum(s.base)));
          li.appendChild(val);
          ul.appendChild(li);
        });
        card.appendChild(ul);
      }
      list.appendChild(card);
    });
    return list;
  }

  function section(titleText, icon, node) {
    var sec = el("section", "cls-section");
    var h = el("h3", "cls-section-title");
    h.appendChild(iconEl("fa-solid " + icon));
    h.appendChild(el("span", null, titleText));
    sec.appendChild(h);
    sec.appendChild(node);
    return sec;
  }

  // React to back/forward navigation between deep-linked classes.
  window.addEventListener("hashchange", function () {
    var tech = (location.hash || "").replace(/^#/, "");
    if (byTech[tech]) select(tech, false);
  });

  // Re-render from already-fetched data when the UI language changes (the i18n
  // runtime fires this on the initial locale load too). Rebuild the nav +
  // detail so tr()-wrapped labels pick up the new locale; keep the current
  // class selected.
  document.addEventListener("btt-lang-changed", function () {
    if (!loaded) return;
    if (!CLASSES.length) { setEmpty(); return; }
    buildNav();
    select(selectedTech && byTech[selectedTech] ? selectedTech : CLASSES[0].tech_name, false);
  });
})();
